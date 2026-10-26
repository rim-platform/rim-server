//! Extension trait on [`async_graphql::Context`] that provides clean, typed
//! helpers for resolving nested GraphQL fields via DataLoaders.
//!
//! Resolvers should call these helpers instead of interacting with DataLoaders
//! directly. This keeps loader wiring (key construction, error remapping) in
//! one place and keeps resolver code focused on field semantics.

use async_graphql::{Context, Result};

use rim_core::message::model::MessageId;
use rim_core::message_thread::model::MessageThreadId;
use rim_core::user::model::UserId;
use rim_services::ServiceError;
use rim_services::conversation::command::GetConversationQuery;
use rim_services::conversation::dto::ConversationDto;
use rim_services::conversation::service::ConversationService;

use crate::loaders::{
    LinkPreviewLoader, MentionLoader, PinLoader, ReactionLoader, ReactionLoaderKey,
    ThreadReplyCountLoader, UserLoader,
};
use crate::types::{LinkPreview, ReactionGroup, User};

/// Helper to convert a `DataLoader` error (`Arc<async_graphql::Error>`) into a
/// plain `async_graphql::Error`, preserving extensions and source.
fn loader_err(e: std::sync::Arc<async_graphql::Error>) -> async_graphql::Error {
    std::sync::Arc::try_unwrap(e).unwrap_or_else(|arc| (*arc).clone())
}

/// Convenience accessors that wrap DataLoader calls behind typed helpers.
pub trait ResolveExt {
    /// Load a single user by their GraphQL ID string.
    /// Returns `None` if the user does not exist.
    fn load_user(
        &self,
        graphql_id: &async_graphql::ID,
    ) -> impl Future<Output = Result<Option<User>>> + Send;

    /// Load multiple users by their domain IDs, preserving input ordering.
    fn load_users(&self, ids: Vec<UserId>) -> impl Future<Output = Result<Vec<User>>> + Send;

    /// Load reaction groups for a message, resolved relative to the current actor.
    fn load_reactions(
        &self,
        message_id: &async_graphql::ID,
    ) -> impl Future<Output = Result<Vec<ReactionGroup>>> + Send;

    /// Check whether a message is pinned.
    fn load_is_pinned(
        &self,
        message_id: &async_graphql::ID,
    ) -> impl Future<Output = Result<bool>> + Send;

    /// Count the live replies in a thread; 0 when it has none.
    fn load_thread_reply_count(
        &self,
        thread_id: &async_graphql::ID,
    ) -> impl Future<Output = Result<i64>> + Send;

    /// Load mentioned user IDs for a message.
    fn load_mentioned_user_ids(
        &self,
        message_id: &async_graphql::ID,
    ) -> impl Future<Output = Result<Vec<UserId>>> + Send;

    /// Load mentioned users (fully resolved) for a message.
    fn load_mentions(
        &self,
        message_id: &async_graphql::ID,
    ) -> impl Future<Output = Result<Vec<User>>> + Send;

    /// Load link previews for a message.
    fn load_link_previews(
        &self,
        message_id: &async_graphql::ID,
    ) -> impl Future<Output = Result<Vec<LinkPreview>>> + Send;

    /// Load a conversation the current actor can read; `None` when they cannot (not a member of
    /// a private channel, removed from a group) or it is gone.
    fn load_readable_conversation(
        &self,
        server_id: &async_graphql::ID,
        conversation_id: &async_graphql::ID,
    ) -> impl Future<Output = Result<Option<ConversationDto>>> + Send;
}

