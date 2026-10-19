//! The dataloaders: batching, mapping each key back to its value, and errors.

use std::collections::HashMap;
use std::hash::Hash;

use async_graphql::dataloader::{DataLoader, Loader};
use futures_util::future::join_all;

use rim_core::link_preview::model::LinkPreviewId;
use rim_services::link_preview::dto::LinkPreviewDto;
use rim_services::message_reaction::dto::ReactionGroupDto;

use super::*;
use crate::loaders::{
    LinkPreviewLoader, MentionLoader, PinLoader, ReactionLoader, ReactionLoaderKey, UserLoader,
};

/// Loads every key concurrently, as sibling resolvers do, so the loader sees one batch.
async fn load_each<K, L>(loader: L, keys: &[K]) -> Vec<Result<Option<L::Value>, L::Error>>
where
    K: Send + Sync + Hash + Eq + Clone + 'static,
    L: Loader<K>,
{
    let loader = DataLoader::new(loader, tokio::spawn);
    join_all(keys.iter().map(|k| loader.load_one(k.clone()))).await
}

/// The `(code, message)` of the error every key got.
fn errors<V>(results: Vec<Result<V, Arc<async_graphql::Error>>>) -> Vec<(Value, String)> {
    results
        .into_iter()
        .map(|r| match r {
            Ok(_) => panic!("expected an error"),
            Err(e) => (
                e.extensions
                    .as_ref()
                    .and_then(|ext| ext.get("code").cloned())
                    .unwrap_or(Value::Null),
                e.message.clone(),
            ),
        })
        .collect()
}

fn reaction(emoji: &str) -> ReactionGroupDto {
    ReactionGroupDto {
        emoji: emoji.to_string(),
        count: 1,
        reacted_by_me: true,
        user_ids: vec![],
    }
}

fn preview(message_id: MessageId, url: &str) -> LinkPreviewDto {
    LinkPreviewDto {
        id: LinkPreviewId::new(),
        message_id,
        url: url.to_string(),
        title: None,
        description: None,
        image_url: None,
        site_name: None,
        fetched_at: chrono::Utc::now(),
    }
}

/// Concurrent reaction loads of one viewer make one service call with every message id, and
/// each message gets its own reactions, or none.
#[tokio::test]
async fn reaction_loader_batches_one_viewers_messages_into_one_call() {
    let viewer = UserId::new();
    let [reacted, quiet] = [MessageId::new(), MessageId::new()];
    let mut service = MockMessageReactionService::new();
    service
        .expect_get_reactions_for_messages()
        .withf(move |q| {
            let mut ids = q.message_ids.clone();
            ids.sort_by_key(|id| id.to_string());
            let mut want = vec![reacted, quiet];
            want.sort_by_key(|id| id.to_string());
            q.actor.user_id == viewer && ids == want
        })
        .times(1)
        .returning(move |_| {
            Box::pin(async move { Ok(HashMap::from([(reacted, vec![reaction("👍")])])) })
        });

    let keys = [reacted, quiet].map(|message_id| ReactionLoaderKey {
        message_id,
        actor_user_id: viewer,
    });
    let loaded: Vec<_> = load_each(ReactionLoader::new(Arc::new(service)), &keys)
        .await
        .into_iter()
        .map(|r| r.expect("loaded").expect("present"))
        .collect();
    assert_eq!(loaded[0].len(), 1);
    assert_eq!(loaded[0][0].emoji, "👍");
    assert!(loaded[1].is_empty());
}

/// One batch holding two viewers' loads makes one service call per viewer with that viewer's
/// message ids, and each viewer gets `reacted_by_me` computed for themselves.
#[tokio::test]
async fn reaction_loader_answers_each_viewer_for_themselves() {
    let [reactor, onlooker] = [UserId::new(), UserId::new()];
    let [shared, other] = [MessageId::new(), MessageId::new()];
    let mut service = MockMessageReactionService::new();
    service
        .expect_get_reactions_for_messages()
        .withf(move |q| {
            let mut ids = q.message_ids.clone();
            ids.sort_by_key(|id| id.to_string());
            let mut want = vec![shared, other];
            want.sort_by_key(|id| id.to_string());
            q.actor.user_id == reactor && ids == want
        })
        .times(1)
        .returning(move |_| {
            Box::pin(async move { Ok(HashMap::from([(shared, vec![reaction("👍")])])) })
        });
    service
        .expect_get_reactions_for_messages()
        .withf(move |q| q.actor.user_id == onlooker && q.message_ids == [shared])
        .times(1)
        .returning(move |_| {
            let seen_by_onlooker = ReactionGroupDto {
                reacted_by_me: false,
                ..reaction("👍")
            };
            Box::pin(async move { Ok(HashMap::from([(shared, vec![seen_by_onlooker])])) })
        });

    let keys = [(shared, reactor), (other, reactor), (shared, onlooker)].map(
        |(message_id, actor_user_id)| ReactionLoaderKey {
            message_id,
            actor_user_id,
        },
    );
    let loaded: Vec<_> = load_each(ReactionLoader::new(Arc::new(service)), &keys)
        .await
        .into_iter()
        .map(|r| r.expect("loaded").expect("present"))
        .collect();
    assert!(
        loaded[0][0].reacted_by_me,
        "the reactor sees their reaction"
    );
    assert!(loaded[1].is_empty());
    assert!(
        !loaded[2][0].reacted_by_me,
        "the onlooker does not see it as theirs"
    );
}

