use async_graphql::*;
use base64::Engine;
use chrono::{DateTime, Utc};
use rim_services::Actor;
use rim_services::auth::command::{
    AcceptInviteCmd, BootstrapCmd, CreateInstanceInviteCmd, CreateInviteCmd, JoinServerByInviteCmd,
    LoginCmd, LogoutCmd, RevokeInstanceInviteCmd, RevokeInviteCmd, SignupOpenCmd,
    SignupWithBootstrapCmd, SignupWithInstanceInviteCmd,
};
use rim_services::auth::service::AuthService;
use rim_services::command::dto::UserCommandStepDto;
use rim_services::command::initiator::CommandInitiator as CommandInitiatorDto;
use rim_services::command::request::{CreateUserCommandCmd, ExecuteCommandCmd};
use rim_services::command::request::{DeleteUserCommandCmd, UpdateUserCommandCmd};
use rim_services::command::sensitivity::CommandSensitivityDto;
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
use rim_services::user::command::UpdateMyProfileCmd;
use rim_services::user::service::UserService;

use crate::auth_context::{require_actor, require_session_id};
use crate::auth_intent::{AuthIntent, AuthIntentCtx};
use crate::errors::map_service_error;
use crate::ids::parse_uuid_id;
use crate::types::{
    Attachment, Command, CommandContextInput, CommandInitiator, CommandResult, CommandSensitivity,
    Conversation, CreateInstanceInvitePayloadGql, InstanceInviteGql, Keybind, KeybindContext,
    KeybindScope, MacroDef, Message, PresenceStatus, ReactionGroup, Server,
    SignupWithInvitePayloadGql, UploadTicket, User,
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
    /// Optional recipient to email the invite link to (Track B3). Validated server-side;
    /// malformed addresses return a validation error.
    pub recipient_email: Option<String>,
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
    /// **Deprecated** — use `confirmation` instead. Pre-Phase-1 callers
    /// (the unmigrated frontend) set this to `true` after `window.confirm()`.
    /// When `confirmation` is absent and this is `Some(true)`, the server
    /// synthesizes a self-issued in-band proof so the H1 gate has a single
    /// authoritative path. Removed in Phase 4 once the frontend modal lands.
    pub confirmed_dangerous: Option<bool>,
    /// Server-issued, server-verified confirmation proof. Required when
    /// re-submitting after a previous `executeCommand` returned
    /// `status: CONFIRMATION_REQUIRED`. Closes H1 (TOCTOU) by binding the
    /// proof to `(actor, commandId, args, context)` server-side.
    pub confirmation: Option<crate::types::ConfirmationProofInput>,
    pub is_pipe: Option<bool>,
    /// Who issued this command. Optional for back-compat — when omitted, the
    /// backend defaults to `PALETTE` (the conservative human default). Phase
    /// 4 frontend dispatcher will start sending the explicit value from every
    /// call site (palette / agent / keybind / context-menu / macro). Until
    /// then this field is opt-in.
    ///
    /// Per Fury v2 §3.11. The default is enforced server-side
    /// (`CommandInitiator::default() == Palette`) so a client that submits
    /// `null` is treated as a human action — never as agent action.
    pub initiator: Option<CommandInitiator>,
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

/// Patch fields on the authenticated user's profile.
///
/// Each field is independently optional. Omitting a field leaves the existing
/// value unchanged. Sending an explicit empty string clears the field
/// (after trim + control-char strip).
#[derive(InputObject)]
pub struct UpdateMyProfileInput {
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub status_text: Option<String>,
    pub bio: Option<String>,
}

#[Object]
impl Mutation {
    #[graphql(
        deprecation = "Use signupWithInvite with a bootstrapToken issued by the \
        rim_server bootstrap-user CLI command. The BOOTSTRAP_SECRET path will be \
        removed in the next minor release."
    )]
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
                recipient_email: input.recipient_email,
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

    /// Unified signup mutation for all token-based and open-mode registration paths.
    ///
    /// In invite-only mode, exactly one of `bootstrapToken`, `instanceInviteToken`,
    /// or `workspaceInviteToken` must be supplied.
    /// In open mode, all token fields must be absent.
    ///
    /// On success, sets session and CSRF cookies via `AuthIntent`.
    async fn signup_with_invite(
        &self,
        ctx: &Context<'_>,
        input: SignupWithInviteInput,
    ) -> Result<SignupWithInvitePayloadGql> {
        use rim_services::auth::dto::RegistrationMode;

        let auth = ctx.data::<std::sync::Arc<dyn AuthService>>()?;
        let mode = ctx.data::<std::sync::Arc<RegistrationMode>>()?;

        let token_count = input.bootstrap_token.is_some() as u8
            + input.instance_invite_token.is_some() as u8
            + input.workspace_invite_token.is_some() as u8;

        let (user_id, server_id, session_token, csrf_token) = match mode.as_ref() {
            RegistrationMode::InviteOnly => {
                if token_count != 1 {
                    return Err(async_graphql::Error::new(
                        "exactly one token is required in invite-only mode",
                    )
                    .extend_with(|_, e| e.set("code", "VALIDATION")));
                }

                // Normalise email at the resolver boundary (trim + lowercase) as
                // defence in depth; the service also validates on the way in.
                let normalised_email = input.email.trim().to_lowercase();

                if let Some(raw) = input.bootstrap_token {
                    let res = auth
                        .signup_with_bootstrap(SignupWithBootstrapCmd {
                            bootstrap_token: raw,
                            username: input.username,
                            password: input.password,
                            email: normalised_email,
                            display_name: input.display_name,
                        })
                        .await
                        .map_err(map_service_error)?;
                    (
                        res.user_id,
                        res.server_id,
                        res.session_token,
                        res.csrf_token,
                    )
                } else if let Some(raw) = input.instance_invite_token {
                    let res = auth
                        .signup_with_instance_invite(SignupWithInstanceInviteCmd {
                            instance_invite_token: raw,
                            username: input.username,
                            password: input.password,
                            email: normalised_email,
                            display_name: input.display_name,
                        })
                        .await
                        .map_err(map_service_error)?;
                    (
                        res.user_id,
                        res.server_id,
                        res.session_token,
                        res.csrf_token,
                    )
                } else {
                    // workspace_invite_token — delegate to the existing accept_invite path.
                    let raw = input.workspace_invite_token.unwrap();
                    let res = auth
                        .accept_invite(AcceptInviteCmd {
                            invite_token: raw,
                            username: input.username,
                            password: input.password,
                        })
                        .await
                        .map_err(map_service_error)?;
                    (
                        res.user_id,
                        Some(res.server_id),
                        res.session_token,
                        res.csrf_token,
                    )
                }
            }
            RegistrationMode::Open => {
                if token_count > 0 {
                    return Err(async_graphql::Error::new(
                        "tokens must not be supplied in open registration mode",
                    )
                    .extend_with(|_, e| e.set("code", "VALIDATION")));
                }

                // Proof-of-wiring: fail closed if signup rate limiter token is absent.
                let _ =
                    ctx.data::<std::sync::Arc<rim_services::auth::dto::SignupRateLimiterToken>>();

                // Remote IP for rate limiting — service enforces the limit.
                let remote_ip = ctx
                    .data::<crate::auth_context::RemoteIp>()
                    .map(|r| r.0.clone())
                    .unwrap_or_else(|_| "unknown".to_string());

                let normalised_email_open = input.email.trim().to_lowercase();
                let res = auth
                    .signup_open(SignupOpenCmd {
                        username: input.username,
                        password: input.password,
                        remote_ip,
                        email: normalised_email_open,
                        display_name: input.display_name,
                    })
                    .await
                    .map_err(map_service_error)?;
                (
                    res.user_id,
                    res.server_id,
                    res.session_token,
                    res.csrf_token,
                )
            }
        };

        if let Ok(intent) = ctx.data::<std::sync::Arc<AuthIntentCtx>>() {
            intent.set(AuthIntent::SetSession {
                session_token,
                csrf_token,
            });
        }

        Ok(SignupWithInvitePayloadGql {
            user_id: ID(user_id.to_string()),
            server_id: server_id.map(|id| ID(id.to_string())),
        })
    }

    /// Create a new instance invite (admin-only gate: must own at least one workspace).
    ///
    /// Returns the raw invite token exactly once in the payload. Store it; it cannot
    /// be retrieved again.
    async fn create_instance_invite(
        &self,
        ctx: &Context<'_>,
        input: CreateInstanceInviteInput,
    ) -> Result<CreateInstanceInvitePayloadGql> {
        let actor = require_actor(ctx)?;
        let auth = ctx.data::<std::sync::Arc<dyn AuthService>>()?;

        let res = auth
            .create_instance_invite(CreateInstanceInviteCmd {
                actor: Actor::new(actor),
                expires_at: input.expires_at,
                max_uses: input.max_uses,
                recipient_email: input.recipient_email,
            })
            .await
            .map_err(map_service_error)?;

        Ok(CreateInstanceInvitePayloadGql {
            invite_token: res.invite_token,
            invite: InstanceInviteGql::from(res.invite),
        })
    }

    /// Revoke an existing instance invite (admin-only gate: must own at least one workspace).
    ///
    /// Returns `true` on success. Returns `NOT_FOUND` if the invite does not exist.
    async fn revoke_instance_invite(&self, ctx: &Context<'_>, invite_id: ID) -> Result<bool> {
        let actor = require_actor(ctx)?;
        let invite_id = parse_uuid_id::<rim_core::auth::model::InstanceInviteId>(&invite_id)?;
        let auth = ctx.data::<std::sync::Arc<dyn AuthService>>()?;

        auth.revoke_instance_invite(RevokeInstanceInviteCmd {
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

        // ── H1 back-compat bridge ──────────────────────────────────────
        //
        // The pre-reframe wire field `confirmed_dangerous: Option<bool>`
        // is still on the input for back-compat with the unmigrated
        // frontend. Resolution:
        //
        // 1. If the new `confirmation` field is populated, use it
        //    (modern flow — frontend has the modal).
        // 2. Else if `confirmed_dangerous: Some(true)` AND the new field
        //    is absent, log a deprecation warning. The proof stays
        //    `None` here; the service-layer gate will issue a fresh
        //    challenge and return `CONFIRMATION_REQUIRED`. Pre-Phase-1
        //    behavior (silent execute on bool=true) is intentionally
        //    NOT preserved — passing a bool used to be the bypass; we
        //    are closing it on purpose.
        // 3. Both `None` ⇒ no proof; gate issues a challenge.
        //
        // Removed entirely in Phase 4 when the frontend modal lands.
        let proof = match input.confirmation {
            Some(c) => Some(c.into_proof()?),
            None => {
                if input.confirmed_dangerous == Some(true) {
                    tracing::warn!(
                        target = "audit",
                        event = "deprecated_confirmed_dangerous_bool",
                        command_id = %input.command_id,
                        "client sent legacy confirmed_dangerous=true; ignoring (H1 closure). \
                         Frontend must migrate to ConfirmationProof flow.",
                    );
                }
                None
            }
        };

        let result = svc
            .execute_command(ExecuteCommandCmd {
                actor: Actor::new(actor),
                command_id: input.command_id,
                args: input.args.map(|json| json.0),
                context: input.command_context.map(TryInto::try_into).transpose()?,
                confirmation: proof,
                is_pipe: input.is_pipe.unwrap_or(false),
                // Default to Palette per Fury v2 §3.11 conservative-default
                // rule. Until Phase 4's IPC proxy stamps the initiator from
                // the webview origin, this is the only safe default.
                initiator: input
                    .initiator
                    .map(CommandInitiatorDto::from)
                    .unwrap_or_default(),
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
            ctx.data::<std::sync::Arc<dyn rim_services::typing::TypingService>>()?;

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
            use rim_services::events::EventObserver;
            observers.emit(rim_services::events::WorkspaceEvent::TypingStarted {
                workspace_id: server_id,
                conversation_id,
                user_id: actor,
                conversation_type,
                member_user_ids,
            });
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
            ctx.data::<std::sync::Arc<dyn rim_services::presence::PresenceService>>()?;
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
                ai_callable: input.ai_callable,
                nl_description: input.nl_description,
                intent_samples: input.intent_samples,
                sensitivity: CommandSensitivityDto::from(input.sensitivity),
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
                ai_callable: input.ai_callable,
                nl_description: input.nl_description,
                intent_samples: input.intent_samples,
                sensitivity: CommandSensitivityDto::from(input.sensitivity),
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

        // Same H1 back-compat bridge as executeCommand. The legacy
        // `confirmed_dangerous: true` is no longer self-confirming —
        // the inner ExecuteCommandCmd's confirmation gate will issue a
        // per-step challenge when it hits a dangerous step.
        let proof = match input.confirmation {
            Some(c) => Some(c.into_proof()?),
            None => {
                if input.confirmed_dangerous == Some(true) {
                    tracing::warn!(
                        target = "audit",
                        event = "deprecated_confirmed_dangerous_bool",
                        macro_id = %input.macro_id.as_str(),
                        "client sent legacy confirmed_dangerous=true on executeMacro; ignoring (H1 closure).",
                    );
                }
                None
            }
        };

        let result = svc
            .execute_macro(ExecuteMacroCmd {
                actor: Actor::new(actor),
                macro_id: parse_uuid_id(&input.macro_id)?,
                command_context: input.command_context.map(TryInto::try_into).transpose()?,
                confirmation: proof,
                // Same back-compat default rule as `executeCommand` — `Macro`
                // here means "the macro execution itself"; nested
                // `ExecuteCommandCmd` calls produced by macro expansion will
                // override to whatever the macro service decides per
                // step (Phase 1: same value as the outer initiator,
                // Phase 4: see propagation rule in CommandInitiator docs).
                initiator: input
                    .initiator
                    .map(CommandInitiatorDto::from)
                    .unwrap_or_default(),
            })
            .await
            .map_err(map_service_error)?;

        Ok(result.into())
    }

    // ── Skill Profile ──────────────────────────────────────────────────

    async fn record_feature_usage(
        &self,
        ctx: &Context<'_>,
        feature_id: String,
    ) -> Result<crate::types::SkillProfileGql> {
        let actor = require_actor(ctx)?;
        let svc = ctx
            .data::<std::sync::Arc<dyn rim_services::skill_profile::service::SkillProfileService>>(
            )?;

        let profile = svc
            .record_feature_usage(rim_services::skill_profile::dto::RecordFeatureUsageCmd {
                actor: Actor::new(actor),
                feature_id,
            })
            .await
            .map_err(map_service_error)?;

        Ok(profile.into())
    }

    async fn override_skill_tier(
        &self,
        ctx: &Context<'_>,
        tier: crate::types::SkillTier,
    ) -> Result<crate::types::SkillProfileGql> {
        let actor = require_actor(ctx)?;
        let svc = ctx
            .data::<std::sync::Arc<dyn rim_services::skill_profile::service::SkillProfileService>>(
            )?;

        let profile = svc
            .override_skill_tier(rim_services::skill_profile::dto::OverrideSkillTierCmd {
                actor: Actor::new(actor),
                tier: tier.into(),
            })
            .await
            .map_err(map_service_error)?;

        Ok(profile.into())
    }

    // ── Channel Lenses ────────────────────────────────────────────────────

    async fn create_channel_lens(
        &self,
        ctx: &Context<'_>,
        input: CreateChannelLensInput,
    ) -> Result<crate::types::ChannelLens> {
        let actor = require_actor(ctx)?;
        let svc = ctx
            .data::<std::sync::Arc<dyn rim_services::channel_lens::service::ChannelLensService>>(
            )?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&input.server_id)?;
        let conversation_id =
            parse_uuid_id::<rim_core::conversation::model::ConversationId>(&input.conversation_id)?;

        let lens = svc
            .create_lens(rim_services::channel_lens::command::CreateLensCmd {
                actor: Actor::new(actor),
                server_id,
                conversation_id,
                name: input.name,
                filter_spec: input.filter_spec.into(),
                is_shared: input.is_shared.unwrap_or(false),
            })
            .await
            .map_err(map_service_error)?;

        Ok(lens.into())
    }

    async fn update_channel_lens(
        &self,
        ctx: &Context<'_>,
        input: UpdateChannelLensInput,
    ) -> Result<crate::types::ChannelLens> {
        let actor = require_actor(ctx)?;
        let svc = ctx
            .data::<std::sync::Arc<dyn rim_services::channel_lens::service::ChannelLensService>>(
            )?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&input.server_id)?;
        let lens_id =
            parse_uuid_id::<rim_core::channel_lens::model::ChannelLensId>(&input.lens_id)?;

        let lens = svc
            .update_lens(rim_services::channel_lens::command::UpdateLensCmd {
                actor: Actor::new(actor),
                server_id,
                lens_id,
                name: input.name,
                filter_spec: input.filter_spec.map(Into::into),
                is_shared: input.is_shared,
            })
            .await
            .map_err(map_service_error)?;

        Ok(lens.into())
    }

    async fn delete_channel_lens(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
        lens_id: ID,
    ) -> Result<bool> {
        let actor = require_actor(ctx)?;
        let svc = ctx
            .data::<std::sync::Arc<dyn rim_services::channel_lens::service::ChannelLensService>>(
            )?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;
        let lens_id = parse_uuid_id::<rim_core::channel_lens::model::ChannelLensId>(&lens_id)?;

        svc.delete_lens(rim_services::channel_lens::command::DeleteLensCmd {
            actor: Actor::new(actor),
            server_id,
            lens_id,
        })
        .await
        .map_err(map_service_error)?;

        Ok(true)
    }

    // ── Incidents ──────────────────────────────────────────────────────

    async fn start_incident(
        &self,
        ctx: &Context<'_>,
        input: StartIncidentInput,
    ) -> Result<crate::types::IncidentGql> {
        let actor = require_actor(ctx)?;
        let svc =
            ctx.data::<std::sync::Arc<dyn rim_services::incident::service::IncidentService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&input.server_id)?;
        let conversation_id =
            parse_uuid_id::<rim_core::conversation::model::ConversationId>(&input.conversation_id)?;

        let escalation_policy_id = input
            .escalation_policy_id
            .as_ref()
            .map(parse_uuid_id::<rim_core::escalation::model::EscalationPolicyId>)
            .transpose()?;

        let incident = svc
            .start_incident(rim_services::incident::command::StartIncidentCmd {
                actor: Actor::new(actor),
                server_id,
                conversation_id,
                title: input.title,
                severity: input.severity.map(Into::into),
                // An explicit override wins; `None` falls back to the conversation's default
                // policy (§3/§10).
                escalation_policy_id,
            })
            .await
            .map_err(map_service_error)?;

        Ok(incident.into())
    }

    async fn update_incident_status(
        &self,
        ctx: &Context<'_>,
        input: UpdateIncidentStatusInput,
    ) -> Result<crate::types::IncidentGql> {
        let actor = require_actor(ctx)?;
        let svc =
            ctx.data::<std::sync::Arc<dyn rim_services::incident::service::IncidentService>>()?;
        let incident_id =
            parse_uuid_id::<rim_core::incident::model::IncidentId>(&input.incident_id)?;

        let incident = svc
            .update_incident_status(rim_services::incident::command::UpdateIncidentStatusCmd {
                actor: Actor::new(actor),
                incident_id,
                status: input.status.into(),
            })
            .await
            .map_err(map_service_error)?;

        Ok(incident.into())
    }

    async fn advance_incident_phase(
        &self,
        ctx: &Context<'_>,
        input: AdvanceIncidentPhaseInput,
    ) -> Result<crate::types::IncidentPhaseGql> {
        let actor = require_actor(ctx)?;
        let svc =
            ctx.data::<std::sync::Arc<dyn rim_services::incident::service::IncidentService>>()?;
        let incident_id =
            parse_uuid_id::<rim_core::incident::model::IncidentId>(&input.incident_id)?;

        let phase = svc
            .advance_phase(rim_services::incident::command::AdvancePhaseCmd {
                actor: Actor::new(actor),
                incident_id,
                next_phase: input.next_phase.into(),
                note: input.note,
            })
            .await
            .map_err(map_service_error)?;

        Ok(phase.into())
    }

    async fn tag_incident_message(
        &self,
        ctx: &Context<'_>,
        input: TagIncidentMessageInput,
    ) -> Result<crate::types::IncidentMessageTagGql> {
        let actor = require_actor(ctx)?;
        let svc =
            ctx.data::<std::sync::Arc<dyn rim_services::incident::service::IncidentService>>()?;
        let incident_id =
            parse_uuid_id::<rim_core::incident::model::IncidentId>(&input.incident_id)?;
        let message_id = parse_uuid_id::<rim_core::message::model::MessageId>(&input.message_id)?;

        let tag = svc
            .tag_message(rim_services::incident::command::TagMessageCmd {
                actor: Actor::new(actor),
                incident_id,
                message_id,
                tag_type: input.tag_type.map(Into::into),
            })
            .await
            .map_err(map_service_error)?;

        Ok(tag.into())
    }

    async fn generate_postmortem(
        &self,
        ctx: &Context<'_>,
        incident_id: ID,
    ) -> Result<crate::types::IncidentGql> {
        let actor = require_actor(ctx)?;
        let svc =
            ctx.data::<std::sync::Arc<dyn rim_services::incident::service::IncidentService>>()?;
        let incident_id = parse_uuid_id::<rim_core::incident::model::IncidentId>(&incident_id)?;

        let incident = svc
            .generate_postmortem(rim_services::incident::command::GeneratePostmortemCmd {
                actor: Actor::new(actor),
                incident_id,
            })
            .await
            .map_err(map_service_error)?;

        Ok(incident.into())
    }

    async fn resolve_incident(
        &self,
        ctx: &Context<'_>,
        incident_id: ID,
    ) -> Result<crate::types::IncidentGql> {
        let actor = require_actor(ctx)?;
        let svc =
            ctx.data::<std::sync::Arc<dyn rim_services::incident::service::IncidentService>>()?;
        let incident_id = parse_uuid_id::<rim_core::incident::model::IncidentId>(&incident_id)?;

        let incident = svc
            .resolve_incident(rim_services::incident::command::ResolveIncidentCmd {
                actor: Actor::new(actor),
                incident_id,
            })
            .await
            .map_err(map_service_error)?;

        Ok(incident.into())
    }

    /// Acknowledge an incident (§4). Any server member may ack, not owner-gated — whoever
    /// responds picked it up. Idempotent: acking an already-acked incident returns it unchanged.
    /// Cancels the incident's active escalation run, if any.
    async fn acknowledge_incident(
        &self,
        ctx: &Context<'_>,
        incident_id: ID,
    ) -> Result<crate::types::IncidentGql> {
        let actor = require_actor(ctx)?;
        let svc =
            ctx.data::<std::sync::Arc<dyn rim_services::incident::service::IncidentService>>()?;
        let incident_id = parse_uuid_id::<rim_core::incident::model::IncidentId>(&incident_id)?;

        let incident = svc
            .acknowledge_incident(rim_services::incident::command::AcknowledgeIncidentCmd {
                actor: Actor::new(actor),
                incident_id,
                initiator: None,
            })
            .await
            .map_err(map_service_error)?;

        Ok(incident.into())
    }

    // ── Escalation policies (Track C1b/C1c) ───────────────────────────

    /// Create an escalation policy. Owner+Moderator (server-scoped — per-channel moderator
    /// roles don't apply). Every step target is validated to belong to this server (C1 security
    /// review): a `User` target must be a current member, an `OnCall` target's schedule must
    /// belong to this server.
    async fn create_escalation_policy(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
        name: String,
        steps: Vec<crate::types::EscalationStepInputGql>,
    ) -> Result<crate::types::EscalationPolicyGql> {
        let actor = require_actor(ctx)?;
        let svc = ctx
            .data::<std::sync::Arc<dyn rim_services::escalation::service::EscalationPolicyService>>(
            )?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;
        let steps = steps
            .into_iter()
            .map(TryInto::try_into)
            .collect::<Result<Vec<_>>>()?;

        let policy = svc
            .create_policy(
                rim_services::escalation::command::CreateEscalationPolicyCmd {
                    actor: Actor::new(actor),
                    server_id,
                    name,
                    steps,
                },
            )
            .await
            .map_err(map_service_error)?;

        Ok(policy.into())
    }

    /// Update an escalation policy's `name` and/or full step list. `steps: Some(_)` replaces
    /// every step atomically; `None` leaves the existing steps untouched. Owner+Moderator
    /// (resolved via the policy's own server); same per-target server constraint as create.
    async fn update_escalation_policy(
        &self,
        ctx: &Context<'_>,
        policy_id: ID,
        name: Option<String>,
        steps: Option<Vec<crate::types::EscalationStepInputGql>>,
    ) -> Result<crate::types::EscalationPolicyGql> {
        let actor = require_actor(ctx)?;
        let svc = ctx
            .data::<std::sync::Arc<dyn rim_services::escalation::service::EscalationPolicyService>>(
            )?;
        let policy_id =
            parse_uuid_id::<rim_core::escalation::model::EscalationPolicyId>(&policy_id)?;
        let steps = steps
            .map(|steps| {
                steps
                    .into_iter()
                    .map(TryInto::try_into)
                    .collect::<Result<Vec<_>>>()
            })
            .transpose()?;

        let policy = svc
            .update_policy(
                rim_services::escalation::command::UpdateEscalationPolicyCmd {
                    actor: Actor::new(actor),
                    policy_id,
                    name,
                    steps,
                },
            )
            .await
            .map_err(map_service_error)?;

        Ok(policy.into())
    }

    /// Delete an escalation policy. Owner+Moderator (resolved via the policy's own server).
    async fn delete_escalation_policy(&self, ctx: &Context<'_>, policy_id: ID) -> Result<bool> {
        let actor = require_actor(ctx)?;
        let svc = ctx
            .data::<std::sync::Arc<dyn rim_services::escalation::service::EscalationPolicyService>>(
            )?;
        let policy_id =
            parse_uuid_id::<rim_core::escalation::model::EscalationPolicyId>(&policy_id)?;

        svc.delete_policy(
            rim_services::escalation::command::DeleteEscalationPolicyCmd {
                actor: Actor::new(actor),
                policy_id,
            },
        )
        .await
        .map_err(map_service_error)?;

        Ok(true)
    }

    /// Set a conversation's default escalation policy — the policy `ingestAlert`-declared
    /// incidents always use (there's no human to pick an override for an alert-declared
    /// incident), and `startIncident`'s fallback when no per-call override is given.
    /// Owner+Moderator.
    async fn set_channel_escalation_policy(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
        conversation_id: ID,
        policy_id: ID,
    ) -> Result<bool> {
        let actor = require_actor(ctx)?;
        let svc = ctx
            .data::<std::sync::Arc<dyn rim_services::escalation::service::EscalationPolicyService>>(
            )?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;
        let conversation_id =
            parse_uuid_id::<rim_core::conversation::model::ConversationId>(&conversation_id)?;
        let policy_id =
            parse_uuid_id::<rim_core::escalation::model::EscalationPolicyId>(&policy_id)?;

        svc.set_channel_policy(
            rim_services::escalation::command::SetChannelEscalationPolicyCmd {
                actor: Actor::new(actor),
                server_id,
                conversation_id,
                policy_id,
            },
        )
        .await
        .map_err(map_service_error)?;

        Ok(true)
    }

    // ── Alert ingest tokens (Track B2) ────────────────────────────────

    /// Mint a new alert-ingest token for a channel. Owner-only. The raw token is returned
    /// exactly once, in `rawToken` — it is never retrievable again after this call.
    async fn create_ingest_token(
        &self,
        ctx: &Context<'_>,
        input: CreateIngestTokenInput,
    ) -> Result<crate::types::CreateIngestTokenResultGql> {
        let actor = require_actor(ctx)?;
        let svc = ctx
            .data::<std::sync::Arc<dyn rim_services::ingest_token::service::IngestTokenService>>(
            )?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&input.server_id)?;
        let conversation_id =
            parse_uuid_id::<rim_core::conversation::model::ConversationId>(&input.conversation_id)?;

        let result = svc
            .create_token(rim_services::ingest_token::command::CreateIngestTokenCmd {
                actor: Actor::new(actor),
                server_id,
                conversation_id,
                label: input.label,
                severity_default: input.severity_default.map(Into::into),
            })
            .await
            .map_err(map_service_error)?;

        Ok(result.into())
    }

    /// Revoke an ingest token. Owner-only, permanent (no un-revoke).
    async fn revoke_ingest_token(&self, ctx: &Context<'_>, token_id: ID) -> Result<bool> {
        let actor = require_actor(ctx)?;
        let svc = ctx
            .data::<std::sync::Arc<dyn rim_services::ingest_token::service::IngestTokenService>>(
            )?;
        let token_id = parse_uuid_id::<rim_core::ingest_token::model::IngestTokenId>(&token_id)?;

        svc.revoke_token(rim_services::ingest_token::command::RevokeIngestTokenCmd {
            actor: Actor::new(actor),
            token_id,
        })
        .await
        .map_err(map_service_error)?;

        Ok(true)
    }

    async fn set_workspace_state(
        &self,
        ctx: &Context<'_>,
        input: crate::types::SetWorkspaceStateInput,
    ) -> Result<crate::types::WorkspaceStateEntryGql> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<
            dyn rim_services::workspace_state::service::WorkspaceStateService,
        >>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&input.server_id)?;

        let entry = svc
            .set(
                rim_services::workspace_state::request::SetWorkspaceStateCmd {
                    actor: Actor::new(actor),
                    server_id,
                    key: input.key,
                    value: input.value.0,
                    scope: input.scope.into(),
                    ttl_seconds: input.ttl_seconds,
                },
            )
            .await
            .map_err(map_service_error)?;

        Ok(entry.into())
    }

    async fn delete_workspace_state(
        &self,
        ctx: &Context<'_>,
        input: crate::types::DeleteWorkspaceStateInput,
    ) -> Result<bool> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<
            dyn rim_services::workspace_state::service::WorkspaceStateService,
        >>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&input.server_id)?;

        let deleted = svc
            .delete(
                rim_services::workspace_state::request::DeleteWorkspaceStateCmd {
                    actor: Actor::new(actor),
                    server_id,
                    key: input.key,
                    scope: input.scope.into(),
                },
            )
            .await
            .map_err(map_service_error)?;

        Ok(deleted)
    }

    async fn increment_workspace_state(
        &self,
        ctx: &Context<'_>,
        input: crate::types::IncrementWorkspaceStateInput,
    ) -> Result<crate::types::WorkspaceStateEntryGql> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<
            dyn rim_services::workspace_state::service::WorkspaceStateService,
        >>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&input.server_id)?;

        let entry = svc
            .increment(
                rim_services::workspace_state::request::IncrementWorkspaceStateCmd {
                    actor: Actor::new(actor),
                    server_id,
                    key: input.key,
                    scope: input.scope.into(),
                    delta: input.delta,
                },
            )
            .await
            .map_err(map_service_error)?;

        Ok(entry.into())
    }

    async fn create_pipeline(
        &self,
        ctx: &Context<'_>,
        input: crate::types::CreatePipelineInput,
    ) -> Result<crate::types::PipelineGql> {
        let actor = require_actor(ctx)?;
        let svc =
            ctx.data::<std::sync::Arc<dyn rim_services::pipeline::service::PipelineService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&input.server_id)?;

        let trigger = build_pipeline_trigger_dto(input.trigger)?;
        let filter = input.filter.map(build_pipeline_filter_dto).transpose()?;
        let actions = input
            .actions
            .into_iter()
            .map(
                |a| rim_services::pipeline::dto::PipelineActionDto::ExecuteCommand {
                    command_id: a.command_id,
                    args: a.args.map(|j| j.0),
                },
            )
            .collect();

        let pipeline = svc
            .create(rim_services::pipeline::request::CreatePipelineCmd {
                actor: Actor::new(actor),
                server_id,
                name: input.name,
                description: input.description,
                trigger,
                filter,
                actions,
                cooldown_secs: input.cooldown_secs,
            })
            .await
            .map_err(map_service_error)?;

        Ok(pipeline.into())
    }

    async fn update_pipeline(
        &self,
        ctx: &Context<'_>,
        input: crate::types::UpdatePipelineInput,
    ) -> Result<crate::types::PipelineGql> {
        let actor = require_actor(ctx)?;
        let svc =
            ctx.data::<std::sync::Arc<dyn rim_services::pipeline::service::PipelineService>>()?;
        let pipeline_id =
            parse_uuid_id::<rim_core::pipeline::model::PipelineId>(&input.pipeline_id)?;

        let trigger = build_pipeline_trigger_dto(input.trigger)?;
        let filter = input.filter.map(build_pipeline_filter_dto).transpose()?;
        let actions = input
            .actions
            .into_iter()
            .map(
                |a| rim_services::pipeline::dto::PipelineActionDto::ExecuteCommand {
                    command_id: a.command_id,
                    args: a.args.map(|j| j.0),
                },
            )
            .collect();

        let pipeline = svc
            .update(rim_services::pipeline::request::UpdatePipelineCmd {
                actor: Actor::new(actor),
                pipeline_id,
                name: input.name,
                description: input.description,
                trigger,
                filter,
                actions,
                cooldown_secs: input.cooldown_secs,
            })
            .await
            .map_err(map_service_error)?;

        Ok(pipeline.into())
    }

    async fn delete_pipeline(
        &self,
        ctx: &Context<'_>,
        pipeline_id: async_graphql::ID,
    ) -> Result<bool> {
        let actor = require_actor(ctx)?;
        let svc =
            ctx.data::<std::sync::Arc<dyn rim_services::pipeline::service::PipelineService>>()?;
        let pipeline_id = parse_uuid_id::<rim_core::pipeline::model::PipelineId>(&pipeline_id)?;

        svc.delete(rim_services::pipeline::request::DeletePipelineCmd {
            actor: Actor::new(actor),
            pipeline_id,
        })
        .await
        .map_err(map_service_error)?;

        Ok(true)
    }

    async fn toggle_pipeline(
        &self,
        ctx: &Context<'_>,
        pipeline_id: async_graphql::ID,
        enabled: bool,
    ) -> Result<crate::types::PipelineGql> {
        let actor = require_actor(ctx)?;
        let svc =
            ctx.data::<std::sync::Arc<dyn rim_services::pipeline::service::PipelineService>>()?;
        let pipeline_id = parse_uuid_id::<rim_core::pipeline::model::PipelineId>(&pipeline_id)?;

        let pipeline = svc
            .toggle(rim_services::pipeline::request::TogglePipelineCmd {
                actor: Actor::new(actor),
                pipeline_id,
                enabled,
            })
            .await
            .map_err(map_service_error)?;

        Ok(pipeline.into())
    }

    /// Update fields on the authenticated user's profile.
    ///
    /// Returns the full updated `User` so the client can refresh local cache
    /// without an extra `me { ... }` round-trip.
    ///
    /// # Errors
    ///
    /// - `UNAUTHENTICATED` — no session.
    /// - `VALIDATION` — `displayName` > 64 chars, `statusText` > 256 chars,
    ///   `bio` > 2000 chars, or `avatarUrl` not under `/api/files/`.
    /// - `NOT_FOUND` — caller's user row missing (should not happen in practice).
    /// - `INTERNAL` — repository failure.
    ///
    /// # Examples
    ///
    /// ```graphql
    /// mutation Update($input: UpdateMyProfileInput!) {
    ///   updateMyProfile(input: $input) {
    ///     id username displayName avatarUrl statusText bio
    ///   }
    /// }
    /// ```
    async fn update_my_profile(
        &self,
        ctx: &Context<'_>,
        input: UpdateMyProfileInput,
    ) -> Result<User> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn UserService>>()?;

        let dto = svc
            .update_my_profile(UpdateMyProfileCmd {
                actor: Actor::new(actor),
                display_name: input.display_name,
                avatar_url: input.avatar_url,
                status_text: input.status_text,
                bio: input.bio,
            })
            .await
            .map_err(map_service_error)?;

        Ok(dto.into())
    }

    /// Create an on-call schedule. Owner-only.
    async fn create_on_call_schedule(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
        name: String,
        timezone: String,
    ) -> Result<crate::types::OnCallScheduleGql> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn rim_services::oncall::service::OnCallService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;

        let dto = svc
            .create_schedule(rim_services::oncall::command::CreateOnCallScheduleCmd {
                actor: Actor::new(actor),
                server_id,
                name,
                timezone,
            })
            .await
            .map_err(map_service_error)?;

        Ok(dto.into())
    }

    /// Update an on-call schedule's `name`/`timezone`. Owner-only.
    async fn update_on_call_schedule(
        &self,
        ctx: &Context<'_>,
        schedule_id: ID,
        name: Option<String>,
        timezone: Option<String>,
    ) -> Result<crate::types::OnCallScheduleGql> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn rim_services::oncall::service::OnCallService>>()?;
        let schedule_id = parse_uuid_id::<rim_core::oncall::model::OnCallScheduleId>(&schedule_id)?;

        let dto = svc
            .update_schedule(rim_services::oncall::command::UpdateOnCallScheduleCmd {
                actor: Actor::new(actor),
                schedule_id,
                name,
                timezone,
            })
            .await
            .map_err(map_service_error)?;

        Ok(dto.into())
    }

    /// Delete an on-call schedule (cascades to its rotation layers/participants/overrides).
    /// Owner-only.
    async fn delete_on_call_schedule(&self, ctx: &Context<'_>, schedule_id: ID) -> Result<bool> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn rim_services::oncall::service::OnCallService>>()?;
        let schedule_id = parse_uuid_id::<rim_core::oncall::model::OnCallScheduleId>(&schedule_id)?;

        svc.delete_schedule(rim_services::oncall::command::DeleteOnCallScheduleCmd {
            actor: Actor::new(actor),
            schedule_id,
        })
        .await
        .map_err(map_service_error)?;

        Ok(true)
    }

    /// Add a rotation layer to a schedule. Owner-only.
    #[allow(clippy::too_many_arguments)]
    async fn add_rotation_layer(
        &self,
        ctx: &Context<'_>,
        schedule_id: ID,
        name: String,
        priority: i32,
        kind: crate::types::RotationKindInputGql,
        anchor_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<crate::types::RotationLayerGql> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn rim_services::oncall::service::OnCallService>>()?;
        let schedule_id = parse_uuid_id::<rim_core::oncall::model::OnCallScheduleId>(&schedule_id)?;

        let dto = svc
            .add_rotation_layer(rim_services::oncall::command::AddRotationLayerCmd {
                actor: Actor::new(actor),
                schedule_id,
                name,
                priority,
                kind: kind.try_into()?,
                anchor_at,
            })
            .await
            .map_err(map_service_error)?;

        Ok(dto.into())
    }

    /// Update a rotation layer's `name`/`priority`/`kind`/`anchorAt`. Owner-only (resolved via
    /// the layer's owning schedule).
    #[allow(clippy::too_many_arguments)]
    async fn update_rotation_layer(
        &self,
        ctx: &Context<'_>,
        layer_id: ID,
        name: Option<String>,
        priority: Option<i32>,
        kind: Option<crate::types::RotationKindInputGql>,
        anchor_at: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<crate::types::RotationLayerGql> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn rim_services::oncall::service::OnCallService>>()?;
        let layer_id = parse_uuid_id::<rim_core::oncall::model::RotationLayerId>(&layer_id)?;

        let dto = svc
            .update_rotation_layer(rim_services::oncall::command::UpdateRotationLayerCmd {
                actor: Actor::new(actor),
                layer_id,
                name,
                priority,
                kind: kind.map(TryInto::try_into).transpose()?,
                anchor_at,
            })
            .await
            .map_err(map_service_error)?;

        Ok(dto.into())
    }

    /// Remove a rotation layer (cascades to its participants). Owner-only.
    async fn remove_rotation_layer(&self, ctx: &Context<'_>, layer_id: ID) -> Result<bool> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn rim_services::oncall::service::OnCallService>>()?;
        let layer_id = parse_uuid_id::<rim_core::oncall::model::RotationLayerId>(&layer_id)?;

        svc.remove_rotation_layer(rim_services::oncall::command::RemoveRotationLayerCmd {
            actor: Actor::new(actor),
            layer_id,
        })
        .await
        .map_err(map_service_error)?;

        Ok(true)
    }

    /// Replace a rotation layer's full ordered participant list. `userIds[0]` becomes the first
    /// shift. Owner-only.
    async fn set_rotation_participants(
        &self,
        ctx: &Context<'_>,
        layer_id: ID,
        user_ids: Vec<ID>,
    ) -> Result<crate::types::RotationLayerGql> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn rim_services::oncall::service::OnCallService>>()?;
        let layer_id = parse_uuid_id::<rim_core::oncall::model::RotationLayerId>(&layer_id)?;
        let user_ids = user_ids
            .iter()
            .map(parse_uuid_id::<rim_core::user::model::UserId>)
            .collect::<Result<Vec<_>>>()?;

        let dto = svc
            .set_rotation_participants(rim_services::oncall::command::SetRotationParticipantsCmd {
                actor: Actor::new(actor),
                layer_id,
                user_ids,
            })
            .await
            .map_err(map_service_error)?;

        Ok(dto.into())
    }

    /// Create a one-off on-call override for a schedule. Owner-only.
    async fn create_on_call_override(
        &self,
        ctx: &Context<'_>,
        schedule_id: ID,
        user_id: ID,
        start_at: chrono::DateTime<chrono::Utc>,
        end_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<crate::types::OnCallOverrideGql> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn rim_services::oncall::service::OnCallService>>()?;
        let schedule_id = parse_uuid_id::<rim_core::oncall::model::OnCallScheduleId>(&schedule_id)?;
        let user_id = parse_uuid_id::<rim_core::user::model::UserId>(&user_id)?;

        let dto = svc
            .create_override(rim_services::oncall::command::CreateOnCallOverrideCmd {
                actor: Actor::new(actor),
                schedule_id,
                user_id,
                start_at,
                end_at,
            })
            .await
            .map_err(map_service_error)?;

        Ok(dto.into())
    }

    /// Delete an on-call override. Owner-only (resolved via the override's owning schedule).
    async fn delete_on_call_override(&self, ctx: &Context<'_>, override_id: ID) -> Result<bool> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn rim_services::oncall::service::OnCallService>>()?;
        let override_id = parse_uuid_id::<rim_core::oncall::model::OnCallOverrideId>(&override_id)?;

        svc.delete_override(rim_services::oncall::command::DeleteOnCallOverrideCmd {
            actor: Actor::new(actor),
            override_id,
        })
        .await
        .map_err(map_service_error)?;

        Ok(true)
    }
}

