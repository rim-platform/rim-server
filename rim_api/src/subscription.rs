use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context as TaskContext, Poll};

use async_graphql::*;
use futures_util::Stream;
use rim_core::user::model::UserId;
use rim_services::events::WorkspaceEvent;
use rim_services::events::visibility::user_can_see_event_as;
use rim_services::presence::PresenceService;
use rim_services::remote_workspace::service::WorkspaceRelayService;

use crate::broadcast::InMemoryBroadcaster;
use crate::types::NotificationDueEvent;

pub struct Subscription;

/// Wrapper stream that calls `PresenceService::mark_offline` when dropped
/// (i.e. when the WebSocket disconnects and the subscription stream ends).
struct DisconnectGuardStream<S> {
    inner: Pin<Box<S>>,
    presence: Arc<dyn PresenceService>,
    user_id: UserId,
}

impl<S: Stream> Stream for DisconnectGuardStream<S> {
    type Item = S::Item;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<Option<Self::Item>> {
        self.inner.as_mut().poll_next(cx)
    }
}

impl<S> Drop for DisconnectGuardStream<S> {
    fn drop(&mut self) {
        tracing::info!(user_id = %self.user_id, "WS subscription stream dropped, marking user offline");
        self.presence.mark_offline(self.user_id);
    }
}

/// A relay stream that tells the relay when it opens and when it closes, so the hosts of the
/// user's workspaces learn whether they are online.
struct RelayStreamPresence<S> {
    inner: Pin<Box<S>>,
    workspaces: Arc<dyn WorkspaceRelayService>,
    user_id: UserId,
}

impl<S> RelayStreamPresence<S> {
    fn new(inner: S, workspaces: Arc<dyn WorkspaceRelayService>, user_id: UserId) -> Self {
        workspaces.stream_opened(user_id);
        Self {
            inner: Box::pin(inner),
            workspaces,
            user_id,
        }
    }
}

impl<S: Stream> Stream for RelayStreamPresence<S> {
    type Item = S::Item;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<Option<Self::Item>> {
        self.inner.as_mut().poll_next(cx)
    }
}

impl<S> Drop for RelayStreamPresence<S> {
    fn drop(&mut self) {
        self.workspaces.stream_closed(self.user_id);
    }
}

/// `event` as the notification `viewer` should receive: `None` for any other event, for another
/// person's notification, and for a reason this build does not know.
fn notification_due_for(event: &WorkspaceEvent, viewer: UserId) -> Option<NotificationDueEvent> {
    let WorkspaceEvent::NotificationDue {
        workspace_id,
        notification_id,
        conversation_id,
        message_id,
        reason,
        quiet,
        reaction_emoji,
        reaction_by,
        ..
    } = event
    else {
        return None;
    };
    if !user_can_see_event_as(event, &viewer.to_string(), false) {
        return None;
    }
    let reason = match reason.parse::<rim_core::notification::model::NotificationReason>() {
        Ok(reason) => reason,
        Err(e) => {
            tracing::warn!(error = %e, "notification with an unknown reason skipped");
            return None;
        }
    };
    Some(NotificationDueEvent {
        workspace_id: ID::from(workspace_id.to_string()),
        notification_id: ID::from(notification_id.to_string()),
        conversation_id: ID::from(conversation_id.to_string()),
        message_id: ID::from(message_id.to_string()),
        reason: reason.into(),
        quiet: *quiet,
        reaction_emoji: reaction_emoji.clone(),
        reaction_by: reaction_by.map(|id| ID::from(id.to_string())),
    })
}

