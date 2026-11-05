use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use futures_util::StreamExt;
use rim_core::server_member::model::ServerMemberRole;
use rim_services::events::{Broadcaster, WorkspaceEvent};
use rim_services::workspace_state::dto::WorkspaceStateScopeDto;

use super::*;

// ── workspaceEvents ─────────────────────────────────────────────────────

/// A member mock answering the subscription's membership check with `initial`, then the check
/// after a lag with `reloaded`, or with `NotFound` when `reloaded` is `None`.
fn member_checks(
    server_id: ServerId,
    actor: UserId,
    initial: ServerMemberRole,
    reloaded: Option<ServerMemberRole>,
) -> MockServerMemberService {
    let calls = AtomicUsize::new(0);
    let mut server_member = MockServerMemberService::new();
    server_member
        .expect_get_member()
        .withf(move |q| q.server_id == server_id && q.user_id == actor && q.actor.user_id == actor)
        .times(2)
        .returning(move |_| {
            let role = match calls.fetch_add(1, Ordering::SeqCst) {
                0 => Some(initial),
                _ => reloaded,
            };
            let member = role
                .map(
                    |member_role| rim_services::server_member::dto::ServerMemberDto {
                        server_id,
                        user_id: actor,
                        member_role,
                        ..test_server_member_dto()
                    },
                )
                .ok_or(ServiceError::NotFound);
            Box::pin(async move { member })
        });
    server_member
}

/// Opens `workspaceEvents` for `actor` on a schema whose broadcaster keeps two events, and polls
/// it once so the subscription is listening before events are sent.
async fn open_workspace_events(
    server_member: MockServerMemberService,
    server_id: ServerId,
    actor: UserId,
) -> (
    InMemoryBroadcaster,
    futures_util::stream::BoxStream<'static, async_graphql::Response>,
) {
    let broadcaster = InMemoryBroadcaster::new(2);
    let schema =
        schema_with_server_mocks_on(MockServerService::new(), server_member, broadcaster.clone());
    let request = authed_request(
        &format!("subscription {{ workspaceEvents(workspaceId: \"{server_id}\") }}"),
        actor,
    );
    let mut stream = schema.execute_stream(request).boxed();
    assert!(
        tokio::time::timeout(Duration::from_millis(50), stream.next())
            .await
            .is_err(),
        "no event is sent yet"
    );
    (broadcaster, stream)
}

fn state_changed(server_id: ServerId, version: i64) -> WorkspaceEvent {
    WorkspaceEvent::WorkspaceStateChanged {
        workspace_id: server_id,
        key: "topic".to_string(),
        scope: WorkspaceStateScopeDto::Server,
        user_id: None,
        version,
    }
}

/// The next event the stream yields, as JSON, or `None` when the stream ended.
async fn next_event(
    stream: &mut futures_util::stream::BoxStream<'static, async_graphql::Response>,
) -> Option<serde_json::Value> {
    let response = tokio::time::timeout(Duration::from_secs(2), stream.next())
        .await
        .ok()?;
    let response = response?;
    assert_no_errors(&response);
    let data = response.data.into_json().ok()?;
    serde_json::from_str(data["workspaceEvents"].as_str()?).ok()
}

#[tokio::test]
async fn workspace_events_end_after_lag_when_subscriber_was_removed() {
    let actor = UserId::new();
    let server_id = ServerId::new();
    let server_member = member_checks(server_id, actor, ServerMemberRole::Member, None);
    let (broadcaster, mut stream) = open_workspace_events(server_member, server_id, actor).await;

    // The removal is pushed out of the two-event buffer by later events.
    broadcaster.broadcast(WorkspaceEvent::MemberLeft {
        workspace_id: server_id,
        user_id: actor,
        cause: None,
        until: None,
        reason: None,
    });
    for version in 0..3 {
        broadcaster.broadcast(state_changed(server_id, version));
    }

    let ended = tokio::time::timeout(Duration::from_secs(2), stream.next())
        .await
        .expect("the stream answers");
    assert!(ended.is_none(), "stream delivered {ended:?}");
}

