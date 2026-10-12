use std::sync::Arc;

use async_trait::async_trait;

use rim_core::custom_emoji::model::{CustomEmojiId, NewCustomEmoji};
use rim_core::custom_emoji::repository::CustomEmojiRepository;
use rim_core::server::model::ServerId;
use rim_core::server_member::repository::ServerMemberRepository;
use rim_services::custom_emoji::command::{
    CreateCustomEmojiCmd, DeleteCustomEmojiCmd, ListCustomEmojisQuery,
};
use rim_services::custom_emoji::dto::CustomEmojiDto;
use rim_services::custom_emoji::service::CustomEmojiService;
use rim_services::storage::service::StorageService;
use rim_services::{ServiceError, ServiceResult};

/// Maximum image data size: 256 KB.
const MAX_IMAGE_SIZE: usize = 256 * 1024;

/// Maximum emoji name length.
const MAX_NAME_LENGTH: usize = 64;

#[derive(Clone)]
pub struct CustomEmojiRepos {
    pub custom_emoji_repo: Arc<dyn CustomEmojiRepository>,
    pub server_member_repo: Arc<dyn ServerMemberRepository>,
}

pub struct DefaultCustomEmojiService {
    repos: CustomEmojiRepos,
    storage_service: Arc<dyn StorageService>,
    /// The relay's direct space, which has no custom emojis.
    direct_space_id: Option<ServerId>,
}

impl DefaultCustomEmojiService {
    pub fn new(repos: CustomEmojiRepos, storage_service: Arc<dyn StorageService>) -> Self {
        Self {
            repos,
            storage_service,
            direct_space_id: None,
        }
    }

    /// The relay's direct space, or `None` where none is wired.
    pub fn with_direct_space(mut self, direct_space_id: Option<ServerId>) -> Self {
        self.direct_space_id = direct_space_id;
        self
    }

    fn ensure_workspace_features(&self, server_id: ServerId) -> ServiceResult<()> {
        crate::util::ensure_workspace_features(self.direct_space_id, server_id)
    }
}

fn validate_emoji_name(name: &str) -> ServiceResult<()> {
    if name.is_empty() || name.chars().count() > MAX_NAME_LENGTH {
        return Err(ServiceError::Validation(format!(
            "emoji name must be between 1 and {} characters",
            MAX_NAME_LENGTH
        )));
    }
    if !name.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return Err(ServiceError::Validation(
            "emoji name must contain only alphanumeric characters and underscores".into(),
        ));
    }
    Ok(())
}

#[async_trait]
impl CustomEmojiService for DefaultCustomEmojiService {
    async fn create(&self, cmd: CreateCustomEmojiCmd) -> ServiceResult<CustomEmojiDto> {
        self.ensure_workspace_features(cmd.server_id)?;
        // Validate actor is a server member
        self.repos
            .server_member_repo
            .get_member(cmd.server_id, cmd.actor.user_id)
            .await?
            .ok_or(ServiceError::PermissionDenied)?;

        validate_emoji_name(&cmd.name)?;

        if cmd.image_data.len() > MAX_IMAGE_SIZE {
            return Err(ServiceError::Validation(format!(
                "image must be at most {} KB",
                MAX_IMAGE_SIZE / 1024
            )));
        }

        if cmd.image_data.is_empty() {
            return Err(ServiceError::Validation("image data is empty".into()));
        }

        // Generate a temporary ID for the storage key
        let emoji_id = CustomEmojiId::new();
        let storage_key = format!("{}/emoji/{}.png", cmd.server_id, emoji_id);

        let stored = self
            .storage_service
            .upload_file(&storage_key, &cmd.image_data, "image/png")
            .await?;

        let created = self
            .repos
            .custom_emoji_repo
            .create(NewCustomEmoji {
                server_id: cmd.server_id,
                name: cmd.name,
                image_url: stored.key.clone(),
                uploaded_by: cmd.actor.user_id,
            })
            .await;

        match created {
            Ok(emoji) => Ok(CustomEmojiDto::from(emoji)),
            Err(err) => {
                // A refused row, such as a duplicate name, must not leave its image behind.
                let _ = self.storage_service.delete_file(&stored.key).await;
                Err(err.into())
            }
        }
    }

