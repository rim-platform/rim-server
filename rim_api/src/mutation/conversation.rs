use async_graphql::*;
use chrono::{DateTime, Utc};
use rim_services::Actor;
use rim_services::conversation::command::{
    AddConversationMembersCmd, ArchiveConversationCmd, CreateConversationCmd, JoinChannelCmd,
    LeaveConversationCmd, RemoveConversationMemberCmd, UpdateConversationTopicCmd,
};
use rim_services::conversation::service::ConversationService;

use crate::auth_context::require_actor;
use crate::errors::map_service_error;
use crate::ids::parse_uuid_id;
use crate::types::Conversation;

#[derive(Default)]
pub(crate) struct ConversationMutations;

#[Object]
impl ConversationMutations {
    async fn create_conversation(
        &self,
        ctx: &Context<'_>,
        input: CreateConversationInput,
    ) -> Result<Conversation> {
        let actor = require_actor(ctx)?;

        let svc = ctx.data::<std::sync::Arc<dyn ConversationService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&input.server_id)?;

        let remote_ids = remote_member_ids(ctx, actor, server_id, &input).await?;
        let participants = if input.is_channel.unwrap_or(false) {
            rim_services::conversation::types::ConversationParticipants::Channel
        } else if let Some(other_user_id) = &input.other_user_id {
            if !remote_ids.is_empty() {
                return Err(Error::new(
                    "a DM names otherUserId or one remote member, not both",
                ));
            }
            rim_services::conversation::types::ConversationParticipants::DirectMessage {
                other_user_id: parse_uuid_id::<rim_core::user::model::UserId>(other_user_id)?,
            }
        } else if input.member_ids.is_none()
            && input.remote_members.as_ref().map_or(0, Vec::len) == 1
            && remote_ids.len() == 1
        {
            rim_services::conversation::types::ConversationParticipants::DirectMessage {
                other_user_id: remote_ids[0],
            }
        } else {
            let member_ids = input.member_ids.clone().unwrap_or_default();
            let mut ids = Vec::with_capacity(member_ids.len() + remote_ids.len());
            for id in member_ids {
                ids.push(parse_uuid_id::<rim_core::user::model::UserId>(&id)?);
            }
            ids.extend(remote_ids);
            rim_services::conversation::types::ConversationParticipants::Group { member_ids: ids }
        };

        let conv = svc
            .create_conversation(CreateConversationCmd {
                actor: Actor::new(actor),
                server_id,
                participants,
                title: input.title,
                is_private: input.is_private,
                encrypted: input.encrypted.unwrap_or(false),
            })
            .await
            .map_err(map_service_error)?;

        if let Some(predecessor) = input.predecessor {
            let federation = crate::federation::direct_space(ctx)?;
            let predecessor_id = parse_uuid_id::<rim_core::conversation::model::ConversationId>(
                &predecessor.conversation_id,
            )?;
            federation
                .host
                .set_predecessor(conv.id, predecessor.host_server_id, predecessor_id.into())
                .await
                .map_err(map_service_error)?;
        }