fn build_pipeline_trigger_dto(
    input: crate::types::PipelineTriggerInput,
) -> async_graphql::Result<rim_services::pipeline::dto::PipelineTriggerDto> {
    let conversation_id = input
        .conversation_id
        .as_ref()
        .map(parse_uuid_id::<rim_core::conversation::model::ConversationId>)
        .transpose()?;
    Ok(rim_services::pipeline::dto::PipelineTriggerDto {
        kind: input.kind.into(),
        conversation_id,
    })
}

fn build_pipeline_filter_dto(
    input: crate::types::PipelineFilterInput,
) -> async_graphql::Result<rim_services::pipeline::dto::PipelineFilterDto> {
    let mut conditions = Vec::new();
    for c in input.conditions {
        let cond = match c.condition_type.as_str() {
            "MessageContains" => {
                let pattern = c.pattern.ok_or_else(|| {
                    async_graphql::Error::new("MessageContains requires a pattern")
                })?;
                rim_services::pipeline::dto::FilterConditionDto::MessageContains { pattern }
            }
            "MessageFromUser" => {
                let uid_str = c.user_id.ok_or_else(|| {
                    async_graphql::Error::new("MessageFromUser requires a user_id")
                })?;
                let user_id = parse_uuid_id::<rim_core::user::model::UserId>(&uid_str)?;
                rim_services::pipeline::dto::FilterConditionDto::MessageFromUser { user_id }
            }
            "ConversationIs" => {
                let conv_str = c.conversation_id.ok_or_else(|| {
                    async_graphql::Error::new("ConversationIs requires a conversation_id")
                })?;
                let conversation_id =
                    parse_uuid_id::<rim_core::conversation::model::ConversationId>(&conv_str)?;
                rim_services::pipeline::dto::FilterConditionDto::ConversationIs { conversation_id }
            }
            other => {
                return Err(async_graphql::Error::new(format!(
                    "unknown filter condition type: {other}"
                )));
            }
        };
        conditions.push(cond);
    }
    Ok(rim_services::pipeline::dto::PipelineFilterDto { conditions })
}

