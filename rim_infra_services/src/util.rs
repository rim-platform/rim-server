use std::sync::Arc;

use rim_core::conversation::model::{Conversation, ConversationId};
use rim_core::conversation::repository::ConversationRepository;
use rim_core::conversation_member::repository::ConversationMemberRepository;
use rim_core::server::model::ServerId;
use rim_core::server::repository::ServerRepository;
use rim_core::server_member::model::ServerMemberRole;
use rim_core::server_member::repository::ServerMemberRepository;
use rim_core::user::model::UserId;
use rim_services::{ServiceError, ServiceResult};

/// Minimum repo set needed by [`ensure_conversation_member_access`].
/// Every service that holds a `*Repos` struct already has these three fields.
pub struct ConversationAccessRepos<'a> {
    pub server_member_repo: &'a dyn ServerMemberRepository,
    pub conversation_repo: &'a dyn ConversationRepository,
    pub conversation_member_repo: &'a dyn ConversationMemberRepository,
}

impl<'a> ConversationAccessRepos<'a> {
    pub fn from_arcs(
        server_member_repo: &'a Arc<dyn ServerMemberRepository>,
        conversation_repo: &'a Arc<dyn ConversationRepository>,
        conversation_member_repo: &'a Arc<dyn ConversationMemberRepository>,
    ) -> Self {
        Self {
            server_member_repo: server_member_repo.as_ref(),
            conversation_repo: conversation_repo.as_ref(),
            conversation_member_repo: conversation_member_repo.as_ref(),
        }
    }
}

/// Enforces the full three-step access gate in security-correct order:
/// 1. Server membership (PermissionDenied on failure)
/// 2. Conversation exists in that server (NotFound on failure)
/// 3. Conversation-level access (PermissionDenied on failure for DM/group;
///    channels pass with server membership alone)
///
/// Returns the loaded `Conversation` so callers avoid a second DB round-trip
/// for `conversation_type` lookups.
///
/// Error-variant ordering is security-intentional: `NotFound` leaks no
/// information about whether a conversation exists to non-server-members.
pub async fn ensure_conversation_member_access(
    repos: ConversationAccessRepos<'_>,
    actor: UserId,
    server_id: ServerId,
    conversation_id: ConversationId,
) -> ServiceResult<Conversation> {
    ensure_server_member(repos.server_member_repo, server_id, actor).await?;
    let conv =
        ensure_conversation_in_server(repos.conversation_repo, conversation_id, server_id).await?;
    ensure_conversation_access(repos.conversation_member_repo, &conv, actor).await?;
    Ok(conv)
}

pub async fn ensure_server_member(
    server_member_repo: &dyn ServerMemberRepository,
    server_id: ServerId,
    user_id: rim_core::user::model::UserId,
) -> ServiceResult<()> {
    let is_member = server_member_repo.is_member(server_id, user_id).await?;

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
        .await?
        .ok_or(ServiceError::PermissionDenied)?;

    if member.member_role != ServerMemberRole::Owner {
        return Err(ServiceError::PermissionDenied);
    }

    Ok(())
}

