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

/// Whether `user_id` may see an event scoped to one conversation: public channels are visible to
/// every workspace member, private channels, DMs and groups only to their members.
fn conversation_event_visible(
    conversation_type: &str,
    is_private: bool,
    member_user_ids: &[UserId],
    user_id: &str,
) -> bool {
    (conversation_type == "Channel" && !is_private)
        || member_user_ids.iter().any(|id| id.to_string() == user_id)
}

/// Check whether a subscriber should see a given workspace event.
///
/// - Public channel events are visible to all workspace members.
/// - Private channel, DM and group events are only visible to conversation members.
/// - Conversation membership events (MemberAddedToConversation,
///   MemberRemovedFromConversation) are visible to the conversation's members (plus the removed
///   user); MemberJoined and PresenceChanged to all workspace members.
fn user_can_see_event(event: &WorkspaceEvent, user_id: &str) -> bool {
    match event {
        // Conversation-scoped events — public channel, else membership.
        WorkspaceEvent::NewMessage {
            conversation_type,
            is_private,
            member_user_ids,
            ..
        }
        | WorkspaceEvent::MessageUpdated {
            conversation_type,
            is_private,
            member_user_ids,
            ..
        }
        | WorkspaceEvent::MessageDeleted {
            conversation_type,
            is_private,
            member_user_ids,
            ..
        }
        | WorkspaceEvent::ReactionsUpdated {
            conversation_type,
            is_private,
            member_user_ids,
            ..
        }
        | WorkspaceEvent::ConversationCreated {
            conversation_type,
            is_private,
            member_user_ids,
            ..
        }
        | WorkspaceEvent::ThreadCreated {
            conversation_type,
            is_private,
            member_user_ids,
            ..
        }
        | WorkspaceEvent::ConversationUpdated {
            conversation_type,
            is_private,
            member_user_ids,
            ..
        }
        | WorkspaceEvent::TypingStarted {
            conversation_type,
            is_private,
            member_user_ids,
            ..
        }
        // Pin events: a public channel's member list only covers those who joined it, so the
        // channel bypass is what makes channel pins reach every workspace member.
        | WorkspaceEvent::MessagePinned {
            conversation_type,
            is_private,
            member_user_ids,
            ..
        }
        | WorkspaceEvent::MessageUnpinned {
            conversation_type,
            is_private,
            member_user_ids,
            ..
        }
        | WorkspaceEvent::LinkPreviewReady {
            conversation_type,
            is_private,
            member_user_ids,
            ..
        }
        | WorkspaceEvent::IncidentDeclared {
            conversation_type,
            is_private,
            member_user_ids,
            ..
        }
        | WorkspaceEvent::IncidentUpdated {
            conversation_type,
            is_private,
            member_user_ids,
            ..
        }
        | WorkspaceEvent::EscalationStepFired {
            conversation_type,
            is_private,
            member_user_ids,
            ..
        }
        | WorkspaceEvent::IncidentAcknowledged {
            conversation_type,
            is_private,
            member_user_ids,
            ..
        } => conversation_event_visible(conversation_type, *is_private, member_user_ids, user_id),

        // ReadStateUpdated — only visible to the owning user
        WorkspaceEvent::ReadStateUpdated {
            user_id: event_user_id,
            ..
        } => event_user_id.to_string() == user_id,

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
    use rim_core::escalation::model::EscalationPolicyId;
    use rim_core::incident::model::IncidentId;
    use rim_core::message::model::MessageId;
    use rim_core::message_thread::model::MessageThreadId;
    use rim_core::server::model::ServerId;
    use rim_services::events::MessageEventData;

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
            is_private: false,
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
            is_private: false,
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
            is_private: false,
        };

        assert!(user_can_see_event(&event, &member_id.to_string()));
        assert!(!user_can_see_event(&event, &non_member_id.to_string()));
    }

    // ── Track 1.1 visibility matrix ──
    //
    // Every variant that carries `conversation_type`: a public channel's event reaches every
    // workspace member; a private channel's only its members; DM/Group events only members.

    /// One instance of every conversation-scoped variant, all for the same conversation.
    fn conversation_scoped_events(
        conversation_type: &str,
        is_private: bool,
        member_user_ids: Vec<UserId>,
    ) -> Vec<WorkspaceEvent> {
        let workspace_id = ServerId::new();
        let conversation_id = ConversationId::new();
        let conversation_type = conversation_type.to_string();
        let message = MessageEventData {
            id: MessageId::new(),
            server_id: workspace_id,
            conversation_id,
            thread_id: None,
            root_thread_id: None,
            author_id: UserId::new(),
            text: Some("hi".to_string()),
            has_attachments: false,
            parent_message_id: None,
            forwarded_from_message_id: None,
            is_deleted: false,
            sent_at: chrono::Utc::now(),
            reactions: vec![],
        };
        let m = member_user_ids;
        let t = conversation_type;
        vec![
            WorkspaceEvent::NewMessage {
                workspace_id,
                conversation_id,
                conversation_type: t.clone(),
                member_user_ids: m.clone(),
                is_private,
                message: message.clone(),
            },
            WorkspaceEvent::MessageUpdated {
                workspace_id,
                conversation_id,
                conversation_type: t.clone(),
                member_user_ids: m.clone(),
                is_private,
                message,
            },
            WorkspaceEvent::MessageDeleted {
                workspace_id,
                conversation_id,
                conversation_type: t.clone(),
                member_user_ids: m.clone(),
                is_private,
                message_id: MessageId::new(),
                thread_id: None,
            },
            WorkspaceEvent::ReactionsUpdated {
                workspace_id,
                conversation_id,
                conversation_type: t.clone(),
                member_user_ids: m.clone(),
                is_private,
                message_id: MessageId::new(),
                reactions: vec![],
            },
            WorkspaceEvent::ConversationCreated {
                workspace_id,
                conversation_id,
                title: None,
                conversation_type: t.clone(),
                member_user_ids: m.clone(),
                is_private,
            },
            WorkspaceEvent::ThreadCreated {
                workspace_id,
                conversation_id,
                thread_id: MessageThreadId::new(),
                root_message_id: MessageId::new(),
                conversation_type: t.clone(),
                member_user_ids: m.clone(),
                is_private,
            },
            WorkspaceEvent::ConversationUpdated {
                workspace_id,
                conversation_id,
                conversation_type: t.clone(),
                is_private,
                member_user_ids: m.clone(),
            },
            WorkspaceEvent::MessagePinned {
                workspace_id,
                conversation_id,
                message_id: MessageId::new(),
                pinned_by: UserId::new(),
                conversation_type: t.clone(),
                member_user_ids: m.clone(),
                is_private,
            },
            WorkspaceEvent::MessageUnpinned {
                workspace_id,
                conversation_id,
                message_id: MessageId::new(),
                conversation_type: t.clone(),
                member_user_ids: m.clone(),
                is_private,
            },
            WorkspaceEvent::TypingStarted {
                workspace_id,
                conversation_id,
                user_id: UserId::new(),
                conversation_type: t.clone(),
                member_user_ids: m.clone(),
                is_private,
            },
            WorkspaceEvent::LinkPreviewReady {
                workspace_id,
                conversation_id,
                message_id: MessageId::new(),
                conversation_type: t.clone(),
                is_private,
                member_user_ids: m.clone(),
            },
            WorkspaceEvent::IncidentDeclared {
                workspace_id,
                conversation_id,
                conversation_type: t.clone(),
                incident_id: IncidentId::new(),
                severity: None,
                title: "outage".to_string(),
                member_user_ids: m.clone(),
                is_private,
            },
            WorkspaceEvent::IncidentUpdated {
                workspace_id,
                conversation_id,
                conversation_type: t.clone(),
                incident_id: IncidentId::new(),
                status: "resolved".to_string(),
                member_user_ids: m.clone(),
                is_private,
            },
            WorkspaceEvent::EscalationStepFired {
                workspace_id,
                conversation_id,
                conversation_type: t.clone(),
                incident_id: IncidentId::new(),
                escalation_policy_id: EscalationPolicyId::new(),
                step_index: 0,
                target_user_ids: vec![],
                member_user_ids: m.clone(),
                is_private,
            },
            WorkspaceEvent::IncidentAcknowledged {
                workspace_id,
                conversation_id,
                conversation_type: t,
                incident_id: IncidentId::new(),
                acknowledged_by: UserId::new(),
                member_user_ids: m,
                is_private,
            },
        ]
    }

    /// Spec case: alice posts in a private channel with bob; carol (a workspace member, subscribed
    /// to the same workspace) must not receive the `NewMessage`.
    #[test]
    fn private_channel_new_message_not_delivered_to_non_member() {
        let alice = UserId::new();
        let bob = UserId::new();
        let carol = UserId::new();
        let new_message = conversation_scoped_events("Channel", true, vec![alice, bob])
            .into_iter()
            .find(|e| matches!(e, WorkspaceEvent::NewMessage { .. }))
            .expect("NewMessage");
        assert!(user_can_see_event(&new_message, &alice.to_string()));
        assert!(user_can_see_event(&new_message, &bob.to_string()));
        assert!(!user_can_see_event(&new_message, &carol.to_string()));
    }

    /// The fixture must cover every conversation-scoped variant; a new one has to be added here.
    #[test]
    fn visibility_fixture_covers_every_conversation_scoped_variant() {
        assert_eq!(
            conversation_scoped_events("Channel", false, vec![]).len(),
            15
        );
    }

    #[test]
    fn public_channel_events_visible_to_non_member_workspace_members() {
        let member = UserId::new();
        let non_member = UserId::new();
        for event in conversation_scoped_events("Channel", false, vec![member]) {
            assert!(user_can_see_event(&event, &member.to_string()), "{event:?}");
            assert!(
                user_can_see_event(&event, &non_member.to_string()),
                "public channel event hidden from a non-member: {event:?}"
            );
        }
    }

    #[test]
    fn private_channel_events_visible_to_members_only() {
        let member = UserId::new();
        let non_member = UserId::new();
        for event in conversation_scoped_events("Channel", true, vec![member]) {
            assert!(user_can_see_event(&event, &member.to_string()), "{event:?}");
            assert!(
                !user_can_see_event(&event, &non_member.to_string()),
                "private channel event leaked to a non-member: {event:?}"
            );
        }
    }

    #[test]
    fn direct_and_group_events_visible_to_members_only() {
        let member = UserId::new();
        let non_member = UserId::new();
        for kind in ["Direct", "Group"] {
            for event in conversation_scoped_events(kind, false, vec![member]) {
                assert!(user_can_see_event(&event, &member.to_string()), "{event:?}");
                assert!(
                    !user_can_see_event(&event, &non_member.to_string()),
                    "{kind} event leaked to a non-member: {event:?}"
                );
            }
        }
    }

    /// The `is_private` flag (like `member_user_ids`) never reaches the wire: the event JSON is a
    /// frozen cross-repo contract.
    #[test]
    fn visibility_fields_are_not_serialized() {
        for event in conversation_scoped_events("Channel", true, vec![UserId::new()]) {
            let json = serde_json::to_value(&event).expect("serialize");
            let obj = json.as_object().expect("object");
            assert!(!obj.contains_key("is_private"), "{json}");
            assert!(!obj.contains_key("member_user_ids"), "{json}");
        }
        let updated = conversation_scoped_events("Channel", true, vec![UserId::new()])
            .into_iter()
            .find(|e| matches!(e, WorkspaceEvent::ConversationUpdated { .. }))
            .expect("ConversationUpdated");
        let json = serde_json::to_value(&updated).expect("serialize");
        let mut keys: Vec<_> = json.as_object().expect("object").keys().cloned().collect();
        keys.sort();
        assert_eq!(keys, ["conversation_id", "type", "workspace_id"]);
    }
}