#[derive(InputObject)]
struct StartIncidentInput {
    server_id: ID,
    conversation_id: ID,
    title: String,
    severity: Option<crate::types::IncidentSeverityGql>,
    /// Overrides the conversation's default escalation policy (`setChannelEscalationPolicy`) for
    /// just this incident (§3/§10). `None` falls back to that default (or no escalation chain at
    /// all if none is set).
    escalation_policy_id: Option<ID>,
}

#[derive(InputObject)]
struct UpdateIncidentStatusInput {
    incident_id: ID,
    status: crate::types::IncidentStatusGql,
}

#[derive(InputObject)]
struct AdvanceIncidentPhaseInput {
    incident_id: ID,
    next_phase: crate::types::IncidentPhaseNameGql,
    note: Option<String>,
}

#[derive(InputObject)]
struct TagIncidentMessageInput {
    incident_id: ID,
    message_id: ID,
    tag_type: Option<crate::types::MessageTagTypeGql>,
}

#[derive(InputObject)]
struct CreateIngestTokenInput {
    server_id: ID,
    conversation_id: ID,
    label: String,
    severity_default: Option<crate::types::IncidentSeverityGql>,
}

#[derive(InputObject)]
struct CreateMacroInput {
    server_id: ID,
    name: String,
    description: Option<String>,
    steps: Vec<MacroStepInput>,
    #[graphql(default)]
    ai_callable: bool,
    #[graphql(default)]
    nl_description: String,
    #[graphql(default)]
    intent_samples: Vec<String>,
    #[graphql(default_with = "CommandSensitivity::Safe")]
    sensitivity: CommandSensitivity,
}

