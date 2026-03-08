use async_graphql::*;
use chrono::{DateTime, Utc};
use rim_services::auth::service::AuthService;
use rim_services::conversation::command::CreateConversationCmd;
use rim_services::conversation::service::ConversationService;
use rim_services::message::command::SendMessageCmd;
use rim_services::message::command::{DeleteMessageCmd, EditMessageCmd, SendThreadMessageCmd};
use rim_services::message::service::MessageService;
use rim_services::message_thread::command::CreateThreadCmd;
use rim_services::message_thread::service::ThreadService;
use rim_services::{Actor, ServiceError};

use crate::auth_intent::{AuthIntent, AuthIntentCtx};
use crate::ids::parse_uuid_id;
use crate::query::AuthActorCtx;
use crate::types::{Conversation, Message, Thread};

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
pub struct SendMessageInput {
    pub server_id: ID,
    pub conversation_id: ID,
    pub text: String,
    pub parent_message_id: Option<ID>,
}

#[derive(InputObject)]
pub struct SendThreadMessageInput {
    pub server_id: ID,
    pub conversation_id: ID,
    pub thread_id: ID,
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
pub struct CreateThreadInput {
    pub server_id: ID,
    pub conversation_id: ID,
    pub root_message_id: ID,
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
            .bootstrap(
                &input.secret,
                &input.username,
                &input.password,
                &input.server_name,
            )
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
            .login(&input.username, &input.password)
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
        let actor = ctx
            .data_unchecked::<AuthActorCtx>()
            .actor()
            .ok_or_else(|| Error::new("unauthenticated"))?;
        let session_id = ctx
            .data_unchecked::<AuthActorCtx>()
            .session_id()
            .ok_or_else(|| Error::new("unauthenticated"))?;

        let auth = ctx.data::<std::sync::Arc<dyn AuthService>>()?;
        auth.logout(Actor::new(actor), session_id)
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
        let actor = ctx
            .data_unchecked::<AuthActorCtx>()
            .actor()
            .ok_or_else(|| Error::new("unauthenticated"))?;

        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&input.server_id)?;
        let auth = ctx.data::<std::sync::Arc<dyn AuthService>>()?;
        let res = auth
            .create_invite(
                Actor::new(actor),
                server_id,
                input.expires_at,
                input.max_uses.unwrap_or(1),
            )
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
            .accept_invite(&input.invite_token, &input.username, &input.password)
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

    async fn send_message(&self, ctx: &Context<'_>, input: SendMessageInput) -> Result<Message> {
        let actor = ctx
            .data_unchecked::<AuthActorCtx>()
            .actor()
            .ok_or_else(|| Error::new("unauthenticated"))?;

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

    async fn create_conversation(
        &self,
        ctx: &Context<'_>,
        input: CreateConversationInput,
    ) -> Result<Conversation> {
        let actor = ctx
            .data_unchecked::<AuthActorCtx>()
            .actor()
            .ok_or_else(|| Error::new("unauthenticated"))?;

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

    async fn send_thread_message(
        &self,
        ctx: &Context<'_>,
        input: SendThreadMessageInput,
    ) -> Result<Message> {
        let actor = ctx
            .data_unchecked::<AuthActorCtx>()
            .actor()
            .ok_or_else(|| Error::new("unauthenticated"))?;

        let svc = ctx.data::<std::sync::Arc<dyn MessageService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&input.server_id)?;
        let conversation_id =
            parse_uuid_id::<rim_core::conversation::model::ConversationId>(&input.conversation_id)?;
        let thread_id =
            parse_uuid_id::<rim_core::message_thread::model::MessageThreadId>(&input.thread_id)?;
        let parent_message_id = input
            .parent_message_id
            .as_ref()
            .map(parse_uuid_id::<rim_core::message::model::MessageId>)
            .transpose()?;

        let msg = svc
            .send_thread_message(SendThreadMessageCmd {
                actor: Actor::new(actor),
                server_id,
                conversation_id,
                thread_id,
                text: Some(input.text),
                parent_message_id,
                attachments: vec![],
            })
            .await
            .map_err(map_service_error)?;

        Ok(msg.into())
    }

    async fn edit_message(&self, ctx: &Context<'_>, input: EditMessageInput) -> Result<Message> {
        let actor = ctx
            .data_unchecked::<AuthActorCtx>()
            .actor()
            .ok_or_else(|| Error::new("unauthenticated"))?;

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
        let actor = ctx
            .data_unchecked::<AuthActorCtx>()
            .actor()
            .ok_or_else(|| Error::new("unauthenticated"))?;

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

    async fn create_thread(&self, ctx: &Context<'_>, input: CreateThreadInput) -> Result<Thread> {
        let actor = ctx
            .data_unchecked::<AuthActorCtx>()
            .actor()
            .ok_or_else(|| Error::new("unauthenticated"))?;

        let svc = ctx.data::<std::sync::Arc<dyn ThreadService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&input.server_id)?;
        let conversation_id =
            parse_uuid_id::<rim_core::conversation::model::ConversationId>(&input.conversation_id)?;
        let root_message_id =
            parse_uuid_id::<rim_core::message::model::MessageId>(&input.root_message_id)?;

        let thread = svc
            .create_thread(CreateThreadCmd {
                actor: Actor::new(actor),
                server_id,
                conversation_id,
                root_message_id,
            })
            .await
            .map_err(map_service_error)?;

        Ok(thread.into())
    }
}

pub(crate) fn map_service_error(err: ServiceError) -> Error {
    match err {
        ServiceError::PermissionDenied => {
            Error::new("permission denied").extend_with(|_err, e| e.set("code", "FORBIDDEN"))
        }
        ServiceError::Validation(m) => {
            Error::new(m).extend_with(|_err, e| e.set("code", "VALIDATION"))
        }
        ServiceError::Conflict(m) => Error::new(m).extend_with(|_err, e| e.set("code", "CONFLICT")),
        ServiceError::NotFound => {
            Error::new("not found").extend_with(|_err, e| e.set("code", "NOT_FOUND"))
        }
        ServiceError::Unavailable => {
            Error::new("unavailable").extend_with(|_err, e| e.set("code", "UNAVAILABLE"))
        }
        ServiceError::Timeout => {
            Error::new("timeout").extend_with(|_err, e| e.set("code", "TIMEOUT"))
        }
        ServiceError::Internal(_) => {
            Error::new("internal error").extend_with(|_err, e| e.set("code", "INTERNAL"))
        }
    }
}