#[tokio::test]
async fn workspace_events_filter_as_guest_after_lag_when_subscriber_was_demoted() {
    let actor = UserId::new();
    let server_id = ServerId::new();
    let server_member = member_checks(
        server_id,
        actor,
        ServerMemberRole::Member,
        Some(ServerMemberRole::Guest),
    );
    let (broadcaster, mut stream) = open_workspace_events(server_member, server_id, actor).await;

    // The demotion is pushed out of the two-event buffer; a guest does not see another
    // member's join.
    broadcaster.broadcast(WorkspaceEvent::MemberRoleChanged {
        workspace_id: server_id,
        user_id: actor,
        role: ServerMemberRole::Guest.as_str().to_string(),
    });
    broadcaster.broadcast(state_changed(server_id, 0));
    broadcaster.broadcast(WorkspaceEvent::MemberJoined {
        workspace_id: server_id,
        user_id: UserId::new(),
        username: "other".to_string(),
        source: None,
    });
    broadcaster.broadcast(state_changed(server_id, 1));

    let event = next_event(&mut stream).await.expect("an event");
    assert_eq!(event["type"], "WorkspaceStateChanged", "{event}");
    assert_eq!(event["version"], 1, "{event}");
}

#[tokio::test]
async fn workspace_events_continue_after_lag_when_subscriber_is_still_member() {
    let actor = UserId::new();
    let server_id = ServerId::new();
    let other = UserId::new();
    let server_member = member_checks(
        server_id,
        actor,
        ServerMemberRole::Member,
        Some(ServerMemberRole::Member),
    );
    let (broadcaster, mut stream) = open_workspace_events(server_member, server_id, actor).await;

    for version in 0..2 {
        broadcaster.broadcast(state_changed(server_id, version));
    }
    broadcaster.broadcast(WorkspaceEvent::MemberJoined {
        workspace_id: server_id,
        user_id: other,
        username: "other".to_string(),
        source: None,
    });
    broadcaster.broadcast(state_changed(server_id, 2));

    let joined = next_event(&mut stream).await.expect("the join");
    assert_eq!(joined["type"], "MemberJoined", "{joined}");
    assert_eq!(joined["user_id"], other.to_string(), "{joined}");
    let state = next_event(&mut stream).await.expect("the state change");
    assert_eq!(state["type"], "WorkspaceStateChanged", "{state}");
    assert_eq!(state["version"], 2, "{state}");
}

// ── notificationEvents ──────────────────────────────────────────────────

const NOTIFICATION_EVENTS: &str = "subscription { notificationEvents { workspaceId notificationId conversationId messageId reason quiet reactionEmoji reactionBy } }";

fn notification_due(workspace_id: ServerId, recipient: UserId, reason: &str) -> WorkspaceEvent {
    WorkspaceEvent::NotificationDue {
        workspace_id,
        user_id: recipient,
        notification_id: rim_core::notification::model::UserNotificationId::new(),
        conversation_id: rim_core::conversation::model::ConversationId::new(),
        message_id: rim_core::message::model::MessageId::new(),
        reason: reason.to_string(),
        quiet: false,
        reaction_emoji: None,
        reaction_by: None,
        member_user_ids: vec![recipient],
    }
}

/// Opens `notificationEvents` for `actor` on a schema whose broadcaster keeps `capacity` events,
/// and polls it once so the subscription is listening before events are sent.
async fn open_notification_events(
    actor: UserId,
    capacity: usize,
) -> (
    InMemoryBroadcaster,
    futures_util::stream::BoxStream<'static, async_graphql::Response>,
) {
    let broadcaster = InMemoryBroadcaster::new(capacity);
    let schema = schema_with_server_mocks_on(
        MockServerService::new(),
        MockServerMemberService::new(),
        broadcaster.clone(),
    );
    let mut stream = schema
        .execute_stream(authed_request(NOTIFICATION_EVENTS, actor))
        .boxed();
    assert!(
        tokio::time::timeout(Duration::from_millis(50), stream.next())
            .await
            .is_err(),
        "no event is sent yet"
    );
    (broadcaster, stream)
}