        Ok(conv.into())
    }

    async fn update_conversation_topic(
        &self,
        ctx: &Context<'_>,
        input: UpdateConversationTopicInput,
    ) -> Result<Conversation> {
        let actor = require_actor(ctx)?;

        let svc = ctx.data::<std::sync::Arc<dyn ConversationService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&input.server_id)?;
        let conversation_id =
            parse_uuid_id::<rim_core::conversation::model::ConversationId>(&input.conversation_id)?;

        let conv = svc
            .update_topic(UpdateConversationTopicCmd {
                actor: Actor::new(actor),
                server_id,
                conversation_id,
                topic: input.topic,
            })
            .await
            .map_err(map_service_error)?;

        Ok(conv.into())
    }

    /// Join a public, unarchived channel. Idempotent.
    async fn join_channel(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
        conversation_id: ID,
    ) -> Result<Conversation> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn ConversationService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;
        let conversation_id =
            parse_uuid_id::<rim_core::conversation::model::ConversationId>(&conversation_id)?;

        let conv = svc
            .join_channel(JoinChannelCmd {
                actor: Actor::new(actor),
                server_id,
                conversation_id,
            })
            .await
            .map_err(map_service_error)?;

        Ok(conv.into())
    }

    /// Leave a channel or group (not a DM).
    async fn leave_conversation(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
        conversation_id: ID,
    ) -> Result<bool> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn ConversationService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;
        let conversation_id =
            parse_uuid_id::<rim_core::conversation::model::ConversationId>(&conversation_id)?;

        svc.leave_conversation(LeaveConversationCmd {
            actor: Actor::new(actor),
            server_id,
            conversation_id,
        })
        .await
        .map_err(map_service_error)?;

        Ok(true)
    }

    /// Add (invite) workspace members to a channel or group the caller belongs to.
    async fn add_conversation_members(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
        conversation_id: ID,
        user_ids: Vec<ID>,
    ) -> Result<Conversation> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn ConversationService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;
        let conversation_id =
            parse_uuid_id::<rim_core::conversation::model::ConversationId>(&conversation_id)?;
        let mut ids = Vec::with_capacity(user_ids.len());
        for id in &user_ids {
            ids.push(parse_uuid_id::<rim_core::user::model::UserId>(id)?);
        }

        let conv = svc
            .add_conversation_members(AddConversationMembersCmd {
                actor: Actor::new(actor),
                server_id,
                conversation_id,
                user_ids: ids,
            })
            .await
            .map_err(map_service_error)?;

        Ok(conv.into())
    }

    /// Remove a member from a channel or group: Owner/Moderator may remove anyone, others only
    /// themselves.
    async fn remove_conversation_member(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
        conversation_id: ID,
        user_id: ID,
    ) -> Result<bool> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn ConversationService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;
        let conversation_id =
            parse_uuid_id::<rim_core::conversation::model::ConversationId>(&conversation_id)?;
        let user_id = parse_uuid_id::<rim_core::user::model::UserId>(&user_id)?;

        svc.remove_conversation_member(RemoveConversationMemberCmd {
            actor: Actor::new(actor),
            server_id,
            conversation_id,
            user_id,
        })
        .await
        .map_err(map_service_error)?;

        Ok(true)
    }

    /// Archive or unarchive a channel (Owner/Moderator). Archived channels are read-only.
    async fn set_conversation_archived(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
        conversation_id: ID,
        archived: bool,
    ) -> Result<Conversation> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn ConversationService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;
        let conversation_id =
            parse_uuid_id::<rim_core::conversation::model::ConversationId>(&conversation_id)?;

        let conv = svc
            .set_conversation_archived(ArchiveConversationCmd {
                actor: Actor::new(actor),
                server_id,
                conversation_id,
                archived,
            })
            .await
            .map_err(map_service_error)?;

        Ok(conv.into())
    }
}

#[derive(Default)]
pub(crate) struct ReadStateMutations;

#[Object]
impl ReadStateMutations {
    async fn mark_as_read(&self, ctx: &Context<'_>, input: MarkAsReadInput) -> Result<bool> {
        let actor = require_actor(ctx)?;
        let svc =
            ctx.data::<std::sync::Arc<dyn rim_services::read_state::service::ReadStateService>>()?;
        let workspace_id = parse_uuid_id::<rim_core::server::model::ServerId>(&input.workspace_id)?;
        let conversation_id =
            parse_uuid_id::<rim_core::conversation::model::ConversationId>(&input.conversation_id)?;

        svc.mark_as_read(rim_services::read_state::dto::MarkAsReadCmd {
            actor: Actor::new(actor),
            workspace_id,
            conversation_id,
            read_at: input.read_at,
        })
        .await
        .map_err(map_service_error)?;

        Ok(true)
    }

    async fn mark_all_as_read(&self, ctx: &Context<'_>, input: MarkAllAsReadInput) -> Result<bool> {
        let actor = require_actor(ctx)?;
        let svc =
            ctx.data::<std::sync::Arc<dyn rim_services::read_state::service::ReadStateService>>()?;
        let workspace_id = parse_uuid_id::<rim_core::server::model::ServerId>(&input.workspace_id)?;

        svc.mark_all_as_read(rim_services::read_state::dto::MarkAllAsReadCmd {
            actor: Actor::new(actor),
            workspace_id,
        })
        .await
        .map_err(map_service_error)?;

        Ok(true)
    }
}

#[derive(Default)]
pub(crate) struct ConversationMuteMutations;

#[Object]
impl ConversationMuteMutations {
    // ── Mute / Unmute ────────────────────────────────────────────────────

