use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context as TaskContext, Poll};

use async_graphql::*;
use futures_util::Stream;
use rim_core::user::model::UserId;
use rim_services::events::WorkspaceEvent;
use rim_services::presence::PresenceService;
use rim_services::workspace_state::dto::WorkspaceStateScopeDto;

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

        // ReadStateUpdated — only visible to the owning user
        WorkspaceEvent::ReadStateUpdated {
            user_id: event_user_id,
            ..
        } => event_user_id.to_string() == user_id,

        // ConversationUpdated is visible to all workspace members
        WorkspaceEvent::ConversationUpdated { .. } => true,

        // TypingStarted — filter by conversation type like messages
        WorkspaceEvent::TypingStarted {
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

        // Pin events — filter by conversation type like messages: channels are visible to all
        // workspace members (channels have no conversation_member rows, so member_user_ids
        // would be empty there), DM/Group pins only to conversation members.
        WorkspaceEvent::MessagePinned {
            conversation_type,
            member_user_ids,
            ..
        }
        | WorkspaceEvent::MessageUnpinned {
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
        WorkspaceEvent::MemberJoined { .. } | WorkspaceEvent::PresenceChanged { .. } => true,

        // Conversation membership events: only visible to conversation members
        WorkspaceEvent::MemberAddedToConversation {
            member_user_ids, ..
        }
        | WorkspaceEvent::MemberRemovedFromConversation {
            member_user_ids, ..
        } => member_user_ids.iter().any(|id| id.to_string() == user_id),

        // UserMentioned is only visible to mentioned users
        WorkspaceEvent::UserMentioned {
            mentioned_user_ids, ..
        } => mentioned_user_ids
            .iter()
            .any(|id| id.to_string() == user_id),

        // LinkPreviewReady is visible to all workspace members
        WorkspaceEvent::LinkPreviewReady { .. } => true,

        // WorkspaceStateChanged: server-scope visible to all, user-scope only to owning user
        WorkspaceEvent::WorkspaceStateChanged {
            scope,
            user_id: event_user_id,
            ..
        } => match scope {
            WorkspaceStateScopeDto::Server => true,
            WorkspaceStateScopeDto::User => event_user_id
                .as_ref()
                .is_some_and(|uid| uid.to_string() == user_id),
        },

        // WorkspaceStateDeleted: server-scope visible to all, user-scope only to owning user
        WorkspaceEvent::WorkspaceStateDeleted {
            scope,
            user_id: event_user_id,
            ..
        } => match scope {
            WorkspaceStateScopeDto::Server => true,
            WorkspaceStateScopeDto::User => event_user_id
                .as_ref()
                .is_some_and(|uid| uid.to_string() == user_id),
        },

        // Pipeline events are visible to all workspace members.
        WorkspaceEvent::PipelineFired { .. } | WorkspaceEvent::PipelineSuspended { .. } => true,

        // Incident events — filter by conversation type like messages: channels are visible to
        // all workspace members (channels have no conversation_member rows, so member_user_ids
        // would be empty there), DM/Group-scoped incidents only to conversation members.
        WorkspaceEvent::IncidentDeclared {
            conversation_type,
            member_user_ids,
            ..
        }
        | WorkspaceEvent::IncidentUpdated {
            conversation_type,
            member_user_ids,
            ..
        }
        | WorkspaceEvent::EscalationStepFired {
            conversation_type,
            member_user_ids,
            ..
        }
        | WorkspaceEvent::IncidentAcknowledged {
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

        let presence = ctx.data::<Arc<dyn PresenceService>>()?;
        let broadcaster = ctx.data::<InMemoryBroadcaster>()?;
        let rx = broadcaster.subscribe();
        let ws_id = workspace_id.to_string();
        let subscriber_user_id = actor.to_string();

        // Use async_stream to handle Lagged errors gracefully without ending the stream.
        // BroadcastStream ends on Lagged, so we use the raw receiver directly.
        let inner = async_stream::stream! {
            let mut rx = rx;
            loop {
                match rx.recv().await {
                    Ok(event) => {
                        if event.workspace_id() == ws_id
                            && user_can_see_event(&event, &subscriber_user_id)
                            && let Ok(json) = serde_json::to_string(&event) {
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
        };

        // Wrap the stream so mark_offline is called when the client disconnects.
        Ok(DisconnectGuardStream {
            inner: Box::pin(inner),
            presence: Arc::clone(presence),
            user_id: actor,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rim_core::conversation::model::ConversationId;
    use rim_core::message::model::MessageId;
    use rim_core::server::model::ServerId;

    /// Regression guard for the channel-pin visibility bug
    /// (`docs/specs/fix-channel-pin-event-visibility.md`): a channel-scoped pin has no
    /// `conversation_member` rows, so `member_user_ids` is always empty — before the fix, this
    /// meant `MessagePinned`/`MessageUnpinned` never reached any subscriber for a channel. This
    /// test fails on the pre-fix filter (bare `member_user_ids.iter().any(...)` over an empty
    /// vec is always `false`).
    #[test]
    fn channel_pin_visible_despite_empty_member_user_ids() {
        let event = WorkspaceEvent::MessagePinned {
            workspace_id: ServerId::new(),
            conversation_id: ConversationId::new(),
            message_id: MessageId::new(),
            pinned_by: UserId::new(),
            conversation_type: "Channel".to_string(),
            member_user_ids: vec![],
        };

        assert!(user_can_see_event(&event, &UserId::new().to_string()));
    }

    #[test]
    fn channel_unpin_visible_despite_empty_member_user_ids() {
        let event = WorkspaceEvent::MessageUnpinned {
            workspace_id: ServerId::new(),
            conversation_id: ConversationId::new(),
            message_id: MessageId::new(),
            conversation_type: "Channel".to_string(),
            member_user_ids: vec![],
        };

        assert!(user_can_see_event(&event, &UserId::new().to_string()));
    }

    /// Symmetric case: a Group-scoped pin is still gated on membership — the channel bypass
    /// must not accidentally widen visibility for DM/Group pins.
    #[test]
    fn group_pin_not_visible_to_non_member() {
        let member_id = UserId::new();
        let non_member_id = UserId::new();

        let event = WorkspaceEvent::MessagePinned {
            workspace_id: ServerId::new(),
            conversation_id: ConversationId::new(),
            message_id: MessageId::new(),
            pinned_by: member_id,
            conversation_type: "Group".to_string(),
            member_user_ids: vec![member_id],
        };

        assert!(user_can_see_event(&event, &member_id.to_string()));
        assert!(!user_can_see_event(&event, &non_member_id.to_string()));
    }
}
