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
                                yield json;
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
}