/// The `notificationEvents` item the stream yields next, or `None` when it ended or stayed
/// silent for two seconds.
async fn next_notification(
    stream: &mut futures_util::stream::BoxStream<'static, async_graphql::Response>,
) -> Option<serde_json::Value> {
    let response = tokio::time::timeout(Duration::from_secs(2), stream.next())
        .await
        .ok()??;
    assert_no_errors(&response);
    let mut data = response.data.into_json().ok()?;
    Some(data["notificationEvents"].take())
}

#[tokio::test]
async fn notification_events_deliver_the_viewers_notifications_from_every_workspace() {
    let actor = UserId::new();
    let (acme, beta) = (ServerId::new(), ServerId::new());
    let (broadcaster, mut stream) = open_notification_events(actor, 16).await;

    broadcaster.broadcast(notification_due(acme, actor, "mention"));
    broadcaster.broadcast(notification_due(beta, actor, "mention"));

    for workspace in [acme, beta] {
        let item = next_notification(&mut stream)
            .await
            .expect("a notification");
        assert_eq!(item["workspaceId"], workspace.to_string(), "{item}");
        assert_eq!(item["reason"], "MENTION", "{item}");
        assert_eq!(item["quiet"], false, "{item}");
        assert!(item["reactionEmoji"].is_null(), "{item}");
        assert!(item["reactionBy"].is_null(), "{item}");
    }
}

#[tokio::test]
async fn notification_events_skip_other_people_and_other_events() {
    let actor = UserId::new();
    let workspace = ServerId::new();
    let (broadcaster, mut stream) = open_notification_events(actor, 16).await;

    broadcaster.broadcast(notification_due(workspace, UserId::new(), "mention"));
    broadcaster.broadcast(state_changed(workspace, 0));
    assert!(
        tokio::time::timeout(Duration::from_millis(100), stream.next())
            .await
            .is_err(),
        "nothing is delivered for them"
    );

    broadcaster.broadcast(notification_due(workspace, actor, "direct"));
    let item = next_notification(&mut stream)
        .await
        .expect("own notification");
    assert_eq!(item["reason"], "DIRECT", "{item}");
}

#[tokio::test]
async fn notification_events_carry_a_reaction() {
    let actor = UserId::new();
    let reactor = UserId::new();
    let (broadcaster, mut stream) = open_notification_events(actor, 16).await;

    let mut event = notification_due(ServerId::new(), actor, "reaction");
    if let WorkspaceEvent::NotificationDue {
        reaction_emoji,
        reaction_by,
        ..
    } = &mut event
    {
        *reaction_emoji = Some("🎉".to_string());
        *reaction_by = Some(reactor);
    }
    broadcaster.broadcast(event);

    let item = next_notification(&mut stream)
        .await
        .expect("a notification");
    assert_eq!(item["reason"], "REACTION", "{item}");
    assert_eq!(item["reactionEmoji"], "🎉", "{item}");
    assert_eq!(item["reactionBy"], reactor.to_string(), "{item}");
}

#[tokio::test]
async fn notification_events_continue_after_a_lag() {
    let actor = UserId::new();
    let workspace = ServerId::new();
    let (broadcaster, mut stream) = open_notification_events(actor, 2).await;

    for _ in 0..4 {
        broadcaster.broadcast(notification_due(workspace, actor, "all"));
    }
    // The two newest survive the lag; drain them, then a later one still arrives.
    for _ in 0..2 {
        next_notification(&mut stream).await.expect("a kept event");
    }
    broadcaster.broadcast(notification_due(workspace, actor, "keyword"));
    let item = next_notification(&mut stream).await.expect("after the lag");
    assert_eq!(item["reason"], "KEYWORD", "{item}");
}

#[tokio::test]
async fn notification_events_need_a_session() {
    let schema = schema_with_server_mocks_on(
        MockServerService::new(),
        MockServerMemberService::new(),
        InMemoryBroadcaster::new(2),
    );
    let request = Request::new(NOTIFICATION_EVENTS).data(AuthActorCtx::new(None, None));
    let mut stream = schema.execute_stream(request).boxed();
    let response = tokio::time::timeout(Duration::from_secs(2), stream.next())
        .await
        .expect("an answer")
        .expect("a response");
    assert!(!response.errors.is_empty(), "anonymous is refused");
}
