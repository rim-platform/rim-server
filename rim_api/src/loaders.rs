use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use async_graphql::dataloader::Loader;

use rim_core::message::model::MessageId;
use rim_core::user::model::UserId;
use rim_services::Actor;
use rim_services::link_preview::LinkPreviewService;
use rim_services::mention::service::MentionService;
use rim_services::message_reaction::command::GetReactionsForMessagesQuery;
use rim_services::message_reaction::dto::ReactionGroupDto;
use rim_services::message_reaction::service::MessageReactionService;
use rim_services::pin::service::PinService;
use rim_services::user::dto::UserDto;
use rim_services::user::service::UserService;

use crate::errors::map_service_error;

/// A failed batch as every key of it sees the error: the same code and message as a root field.
fn loader_error(err: rim_services::ServiceError) -> Arc<async_graphql::Error> {
    Arc::new(map_service_error(err))
}

// ── UserLoader ─────────────────────────────────────────────────────────────

pub struct UserLoader {
    user_service: Arc<dyn UserService>,
}

impl UserLoader {
    pub fn new(user_service: Arc<dyn UserService>) -> Self {
        Self { user_service }
    }
}

impl Loader<UserId> for UserLoader {
    type Value = UserDto;
    type Error = Arc<async_graphql::Error>;

    fn load(
        &self,
        keys: &[UserId],
    ) -> impl Future<Output = Result<HashMap<UserId, Self::Value>, Self::Error>> + Send {
        let keys = keys.to_vec();
        let svc = Arc::clone(&self.user_service);
        async move {
            let users = svc.get_users_by_ids(&keys).await.map_err(loader_error)?;
            Ok(users.into_iter().map(|u| (u.id, u)).collect())
        }
    }
}

// ── ReactionLoader ─────────────────────────────────────────────────────────

/// Key for the reaction loader: a (MessageId, actor UserId) pair.
/// The actor is needed because `reacted_by_me` depends on who is viewing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ReactionLoaderKey {
    pub message_id: MessageId,
    pub actor_user_id: UserId,
}

pub struct ReactionLoader {
    reaction_service: Arc<dyn MessageReactionService>,
}

impl ReactionLoader {
    pub fn new(reaction_service: Arc<dyn MessageReactionService>) -> Self {
        Self { reaction_service }
    }
}

impl Loader<ReactionLoaderKey> for ReactionLoader {
    type Value = Vec<ReactionGroupDto>;
    type Error = Arc<async_graphql::Error>;

    fn load(
        &self,
        keys: &[ReactionLoaderKey],
    ) -> impl Future<Output = Result<HashMap<ReactionLoaderKey, Self::Value>, Self::Error>> + Send
    {
        let keys = keys.to_vec();
        let svc = Arc::clone(&self.reaction_service);
        async move {
            if keys.is_empty() {
                return Ok(HashMap::new());
            }

            // All keys in a single request share the same actor.
            let actor_user_id = keys[0].actor_user_id;
            let message_ids: Vec<MessageId> = keys.iter().map(|k| k.message_id).collect();

            let reaction_map = svc
                .get_reactions_for_messages(GetReactionsForMessagesQuery {
                    actor: Actor::new(actor_user_id),
                    message_ids,
                })
                .await
                .map_err(loader_error)?;

            Ok(keys
                .into_iter()
                .map(|k| {
                    let reactions = reaction_map.get(&k.message_id).cloned().unwrap_or_default();
                    (k, reactions)
                })
                .collect())
        }
    }
}

// ── MentionLoader ─────────────────────────────────────────────────────────

/// Batches mention lookups by MessageId.
pub struct MentionLoader {
    mention_service: Arc<dyn MentionService>,
}

impl MentionLoader {
    pub fn new(mention_service: Arc<dyn MentionService>) -> Self {
        Self { mention_service }
    }
}

impl Loader<MessageId> for MentionLoader {
    type Value = Vec<UserId>;
    type Error = Arc<async_graphql::Error>;

    fn load(
        &self,
        keys: &[MessageId],
    ) -> impl Future<Output = Result<HashMap<MessageId, Self::Value>, Self::Error>> + Send {
        let keys = keys.to_vec();
        let svc = Arc::clone(&self.mention_service);
        async move {
            let mention_map = svc
                .get_mentions_for_messages(&keys)
                .await
                .map_err(loader_error)?;

            Ok(keys
                .into_iter()
                .map(|id| {
                    let mentions = mention_map.get(&id).cloned().unwrap_or_default();
                    (id, mentions)
                })
                .collect())
        }
    }
}

// ── PinLoader ─────────────────────────────────────────────────────────────

/// Batches `isPinned` checks by MessageId.
pub struct PinLoader {
    pin_service: Arc<dyn PinService>,
}

impl PinLoader {
    pub fn new(pin_service: Arc<dyn PinService>) -> Self {
        Self { pin_service }
    }
}

impl Loader<MessageId> for PinLoader {
    /// `true` if the message is pinned, absent if not.
    type Value = bool;
    type Error = Arc<async_graphql::Error>;

    fn load(
        &self,
        keys: &[MessageId],
    ) -> impl Future<Output = Result<HashMap<MessageId, Self::Value>, Self::Error>> + Send {
        let keys = keys.to_vec();
        let svc = Arc::clone(&self.pin_service);
        async move {
            let pinned_ids = svc.is_pinned_batch(&keys).await.map_err(loader_error)?;

            let pinned_set: HashSet<MessageId> = pinned_ids.into_iter().collect();
            Ok(keys
                .into_iter()
                .map(|id| (id, pinned_set.contains(&id)))
                .collect())
        }
    }
}

// ── LinkPreviewLoader ─────────────────────────────────────────────────────

pub struct LinkPreviewLoader {
    link_preview_service: Arc<dyn LinkPreviewService>,
}

impl LinkPreviewLoader {
    pub fn new(link_preview_service: Arc<dyn LinkPreviewService>) -> Self {
        Self {
            link_preview_service,
        }
    }
}

impl Loader<MessageId> for LinkPreviewLoader {
    type Value = Vec<crate::types::LinkPreview>;
    type Error = Arc<async_graphql::Error>;

    fn load(
        &self,
        keys: &[MessageId],
    ) -> impl Future<Output = Result<HashMap<MessageId, Self::Value>, Self::Error>> + Send {
        let keys = keys.to_vec();
        let svc = Arc::clone(&self.link_preview_service);
        async move {
            let preview_map = svc
                .get_previews_for_messages(&keys)
                .await
                .map_err(loader_error)?;

            Ok(keys
                .into_iter()
                .map(|id| {
                    let previews = preview_map
                        .get(&id)
                        .cloned()
                        .unwrap_or_default()
                        .into_iter()
                        .map(Into::into)
                        .collect();
                    (id, previews)
                })
                .collect())
        }
    }
}
