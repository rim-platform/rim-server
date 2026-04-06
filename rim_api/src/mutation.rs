use async_graphql::*;
use base64::Engine;
use chrono::{DateTime, Utc};
use rim_services::Actor;
use rim_services::auth::command::{
    AcceptInviteCmd, BootstrapCmd, CreateInviteCmd, JoinServerByInviteCmd, LoginCmd, LogoutCmd,
    RevokeInviteCmd,
};
use rim_services::auth::service::AuthService;
use rim_services::command::dto::UserCommandStepDto;
use rim_services::command::request::{CreateUserCommandCmd, ExecuteCommandCmd};
use rim_services::command::request::{DeleteUserCommandCmd, UpdateUserCommandCmd};
use rim_services::command::service::CommandService;
use rim_services::conversation::command::{CreateConversationCmd, UpdateConversationTopicCmd};
use rim_services::conversation::service::ConversationService;
use rim_services::keybind::request::{CreateKeybindCmd, DeleteKeybindCmd, UpdateKeybindCmd};
use rim_services::keybind::service::KeybindService;
use rim_services::macro_def::dto::MacroStepDto;
use rim_services::macro_def::request::{
    CreateMacroCmd, DeleteMacroCmd, ExecuteMacroCmd, UpdateMacroCmd,
};
use rim_services::macro_def::service::MacroService;
use rim_services::message::command::{DeleteMessageCmd, EditMessageCmd};
use rim_services::message::command::{ReplyToMessageCmd, SendMessageCmd};
use rim_services::message::service::MessageService;
use rim_services::message_attachment::command::{
    CreateAttachmentCmd, DeleteAttachmentCmd, FinalizeAttachmentCmd,
};
use rim_services::message_attachment::service::MessageAttachmentService;
use rim_services::message_reaction::command::ToggleReactionCmd;
use rim_services::message_reaction::service::MessageReactionService;
use rim_services::pin::command::{PinMessageCmd, UnpinMessageCmd};
use rim_services::pin::service::PinService;
use rim_services::server::command::CreateServerCmd;
use rim_services::server::service::ServerService;

use crate::auth_context::{require_actor, require_session_id};
use crate::auth_intent::{AuthIntent, AuthIntentCtx};
use crate::errors::map_service_error;
use crate::ids::parse_uuid_id;
use crate::types::{
    Attachment, Command, CommandContextInput, CommandResult, Conversation, Keybind, KeybindContext,
    KeybindScope, MacroDef, Message, PresenceStatus, ReactionGroup, Server, UploadTicket,
};

pub struct Mutation;

#[derive(InputObject)]
pub struct BootstrapInput {
    pub secret: String,
    pub username: String,
    pub password: String,
    pub server_name: String,
}

#[derive(SimpleObject)]
pub struct BootstrapPayload {
    pub user_id: ID,
    pub server_id: ID,
}

#[derive(InputObject)]
pub struct LoginInput {
    pub username: String,
    pub password: String,
}

#[derive(SimpleObject)]
pub struct LoginPayload {
    pub user_id: ID,
}

#[derive(InputObject)]
pub struct CreateInviteInput {
    pub server_id: ID,
    pub expires_at: Option<DateTime<Utc>>,
    pub max_uses: Option<i32>,
}

#[derive(SimpleObject)]
pub struct CreateInvitePayload {
    pub invite_token: String,
    pub invite_id: ID,
}

#[derive(InputObject)]
pub struct AcceptInviteInput {
    pub invite_token: String,
    pub username: String,
    pub password: String,
}

#[derive(SimpleObject)]
pub struct AcceptInvitePayload {
    pub user_id: ID,
    pub server_id: ID,
}

#[derive(InputObject)]
pub struct JoinServerByInviteInput {
    pub invite_token: String,
}

#[derive(SimpleObject)]
pub struct JoinServerByInvitePayload {
    pub server_id: ID,
}

#[derive(InputObject)]
pub struct SendMessageInput {
    pub server_id: ID,
    pub conversation_id: ID,
    pub text: String,
    pub parent_message_id: Option<ID>,
}

#[derive(InputObject)]
pub struct ReplyToMessageInput {
    pub server_id: ID,
    pub conversation_id: ID,
    pub root_message_id: ID,
    pub text: String,
    pub parent_message_id: Option<ID>,
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
}

#[derive(InputObject)]
pub struct UpdateConversationTopicInput {
    pub server_id: ID,
    pub conversation_id: ID,
    pub topic: Option<String>,
}

#[derive(InputObject)]
pub struct EditMessageInput {
    pub server_id: ID,
    pub conversation_id: ID,
    pub message_id: ID,
    pub text: String,
}