/// The reaction loader asked for no keys answers nothing without calling the service.
#[tokio::test]
async fn reaction_loader_makes_no_call_for_no_keys() {
    let loader = ReactionLoader::new(Arc::new(MockMessageReactionService::new()));
    let loaded = loader.load(&[]).await.expect("loaded");
    assert!(loaded.is_empty());
}

/// The pin loader answers `true` for a pinned message and `false` for any other, from one call.
#[tokio::test]
async fn pin_loader_marks_each_message_pinned_or_not() {
    let [pinned, plain] = [MessageId::new(), MessageId::new()];
    let mut service = MockPinService::new();
    service
        .expect_is_pinned_batch()
        .times(1)
        .returning(move |_| Box::pin(async move { Ok(vec![pinned]) }));

    let loaded = load_each(PinLoader::new(Arc::new(service)), &[pinned, plain]).await;
    let loaded: Vec<_> = loaded.into_iter().map(|r| r.expect("loaded")).collect();
    assert_eq!(loaded, vec![Some(true), Some(false)]);
}

/// The link preview loader maps each message to its previews, or none, from one call.
#[tokio::test]
async fn link_preview_loader_maps_each_message_to_its_previews() {
    let [linked, plain] = [MessageId::new(), MessageId::new()];
    let mut service = MockLinkPreviewService::new();
    service
        .expect_get_previews_for_messages()
        .times(1)
        .returning(move |_| {
            Box::pin(async move {
                Ok(HashMap::from([(
                    linked,
                    vec![
                        preview(linked, "https://a.example"),
                        preview(linked, "https://b.example"),
                    ],
                )]))
            })
        });

    let loaded = load_each(LinkPreviewLoader::new(Arc::new(service)), &[linked, plain]).await;
    let urls: Vec<Vec<String>> = loaded
        .into_iter()
        .map(|r| {
            r.expect("loaded")
                .expect("present")
                .into_iter()
                .map(|p| p.url)
                .collect()
        })
        .collect();
    assert_eq!(
        urls,
        vec![
            vec![
                "https://a.example".to_string(),
                "https://b.example".to_string()
            ],
            vec![]
        ]
    );
}

/// A failing service call fails the load of every key in the batch with the code and message a
/// root field would carry.
#[tokio::test]
async fn a_service_error_fails_every_key_of_the_batch() {
    let denied = (
        Value::from("FORBIDDEN"),
        ServiceError::PermissionDenied.to_string(),
    );
    let messages = [MessageId::new(), MessageId::new()];
    let both = vec![denied.clone(), denied.clone()];

    let mut reactions = MockMessageReactionService::new();
    reactions
        .expect_get_reactions_for_messages()
        .times(1)
        .returning(|_| Box::pin(async { Err(ServiceError::PermissionDenied) }));
    let viewer = UserId::new();
    let keys = messages.map(|message_id| ReactionLoaderKey {
        message_id,
        actor_user_id: viewer,
    });
    let loaded = load_each(ReactionLoader::new(Arc::new(reactions)), &keys).await;
    assert_eq!(errors(loaded), both, "reactions");

    let mut pins = MockPinService::new();
    pins.expect_is_pinned_batch()
        .times(1)
        .returning(|_| Box::pin(async { Err(ServiceError::PermissionDenied) }));
    let loaded = load_each(PinLoader::new(Arc::new(pins)), &messages).await;
    assert_eq!(errors(loaded), both, "pins");

    let mut previews = MockLinkPreviewService::new();
    previews
        .expect_get_previews_for_messages()
        .times(1)
        .returning(|_| Box::pin(async { Err(ServiceError::PermissionDenied) }));
    let loaded = load_each(LinkPreviewLoader::new(Arc::new(previews)), &messages).await;
    assert_eq!(errors(loaded), both, "link previews");

    let mut mentions = MockMentionService::new();
    mentions
        .expect_get_mentions_for_messages()
        .times(1)
        .returning(|_| Box::pin(async { Err(ServiceError::PermissionDenied) }));
    let loaded = load_each(MentionLoader::new(Arc::new(mentions)), &messages).await;
    assert_eq!(errors(loaded), both, "mentions");

    let mut users = MockUserService::new();
    users
        .expect_get_users_by_ids()
        .times(1)
        .returning(|_| Box::pin(async { Err(ServiceError::PermissionDenied) }));
    let loaded = load_each(
        UserLoader::new(Arc::new(users)),
        &[UserId::new(), UserId::new()],
    )
    .await;
    assert_eq!(errors(loaded), both, "users");
}