#[derive(InputObject)]
struct UpdateMacroInput {
    macro_id: ID,
    name: String,
    description: Option<String>,
    steps: Vec<MacroStepInput>,
    #[graphql(default)]
    ai_callable: bool,
    #[graphql(default)]
    nl_description: String,
    #[graphql(default)]
    intent_samples: Vec<String>,
    #[graphql(default_with = "CommandSensitivity::Safe")]
    sensitivity: CommandSensitivity,
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
    /// **Deprecated** — see `ExecuteCommandInput.confirmed_dangerous`.
    /// Removed in Phase 4.
    confirmed_dangerous: Option<bool>,
    /// Server-issued confirmation proof. Same shape and semantics as
    /// `ExecuteCommandInput.confirmation`. Phase 1 note: macros are an
    /// "all-or-nothing" confirmation surface — a single proof issued
    /// at macro-execute time covers the whole expansion. Phase 4 may
    /// add per-step proofs once the modal can render a step list.
    confirmation: Option<crate::types::ConfirmationProofInput>,
    /// Who triggered this macro. Same back-compat semantics as
    /// `ExecuteCommandInput.initiator`: optional, server-defaults to
    /// `PALETTE`.
    initiator: Option<CommandInitiator>,
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

#[derive(InputObject)]
struct CreateChannelLensInput {
    server_id: ID,
    conversation_id: ID,
    name: String,
    filter_spec: crate::types::FilterSpecInput,
    is_shared: Option<bool>,
}

#[derive(InputObject)]
struct UpdateChannelLensInput {
    server_id: ID,
    lens_id: ID,
    name: Option<String>,
    filter_spec: Option<crate::types::FilterSpecInput>,
    is_shared: Option<bool>,
}

// ── Registration inputs ───────────────────────────────────────────────────

/// Input for `signupWithInvite`.
///
/// Exactly one token MUST be set in invite-only mode; all must be absent in
/// open mode. The resolver enforces this with a `Validation` error.
///
/// `email` is required (max 254 chars, server trims + lowercases, case-insensitive unique).
/// `displayName` is optional; falls back to `username` server-side if absent or blank.
#[derive(InputObject)]
pub struct SignupWithInviteInput {
    pub username: String,
    /// Required. The user's email address. Trimmed and lowercased before storage.
    /// Must match the pragmatic email regex from spec §13.3.
    pub email: String,
    /// Optional display name (1–80 chars when present). Falls back to `username`
    /// when absent or blank after trim.
    pub display_name: Option<String>,
    pub password: String,
    pub bootstrap_token: Option<String>,
    pub instance_invite_token: Option<String>,
    pub workspace_invite_token: Option<String>,
}

/// Input for `createInstanceInvite`.
///
/// Both fields are optional: `expires_at` defaults to `now() + 7 days`,
/// `max_uses` defaults to 1.
#[derive(InputObject)]
pub struct CreateInstanceInviteInput {
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    pub max_uses: Option<i32>,
    /// Optional recipient to email the invite link to (Track B3). Validated server-side;
    /// malformed addresses return a validation error.
    pub recipient_email: Option<String>,
}

fn spawn_link_preview_fetch(
    ctx: &Context<'_>,
    message_id: rim_core::message::model::MessageId,
    workspace_id: rim_core::server::model::ServerId,
    conversation_id: rim_core::conversation::model::ConversationId,
    text: &str,
) -> Result<()> {
    let urls = rim_services::link_preview::extract_urls(text);
    if urls.is_empty() {
        return Ok(());
    }
    let link_svc = ctx
        .data::<std::sync::Arc<dyn rim_services::link_preview::LinkPreviewService>>()?
        .clone();
    let observers = ctx
        .data::<std::sync::Arc<rim_services::events::Observers>>()?
        .clone();

    tokio::spawn(async move {
        match link_svc.fetch_and_store(message_id, urls).await {
            Ok(previews) if !previews.is_empty() => {
                use rim_services::events::EventObserver;
                observers.emit(rim_services::events::WorkspaceEvent::LinkPreviewReady {
                    workspace_id,
                    conversation_id,
                    message_id,
                });
            }
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(error = %e, "link preview fetch failed");
            }
        }
    });

    Ok(())
}