#[derive(InputObject)]
pub struct ForwardMessageInput {
    pub server_id: ID,
    pub source_conversation_id: ID,
    pub target_conversation_id: ID,
    pub message_id: ID,
}

#[derive(InputObject)]
pub struct DeleteMessageInput {
    pub server_id: ID,
    pub conversation_id: ID,
    pub message_id: ID,
}

#[derive(InputObject)]
pub struct CreateAttachmentInput {
    pub server_id: ID,
    pub conversation_id: ID,
    pub message_id: Option<ID>,
    pub filename: String,
    pub content_type: String,
    pub size_bytes: u64,
}

#[derive(InputObject)]
pub struct FinalizeAttachmentInput {
    pub server_id: ID,
    pub attachment_id: ID,
}

#[derive(InputObject)]
pub struct DeleteAttachmentInput {
    pub server_id: ID,
    pub attachment_id: ID,
}

#[derive(InputObject)]
pub struct ExecuteCommandInput {
    pub command_id: String,
    pub args: Option<async_graphql::Json<serde_json::Value>>,
    pub command_context: Option<CommandContextInput>,
    pub confirmed_dangerous: Option<bool>,
}

#[derive(InputObject)]
pub struct UserCommandStepInput {
    pub command_id: String,
    pub args: Option<async_graphql::Json<serde_json::Value>>,
}

#[derive(InputObject)]
pub struct CreateUserCommandInput {
    pub command_id: String,
    pub title: String,
    pub description: String,
    pub steps: Vec<UserCommandStepInput>,
}

#[derive(InputObject)]
pub struct UpdateUserCommandInput {
    pub command_id: String,
    pub title: String,
    pub description: String,
    pub steps: Vec<UserCommandStepInput>,
    pub is_enabled: bool,
}

#[derive(InputObject)]
pub struct DeleteUserCommandInput {
    pub command_id: String,
}

#[derive(InputObject)]
pub struct CreateKeybindInput {
    pub scope: KeybindScope,
    pub workspace_id: Option<ID>,
    pub keystroke: String,
    pub command_id: String,
    pub context: KeybindContext,
    pub preset_args: Option<async_graphql::Json<serde_json::Value>>,
}

#[derive(InputObject)]
pub struct UpdateKeybindInput {
    pub keybind_id: ID,
    pub keystroke: String,
    pub command_id: String,
    pub context: KeybindContext,
    pub preset_args: Option<async_graphql::Json<serde_json::Value>>,
    pub is_enabled: bool,
}

#[derive(InputObject)]
pub struct DeleteKeybindInput {
    pub keybind_id: ID,
}

#[derive(InputObject)]
pub struct PinMessageInput {
    pub server_id: ID,
    pub conversation_id: ID,
    pub message_id: ID,
}

#[derive(InputObject)]
pub struct UnpinMessageInput {
    pub server_id: ID,
    pub conversation_id: ID,
    pub message_id: ID,
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
pub struct HeartbeatInput {
    pub workspace_id: ID,
    pub status: PresenceStatus,
}

#[derive(InputObject)]
pub struct CreateServerInput {
    pub name: String,
}

#[derive(InputObject)]
pub struct ToggleReactionInput {
    pub server_id: ID,
    pub conversation_id: ID,
    pub message_id: ID,
    pub emoji: String,
}

#[Object]
impl Mutation {
    async fn bootstrap(
        &self,
        ctx: &Context<'_>,
        input: BootstrapInput,
    ) -> Result<BootstrapPayload> {
        let auth = ctx.data::<std::sync::Arc<dyn AuthService>>()?;
        let res = auth
            .bootstrap(BootstrapCmd {
                secret: input.secret,
                username: input.username,
                password: input.password,
                server_name: input.server_name,
            })
            .await
            .map_err(map_service_error)?;

        if let Ok(intent) = ctx.data::<std::sync::Arc<AuthIntentCtx>>() {
            intent.set(AuthIntent::SetSession {
                session_token: res.session_token,
                csrf_token: res.csrf_token,
            });
        }

        Ok(BootstrapPayload {
            user_id: ID(res.user_id.to_string()),
            server_id: ID(res.server_id.to_string()),
        })
    }

    async fn login(&self, ctx: &Context<'_>, input: LoginInput) -> Result<LoginPayload> {
        let auth = ctx.data::<std::sync::Arc<dyn AuthService>>()?;
        let res = auth
            .login(LoginCmd {
                username: input.username,
                password: input.password,
            })
            .await
            .map_err(map_service_error)?;

        if let Ok(intent) = ctx.data::<std::sync::Arc<AuthIntentCtx>>() {
            intent.set(AuthIntent::SetSession {
                session_token: res.session_token,
                csrf_token: res.csrf_token,
            });
        }

        Ok(LoginPayload {
            user_id: ID(res.user_id.to_string()),
        })
    }

