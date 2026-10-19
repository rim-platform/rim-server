use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use async_graphql::dataloader::Loader;
use futures_util::future::try_join_all;

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

/// Batches reaction lookups with one service call per viewer in the batch, so each key's
/// `reacted_by_me` is computed for its own viewer even when one batch holds several requests.
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
            let mut by_actor: HashMap<UserId, Vec<MessageId>> = HashMap::new();
            for key in keys {
                by_actor
                    .entry(key.actor_user_id)
                    .or_default()
                    .push(key.message_id);
            }

            let calls = by_actor.into_iter().map(|(actor_user_id, message_ids)| {
                let svc = Arc::clone(&svc);
                async move {
                    let reaction_map = svc
                        .get_reactions_for_messages(GetReactionsForMessagesQuery {
                            actor: Actor::new(actor_user_id),
                            message_ids: message_ids.clone(),
                        })
                        .await
                        .map_err(loader_error)?;
                    Ok::<_, Self::Error>(
                        message_ids
                            .into_iter()
                            .map(|message_id| {
                                let key = ReactionLoaderKey {
                                    message_id,
                                    actor_user_id,
                                };
                                let reactions =
                                    reaction_map.get(&message_id).cloned().unwrap_or_default();
                                (key, reactions)
                            })
                            .collect::<Vec<_>>(),
                    )
                }
            });

            Ok(try_join_all(calls).await?.into_iter().flatten().collect())
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
