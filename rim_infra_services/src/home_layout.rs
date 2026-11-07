//! The signed-in person's Home layout: the checks a layout passes and the newest-edit-wins save.

use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use rim_core::home_layout::repository::HomeLayoutRepository;
use rim_core::host::model::UserKind;
use rim_core::user::model::UserId;
use rim_services::home_layout::dto::{HomeLayoutDto, HomeLayoutSavedDto};
use rim_services::home_layout::request::SaveHomeLayoutCmd;
use rim_services::home_layout::service::HomeLayoutService;
use rim_services::{Actor, ServiceError, ServiceResult};

/// The most bytes a layout takes as compact JSON.
const MAX_LAYOUT_BYTES: usize = 16 * 1024;
/// The most boxes a layout holds.
const MAX_BOXES: usize = 30;
/// The most characters of a box's `id` and of its `kind`.
const MAX_NAME_CHARS: usize = 64;
/// How often a save is tried when a reset deletes the stored layout between its two steps.
const SAVE_ATTEMPTS: usize = 2;

/// The Home layout service on the account's own server.
pub(crate) struct DefaultHomeLayoutService {
    repo: Arc<dyn HomeLayoutRepository>,
}

impl DefaultHomeLayoutService {
    pub(crate) fn new(repo: Arc<dyn HomeLayoutRepository>) -> Self {
        Self { repo }
    }

    /// Refuse anyone but an account of this server: a person from another server keeps their
    /// layout at their own server, and a bot has none.
    async fn ensure_local(&self, user_id: UserId) -> ServiceResult<()> {
        match self.repo.account_kind(user_id).await? {
            Some(UserKind::Local) => Ok(()),
            Some(UserKind::Remote) => Err(ServiceError::Forbidden(
                "a person from another server keeps their Home layout at their own server"
                    .to_string(),
            )),
            Some(UserKind::Bot) => Err(ServiceError::Forbidden(
                "a bot has no Home layout".to_string(),
            )),
            None => Err(ServiceError::NotFound),
        }
    }
}

fn invalid(message: &str) -> ServiceError {
    ServiceError::Validation(message.to_string())
}

/// Whether `value` is a string of 1 to [`MAX_NAME_CHARS`] characters.
fn is_name(value: Option<&serde_json::Value>) -> bool {
    value
        .and_then(serde_json::Value::as_str)
        .is_some_and(|s| (1..=MAX_NAME_CHARS).contains(&s.chars().count()))
}

/// Check a layout's shape: its size, `version`, the number of boxes, and each box's `id`,
/// `kind`, `width` and `settings`. Kinds and settings are not read further.
///
/// # Errors
///
/// `Validation` naming the first check the layout fails.
fn validate_layout(layout: &serde_json::Value) -> ServiceResult<()> {
    let object = layout
        .as_object()
        .ok_or_else(|| invalid("A Home layout must be a JSON object"))?;
    let bytes = serde_json::to_vec(layout)
        .map_err(|e| ServiceError::Internal(Some(e.into())))?
        .len();
    if bytes > MAX_LAYOUT_BYTES {
        return Err(invalid("A Home layout must be at most 16 KiB"));
    }
    if object.get("version").and_then(serde_json::Value::as_u64) != Some(1) {
        return Err(invalid("A Home layout's version must be 1"));
    }
    let boxes = object
        .get("boxes")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| invalid("A Home layout's boxes must be a list"))?;
    if boxes.len() > MAX_BOXES {
        return Err(invalid("A Home layout holds at most 30 boxes"));
    }
    let mut ids = HashSet::new();
    for item in boxes {
        let item = item
            .as_object()
            .ok_or_else(|| invalid("A box must be a JSON object"))?;
        if !is_name(item.get("id")) {
            return Err(invalid("A box id must be 1 to 64 characters"));
        }
        if !ids.insert(item.get("id").and_then(serde_json::Value::as_str)) {
            return Err(invalid("A box id must be unique in the layout"));
        }
        if !is_name(item.get("kind")) {
            return Err(invalid("A box kind must be 1 to 64 characters"));
        }
        match item.get("width").and_then(serde_json::Value::as_str) {
            Some("half" | "full") => {}
            _ => return Err(invalid("A box width must be half or full")),
        }
        if item.get("settings").is_some_and(|s| !s.is_object()) {
            return Err(invalid("A box's settings must be a JSON object"));
        }
    }
    Ok(())
}

#[async_trait]
impl HomeLayoutService for DefaultHomeLayoutService {
    async fn home_layout(&self, actor: Actor) -> ServiceResult<Option<HomeLayoutDto>> {
        self.ensure_local(actor.user_id).await?;
        Ok(self.repo.get(actor.user_id).await?.map(Into::into))
    }