#[Subscription]
impl Subscription {
    /// Subscribe to all real-time events for a workspace.
    /// Returns events as JSON strings — the client parses the `type` field to determine the event kind.
    ///
    /// The subscriber's membership is checked when the subscription starts. Afterwards their
    /// guest status follows their `MemberRoleChanged` events, and the stream ends after their
    /// `MemberLeft`. When the subscriber falls behind and events are dropped, their membership
    /// and role are loaded again before any further event is delivered; the stream ends when
    /// that load fails, including when they are no longer a member.
    async fn workspace_events(
        &self,
        ctx: &Context<'_>,
        workspace_id: ID,
    ) -> Result<impl futures_util::Stream<Item = String>> {
        // Require authenticated user
        let actor = crate::auth_context::require_actor(ctx)?;

        // Verify user is member of the workspace (server)
        let svc = ctx
            .data::<std::sync::Arc<dyn rim_services::server_member::service::ServerMemberService>>(
            )?;
        let server_id =
            crate::ids::parse_uuid_id::<rim_core::server::model::ServerId>(&workspace_id)?;
        let me = svc
            .get_member(rim_services::server_member::command::GetServerMemberQuery {
                actor: rim_services::Actor::new(actor),
                server_id,
                user_id: actor,
            })
            .await
            .map_err(crate::errors::map_service_error)?;
        let initially_guest = me.member_role.is_guest();
        let members = Arc::clone(svc);

        let presence = ctx.data::<Arc<dyn PresenceService>>()?;
        let broadcaster = ctx.data::<InMemoryBroadcaster>()?;
        let rx = broadcaster.subscribe();
        let ws_id = workspace_id.to_string();
        let subscriber_user_id = actor.to_string();
        // A DM or group in the subscriber's request inbox, or one whose creator they blocked,
        // stays quiet until they accept it or lift the block; channels are not checked.
        let federation = ctx.data::<crate::federation::FederationContext>().ok();
        let in_direct_space =
            federation.is_some_and(|f| f.info.direct_space_id.to_string() == ws_id);
        let inbox = federation.map(|f| Arc::clone(&f.relay));

        // The raw receiver is read directly so a lag can recheck the membership and keep the
        // stream open for a subscriber who is still a member.
        let inner = async_stream::stream! {
            let mut rx = rx;
            // The subscriber's guest status follows their role changes, and the
            // stream ends once they are removed from the workspace (kick/ban), after delivering
            // the `MemberLeft` that tells their client. Dropped events may have carried either
            // change, so after a lag both are read from the membership again.
            let mut is_guest = initially_guest;
            loop {
                match rx.recv().await {
                    Ok(event) => {
                        // A join request's person, not a member of the workspace it is about,
                        // gets its events on their direct-space subscription.
                        let for_me_here = in_direct_space
                            && event
                                .direct_space_recipient()
                                .is_some_and(|u| u.to_string() == subscriber_user_id);
                        if event.workspace_id() != ws_id && !for_me_here {
                            continue;
                        }
                        if let WorkspaceEvent::MemberRoleChanged { user_id, role, .. } = &event
                            && user_id.to_string() == subscriber_user_id
                        {
                            is_guest = role == rim_core::server_member::model::ServerMemberRole::Guest.as_str();
                        }
                        let removed_self = matches!(
                            &event,
                            WorkspaceEvent::MemberLeft { user_id, .. }
                                if user_id.to_string() == subscriber_user_id
                        );
                        if user_can_see_event_as(&event, &subscriber_user_id, is_guest)
                            && let Ok(json) = event.json_for_user(&subscriber_user_id).map(|value| value.to_string()) {
                                let through_inbox = in_direct_space
                                    || event.conversation_type().is_none_or(|t| {
                                        t == rim_core::conversation::model::ConversationType::Direct.as_str()
                                            || t == rim_core::conversation::model::ConversationType::Group.as_str()
                                    });
                                let conversation_id = inbox.as_ref().filter(|_| through_inbox).and_then(|_| {
                                    serde_json::to_value(&event)
                                        .ok()?
                                        .get("conversation_id")?
                                        .as_str()
                                        .and_then(|v| uuid::Uuid::parse_str(v).ok())
                                });
                                let hidden = match (&inbox, conversation_id) {
                                    (Some(relay), Some(conversation_id)) => relay
                                        .conversation_hidden(actor, conversation_id)
                                        .await
                                        .unwrap_or(true),
                                    _ => false,
                                };
                                if !hidden {
                                    yield json;
                                }
                            }
                        if removed_self {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("WS subscriber lagged, skipped {n} events");
                        let reloaded = members
                            .get_member(rim_services::server_member::command::GetServerMemberQuery {
                                actor: rim_services::Actor::new(actor),
                                server_id,
                                user_id: actor,
                            })
                            .await;
                        match reloaded {
                            Ok(me) => is_guest = me.member_role.is_guest(),
                            Err(e) => {
                                tracing::info!(error = %e, user_id = %actor, "lagged WS subscriber's membership not confirmed, ending stream");
                                break;
                            }
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        break; // Channel closed, end stream
                    }
                }
            }
        };

        // Wrap the stream so mark_offline is called when the client disconnects.
        Ok(DisconnectGuardStream {
            inner: Box::pin(inner),
            presence: Arc::clone(presence),
            user_id: actor,
        })
    }

    /// The viewer's notifications from every workspace on this server, as they are stored: the
    /// same decisions as the `NotificationDue` event of `workspaceEvents`, with no other event.
    /// A conversation in the viewer's request inbox, or one they blocked, stays quiet as it does
    /// there. A notification dropped because the subscriber fell behind is still in the inbox.
    /// Listed in `serverInfo.capabilities` as `notification_events`.
    async fn notification_events(
        &self,
        ctx: &Context<'_>,
    ) -> Result<impl futures_util::Stream<Item = NotificationDueEvent>> {
        let actor = crate::auth_context::require_actor(ctx)?;
        let broadcaster = ctx.data::<InMemoryBroadcaster>()?;
        let mut rx = broadcaster.subscribe();
        let federation = ctx.data::<crate::federation::FederationContext>().ok();
        let inbox = federation.map(|f| Arc::clone(&f.relay));
        Ok(async_stream::stream! {
            loop {
                match rx.recv().await {
                    Ok(event) => {
                        let Some(due) = notification_due_for(&event, actor) else {
                            continue;
                        };
                        let hidden = match (&inbox, uuid::Uuid::parse_str(due.conversation_id.as_str())) {
                            (Some(relay), Ok(conversation_id)) => relay
                                .conversation_hidden(actor, conversation_id)
                                .await
                                .unwrap_or(true),
                            (Some(_), Err(_)) => true,
                            (None, _) => false,
                        };
                        if !hidden {
                            yield due;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("notification subscriber lagged, skipped {n} events");
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        })
    }

    /// Host events queued for the session's device on this relay, above `after` (the device's
    /// cursor when omitted), then live ones, as JSON strings:
    /// `{"type":"RelayItem","id","hostServerId","workspaceId","envelope"}`, and the
    /// `DmRequestReceived`, `QueueOverflow` and `RevocationAcked` notices. While one is open
    /// the hosts of the user's workspaces see them online.
    async fn relay_stream(
        &self,
        ctx: &Context<'_>,
        after: Option<ID>,
    ) -> Result<impl futures_util::Stream<Item = String>> {
        let caller = crate::auth_context::require_mls_caller(ctx)?;
        if caller.device_key_id.is_none() {
            return Err(
                Error::new("this session is not bound to a device").extend_with(|_, e| {
                    e.set("code", Value::from("FORBIDDEN"));
                }),
            );
        }
        let federation = crate::federation::relay(ctx)?;
        let relay = Arc::clone(&federation.relay);
        let workspaces = Arc::clone(&federation.workspace_relay);
        let mut after = after
            .as_ref()
            .map(crate::query::relay::parse_item_id)
            .transpose()?;
        let user_id = caller.actor.user_id;
        let mut notices = relay.subscribe();
        let stream = async_stream::stream! {
            loop {
                loop {
                    let page = match relay.relay_items(caller, after, RELAY_STREAM_PAGE).await {
                        Ok(page) => page,
                        Err(e) => {
                            tracing::warn!(error = %e, "relay stream read failed");
                            return;
                        }
                    };
                    for item in &page.items {
                        after = Some(item.id);
                        yield item.stream_json();
                    }
                    if !page.has_more {
                        break;
                    }
                }
                loop {
                    let notice = tokio::select! {
                        notice = notices.recv() => notice,
                        () = tokio::time::sleep(RELAY_STREAM_POLL) => break,
                    };
                    match notice {
                        Ok(notice) if notice.user_id() == user_id => {
                            if let Some(json) = notice.stream_json() {
                                yield json;
                            }
                            break;
                        }
                        Ok(_) => {}
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => break,
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                    }
                }
            }
        };
        Ok(RelayStreamPresence::new(stream, workspaces, user_id))
    }
}

/// Items read per page by `relayStream`.
const RELAY_STREAM_PAGE: i64 = 200;
/// How long `relayStream` waits for a notice before reading the queue again.
const RELAY_STREAM_POLL: std::time::Duration = std::time::Duration::from_secs(30);