    async fn logout(&self, ctx: &Context<'_>) -> Result<bool> {
        let actor = require_actor(ctx)?;
        let session_id = require_session_id(ctx)?;

        let auth = ctx.data::<std::sync::Arc<dyn AuthService>>()?;
        auth.logout(LogoutCmd {
            actor: Actor::new(actor),
            session_id,
        })
        .await
        .map_err(map_service_error)?;

        if let Ok(intent) = ctx.data::<std::sync::Arc<AuthIntentCtx>>() {
            intent.set(AuthIntent::ClearSession);
        }
        Ok(true)
    }

    async fn create_invite(
        &self,
        ctx: &Context<'_>,
        input: CreateInviteInput,
    ) -> Result<CreateInvitePayload> {
        let actor = require_actor(ctx)?;

        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&input.server_id)?;
        let auth = ctx.data::<std::sync::Arc<dyn AuthService>>()?;
        let res = auth
            .create_invite(CreateInviteCmd {
                actor: Actor::new(actor),
                server_id,
                expires_at: input.expires_at,
                max_uses: input.max_uses.unwrap_or(1),
            })
            .await
            .map_err(map_service_error)?;

        Ok(CreateInvitePayload {
            invite_token: res.invite_token,
            invite_id: ID(res.invite.id.to_string()),
        })
    }

    async fn accept_invite(
        &self,
        ctx: &Context<'_>,
        input: AcceptInviteInput,
    ) -> Result<AcceptInvitePayload> {
        let auth = ctx.data::<std::sync::Arc<dyn AuthService>>()?;
        let res = auth
            .accept_invite(AcceptInviteCmd {
                invite_token: input.invite_token,
                username: input.username,
                password: input.password,
            })
            .await
            .map_err(map_service_error)?;

        if let Ok(intent) = ctx.data::<std::sync::Arc<AuthIntentCtx>>() {
            intent.set(AuthIntent::SetSession {
                session_token: res.session_token,
                csrf_token: res.csrf_token,
            });
        }

        Ok(AcceptInvitePayload {
            user_id: ID(res.user_id.to_string()),
            server_id: ID(res.server_id.to_string()),
        })
    }

    async fn join_server_by_invite(
        &self,
        ctx: &Context<'_>,
        input: JoinServerByInviteInput,
    ) -> Result<JoinServerByInvitePayload> {
        let actor = require_actor(ctx)?;

        let auth = ctx.data::<std::sync::Arc<dyn AuthService>>()?;
        let res = auth
            .join_server_by_invite(JoinServerByInviteCmd {
                actor: Actor::new(actor),
                invite_token: input.invite_token,
            })
            .await
            .map_err(map_service_error)?;

        Ok(JoinServerByInvitePayload {
            server_id: ID(res.server_id.to_string()),
        })
    }

    async fn revoke_invite(&self, ctx: &Context<'_>, invite_id: ID) -> Result<bool> {
        let actor = require_actor(ctx)?;
        let invite_id = parse_uuid_id::<rim_core::auth::model::InviteId>(&invite_id)?;

        let auth = ctx.data::<std::sync::Arc<dyn AuthService>>()?;
        auth.revoke_invite(RevokeInviteCmd {
            actor: Actor::new(actor),
            invite_id,
        })
        .await
        .map_err(map_service_error)?;

        Ok(true)
    }

    async fn send_message(&self, ctx: &Context<'_>, input: SendMessageInput) -> Result<Message> {
        let actor = require_actor(ctx)?;

        let svc = ctx.data::<std::sync::Arc<dyn MessageService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&input.server_id)?;
        let conversation_id =
            parse_uuid_id::<rim_core::conversation::model::ConversationId>(&input.conversation_id)?;
        let parent_message_id = match input.parent_message_id {
            None => None,
            Some(id) => Some(parse_uuid_id::<rim_core::message::model::MessageId>(&id)?),
        };

        let text_for_preview = input.text.clone();
        let msg = svc
            .send_message(SendMessageCmd {
                actor: Actor::new(actor),
                server_id,
                conversation_id,
                text: Some(input.text),
                parent_message_id,
                attachments: vec![],
            })
            .await
            .map_err(map_service_error)?;

        // Spawn background link preview fetch
        spawn_link_preview_fetch(ctx, msg.id, server_id, conversation_id, &text_for_preview)?;