/// Gates on `member_role ∈ {Owner, Moderator}` — the edit gate for server-scoped resources that
/// don't have their own per-channel moderator concept (on-call schedules, escalation policies:
/// see spec §10, iron-man's ruling). Per-channel moderator roles do NOT apply here since these
/// resources are server-scoped, so the server role alone governs.
pub async fn ensure_server_owner_or_moderator(
    server_member_repo: &dyn ServerMemberRepository,
    server_id: ServerId,
    user_id: rim_core::user::model::UserId,
) -> ServiceResult<()> {
    let member = server_member_repo
        .get_member(server_id, user_id)
        .await?
        .ok_or(ServiceError::PermissionDenied)?;

    if !matches!(
        member.member_role,
        ServerMemberRole::Owner | ServerMemberRole::Moderator
    ) {
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
        .await?
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
        .await?
        .ok_or(ServiceError::NotFound)?;

    if conv.server_id != expected_server_id {
        return Err(ServiceError::NotFound);
    }

    Ok(conv)
}

/// Check that the user can access the conversation.
/// For channels (public), server membership is sufficient — no conversation_member check needed.
/// For DMs and groups, the user must be an explicit conversation member.
pub async fn ensure_conversation_access(
    conversation_member_repo: &dyn ConversationMemberRepository,
    conversation: &rim_core::conversation::model::Conversation,
    user_id: rim_core::user::model::UserId,
) -> ServiceResult<()> {
    if conversation.conversation_type == rim_core::conversation::model::ConversationType::Channel {
        return Ok(());
    }
    let is_member = conversation_member_repo
        .is_member(conversation.id, user_id)
        .await?;
    if !is_member {
        return Err(ServiceError::PermissionDenied);
    }
    Ok(())
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
            ServiceError::from(
                member_repo
                    .add_member(server_id, user_id, ServerMemberRole::Guest)
                    .await
                    .unwrap_err()
            ),
            ServiceError::Internal(_)
        ));
        assert!(matches!(
            ServiceError::from(
                member_repo
                    .change_member_role(server_id, user_id, ServerMemberRole::Guest)
                    .await
                    .unwrap_err()
            ),
            ServiceError::Internal(_)
        ));
        assert!(matches!(
            ServiceError::from(
                member_repo
                    .remove_member(server_id, user_id)
                    .await
                    .unwrap_err()
            ),
            ServiceError::Internal(_)
        ));

        let server_repo = FakeServerRepo { server: None };
        assert!(matches!(
            ServiceError::from(server_repo.create_server("x").await.unwrap_err()),
            ServiceError::Internal(_)
        ));

        let conv_repo = FakeConversationRepo { conv: None };
        assert!(matches!(
            ServiceError::from(
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

        async fn get_by_ids(
            &self,
            _ids: &[rim_core::conversation::model::ConversationId],
        ) -> RepoResult<Vec<Conversation>> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
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

        async fn exists_channel_by_title(
            &self,
            _server_id: ServerId,
            _title: &str,
        ) -> RepoResult<bool> {
            Ok(false)
        }

        async fn get_channel_by_title(
            &self,
            _server_id: ServerId,
            _title: &str,
        ) -> RepoResult<Option<Conversation>> {
            Ok(None)
        }

        async fn list_accessible_conversation_ids(
            &self,
            _server_id: ServerId,
            _user_id: UserId,
        ) -> RepoResult<Vec<ConversationId>> {
            Ok(vec![])
        }

        async fn touch_last_activity_at(
            &self,
            _conversation_id: rim_core::conversation::model::ConversationId,
            _at: chrono::DateTime<chrono::Utc>,
        ) -> RepoResult<()> {
            Ok(())
        }

        async fn update_topic(
            &self,
            _conversation_id: rim_core::conversation::model::ConversationId,
            _topic: Option<String>,
        ) -> RepoResult<Conversation> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
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
    async fn ensure_server_owner_or_moderator_allows_moderator() {
        let server_id = ServerId::new();
        let user_id = UserId::new();
        let repo = FakeServerMemberRepo {
            is_member: true,
            member: Some(ServerMember {
                id: ServerMemberId::new(),
                server_id,
                user_id,
                member_role: ServerMemberRole::Moderator,
                joined_at: chrono::Utc::now(),
            }),
        };

        ensure_server_owner_or_moderator(&repo, server_id, user_id)
            .await
            .expect("moderator must pass the owner-or-moderator gate");
    }

    #[tokio::test]
    async fn ensure_server_owner_or_moderator_denies_guest() {
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

        let err = ensure_server_owner_or_moderator(&repo, server_id, user_id)
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
                topic: None,
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

    // ── Gate-ordering pinning tests for ensure_conversation_member_access ──
    //
    // These pin the security-intentional gate order:
    //   1. server membership (PermissionDenied)
    //   2. conversation exists in server (NotFound)
    //   3. conversation-level access (PermissionDenied)
    //
    // A refactor that swaps steps 1 and 2 would let non-server-members probe
    // whether a conversation ID exists — observable via PermissionDenied vs
    // NotFound. These tests catch that regression.

    struct FakeConversationMemberRepo {
        is_member: bool,
    }

    #[async_trait]
    impl rim_core::conversation_member::repository::ConversationMemberRepository
        for FakeConversationMemberRepo
    {
        async fn get_by_id(
            &self,
            _id: rim_core::conversation_member::model::ConversationMemberId,
        ) -> RepoResult<Option<rim_core::conversation_member::model::ConversationMember>> {
            Ok(None)
        }

        async fn add_member(
            &self,
            _conversation_id: ConversationId,
            _user_id: UserId,
        ) -> RepoResult<rim_core::conversation_member::model::ConversationMember> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn remove_member(
            &self,
            _conversation_id: ConversationId,
            _user_id: UserId,
        ) -> RepoResult<()> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn get_members_for_conversation(
            &self,
            _conversation_id: ConversationId,
        ) -> RepoResult<Vec<rim_core::conversation_member::model::ConversationMember>> {
            Ok(vec![])
        }

        async fn is_member(
            &self,
            _conversation_id: ConversationId,
            _user_id: UserId,
        ) -> RepoResult<bool> {
            Ok(self.is_member)
        }

        async fn list_member_ids(
            &self,
            _conversation_id: ConversationId,
        ) -> RepoResult<Vec<UserId>> {
            Ok(vec![])
        }

        async fn list_member_ids_for_conversations(
            &self,
            _conversation_ids: &[ConversationId],
        ) -> RepoResult<std::collections::HashMap<ConversationId, Vec<UserId>>> {
            Ok(std::collections::HashMap::new())
        }
    }

    /// Non-server-member gets `PermissionDenied` BEFORE the conversation is
    /// looked up — even if the conversation doesn't exist. Pinning step 1 before
    /// step 2: a swap would return `NotFound` here instead.
    #[tokio::test]
    async fn gate_order_non_server_member_denied_before_conv_lookup() {
        let server_id = ServerId::new();
        let actor = UserId::new();

        let repos = ConversationAccessRepos {
            server_member_repo: &FakeServerMemberRepo {
                is_member: false,
                member: None,
            },
            conversation_repo: &FakeConversationRepo { conv: None },
            conversation_member_repo: &FakeConversationMemberRepo { is_member: false },
        };

        let err = ensure_conversation_member_access(repos, actor, server_id, ConversationId::new())
            .await
            .unwrap_err();

        assert!(
            matches!(err, ServiceError::PermissionDenied),
            "non-server-member must receive PermissionDenied (step 1), not {:?}",
            err
        );
    }

    /// Server-member querying a conversation that doesn't exist (or belongs to a
    /// different server) gets `NotFound` — leaking no extra information about the
    /// conversation beyond what a member is entitled to know. Pinning step 2 before
    /// step 3: if step 3 ran first, an absent conversation_member row would return
    /// `PermissionDenied` instead, masking the real lookup failure.
    #[tokio::test]
    async fn gate_order_missing_conversation_yields_not_found_for_server_member() {
        let server_id = ServerId::new();
        let actor = UserId::new();

        let repos = ConversationAccessRepos {
            server_member_repo: &FakeServerMemberRepo {
                is_member: true,
                member: None,
            },
            conversation_repo: &FakeConversationRepo { conv: None },
            conversation_member_repo: &FakeConversationMemberRepo { is_member: false },
        };

        let err = ensure_conversation_member_access(repos, actor, server_id, ConversationId::new())
            .await
            .unwrap_err();

        assert!(
            matches!(err, ServiceError::NotFound),
            "missing conversation must yield NotFound (step 2) for a server-member, not {:?}",
            err
        );
    }
}
