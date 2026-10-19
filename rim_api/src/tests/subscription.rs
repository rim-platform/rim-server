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
        value: serde_json::json!(version),
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
    });
    broadcaster.broadcast(state_changed(server_id, 2));

    let joined = next_event(&mut stream).await.expect("the join");
    assert_eq!(joined["type"], "MemberJoined", "{joined}");
    assert_eq!(joined["user_id"], other.to_string(), "{joined}");
    let state = next_event(&mut stream).await.expect("the state change");
    assert_eq!(state["type"], "WorkspaceStateChanged", "{state}");
    assert_eq!(state["version"], 2, "{state}");
}