        Ok(msg.into())
    }

    async fn reply_to_message(
        &self,
        ctx: &Context<'_>,
        input: ReplyToMessageInput,
    ) -> Result<Message> {
        let actor = require_actor(ctx)?;

        let svc = ctx.data::<std::sync::Arc<dyn MessageService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&input.server_id)?;
        let conversation_id =
            parse_uuid_id::<rim_core::conversation::model::ConversationId>(&input.conversation_id)?;
        let root_message_id =
            parse_uuid_id::<rim_core::message::model::MessageId>(&input.root_message_id)?;
        let parent_message_id = input
            .parent_message_id
            .as_ref()
            .map(parse_uuid_id::<rim_core::message::model::MessageId>)
            .transpose()?;

        let text_for_preview = input.text.clone();
        let msg = svc
            .reply_to_message(ReplyToMessageCmd {
                actor: Actor::new(actor),
                server_id,
                conversation_id,
                root_message_id,
                text: text_for_preview.clone(),
                parent_message_id,
                attachments: vec![],
            })
            .await
            .map_err(map_service_error)?;

        // Spawn background link preview fetch
        spawn_link_preview_fetch(ctx, msg.id, server_id, conversation_id, &text_for_preview)?;

        Ok(msg.into())
    }

    async fn create_conversation(
        &self,
        ctx: &Context<'_>,
        input: CreateConversationInput,
    ) -> Result<Conversation> {
        let actor = require_actor(ctx)?;

        let svc = ctx.data::<std::sync::Arc<dyn ConversationService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&input.server_id)?;

        let participants = if input.is_channel.unwrap_or(false) {
            rim_services::conversation::types::ConversationParticipants::Channel
        } else if let Some(other_user_id) = input.other_user_id {
            rim_services::conversation::types::ConversationParticipants::DirectMessage {
                other_user_id: parse_uuid_id::<rim_core::user::model::UserId>(&other_user_id)?,
            }
        } else {
            let member_ids = input.member_ids.unwrap_or_default();
            let mut ids = Vec::with_capacity(member_ids.len());
            for id in member_ids {
                ids.push(parse_uuid_id::<rim_core::user::model::UserId>(&id)?);
            }
            rim_services::conversation::types::ConversationParticipants::Group { member_ids: ids }
        };

        let conv = svc
            .create_conversation(CreateConversationCmd {
                actor: Actor::new(actor),
                server_id,
                participants,
                title: input.title,
            })
            .await
            .map_err(map_service_error)?;

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

    async fn edit_message(&self, ctx: &Context<'_>, input: EditMessageInput) -> Result<Message> {
        let actor = require_actor(ctx)?;

        let svc = ctx.data::<std::sync::Arc<dyn MessageService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&input.server_id)?;
        let conversation_id =
            parse_uuid_id::<rim_core::conversation::model::ConversationId>(&input.conversation_id)?;
        let message_id = parse_uuid_id::<rim_core::message::model::MessageId>(&input.message_id)?;

        let msg = svc
            .edit_message(EditMessageCmd {
                actor: Actor::new(actor),
                server_id,
                conversation_id,
                message_id,
                text: input.text,
            })
            .await
            .map_err(map_service_error)?;

        Ok(msg.into())
    }

    async fn forward_message(
        &self,
        ctx: &Context<'_>,
        input: ForwardMessageInput,
    ) -> Result<Message> {
        let actor = require_actor(ctx)?;

        let svc = ctx.data::<std::sync::Arc<dyn MessageService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&input.server_id)?;
        let source_conversation_id = parse_uuid_id::<rim_core::conversation::model::ConversationId>(
            &input.source_conversation_id,
        )?;
        let target_conversation_id = parse_uuid_id::<rim_core::conversation::model::ConversationId>(
            &input.target_conversation_id,
        )?;
        let message_id = parse_uuid_id::<rim_core::message::model::MessageId>(&input.message_id)?;

        let msg = svc
            .forward_message(rim_services::message::command::ForwardMessageCmd {
                actor: Actor::new(actor),
                server_id,
                source_conversation_id,
                target_conversation_id,
                message_id,
            })
            .await
            .map_err(map_service_error)?;

        Ok(msg.into())
    }

    async fn delete_message(&self, ctx: &Context<'_>, input: DeleteMessageInput) -> Result<bool> {
        let actor = require_actor(ctx)?;

        let svc = ctx.data::<std::sync::Arc<dyn MessageService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&input.server_id)?;
        let conversation_id =
            parse_uuid_id::<rim_core::conversation::model::ConversationId>(&input.conversation_id)?;
        let message_id = parse_uuid_id::<rim_core::message::model::MessageId>(&input.message_id)?;

        svc.delete_message(DeleteMessageCmd {
            actor: Actor::new(actor),
            server_id,
            conversation_id,
            message_id,
            hard: false,
        })
        .await
        .map_err(map_service_error)?;

        Ok(true)
    }

    async fn create_attachment(
        &self,
        ctx: &Context<'_>,
        input: CreateAttachmentInput,
    ) -> Result<UploadTicket> {
        let actor = require_actor(ctx)?;

        let svc = ctx.data::<std::sync::Arc<dyn MessageAttachmentService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&input.server_id)?;
        let conversation_id =
            parse_uuid_id::<rim_core::conversation::model::ConversationId>(&input.conversation_id)?;
        let message_id = input
            .message_id
            .as_ref()
            .map(parse_uuid_id::<rim_core::message::model::MessageId>)
            .transpose()?;

        let ticket = svc
            .create_attachment(CreateAttachmentCmd {
                actor: Actor::new(actor),
                server_id,
                conversation_id,
                message_id,
                filename: input.filename,
                content_type: input.content_type,
                size_bytes: input.size_bytes,
            })
            .await
            .map_err(map_service_error)?;

        Ok(ticket.into())
    }

    async fn finalize_attachment(
        &self,
        ctx: &Context<'_>,
        input: FinalizeAttachmentInput,
    ) -> Result<Attachment> {
        let actor = require_actor(ctx)?;

        let svc = ctx.data::<std::sync::Arc<dyn MessageAttachmentService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&input.server_id)?;
        let attachment_id = parse_uuid_id::<
            rim_core::message_attachment::model::MessageAttachmentId,
        >(&input.attachment_id)?;

        let attachment = svc
            .finalize_attachment(FinalizeAttachmentCmd {
                actor: Actor::new(actor),
                server_id,
                attachment_id,
            })
            .await
            .map_err(map_service_error)?;

        Ok(attachment.into())
    }

    async fn delete_attachment(
        &self,
        ctx: &Context<'_>,
        input: DeleteAttachmentInput,
    ) -> Result<bool> {
        let actor = require_actor(ctx)?;

        let svc = ctx.data::<std::sync::Arc<dyn MessageAttachmentService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&input.server_id)?;
        let attachment_id = parse_uuid_id::<
            rim_core::message_attachment::model::MessageAttachmentId,
        >(&input.attachment_id)?;

        svc.delete_attachment(DeleteAttachmentCmd {
            actor: Actor::new(actor),
            server_id,
            attachment_id,
        })
        .await
        .map_err(map_service_error)?;

        Ok(true)
    }

    async fn execute_command(
        &self,
        ctx: &Context<'_>,
        input: ExecuteCommandInput,
    ) -> Result<CommandResult> {
        let actor = require_actor(ctx)?;

        let svc = ctx.data::<std::sync::Arc<dyn CommandService>>()?;
        let result = svc
            .execute_command(ExecuteCommandCmd {
                actor: Actor::new(actor),
                command_id: input.command_id,
                args: input.args.map(|json| json.0),
                context: input.command_context.map(TryInto::try_into).transpose()?,
                confirmed_dangerous: input.confirmed_dangerous.unwrap_or(false),
            })
            .await
            .map_err(map_service_error)?;

        Ok(result.into())
    }

    async fn create_user_command(
        &self,
        ctx: &Context<'_>,
        input: CreateUserCommandInput,
    ) -> Result<Command> {
        let actor = require_actor(ctx)?;

        let svc = ctx.data::<std::sync::Arc<dyn CommandService>>()?;
        let command = svc
            .create_user_command(CreateUserCommandCmd {
                actor: Actor::new(actor),
                command_key: input.command_id,
                title: input.title,
                description: input.description,
                steps: input
                    .steps
                    .into_iter()
                    .map(|step| UserCommandStepDto {
                        command_id: step.command_id,
                        args: step.args.map(|json| json.0),
                    })
                    .collect(),
            })
            .await
            .map_err(map_service_error)?;

        Ok(command.as_command_definition().into())
    }

    async fn update_user_command(
        &self,
        ctx: &Context<'_>,
        input: UpdateUserCommandInput,
    ) -> Result<Command> {
        let actor = require_actor(ctx)?;

        let svc = ctx.data::<std::sync::Arc<dyn CommandService>>()?;
        let command = svc
            .update_user_command(UpdateUserCommandCmd {
                actor: Actor::new(actor),
                command_id: input.command_id,
                title: input.title,
                description: input.description,
                steps: input
                    .steps
                    .into_iter()
                    .map(|step| UserCommandStepDto {
                        command_id: step.command_id,
                        args: step.args.map(|json| json.0),
                    })
                    .collect(),
                is_enabled: input.is_enabled,
            })
            .await
            .map_err(map_service_error)?;

        Ok(command.as_command_definition().into())
    }

    async fn delete_user_command(
        &self,
        ctx: &Context<'_>,
        input: DeleteUserCommandInput,
    ) -> Result<bool> {
        let actor = require_actor(ctx)?;

        let svc = ctx.data::<std::sync::Arc<dyn CommandService>>()?;
        svc.delete_user_command(DeleteUserCommandCmd {
            actor: Actor::new(actor),
            command_id: input.command_id,
        })
        .await
        .map_err(map_service_error)?;

        Ok(true)
    }

    async fn create_keybind(
        &self,
        ctx: &Context<'_>,
        input: CreateKeybindInput,
    ) -> Result<Keybind> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn KeybindService>>()?;
        let keybind = svc
            .create_keybind(CreateKeybindCmd {
                actor: Actor::new(actor),
                scope: input.scope.into(),
                workspace_id: input
                    .workspace_id
                    .as_ref()
                    .map(parse_uuid_id::<rim_core::server::model::WorkspaceId>)
                    .transpose()?,
                keystroke: input.keystroke,
                command_id: input.command_id,
                context: input.context.into(),
                preset_args: input.preset_args.map(|json| json.0),
            })
            .await
            .map_err(map_service_error)?;

        Ok(keybind.into())
    }

    async fn update_keybind(
        &self,
        ctx: &Context<'_>,
        input: UpdateKeybindInput,
    ) -> Result<Keybind> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn KeybindService>>()?;
        let keybind = svc
            .update_keybind(UpdateKeybindCmd {
                actor: Actor::new(actor),
                keybind_id: parse_uuid_id::<rim_core::keybind::model::KeybindId>(
                    &input.keybind_id,
                )?,
                keystroke: input.keystroke,
                command_id: input.command_id,
                context: input.context.into(),
                preset_args: input.preset_args.map(|json| json.0),
                is_enabled: input.is_enabled,
            })
            .await
            .map_err(map_service_error)?;

        Ok(keybind.into())
    }

    async fn toggle_reaction(
        &self,
        ctx: &Context<'_>,
        input: ToggleReactionInput,
    ) -> Result<Vec<ReactionGroup>> {
        let actor = require_actor(ctx)?;

        let svc = ctx.data::<std::sync::Arc<dyn MessageReactionService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&input.server_id)?;
        let conversation_id =
            parse_uuid_id::<rim_core::conversation::model::ConversationId>(&input.conversation_id)?;
        let message_id = parse_uuid_id::<rim_core::message::model::MessageId>(&input.message_id)?;

        let groups = svc
            .toggle_reaction(ToggleReactionCmd {
                actor: Actor::new(actor),
                server_id,
                conversation_id,
                message_id,
                emoji: input.emoji,
            })
            .await
            .map_err(map_service_error)?;

        Ok(groups.into_iter().map(Into::into).collect())
    }

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

    async fn create_server(&self, ctx: &Context<'_>, input: CreateServerInput) -> Result<Server> {
        let actor = require_actor(ctx)?;

        let svc = ctx.data::<std::sync::Arc<dyn ServerService>>()?;
        let server = svc
            .create_server(CreateServerCmd {
                actor: Actor::new(actor),
                name: input.name,
            })
            .await
            .map_err(map_service_error)?;

        Ok(server.into())
    }

    async fn delete_keybind(&self, ctx: &Context<'_>, input: DeleteKeybindInput) -> Result<bool> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn KeybindService>>()?;
        svc.delete_keybind(DeleteKeybindCmd {
            actor: Actor::new(actor),
            keybind_id: parse_uuid_id::<rim_core::keybind::model::KeybindId>(&input.keybind_id)?,
        })
        .await
        .map_err(map_service_error)?;

        Ok(true)
    }

    async fn pin_message(&self, ctx: &Context<'_>, input: PinMessageInput) -> Result<bool> {
        let actor = require_actor(ctx)?;

        let svc = ctx.data::<std::sync::Arc<dyn PinService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&input.server_id)?;
        let conversation_id =
            parse_uuid_id::<rim_core::conversation::model::ConversationId>(&input.conversation_id)?;
        let message_id = parse_uuid_id::<rim_core::message::model::MessageId>(&input.message_id)?;

        svc.pin_message(PinMessageCmd {
            actor: Actor::new(actor),
            server_id,
            conversation_id,
            message_id,
        })
        .await
        .map_err(map_service_error)?;

        Ok(true)
    }

    async fn unpin_message(&self, ctx: &Context<'_>, input: UnpinMessageInput) -> Result<bool> {
        let actor = require_actor(ctx)?;

        let svc = ctx.data::<std::sync::Arc<dyn PinService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&input.server_id)?;
        let conversation_id =
            parse_uuid_id::<rim_core::conversation::model::ConversationId>(&input.conversation_id)?;
        let message_id = parse_uuid_id::<rim_core::message::model::MessageId>(&input.message_id)?;

        svc.unpin_message(UnpinMessageCmd {
            actor: Actor::new(actor),
            server_id,
            conversation_id,
            message_id,
        })
        .await
        .map_err(map_service_error)?;

        Ok(true)
    }

    async fn start_typing(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
        conversation_id: ID,
    ) -> Result<bool> {
        let actor = require_actor(ctx)?;

        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;
        let conversation_id =
            parse_uuid_id::<rim_core::conversation::model::ConversationId>(&conversation_id)?;

        // Verify membership
        let member_svc = ctx
            .data::<std::sync::Arc<dyn rim_services::server_member::service::ServerMemberService>>(
            )?;
        member_svc
            .get_member(rim_services::server_member::command::GetServerMemberQuery {
                actor: Actor::new(actor),
                server_id,
                user_id: actor,
            })
            .await
            .map_err(map_service_error)?;

        let typing_tracker =
            ctx.data::<std::sync::Arc<rim_infra_services::typing::TypingTracker>>()?;

        if typing_tracker.should_broadcast(actor, conversation_id) {
            let conv_svc = ctx.data::<std::sync::Arc<dyn ConversationService>>()?;
            let conversation = conv_svc
                .get_conversation(rim_services::conversation::command::GetConversationQuery {
                    actor: Actor::new(actor),
                    server_id,
                    conversation_id,
                })
                .await
                .map_err(map_service_error)?;
            let conversation_type = conversation.conversation_type.as_str().to_owned();
            let member_user_ids = conversation.member_ids;

            let observers = ctx.data::<std::sync::Arc<rim_services::events::Observers>>()?;
            observers.workspace.on_typing_started(
                server_id,
                conversation_id,
                actor,
                conversation_type,
                member_user_ids,
            );
        }

        Ok(true)
    }

    async fn heartbeat(&self, ctx: &Context<'_>, input: HeartbeatInput) -> Result<bool> {
        let actor = require_actor(ctx)?;

        // CSO: Reject OFFLINE status from clients.
        let status: rim_services::presence::PresenceStatus = input.status.into();
        if status == rim_services::presence::PresenceStatus::Offline {
            return Err(async_graphql::Error::new(
                "Clients cannot set OFFLINE status",
            ));
        }

        let workspace_id = parse_uuid_id::<rim_core::server::model::ServerId>(&input.workspace_id)?;

        // Verify membership.
        let member_svc = ctx
            .data::<std::sync::Arc<dyn rim_services::server_member::service::ServerMemberService>>(
            )?;
        member_svc
            .get_member(rim_services::server_member::command::GetServerMemberQuery {
                actor: Actor::new(actor),
                server_id: workspace_id,
                user_id: actor,
            })
            .await
            .map_err(map_service_error)?;

        let presence_mgr =
            ctx.data::<std::sync::Arc<rim_infra_services::presence::PresenceManager>>()?;
        presence_mgr.heartbeat(actor, workspace_id, status);

        Ok(true)
    }

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

    async fn request_password_reset(&self, ctx: &Context<'_>, username: String) -> Result<bool> {
        let svc = ctx
            .data::<std::sync::Arc<dyn rim_services::password_reset::service::PasswordResetService>>(
            )?;
        svc.request_reset(
            rim_services::password_reset::command::RequestPasswordResetCmd { username },
        )
        .await
        .map_err(map_service_error)?;

        Ok(true)
    }

    async fn reset_password(
        &self,
        ctx: &Context<'_>,
        token: String,
        new_password: String,
    ) -> Result<bool> {
        let svc = ctx
            .data::<std::sync::Arc<dyn rim_services::password_reset::service::PasswordResetService>>(
            )?;
        svc.reset_password(rim_services::password_reset::command::ResetPasswordCmd {
            token,
            new_password,
        })
        .await
        .map_err(map_service_error)?;

        Ok(true)
    }

    async fn create_custom_emoji(
        &self,
        ctx: &Context<'_>,
        input: CreateCustomEmojiInput,
    ) -> Result<crate::types::CustomEmoji> {
        let actor = require_actor(ctx)?;
        let svc = ctx
            .data::<std::sync::Arc<dyn rim_services::custom_emoji::service::CustomEmojiService>>(
            )?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&input.server_id)?;

        let image_data = base64::engine::general_purpose::STANDARD
            .decode(&input.image_base64)
            .map_err(|e| async_graphql::Error::new(format!("invalid base64: {e}")))?;

        let emoji = svc
            .create(rim_services::custom_emoji::command::CreateCustomEmojiCmd {
                actor: Actor::new(actor),
                server_id,
                name: input.name,
                image_data,
            })
            .await
            .map_err(map_service_error)?;

        Ok(emoji.into())
    }

    async fn delete_custom_emoji(&self, ctx: &Context<'_>, emoji_id: ID) -> Result<bool> {
        let actor = require_actor(ctx)?;
        let svc = ctx
            .data::<std::sync::Arc<dyn rim_services::custom_emoji::service::CustomEmojiService>>(
            )?;
        let emoji_id = parse_uuid_id::<rim_core::custom_emoji::model::CustomEmojiId>(&emoji_id)?;

        svc.delete(rim_services::custom_emoji::command::DeleteCustomEmojiCmd {
            actor: Actor::new(actor),
            emoji_id,
        })
        .await
        .map_err(map_service_error)?;

        Ok(true)
    }

    // ── Macros ──────────────────────────────────────────────────────────

    async fn create_macro(&self, ctx: &Context<'_>, input: CreateMacroInput) -> Result<MacroDef> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn MacroService>>()?;

        let result = svc
            .create_macro(CreateMacroCmd {
                actor: Actor::new(actor),
                server_id: parse_uuid_id(&input.server_id)?,
                name: input.name,
                description: input.description,
                steps: input
                    .steps
                    .into_iter()
                    .map(|s| MacroStepDto {
                        command_id: s.command_id,
                        args: s.args.map(|j| j.0),
                    })
                    .collect(),
            })
            .await
            .map_err(map_service_error)?;

        Ok(result.into())
    }

    async fn update_macro(&self, ctx: &Context<'_>, input: UpdateMacroInput) -> Result<MacroDef> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn MacroService>>()?;

        let result = svc
            .update_macro(UpdateMacroCmd {
                actor: Actor::new(actor),
                macro_id: parse_uuid_id(&input.macro_id)?,
                name: input.name,
                description: input.description,
                steps: input
                    .steps
                    .into_iter()
                    .map(|s| MacroStepDto {
                        command_id: s.command_id,
                        args: s.args.map(|j| j.0),
                    })
                    .collect(),
            })
            .await
            .map_err(map_service_error)?;

        Ok(result.into())
    }

    async fn delete_macro(&self, ctx: &Context<'_>, macro_id: ID) -> Result<bool> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn MacroService>>()?;

        svc.delete_macro(DeleteMacroCmd {
            actor: Actor::new(actor),
            macro_id: parse_uuid_id(&macro_id)?,
        })
        .await
        .map_err(map_service_error)?;

        Ok(true)
    }

    async fn execute_macro(
        &self,
        ctx: &Context<'_>,
        input: ExecuteMacroInput,
    ) -> Result<CommandResult> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn MacroService>>()?;

        let result = svc
            .execute_macro(ExecuteMacroCmd {
                actor: Actor::new(actor),
                macro_id: parse_uuid_id(&input.macro_id)?,
                command_context: input.command_context.map(TryInto::try_into).transpose()?,
                confirmed_dangerous: input.confirmed_dangerous.unwrap_or(false),
            })
            .await
            .map_err(map_service_error)?;

        Ok(result.into())
    }
}