    async fn mute_conversation(
        &self,
        ctx: &Context<'_>,
        input: MuteConversationInput,
    ) -> Result<bool> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<
            dyn rim_services::notification_preference::service::NotificationPreferenceService,
        >>()?;

        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&input.server_id)?;
        let conversation_id =
            parse_uuid_id::<rim_core::conversation::model::ConversationId>(&input.conversation_id)?;

        svc.mute_conversation(
            rim_services::notification_preference::command::MuteConversationCmd {
                actor: Actor::new(actor),
                server_id,
                conversation_id,
                muted_until: input.muted_until,
            },
        )
        .await
        .map_err(map_service_error)
    }

    async fn unmute_conversation(
        &self,
        ctx: &Context<'_>,
        input: UnmuteConversationInput,
    ) -> Result<bool> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<
            dyn rim_services::notification_preference::service::NotificationPreferenceService,
        >>()?;

        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&input.server_id)?;
        let conversation_id =
            parse_uuid_id::<rim_core::conversation::model::ConversationId>(&input.conversation_id)?;

        svc.unmute_conversation(
            rim_services::notification_preference::command::UnmuteConversationCmd {
                actor: Actor::new(actor),
                server_id,
                conversation_id,
            },
        )
        .await
        .map_err(map_service_error)
    }
}

#[derive(InputObject)]
pub struct CreateConversationInput {
    pub server_id: ID,

    /// For DM conversations.
    pub other_user_id: Option<ID>,

    /// For Group conversations.
    pub member_ids: Option<Vec<ID>>,

    /// For Channel conversations.
    pub is_channel: Option<bool>,

    pub title: Option<String>,

    /// Channels only: visible to invited members alone. Cannot be changed later.
    #[graphql(default)]
    pub is_private: bool,

    /// DMs and groups only: end-to-end encrypted from creation. Every member needs a device set
    /// up for encryption. Defaults to false.
    pub encrypted: Option<bool>,

    /// People from other servers, in the direct space only. With no `otherUserId` or
    /// `memberIds` and one remote member, the conversation is a DM with them.
    pub remote_members: Option<Vec<crate::types::RemoteMemberInput>>,

    /// The conversation this one succeeds, when its home is gone.
    pub predecessor: Option<crate::types::PredecessorInput>,
}

#[derive(InputObject)]
pub struct UpdateConversationTopicInput {
    pub server_id: ID,
    pub conversation_id: ID,
    pub topic: Option<String>,
}

#[derive(InputObject)]
pub struct MarkAsReadInput {
    pub workspace_id: ID,
    pub conversation_id: ID,
    pub read_at: DateTime<Utc>,
}

#[derive(InputObject)]
pub struct MarkAllAsReadInput {
    pub workspace_id: ID,
}

#[derive(InputObject)]
struct MuteConversationInput {
    server_id: ID,
    conversation_id: ID,
    muted_until: Option<DateTime<Utc>>,
}

#[derive(InputObject)]
struct UnmuteConversationInput {
    server_id: ID,
    conversation_id: ID,
}

/// The users for a new conversation's `remoteMembers`: each a user of this server or fetched
/// from their relays, and made a member of the direct space. They need the direct space and a
/// caller with a published profile card, which the request inbox shows; a conversation with
/// someone on another relay is created encrypted only.
async fn remote_member_ids(
    ctx: &Context<'_>,
    actor: rim_core::user::model::UserId,
    server_id: rim_core::server::model::ServerId,
    input: &CreateConversationInput,
) -> Result<Vec<rim_core::user::model::UserId>> {
    let members = match &input.remote_members {
        Some(members) if !members.is_empty() => members,
        _ => return Ok(Vec::new()),
    };
    let federation = crate::federation::direct_space(ctx)?;
    if server_id != federation.host.direct_space_id() {
        return Err(Error::new(
            "people from other servers join conversations in the direct space",
        ));
    }
    let key_auth =
        ctx.data::<std::sync::Arc<dyn rim_services::key_auth::service::KeyAuthService>>()?;
    let identity = key_auth
        .get_my_key_identity(rim_services::key_auth::command::GetMyKeyIdentityQuery {
            actor: Actor::new(actor),
        })
        .await
        .map_err(map_service_error)?;
    if identity.profile_card.is_none() {
        return Err(Error::new(
            "publish a profile card before starting a conversation with someone on another server",
        ));
    }
    let mut ids = Vec::with_capacity(members.len());
    let mut any_remote = false;
    for member in members {
        let resolved = federation
            .host
            .ensure_remote_member(
                Actor::new(actor),
                rim_services::host::dto::RemoteMemberRef {
                    principal_id: member.principal_id.clone(),
                    relay_urls: member.relay_urls.clone(),
                },
            )
            .await
            .map_err(map_service_error)?;
        any_remote |= resolved.remote;
        if resolved.user_id != actor && !ids.contains(&resolved.user_id) {
            ids.push(resolved.user_id);
        }
    }
    if any_remote && !input.encrypted.unwrap_or(false) {
        return Err(map_service_error(rim_services::ServiceError::Validation(
            "a conversation with someone on another relay is encrypted".to_string(),
        )));
    }
    Ok(ids)
}
