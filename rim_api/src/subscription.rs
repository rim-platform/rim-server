use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context as TaskContext, Poll};

use async_graphql::*;
use futures_util::Stream;
use rim_core::user::model::UserId;
use rim_services::events::WorkspaceEvent;
use rim_services::events::visibility::user_can_see_event_as;
use rim_services::presence::PresenceService;

use crate::broadcast::InMemoryBroadcaster;

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

#[Subscription]
impl Subscription {
    /// Subscribe to all real-time events for a workspace.
    /// Returns events as JSON strings — the client parses the `type` field to determine the event kind.
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

        let presence = ctx.data::<Arc<dyn PresenceService>>()?;
        let broadcaster = ctx.data::<InMemoryBroadcaster>()?;
        let rx = broadcaster.subscribe();
        let ws_id = workspace_id.to_string();
        let subscriber_user_id = actor.to_string();
        // In the direct space, a conversation in the subscriber's request inbox (or from a
        // creator they blocked) stays quiet until they accept it.
        let inbox = ctx
            .data::<crate::federation::FederationContext>()
            .ok()
            .filter(|f| f.info.direct_space_id.to_string() == ws_id)
            .map(|f| Arc::clone(&f.relay));

        // Use async_stream to handle Lagged errors gracefully without ending the stream.
        // BroadcastStream ends on Lagged, so we use the raw receiver directly.
        let inner = async_stream::stream! {
            let mut rx = rx;
            // Track 1.2: the subscriber's guest status follows their role changes, and the
            // stream ends once they are removed from the workspace (kick/ban), after delivering
            // the `MemberLeft` that tells their client.
            let mut is_guest = initially_guest;
            loop {
                match rx.recv().await {
                    Ok(event) => {
                        if event.workspace_id() != ws_id {
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
                            && let Ok(json) = serde_json::to_string(&event) {
                                let conversation_id = inbox.as_ref().and_then(|_| {
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
                        // Continue — don't end the stream
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

    /// Host events queued for the session's device on this relay, above `after` (the device's
    /// cursor when omitted), then live ones, as JSON strings:
    /// `{"type":"RelayItem","id","hostServerId","workspaceId","envelope"}`, and the
    /// `DmRequestReceived` and `QueueOverflow` notices.
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
        let relay = Arc::clone(&crate::federation::relay(ctx)?.relay);
        let mut after = after
            .as_ref()
            .map(crate::query::relay::parse_item_id)
            .transpose()?;
        let user_id = caller.actor.user_id;
        let mut notices = relay.subscribe();
        Ok(async_stream::stream! {
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
        })
    }
}

/// Items read per page by `relayStream`.
const RELAY_STREAM_PAGE: i64 = 200;
/// How long `relayStream` waits for a notice before reading the queue again.
const RELAY_STREAM_POLL: std::time::Duration = std::time::Duration::from_secs(30);