#[derive(InputObject)]
struct CreateMacroInput {
    server_id: ID,
    name: String,
    description: Option<String>,
    steps: Vec<MacroStepInput>,
}

#[derive(InputObject)]
struct UpdateMacroInput {
    macro_id: ID,
    name: String,
    description: Option<String>,
    steps: Vec<MacroStepInput>,
}

#[derive(InputObject)]
struct MacroStepInput {
    command_id: String,
    args: Option<async_graphql::Json<serde_json::Value>>,
}

#[derive(InputObject)]
struct ExecuteMacroInput {
    macro_id: ID,
    command_context: Option<CommandContextInput>,
    confirmed_dangerous: Option<bool>,
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

#[derive(InputObject)]
struct CreateCustomEmojiInput {
    server_id: ID,
    name: String,
    image_base64: String,
}

fn spawn_link_preview_fetch(
    ctx: &Context<'_>,
    message_id: rim_core::message::model::MessageId,
    workspace_id: rim_core::server::model::ServerId,
    conversation_id: rim_core::conversation::model::ConversationId,
    text: &str,
) -> Result<()> {
    let urls = rim_infra_services::link_preview::extract_urls(text);
    if urls.is_empty() {
        return Ok(());
    }

    let link_svc = ctx
        .data::<std::sync::Arc<dyn rim_services::link_preview::service::LinkPreviewService>>()?
        .clone();
    let observers = ctx
        .data::<std::sync::Arc<rim_services::events::Observers>>()?
        .clone();

    tokio::spawn(async move {
        match link_svc.fetch_and_store(message_id, urls).await {
            Ok(previews) if !previews.is_empty() => {
                use rim_services::events::EventObserver;
                observers.workspace.notify(
                    rim_services::events::WorkspaceEvent::LinkPreviewReady {
                        workspace_id,
                        conversation_id,
                        message_id,
                    },
                );
            }
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(error = %e, "link preview fetch failed");
            }
        }
    });

    Ok(())
}
