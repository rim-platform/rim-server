use rim_core::conversation::model::ConversationId;
use rim_core::server::model::ServerId;
use rim_core::server::repository::ServerRepository;
use rim_core::server_member::model::ServerMemberRole;
use rim_core::server_member::repository::ServerMemberRepository;
use rim_services::{ServiceError, ServiceResult};

pub async fn ensure_server_member(
    server_member_repo: &dyn ServerMemberRepository,
    server_id: ServerId,
    user_id: rim_core::user::model::UserId,
) -> ServiceResult<()> {
    let is_member = server_member_repo
        .is_member(server_id, user_id)
        .await
        .map_err(rim_services::map_repo_error)?;

    if !is_member {
        return Err(ServiceError::PermissionDenied);
    }

    Ok(())
}

pub async fn ensure_server_owner(
    server_member_repo: &dyn ServerMemberRepository,
    server_id: ServerId,
    user_id: rim_core::user::model::UserId,
) -> ServiceResult<()> {
    let member = server_member_repo
        .get_member(server_id, user_id)
        .await
        .map_err(rim_services::map_repo_error)?
        .ok_or(ServiceError::PermissionDenied)?;

    if member.member_role != ServerMemberRole::Owner {
        return Err(ServiceError::PermissionDenied);
    }

    Ok(())
}

pub async fn ensure_server_exists(
    server_repo: &dyn ServerRepository,
    server_id: ServerId,
) -> ServiceResult<rim_core::server::model::Server> {
    let server = server_repo
        .get_by_id(server_id)
        .await
        .map_err(rim_services::map_repo_error)?
        .ok_or(ServiceError::NotFound)?;

    Ok(server)
}