    async fn save_home_layout(&self, cmd: SaveHomeLayoutCmd) -> ServiceResult<HomeLayoutSavedDto> {
        let user_id = cmd.actor.user_id;
        self.ensure_local(user_id).await?;
        validate_layout(&cmd.layout)?;
        let edited_at = cmd.edited_at.min(Utc::now());
        for _ in 0..SAVE_ATTEMPTS {
            if let Some(stored) = self
                .repo
                .save_unless_newer(user_id, cmd.layout.clone(), edited_at)
                .await?
            {
                return Ok(HomeLayoutSavedDto {
                    kept: true,
                    layout: stored.into(),
                });
            }
            if let Some(stored) = self.repo.get(user_id).await? {
                return Ok(HomeLayoutSavedDto {
                    kept: false,
                    layout: stored.into(),
                });
            }
        }
        Err(ServiceError::Conflict(
            "the Home layout changed while it was saved; try again".to_string(),
        ))
    }

    async fn reset_home_layout(&self, actor: Actor) -> ServiceResult<bool> {
        self.ensure_local(actor.user_id).await?;
        Ok(self.repo.delete(actor.user_id).await?)
    }
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, Duration};
    use rim_core::home_layout::model::HomeLayout;
    use rim_core::home_layout::repository::MockHomeLayoutRepository;
    use serde_json::json;

    use super::*;

    fn layout_with(boxes: Vec<serde_json::Value>) -> serde_json::Value {
        json!({ "version": 1, "boxes": boxes })
    }

    fn a_box(id: &str) -> serde_json::Value {
        json!({ "id": id, "kind": "waiting", "width": "half" })
    }

    fn stored(user_id: UserId, edited_at: DateTime<Utc>) -> HomeLayout {
        HomeLayout {
            user_id,
            layout: layout_with(vec![a_box("stored")]),
            edited_at,
            updated_at: edited_at,
        }
    }

    fn message(result: ServiceResult<()>) -> String {
        match result {
            Err(ServiceError::Validation(message)) => message,
            other => panic!("expected a validation error, got {other:?}"),
        }
    }

    fn repo_of(kind: UserKind) -> MockHomeLayoutRepository {
        let mut repo = MockHomeLayoutRepository::new();
        repo.expect_account_kind()
            .returning(move |_| Box::pin(async move { Ok(Some(kind)) }));
        repo
    }

    #[test]
    fn a_layout_of_thirty_boxes_passes_and_thirty_one_is_refused() {
        let thirty: Vec<_> = (0..30).map(|i| a_box(&format!("b{i}"))).collect();
        assert!(validate_layout(&layout_with(thirty.clone())).is_ok());
        let mut over = thirty;
        over.push(a_box("b30"));
        assert_eq!(
            message(validate_layout(&layout_with(over))),
            "A Home layout holds at most 30 boxes"
        );
    }

    #[test]
    fn a_layout_of_sixteen_kib_passes_and_one_byte_more_is_refused() {
        let empty = serde_json::to_vec(&layout_with(vec![json!({
            "id": "b", "kind": "quick", "width": "full", "settings": { "note": "" }
        })]))
        .expect("serialize")
        .len();
        let filler = "x".repeat(MAX_LAYOUT_BYTES - empty);
        let at_bound = layout_with(vec![json!({
            "id": "b", "kind": "quick", "width": "full", "settings": { "note": filler }
        })]);
        assert_eq!(
            serde_json::to_vec(&at_bound).expect("serialize").len(),
            MAX_LAYOUT_BYTES
        );
        assert!(validate_layout(&at_bound).is_ok());
        let over = layout_with(vec![json!({
            "id": "b", "kind": "quick", "width": "full", "settings": { "note": format!("{filler}x") }
        })]);
        assert_eq!(
            message(validate_layout(&over)),
            "A Home layout must be at most 16 KiB"
        );
    }

    #[test]
    fn ids_and_kinds_are_counted_in_characters() {
        let sixty_four = "é".repeat(64);
        let ok = layout_with(vec![
            json!({ "id": sixty_four, "kind": sixty_four, "width": "half" }),
        ]);
        assert!(validate_layout(&ok).is_ok());
        let sixty_five = "é".repeat(65);
        let long_id = layout_with(vec![
            json!({ "id": sixty_five, "kind": "waiting", "width": "half" }),
        ]);
        assert_eq!(
            message(validate_layout(&long_id)),
            "A box id must be 1 to 64 characters"
        );
        let long_kind = layout_with(vec![
            json!({ "id": "b", "kind": sixty_five, "width": "half" }),
        ]);
        assert_eq!(
            message(validate_layout(&long_kind)),
            "A box kind must be 1 to 64 characters"
        );
        let empty_id = layout_with(vec![
            json!({ "id": "", "kind": "waiting", "width": "half" }),
        ]);
        assert_eq!(
            message(validate_layout(&empty_id)),
            "A box id must be 1 to 64 characters"
        );
        let empty_kind = layout_with(vec![json!({ "id": "b", "kind": "", "width": "half" })]);
        assert_eq!(
            message(validate_layout(&empty_kind)),
            "A box kind must be 1 to 64 characters"
        );
    }

    #[test]
    fn each_other_check_names_what_failed() {
        let cases = [
            (json!([]), "A Home layout must be a JSON object"),
            (json!({ "boxes": [] }), "A Home layout's version must be 1"),
            (
                json!({ "version": 2, "boxes": [] }),
                "A Home layout's version must be 1",
            ),
            (
                json!({ "version": 1 }),
                "A Home layout's boxes must be a list",
            ),
            (layout_with(vec![json!("b")]), "A box must be a JSON object"),
            (
                layout_with(vec![json!({ "id": 7, "kind": "waiting", "width": "half" })]),
                "A box id must be 1 to 64 characters",
            ),
            (
                layout_with(vec![a_box("b"), a_box("b")]),
                "A box id must be unique in the layout",
            ),
            (
                layout_with(vec![
                    json!({ "id": "b", "kind": "waiting", "width": "wide" }),
                ]),
                "A box width must be half or full",
            ),
            (
                layout_with(vec![json!({ "id": "b", "kind": "waiting" })]),
                "A box width must be half or full",
            ),
            (
                layout_with(vec![
                    json!({ "id": "b", "kind": "waiting", "width": "half", "settings": [1] }),
                ]),
                "A box's settings must be a JSON object",
            ),
        ];
        for (layout, expected) in cases {
            assert_eq!(message(validate_layout(&layout)), expected, "{layout}");
        }
    }

    #[test]
    fn unknown_kinds_settings_and_fields_are_kept_as_sent() {
        let layout = json!({
            "version": 1,
            "extra": true,
            "boxes": [
                { "id": "b", "kind": "fromTheFuture", "width": "full", "settings": { "x": [1] },
                  "pinned": true },
                { "id": "c", "kind": "channel", "width": "half", "settings": { "count": 99 } }
            ]
        });
        assert!(validate_layout(&layout).is_ok());
    }

    #[tokio::test]
    async fn a_save_is_kept_with_the_time_the_person_made_it() {
        let user_id = UserId::new();
        let edited_at = Utc::now() - Duration::minutes(5);
        let mut repo = repo_of(UserKind::Local);
        repo.expect_save_unless_newer()
            .withf(move |_, _, at| *at == edited_at)
            .returning(move |user_id, layout, at| {
                Box::pin(async move {
                    Ok(Some(HomeLayout {
                        user_id,
                        layout,
                        edited_at: at,
                        updated_at: Utc::now(),
                    }))
                })
            });
        let saved = DefaultHomeLayoutService::new(Arc::new(repo))
            .save_home_layout(SaveHomeLayoutCmd {
                actor: Actor::new(user_id),
                layout: layout_with(vec![a_box("new")]),
                edited_at,
            })
            .await
            .expect("save");
        assert!(saved.kept);
        assert_eq!(saved.layout.edited_at, edited_at);
        assert_eq!(saved.layout.layout, layout_with(vec![a_box("new")]));
    }

    #[tokio::test]
    async fn an_edited_at_in_the_future_counts_as_the_server_clock() {
        let user_id = UserId::new();
        let before = Utc::now();
        let mut repo = repo_of(UserKind::Local);
        repo.expect_save_unless_newer()
            .withf(move |_, _, at| *at >= before && *at <= Utc::now())
            .returning(move |user_id, layout, at| {
                Box::pin(async move {
                    Ok(Some(HomeLayout {
                        user_id,
                        layout,
                        edited_at: at,
                        updated_at: at,
                    }))
                })
            });
        let saved = DefaultHomeLayoutService::new(Arc::new(repo))
            .save_home_layout(SaveHomeLayoutCmd {
                actor: Actor::new(user_id),
                layout: layout_with(vec![a_box("new")]),
                edited_at: Utc::now() + Duration::days(365),
            })
            .await
            .expect("save");
        assert!(saved.kept);
        assert!(saved.layout.edited_at <= Utc::now());
    }

    #[tokio::test]
    async fn a_save_older_than_the_stored_layout_answers_the_stored_one() {
        let user_id = UserId::new();
        let newer = Utc::now() - Duration::minutes(1);
        let mut repo = repo_of(UserKind::Local);
        repo.expect_save_unless_newer()
            .times(1)
            .returning(|_, _, _| Box::pin(async { Ok(None) }));
        repo.expect_get()
            .returning(move |user_id| Box::pin(async move { Ok(Some(stored(user_id, newer))) }));
        let saved = DefaultHomeLayoutService::new(Arc::new(repo))
            .save_home_layout(SaveHomeLayoutCmd {
                actor: Actor::new(user_id),
                layout: layout_with(vec![a_box("old")]),
                edited_at: newer - Duration::minutes(10),
            })
            .await
            .expect("save");
        assert!(!saved.kept);
        assert_eq!(saved.layout.edited_at, newer);
        assert_eq!(saved.layout.layout, layout_with(vec![a_box("stored")]));
    }

    #[tokio::test]
    async fn a_save_that_loses_to_a_layout_reset_meanwhile_is_tried_again() {
        let user_id = UserId::new();
        let mut repo = repo_of(UserKind::Local);
        let mut seq = mockall::Sequence::new();
        repo.expect_save_unless_newer()
            .times(1)
            .in_sequence(&mut seq)
            .returning(|_, _, _| Box::pin(async { Ok(None) }));
        repo.expect_get()
            .times(1)
            .in_sequence(&mut seq)
            .returning(|_| Box::pin(async { Ok(None) }));
        repo.expect_save_unless_newer()
            .times(1)
            .in_sequence(&mut seq)
            .returning(|user_id, layout, at| {
                Box::pin(async move {
                    Ok(Some(HomeLayout {
                        user_id,
                        layout,
                        edited_at: at,
                        updated_at: at,
                    }))
                })
            });
        let saved = DefaultHomeLayoutService::new(Arc::new(repo))
            .save_home_layout(SaveHomeLayoutCmd {
                actor: Actor::new(user_id),
                layout: layout_with(vec![a_box("new")]),
                edited_at: Utc::now(),
            })
            .await
            .expect("save");
        assert!(saved.kept);
    }

    #[tokio::test]
    async fn an_invalid_layout_stores_nothing() {
        let mut repo = repo_of(UserKind::Local);
        repo.expect_save_unless_newer().never();
        let err = DefaultHomeLayoutService::new(Arc::new(repo))
            .save_home_layout(SaveHomeLayoutCmd {
                actor: Actor::new(UserId::new()),
                layout: json!({ "version": 2, "boxes": [] }),
                edited_at: Utc::now(),
            })
            .await
            .expect_err("refused");
        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[tokio::test]
    async fn a_person_from_another_server_and_a_bot_are_refused_everything() {
        for kind in [UserKind::Remote, UserKind::Bot] {
            let mut repo = repo_of(kind);
            repo.expect_get().never();
            repo.expect_save_unless_newer().never();
            repo.expect_delete().never();
            let service = DefaultHomeLayoutService::new(Arc::new(repo));
            let actor = Actor::new(UserId::new());
            assert!(matches!(
                service.home_layout(actor).await,
                Err(ServiceError::Forbidden(_))
            ));
            assert!(matches!(
                service
                    .save_home_layout(SaveHomeLayoutCmd {
                        actor,
                        layout: layout_with(vec![]),
                        edited_at: Utc::now(),
                    })
                    .await,
                Err(ServiceError::Forbidden(_))
            ));
            assert!(matches!(
                service.reset_home_layout(actor).await,
                Err(ServiceError::Forbidden(_))
            ));
        }
    }

    #[tokio::test]
    async fn the_owner_reads_and_resets_their_layout() {
        let user_id = UserId::new();
        let at = Utc::now();
        let mut repo = repo_of(UserKind::Local);
        repo.expect_get()
            .withf(move |id| *id == user_id)
            .returning(move |user_id| Box::pin(async move { Ok(Some(stored(user_id, at))) }));
        repo.expect_delete()
            .withf(move |id| *id == user_id)
            .returning(|_| Box::pin(async { Ok(true) }));
        let service = DefaultHomeLayoutService::new(Arc::new(repo));
        let layout = service
            .home_layout(Actor::new(user_id))
            .await
            .expect("read")
            .expect("stored");
        assert_eq!(layout.edited_at, at);
        assert!(
            service
                .reset_home_layout(Actor::new(user_id))
                .await
                .expect("reset")
        );
    }

    #[test]
    fn debug_output_leaves_the_layout_out() {
        let at = Utc::now();
        let secret = json!({ "version": 1, "boxes": [a_box("conversation-secret")] });
        let cmd = SaveHomeLayoutCmd {
            actor: Actor::new(UserId::new()),
            layout: secret.clone(),
            edited_at: at,
        };
        let dto = HomeLayoutDto {
            layout: secret.clone(),
            edited_at: at,
            updated_at: at,
        };
        let model = HomeLayout {
            user_id: UserId::new(),
            layout: secret,
            edited_at: at,
            updated_at: at,
        };
        for printed in [format!("{cmd:?}"), format!("{dto:?}"), format!("{model:?}")] {
            assert!(!printed.contains("conversation-secret"), "{printed}");
        }
    }
}
