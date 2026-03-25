use async_graphql::*;
use chrono::{DateTime, Utc};
use rim_services::Actor;
use rim_services::auth::command::{
    AcceptInviteCmd, BootstrapCmd, CreateInviteCmd, JoinServerByInviteCmd, LoginCmd, LogoutCmd,
};
use rim_services::auth::service::AuthService;
use rim_services::command::dto::UserCommandStepDto;
use rim_services::command::request::{CreateUserCommandCmd, ExecuteCommandCmd};
use rim_services::command::request::{DeleteUserCommandCmd, UpdateUserCommandCmd};
use rim_services::command::service::CommandService;
use rim_services::conversation::command::CreateConversationCmd;
use rim_services::conversation::service::ConversationService;
use rim_services::keybind::request::{CreateKeybindCmd, DeleteKeybindCmd, UpdateKeybindCmd};
use rim_services::keybind::service::KeybindService;
use rim_services::message::command::{DeleteMessageCmd, EditMessageCmd};
use rim_services::message::command::{ReplyToMessageCmd, SendMessageCmd};
use rim_services::message::service::MessageService;
use rim_services::message_attachment::command::{
    CreateAttachmentCmd, DeleteAttachmentCmd, FinalizeAttachmentCmd,
};
use rim_services::message_attachment::service::MessageAttachmentService;
use rim_services::message_reaction::command::ToggleReactionCmd;
use rim_services::message_reaction::service::MessageReactionService;
use rim_services::server::command::CreateServerCmd;
use rim_services::server::service::ServerService;

use crate::auth_context::{require_actor, require_session_id};
use crate::auth_intent::{AuthIntent, AuthIntentCtx};
use crate::errors::map_service_error;
use crate::ids::parse_uuid_id;
use crate::types::{
    Attachment, Command, CommandContextInput, CommandResult, Conversation, Keybind, KeybindContext,
    KeybindScope, Message, ReactionGroup, Server, UploadTicket,
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
pub struct EditMessageInput {
    pub server_id: ID,
    pub conversation_id: ID,
    pub message_id: ID,
    pub text: String,
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

        let msg = svc
            .reply_to_message(ReplyToMessageCmd {
                actor: Actor::new(actor),
                server_id,
                conversation_id,
                root_message_id,
                text: input.text,
                parent_message_id,
                attachments: vec![],
            })
            .await
            .map_err(map_service_error)?;

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
}
