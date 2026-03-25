use async_graphql::*;
use rim_services::events::WorkspaceEvent;

use crate::broadcast::InMemoryBroadcaster;

pub struct Subscription;

/// Check whether a subscriber should see a given workspace event.
///
/// - Channel events are visible to all workspace members.
/// - DM/Group events are only visible to conversation members.
/// - Workspace-level events (MemberJoined, MemberAddedToConversation,
///   MemberRemovedFromConversation) are visible to all workspace members.
fn user_can_see_event(event: &WorkspaceEvent, user_id: &str) -> bool {
    match event {
        // Message events — filter by conversation type
        WorkspaceEvent::NewMessage {
            conversation_type,
            member_user_ids,
            ..
        }
        | WorkspaceEvent::MessageUpdated {
            conversation_type,
            member_user_ids,
            ..
        }
        | WorkspaceEvent::MessageDeleted {
            conversation_type,
            member_user_ids,
            ..
        }
        | WorkspaceEvent::ReactionsUpdated {
            conversation_type,
            member_user_ids,
            ..
        } => {
            if conversation_type == "Channel" {
                true
            } else {
                member_user_ids.iter().any(|id| id.to_string() == user_id)
            }
        }

        // Conversation created — channels are public, DMs/Groups only for members
        WorkspaceEvent::ConversationCreated {
            conversation_type,
            member_user_ids,
            ..
        }
        | WorkspaceEvent::ThreadCreated {
            conversation_type,
            member_user_ids,
            ..
        } => {
            if conversation_type == "Channel" {
                true
            } else {
                member_user_ids.iter().any(|id| id.to_string() == user_id)
            }
        }

        // Workspace-level events: visible to all workspace members
        WorkspaceEvent::MemberJoined { .. }
        | WorkspaceEvent::MemberAddedToConversation { .. }
        | WorkspaceEvent::MemberRemovedFromConversation { .. } => true,
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
        svc.get_member(rim_services::server_member::command::GetServerMemberQuery {
            actor: rim_services::Actor::new(actor),
            server_id,
            user_id: actor,
        })
        .await
        .map_err(crate::errors::map_service_error)?;

        let broadcaster = ctx.data_unchecked::<InMemoryBroadcaster>();
        let rx = broadcaster.subscribe();
        let ws_id = workspace_id.to_string();
        let subscriber_user_id = actor.to_string();

        // Use async_stream to handle Lagged errors gracefully without ending the stream.
        // BroadcastStream ends on Lagged, so we use the raw receiver directly.
        Ok(async_stream::stream! {
            let mut rx = rx;
            loop {
                match rx.recv().await {
                    Ok(event) => {
                        if event.workspace_id() == ws_id
                            && user_can_see_event(&event, &subscriber_user_id)
                            && let Ok(json) = serde_json::to_string(&event)
                        {
                            yield json;
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
        })
    }
}