impl ResolveExt for Context<'_> {
    async fn load_user(&self, graphql_id: &async_graphql::ID) -> Result<Option<User>> {
        let loader = self.data::<async_graphql::dataloader::DataLoader<UserLoader>>()?;
        let user_id = crate::ids::parse_uuid_id::<UserId>(graphql_id)?;
        let user = loader.load_one(user_id).await.map_err(loader_err)?;
        Ok(user.map(Into::into))
    }

    async fn load_users(&self, ids: Vec<UserId>) -> Result<Vec<User>> {
        if ids.is_empty() {
            return Ok(vec![]);
        }
        let loader = self.data::<async_graphql::dataloader::DataLoader<UserLoader>>()?;
        let mut users = loader.load_many(ids.clone()).await.map_err(loader_err)?;
        // Preserve caller's ordering — HashMap iteration is non-deterministic.
        Ok(ids
            .iter()
            .filter_map(|id| users.remove(id).map(Into::into))
            .collect())
    }

    async fn load_reactions(&self, message_id: &async_graphql::ID) -> Result<Vec<ReactionGroup>> {
        let loader = self.data::<async_graphql::dataloader::DataLoader<ReactionLoader>>()?;
        let actor_user_id = crate::auth_context::require_actor(self)?;
        let msg_id = crate::ids::parse_uuid_id::<MessageId>(message_id)?;
        let key = ReactionLoaderKey {
            message_id: msg_id,
            actor_user_id,
        };
        let groups = loader
            .load_one(key)
            .await
            .map_err(loader_err)?
            .unwrap_or_default();
        Ok(groups.into_iter().map(Into::into).collect())
    }

    async fn load_is_pinned(&self, message_id: &async_graphql::ID) -> Result<bool> {
        let loader = self.data::<async_graphql::dataloader::DataLoader<PinLoader>>()?;
        let msg_id = crate::ids::parse_uuid_id::<MessageId>(message_id)?;
        let pinned = loader
            .load_one(msg_id)
            .await
            .map_err(loader_err)?
            .unwrap_or(false);
        Ok(pinned)
    }

    async fn load_thread_reply_count(&self, thread_id: &async_graphql::ID) -> Result<i64> {
        let loader =
            self.data::<async_graphql::dataloader::DataLoader<ThreadReplyCountLoader>>()?;
        let thread_id = crate::ids::parse_uuid_id::<MessageThreadId>(thread_id)?;
        let replies = loader
            .load_one(thread_id)
            .await
            .map_err(loader_err)?
            .unwrap_or(0);
        Ok(replies)
    }

    async fn load_mentioned_user_ids(&self, message_id: &async_graphql::ID) -> Result<Vec<UserId>> {
        let loader = self.data::<async_graphql::dataloader::DataLoader<MentionLoader>>()?;
        let msg_id = crate::ids::parse_uuid_id::<MessageId>(message_id)?;
        let user_ids = loader
            .load_one(msg_id)
            .await
            .map_err(loader_err)?
            .unwrap_or_default();
        Ok(user_ids)
    }

    async fn load_mentions(&self, message_id: &async_graphql::ID) -> Result<Vec<User>> {
        let user_ids = self.load_mentioned_user_ids(message_id).await?;
        self.load_users(user_ids).await
    }

    async fn load_link_previews(&self, message_id: &async_graphql::ID) -> Result<Vec<LinkPreview>> {
        let loader = self.data::<async_graphql::dataloader::DataLoader<LinkPreviewLoader>>()?;
        let msg_id = crate::ids::parse_uuid_id::<MessageId>(message_id)?;
        let previews = loader
            .load_one(msg_id)
            .await
            .map_err(loader_err)?
            .unwrap_or_default();
        Ok(previews)
    }

    async fn load_readable_conversation(
        &self,
        server_id: &async_graphql::ID,
        conversation_id: &async_graphql::ID,
    ) -> Result<Option<ConversationDto>> {
        let actor = crate::auth_context::require_actor(self)?;
        let svc = self.data::<std::sync::Arc<dyn ConversationService>>()?;
        let res = svc
            .get_conversation(GetConversationQuery {
                actor: rim_services::Actor::new(actor),
                server_id: crate::ids::parse_uuid_id(server_id)?,
                conversation_id: crate::ids::parse_uuid_id(conversation_id)?,
            })
            .await;
        match res {
            Ok(conversation) => Ok(Some(conversation)),
            Err(ServiceError::NotFound | ServiceError::PermissionDenied) => Ok(None),
            Err(e) => Err(crate::errors::map_service_error(e)),
        }
    }
}