pub async fn ensure_conversation_in_server(
    conversation_repo: &dyn rim_core::conversation::repository::ConversationRepository,
    conversation_id: ConversationId,
    expected_server_id: ServerId,
) -> ServiceResult<rim_core::conversation::model::Conversation> {
    let conv = conversation_repo
        .get_by_id(conversation_id)
        .await
        .map_err(rim_services::map_repo_error)?
        .ok_or(ServiceError::NotFound)?;

    if conv.server_id != expected_server_id {
        return Err(ServiceError::NotFound);
    }

    Ok(conv)
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use rim_core::conversation::model::{Conversation, ConversationType};
    use rim_core::conversation::repository::{
        ConversationActivityCursor, ConversationCreatedCursor, ConversationRepository,
    };
    use rim_core::repository::error::{RepoError, RepoErrorKind, RepoResult};
    use rim_core::repository::pagination::{Page, PageDir};
    use rim_core::server::model::{Server, ServerId};
    use rim_core::server::repository::ServerRepository;
    use rim_core::server_member::model::{ServerMember, ServerMemberId, ServerMemberRole};
    use rim_core::server_member::repository::ServerMemberRepository;
    use rim_core::user::model::UserId;

    // Ensure we cover the "unsupported" stubs so `cargo llvm-cov` doesn't flag them.
    #[tokio::test]
    async fn fake_repos_unsupported_paths_are_internal_errors() {
        let server_id = ServerId::new();
        let user_id = UserId::new();

        let member_repo = FakeServerMemberRepo {
            is_member: true,
            member: None,
        };
        assert!(matches!(
            rim_services::map_repo_error(
                member_repo
                    .add_member(server_id, user_id, ServerMemberRole::Guest)
                    .await
                    .unwrap_err()
            ),
            ServiceError::Internal(_)
        ));
        assert!(matches!(
            rim_services::map_repo_error(
                member_repo
                    .change_member_role(server_id, user_id, ServerMemberRole::Guest)
                    .await
                    .unwrap_err()
            ),
            ServiceError::Internal(_)
        ));
        assert!(matches!(
            rim_services::map_repo_error(
                member_repo
                    .remove_member(server_id, user_id)
                    .await
                    .unwrap_err()
            ),
            ServiceError::Internal(_)
        ));

        let server_repo = FakeServerRepo { server: None };
        assert!(matches!(
            rim_services::map_repo_error(server_repo.create_server("x").await.unwrap_err()),
            ServiceError::Internal(_)
        ));

        let conv_repo = FakeConversationRepo { conv: None };
        assert!(matches!(
            rim_services::map_repo_error(
                conv_repo
                    .create_conversation(rim_core::conversation::model::NewConversation {
                        server_id,
                        conversation_type: ConversationType::Channel,
                        title: None,
                    })
                    .await
                    .unwrap_err()
            ),
            ServiceError::Internal(_)
        ));
    }

    struct FakeServerMemberRepo {
        is_member: bool,
        member: Option<ServerMember>,
    }

    #[async_trait]
    impl ServerMemberRepository for FakeServerMemberRepo {
        async fn get_by_id(&self, _id: ServerMemberId) -> RepoResult<Option<ServerMember>> {
            Ok(None)
        }

        async fn get_member(
            &self,
            _server_id: ServerId,
            _user_id: UserId,
        ) -> RepoResult<Option<ServerMember>> {
            Ok(self.member.clone())
        }

        async fn add_member(
            &self,
            _server_id: ServerId,
            _user_id: UserId,
            _role: ServerMemberRole,
        ) -> RepoResult<ServerMember> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn change_member_role(
            &self,
            _server_id: ServerId,
            _user_id: UserId,
            _role: ServerMemberRole,
        ) -> RepoResult<()> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn remove_member(&self, _server_id: ServerId, _user_id: UserId) -> RepoResult<()> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn get_members_for_server(
            &self,
            _server_id: ServerId,
        ) -> RepoResult<Vec<ServerMember>> {
            Ok(vec![])
        }

        async fn is_member(&self, _server_id: ServerId, _user_id: UserId) -> RepoResult<bool> {
            Ok(self.is_member)
        }
    }

    struct FakeServerRepo {
        server: Option<Server>,
    }

    #[async_trait]
    impl ServerRepository for FakeServerRepo {
        async fn get_by_id(&self, _id: ServerId) -> RepoResult<Option<Server>> {
            Ok(self.server.clone())
        }

        async fn create_server(&self, _name: &str) -> RepoResult<Server> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn find_by_name(&self, _name: &str) -> RepoResult<Option<Server>> {
            Ok(None)
        }

        async fn get_servers_for_user(&self, _user_id: UserId) -> RepoResult<Vec<Server>> {
            Ok(vec![])
        }
    }

    struct FakeConversationRepo {
        conv: Option<Conversation>,
    }

    #[async_trait]
    impl ConversationRepository for FakeConversationRepo {
        async fn get_by_id(
            &self,
            _id: rim_core::conversation::model::ConversationId,
        ) -> RepoResult<Option<Conversation>> {
            Ok(self.conv.clone())
        }

        async fn create_conversation(
            &self,
            _conv: rim_core::conversation::model::NewConversation,
        ) -> RepoResult<Conversation> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn create_conversation_with_members(
            &self,
            _conversation_type: ConversationType,
            _server_id: ServerId,
            _title: Option<String>,
            _member_ids: &[UserId],
        ) -> RepoResult<Conversation> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn find_direct_between_users(
            &self,
            _server_id: ServerId,
            _user_a: UserId,
            _user_b: UserId,
        ) -> RepoResult<Option<Conversation>> {
            Ok(None)
        }

        async fn list_inbox_conversations_for_user(
            &self,
            _user_id: UserId,
            _server_id: ServerId,
            _limit: u32,
            _dir: PageDir,
            _cursor: Option<ConversationActivityCursor>,
        ) -> RepoResult<Page<Conversation, ConversationActivityCursor>> {
            Ok(Page::new(vec![], None, None))
        }

        async fn list_channels_for_user(
            &self,
            _user_id: UserId,
            _server_id: ServerId,
            _limit: u32,
            _dir: PageDir,
            _cursor: Option<ConversationCreatedCursor>,
        ) -> RepoResult<Page<Conversation, ConversationCreatedCursor>> {
            Ok(Page::new(vec![], None, None))
        }

        async fn touch_last_activity_at(
            &self,
            _conversation_id: rim_core::conversation::model::ConversationId,
            _at: chrono::DateTime<chrono::Utc>,
        ) -> RepoResult<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn ensure_server_member_denies_non_member() {
        let repo = FakeServerMemberRepo {
            is_member: false,
            member: None,
        };

        let err = ensure_server_member(&repo, ServerId::new(), UserId::new())
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::PermissionDenied));
    }

    #[tokio::test]
    async fn ensure_server_owner_denies_when_missing_member() {
        let repo = FakeServerMemberRepo {
            is_member: true,
            member: None,
        };

        let err = ensure_server_owner(&repo, ServerId::new(), UserId::new())
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::PermissionDenied));
    }

    #[tokio::test]
    async fn ensure_server_owner_denies_when_not_owner() {
        let server_id = ServerId::new();
        let user_id = UserId::new();
        let repo = FakeServerMemberRepo {
            is_member: true,
            member: Some(ServerMember {
                id: ServerMemberId::new(),
                server_id,
                user_id,
                member_role: ServerMemberRole::Guest,
                joined_at: chrono::Utc::now(),
            }),
        };

        let err = ensure_server_owner(&repo, server_id, user_id)
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::PermissionDenied));
    }

    #[tokio::test]
    async fn ensure_server_exists_returns_not_found() {
        let repo = FakeServerRepo { server: None };
        let err = ensure_server_exists(&repo, ServerId::new())
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::NotFound));
    }

    #[tokio::test]
    async fn ensure_conversation_in_server_returns_not_found_when_missing() {
        let repo = FakeConversationRepo { conv: None };
        let err = ensure_conversation_in_server(&repo, ConversationId::new(), ServerId::new())
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::NotFound));
    }

    #[tokio::test]
    async fn ensure_conversation_in_server_denies_cross_server() {
        let expected_server = ServerId::new();
        let other_server = ServerId::new();
        let repo = FakeConversationRepo {
            conv: Some(Conversation {
                id: ConversationId::new(),
                conversation_type: ConversationType::Channel,
                title: None,
                created_at: chrono::Utc::now(),
                last_activity_at: chrono::Utc::now(),
                server_id: other_server,
            }),
        };

        let err = ensure_conversation_in_server(&repo, ConversationId::new(), expected_server)
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::NotFound));
    }
}