    async fn delete(&self, cmd: DeleteCustomEmojiCmd) -> ServiceResult<()> {
        let emoji = self
            .repos
            .custom_emoji_repo
            .find_by_id(cmd.emoji_id)
            .await?
            .ok_or(ServiceError::NotFound)?;
        self.ensure_workspace_features(emoji.server_id)?;

        // Check that actor is a member of the server
        let member = self
            .repos
            .server_member_repo
            .get_member(emoji.server_id, cmd.actor.user_id)
            .await?
            .ok_or(ServiceError::PermissionDenied)?;

        // Only owner or the uploader can delete
        if member.member_role != rim_core::server_member::model::ServerMemberRole::Owner
            && emoji.uploaded_by != cmd.actor.user_id
        {
            return Err(ServiceError::PermissionDenied);
        }

        // Delete the stored image
        let _ = self.storage_service.delete_file(&emoji.image_url).await;

        self.repos.custom_emoji_repo.delete(cmd.emoji_id).await?;

        Ok(())
    }

    async fn list(&self, query: ListCustomEmojisQuery) -> ServiceResult<Vec<CustomEmojiDto>> {
        self.ensure_workspace_features(query.server_id)?;
        crate::util::ensure_server_member(
            self.repos.server_member_repo.as_ref(),
            query.server_id,
            query.actor.user_id,
        )
        .await?;

        let emojis = self
            .repos
            .custom_emoji_repo
            .list_by_server(query.server_id)
            .await?;

        Ok(emojis.into_iter().map(Into::into).collect())
    }
}

#[cfg(test)]
mod tests {
    use rim_core::custom_emoji::repository::MockCustomEmojiRepository;
    use rim_core::repository::error::{RepoError, RepoErrorKind};
    use rim_core::server::model::ServerId;
    use rim_core::server_member::model::{ServerMember, ServerMemberId, ServerMemberRole};
    use rim_core::server_member::repository::MockServerMemberRepository;
    use rim_core::storage::StoredFile;
    use rim_core::user::model::UserId;
    use rim_services::Actor;
    use rim_services::storage::service::MockStorageService;

    use super::*;

    #[tokio::test]
    async fn create_deletes_the_uploaded_image_when_the_row_is_refused() {
        let server_id = ServerId::new();
        let user_id = UserId::new();
        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_get_member()
            .returning(move |sid, uid| {
                Ok(Some(ServerMember {
                    id: ServerMemberId::new(),
                    server_id: sid,
                    user_id: uid,
                    member_role: ServerMemberRole::Member,
                    joined_at: chrono::Utc::now(),
                }))
            });
        let mut custom_emoji_repo = MockCustomEmojiRepository::new();
        custom_emoji_repo
            .expect_create()
            .returning(|_| Box::pin(async { Err(RepoError::new(RepoErrorKind::Conflict)) }));
        let mut storage = MockStorageService::new();
        storage.expect_upload_file().returning(|key, data, _| {
            let stored = StoredFile {
                key: key.to_string(),
                size_bytes: data.len() as u64,
            };
            Box::pin(async move { Ok(stored) })
        });
        let prefix = format!("{server_id}/emoji/");
        storage
            .expect_delete_file()
            .withf(move |key| key.starts_with(&prefix))
            .times(1)
            .returning(|_| Box::pin(async { Ok(()) }));
        let svc = DefaultCustomEmojiService::new(
            CustomEmojiRepos {
                custom_emoji_repo: Arc::new(custom_emoji_repo),
                server_member_repo: Arc::new(server_member_repo),
            },
            Arc::new(storage),
        );

        let err = svc
            .create(CreateCustomEmojiCmd {
                actor: Actor::new(user_id),
                server_id,
                name: "party".to_string(),
                image_data: vec![1, 2, 3],
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Conflict(_)), "{err:?}");
    }

    #[test]
    fn validate_emoji_name_rejects_empty() {
        let err = validate_emoji_name("").unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[test]
    fn validate_emoji_name_rejects_too_long() {
        let name = "a".repeat(65);
        let err = validate_emoji_name(&name).unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[test]
    fn validate_emoji_name_rejects_special_chars() {
        let err = validate_emoji_name("my-emoji").unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
        let err = validate_emoji_name("my emoji").unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
        let err = validate_emoji_name("emoji!").unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[test]
    fn validate_emoji_name_accepts_valid() {
        validate_emoji_name("my_emoji").unwrap();
        validate_emoji_name("emoji123").unwrap();
        validate_emoji_name("A").unwrap();
        validate_emoji_name("_underscore_").unwrap();
    }

    #[test]
    fn validate_emoji_name_accepts_max_length() {
        let name = "a".repeat(64);
        validate_emoji_name(&name).unwrap();
    }
}
