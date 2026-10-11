use std::sync::Arc;

use rim_core::conversation::model::{Conversation, ConversationId};
use rim_core::conversation::repository::ConversationRepository;
use rim_core::conversation_member::repository::ConversationMemberRepository;
use rim_core::moderation::repository::ModerationRepository;
use rim_core::server::model::ServerId;
use rim_core::server::repository::ServerRepository;
use rim_core::server_member::model::{MemberStanding, ServerMember, ServerMemberRole};
use rim_core::server_member::repository::ServerMemberRepository;
use rim_core::user::model::UserId;
use rim_services::{ServiceError, ServiceResult};

/// Log the parser's description of a malformed statement; the client sees only the error's
/// fixed message.
pub(crate) fn log_statement_detail(err: &rim_identity::IdentityError) {
    if let Some(detail) = err.detail() {
        tracing::info!(detail, "refused a malformed statement");
    }
}

/// A refused statement as `VALIDATION`, its parser detail logged and not returned.
pub(crate) fn statement_refused(err: rim_identity::IdentityError) -> ServiceError {
    log_statement_detail(&err);
    ServiceError::Validation(err.to_string())
}

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
/// 3. Conversation-level access (see [`ensure_conversation_access_as`]: public channels pass
///    with server membership alone, except for guests; private channels need membership and
///    fail with NotFound; DM/group need membership and fail with PermissionDenied)
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
    Ok(
        ensure_conversation_readable_with_standing(repos, actor, server_id, conversation_id)
            .await?
            .0,
    )
}

/// [`ensure_conversation_member_access`], also returning the actor's [`MemberStanding`] (role
/// and active timeout) for callers that need it (slow mode, moderator deletes).
pub async fn ensure_conversation_readable_with_standing(
    repos: ConversationAccessRepos<'_>,
    actor: UserId,
    server_id: ServerId,
    conversation_id: ConversationId,
) -> ServiceResult<(Conversation, MemberStanding)> {
    let standing = member_standing(repos.server_member_repo, server_id, actor).await?;
    let conv =
        ensure_conversation_in_server(repos.conversation_repo, conversation_id, server_id).await?;
    ensure_conversation_access_as(repos.conversation_member_repo, &conv, actor, standing.role)
        .await?;
    Ok((conv, standing))
}

/// [`ensure_conversation_member_access`] plus [`ensure_not_archived`] and
/// [`ensure_not_timed_out`]: the gate for every write into a conversation (messages, reactions,
/// pins, topic, attachments, typing). Archived conversations stay readable through
/// [`ensure_conversation_member_access`]; a timed-out member keeps reading.
pub async fn ensure_conversation_writable(
    repos: ConversationAccessRepos<'_>,
    actor: UserId,
    server_id: ServerId,
    conversation_id: ConversationId,
) -> ServiceResult<Conversation> {
    Ok(
        ensure_conversation_writable_with_standing(repos, actor, server_id, conversation_id)
            .await?
            .0,
    )
}

/// [`ensure_conversation_writable`], also returning the actor's [`MemberStanding`].
pub async fn ensure_conversation_writable_with_standing(
    repos: ConversationAccessRepos<'_>,
    actor: UserId,
    server_id: ServerId,
    conversation_id: ConversationId,
) -> ServiceResult<(Conversation, MemberStanding)> {
    let (conv, standing) =
        ensure_conversation_readable_with_standing(repos, actor, server_id, conversation_id)
            .await?;
    ensure_not_archived(&conv)?;
    ensure_not_timed_out(&standing)?;
    Ok((conv, standing))
}

/// The actor's role and active timeout in `server_id`; `PermissionDenied` for a non-member.
pub async fn member_standing(
    server_member_repo: &dyn ServerMemberRepository,
    server_id: ServerId,
    user_id: UserId,
) -> ServiceResult<MemberStanding> {
    server_member_repo
        .get_member_standing(server_id, user_id)
        .await?
        .ok_or(ServiceError::PermissionDenied)
}

/// `Err(TimedOut { until })` while the member's timeout is in force.
pub fn ensure_not_timed_out(standing: &MemberStanding) -> ServiceResult<()> {
    match standing.timed_out_until {
        Some(until) if until > chrono::Utc::now() => Err(ServiceError::TimedOut { until }),
        _ => Ok(()),
    }
}

/// Workspace-level write gate for actions not tied to an existing conversation (creating a
/// conversation): the actor must be a member and not timed out. Returns the standing so the
/// caller can apply role rules (guests cannot create channels or groups).
pub async fn ensure_workspace_writable(
    server_member_repo: &dyn ServerMemberRepository,
    server_id: ServerId,
    user_id: UserId,
) -> ServiceResult<MemberStanding> {
    let standing = member_standing(server_member_repo, server_id, user_id).await?;
    ensure_not_timed_out(&standing)?;
    Ok(standing)
}

/// `Err(PermissionDenied)` while `user_id` has an active ban in `server_id`. Called
/// by every workspace join path except workspace creation, where no ban can exist.
pub async fn ensure_not_banned(
    moderation_repo: &dyn ModerationRepository,
    server_id: ServerId,
    user_id: UserId,
) -> ServiceResult<()> {
    if moderation_repo
        .get_active_ban(server_id, user_id, chrono::Utc::now())
        .await?
        .is_some()
    {
        return Err(ServiceError::PermissionDenied);
    }
    Ok(())
}

/// `Err(ConversationArchived)` when `conversation` is archived (read-only).
pub fn ensure_not_archived(conversation: &Conversation) -> ServiceResult<()> {
    if conversation.is_archived() {
        return Err(ServiceError::ConversationArchived);
    }
    Ok(())
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

/// The one way a user becomes a workspace member: inserts the `server_member` row, then joins
/// the user to every public, unarchived channel of the workspace, skipping channels
/// they are already in, so a member who arrives after the channel back-fill does not start with
/// an empty joined-channel list. Private channels still need an invitation. A `Guest` joins no
/// channel: guests only see conversations they are explicitly added to.
///
/// Ban checks ([`ensure_not_banned`]) are the caller's job, before calling this.
///
/// Every server-membership write (server creation, invite acceptance, join by invite, signup by
/// workspace invite, owner adding a member) goes through here, with the caller's repos — so
/// inside the caller's transaction when it has one. Emits no events: callers announce the new member with
/// `MemberJoined` after they commit, and clients refetch their channel list on joining a
/// workspace.
pub async fn add_workspace_member(
    server_member_repo: &dyn ServerMemberRepository,
    conversation_repo: &dyn ConversationRepository,
    conversation_member_repo: &dyn ConversationMemberRepository,
    server_id: ServerId,
    user_id: UserId,
    role: ServerMemberRole,
) -> ServiceResult<ServerMember> {
    let member = server_member_repo
        .add_member(server_id, user_id, role)
        .await?;
    if role.is_guest() {
        return Ok(member);
    }

    // Checked one by one rather than tolerating a unique-index conflict: inside a Postgres
    // transaction a failed insert would abort the whole transaction.
    for conversation_id in conversation_repo.list_public_channel_ids(server_id).await? {
        if !conversation_member_repo
            .is_member(conversation_id, user_id)
            .await?
        {
            conversation_member_repo
                .add_member(conversation_id, user_id)
                .await?;
        }
    }

    Ok(member)
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

/// Gates on `member_role ∈ {Owner, Moderator}` — the edit gate for server-scoped actions that
/// have no per-channel moderator concept (such as archiving a channel), so the server role alone
/// governs.
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

/// [`ensure_conversation_access_as`] for a non-guest member.
pub async fn ensure_conversation_access(
    conversation_member_repo: &dyn ConversationMemberRepository,
    conversation: &rim_core::conversation::model::Conversation,
    user_id: rim_core::user::model::UserId,
) -> ServiceResult<()> {
    ensure_conversation_access_as(
        conversation_member_repo,
        conversation,
        user_id,
        ServerMemberRole::Member,
    )
    .await
}

/// Check that a workspace member with `role` can read the conversation (archived or not).
/// - Public channels: server membership is sufficient — no conversation_member check needed —
///   except for a `Guest`, who must be a conversation member like in a private channel
///   (`NotFound` otherwise).
/// - Private channels: the user must be a conversation member; a non-member gets `NotFound`, so
///   the response does not reveal that the channel exists.
/// - DMs and groups: the user must be a conversation member (`PermissionDenied` otherwise).
pub async fn ensure_conversation_access_as(
    conversation_member_repo: &dyn ConversationMemberRepository,
    conversation: &rim_core::conversation::model::Conversation,
    user_id: rim_core::user::model::UserId,
    role: ServerMemberRole,
) -> ServiceResult<()> {
    if conversation.is_public_channel() && !role.is_guest() {
        return Ok(());
    }
    let is_member = conversation_member_repo
        .is_member(conversation.id, user_id)
        .await?;
    if !is_member {
        return Err(
            if conversation.conversation_type
                == rim_core::conversation::model::ConversationType::Channel
            {
                ServiceError::NotFound
            } else {
                ServiceError::PermissionDenied
            },
        );
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
                    .add_member(server_id, user_id, ServerMemberRole::Member)
                    .await
                    .unwrap_err()
            ),
            ServiceError::Internal(_)
        ));
        assert!(matches!(
            ServiceError::from(
                member_repo
                    .change_member_role(server_id, user_id, ServerMemberRole::Member)
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
                        is_private: false,
                        created_by: None,
                        encrypted: false,
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

        async fn get_member_standing(
            &self,
            _server_id: ServerId,
            _user_id: UserId,
        ) -> RepoResult<Option<MemberStanding>> {
            Ok(self.is_member.then(|| MemberStanding {
                role: self
                    .member
                    .as_ref()
                    .map_or(ServerMemberRole::Member, |m| m.member_role),
                timed_out_until: None,
            }))
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
            _conv: rim_core::conversation::model::NewConversation,
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
            _filter: rim_core::conversation::repository::ChannelListFilter,
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

        async fn list_public_channel_ids(
            &self,
            _server_id: ServerId,
        ) -> RepoResult<Vec<ConversationId>> {
            Ok(vec![])
        }

        async fn list_joined_conversation_ids(
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

        async fn set_archived(
            &self,
            _conversation_id: rim_core::conversation::model::ConversationId,
            _archived_at: Option<chrono::DateTime<chrono::Utc>>,
        ) -> RepoResult<Conversation> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn set_slow_mode(
            &self,
            _conversation_id: rim_core::conversation::model::ConversationId,
            _seconds: i32,
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
                member_role: ServerMemberRole::Member,
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
                member_role: ServerMemberRole::Member,
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
                is_private: false,
                archived_at: None,
                created_by: None,
                slow_mode_seconds: 0,
                encrypted_at: None,
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

    // ── Access-gate matrix ──
    //
    // (public channel | private channel | group | DM) × (member | non-member) × archived, for
    // the read gate (`ensure_conversation_member_access`) and the write gate
    // (`ensure_conversation_writable`). The private-channel non-member must get `NotFound`, never
    // `PermissionDenied`, so the response does not reveal that the channel exists.

    #[derive(Debug, PartialEq, Eq)]
    enum Outcome {
        Ok,
        NotFound,
        Forbidden,
        Archived,
    }

    fn outcome<T>(res: ServiceResult<T>) -> Outcome {
        match res {
            Ok(_) => Outcome::Ok,
            Err(ServiceError::NotFound) => Outcome::NotFound,
            Err(ServiceError::PermissionDenied) => Outcome::Forbidden,
            Err(ServiceError::ConversationArchived) => Outcome::Archived,
            Err(other) => panic!("unexpected error {other:?}"),
        }
    }

    async fn gate_outcomes(
        conversation_type: ConversationType,
        is_private: bool,
        is_member: bool,
        archived: bool,
    ) -> (Outcome, Outcome) {
        let server_id = ServerId::new();
        let conversation = Conversation {
            id: ConversationId::new(),
            conversation_type,
            title: None,
            topic: None,
            created_at: chrono::Utc::now(),
            last_activity_at: chrono::Utc::now(),
            server_id,
            is_private,
            archived_at: archived.then(chrono::Utc::now),
            created_by: None,
            slow_mode_seconds: 0,
            encrypted_at: None,
        };
        let server_member_repo = FakeServerMemberRepo {
            is_member: true,
            member: None,
        };
        let conversation_repo = FakeConversationRepo {
            conv: Some(conversation.clone()),
        };
        let conversation_member_repo = FakeConversationMemberRepo { is_member };
        let repos = || ConversationAccessRepos {
            server_member_repo: &server_member_repo,
            conversation_repo: &conversation_repo,
            conversation_member_repo: &conversation_member_repo,
        };
        let actor = UserId::new();
        let read = outcome(
            ensure_conversation_member_access(repos(), actor, server_id, conversation.id).await,
        );
        let write =
            outcome(ensure_conversation_writable(repos(), actor, server_id, conversation.id).await);
        (read, write)
    }

    #[tokio::test]
    async fn access_gate_matrix() {
        use ConversationType::{Channel, Direct, Group};
        use Outcome::{Archived, Forbidden, NotFound, Ok};

        // (type, private, member, archived) => (read, write)
        let cases = [
            ((Channel, false, true, false), (Ok, Ok)),
            ((Channel, false, false, false), (Ok, Ok)),
            ((Channel, false, true, true), (Ok, Archived)),
            ((Channel, false, false, true), (Ok, Archived)),
            ((Channel, true, true, false), (Ok, Ok)),
            ((Channel, true, false, false), (NotFound, NotFound)),
            ((Channel, true, true, true), (Ok, Archived)),
            ((Channel, true, false, true), (NotFound, NotFound)),
            ((Group, false, true, false), (Ok, Ok)),
            ((Group, false, false, false), (Forbidden, Forbidden)),
            ((Group, false, true, true), (Ok, Archived)),
            ((Direct, false, true, false), (Ok, Ok)),
            ((Direct, false, false, false), (Forbidden, Forbidden)),
        ];
        for ((kind, private, member, archived), expected) in cases {
            let got = gate_outcomes(kind, private, member, archived).await;
            assert_eq!(
                got, expected,
                "type={kind:?} private={private} member={member} archived={archived}"
            );
        }
    }

    #[test]
    fn ensure_not_archived_rejects_archived_only() {
        let mut conversation = Conversation {
            id: ConversationId::new(),
            conversation_type: ConversationType::Channel,
            title: None,
            topic: None,
            created_at: chrono::Utc::now(),
            last_activity_at: chrono::Utc::now(),
            server_id: ServerId::new(),
            is_private: false,
            archived_at: None,
            created_by: None,
            slow_mode_seconds: 0,
            encrypted_at: None,
        };
        assert!(ensure_not_archived(&conversation).is_ok());
        conversation.archived_at = Some(chrono::Utc::now());
        assert!(matches!(
            ensure_not_archived(&conversation),
            Err(ServiceError::ConversationArchived)
        ));
    }

    // ── Guest and timeout gates ──

    fn guest_repo() -> FakeServerMemberRepo {
        FakeServerMemberRepo {
            is_member: true,
            member: Some(ServerMember {
                id: ServerMemberId::new(),
                server_id: ServerId::new(),
                user_id: UserId::new(),
                member_role: ServerMemberRole::Guest,
                joined_at: chrono::Utc::now(),
            }),
        }
    }

    /// A guest reads a public channel only as a conversation member, and a non-member guest
    /// gets `NotFound` (like a private channel), so the response does not confirm the id.
    #[tokio::test]
    async fn guest_needs_membership_for_public_channels() {
        let server_id = ServerId::new();
        let conversation = Conversation {
            id: ConversationId::new(),
            conversation_type: ConversationType::Channel,
            title: None,
            topic: None,
            created_at: chrono::Utc::now(),
            last_activity_at: chrono::Utc::now(),
            server_id,
            is_private: false,
            archived_at: None,
            created_by: None,
            slow_mode_seconds: 0,
            encrypted_at: None,
        };
        let server_member_repo = guest_repo();
        let conversation_repo = FakeConversationRepo {
            conv: Some(conversation.clone()),
        };
        for (is_member, expected) in [(false, Outcome::NotFound), (true, Outcome::Ok)] {
            let conversation_member_repo = FakeConversationMemberRepo { is_member };
            let repos = ConversationAccessRepos {
                server_member_repo: &server_member_repo,
                conversation_repo: &conversation_repo,
                conversation_member_repo: &conversation_member_repo,
            };
            let got = outcome(
                ensure_conversation_member_access(repos, UserId::new(), server_id, conversation.id)
                    .await,
            );
            assert_eq!(got, expected, "guest, conversation member = {is_member}");
        }
    }

    #[test]
    fn ensure_not_timed_out_only_refuses_future_timeouts() {
        let mut standing = MemberStanding::new(ServerMemberRole::Member);
        assert!(ensure_not_timed_out(&standing).is_ok());
        standing.timed_out_until = Some(chrono::Utc::now() - chrono::Duration::seconds(1));
        assert!(ensure_not_timed_out(&standing).is_ok(), "expired timeout");
        let until = chrono::Utc::now() + chrono::Duration::minutes(5);
        standing.timed_out_until = Some(until);
        assert!(matches!(
            ensure_not_timed_out(&standing),
            Err(ServiceError::TimedOut { until: u }) if u == until
        ));
    }

    /// A timed-out member keeps reading a conversation but cannot write to it.
    #[tokio::test]
    async fn timed_out_member_reads_but_cannot_write() {
        use rim_core::server_member::repository::MockServerMemberRepository;

        let server_id = ServerId::new();
        let until = chrono::Utc::now() + chrono::Duration::minutes(10);
        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_get_member_standing()
            .returning(move |_, _| {
                Ok(Some(MemberStanding {
                    role: ServerMemberRole::Member,
                    timed_out_until: Some(until),
                }))
            });
        let conversation = Conversation {
            id: ConversationId::new(),
            conversation_type: ConversationType::Channel,
            title: None,
            topic: None,
            created_at: chrono::Utc::now(),
            last_activity_at: chrono::Utc::now(),
            server_id,
            is_private: false,
            archived_at: None,
            created_by: None,
            slow_mode_seconds: 0,
            encrypted_at: None,
        };
        let conversation_repo = FakeConversationRepo {
            conv: Some(conversation.clone()),
        };
        let conversation_member_repo = FakeConversationMemberRepo { is_member: true };
        let repos = || ConversationAccessRepos {
            server_member_repo: &server_member_repo,
            conversation_repo: &conversation_repo,
            conversation_member_repo: &conversation_member_repo,
        };
        let actor = UserId::new();
        assert!(
            ensure_conversation_member_access(repos(), actor, server_id, conversation.id)
                .await
                .is_ok()
        );
        assert!(matches!(
            ensure_conversation_writable(repos(), actor, server_id, conversation.id).await,
            Err(ServiceError::TimedOut { .. })
        ));
        assert!(matches!(
            ensure_workspace_writable(&server_member_repo, server_id, actor).await,
            Err(ServiceError::TimedOut { .. })
        ));
    }

    #[tokio::test]
    async fn ensure_not_banned_refuses_an_active_ban() {
        use rim_core::moderation::model::ServerBan;
        use rim_core::moderation::repository::MockModerationRepository;

        let banned = UserId::new();
        let mut repo = MockModerationRepository::new();
        repo.expect_get_active_ban().returning(move |sid, uid, _| {
            Box::pin(async move {
                Ok((uid == banned).then(|| ServerBan {
                    server_id: sid,
                    user_id: uid,
                    reason: None,
                    banned_by: None,
                    created_at: chrono::Utc::now(),
                    expires_at: None,
                }))
            })
        });
        let server_id = ServerId::new();
        assert!(matches!(
            ensure_not_banned(&repo, server_id, banned).await,
            Err(ServiceError::PermissionDenied)
        ));
        assert!(
            ensure_not_banned(&repo, server_id, UserId::new())
                .await
                .is_ok()
        );
    }

    /// A guest joins no channel on arrival.
    #[tokio::test]
    async fn add_workspace_member_skips_auto_join_for_guests() {
        use rim_core::conversation::repository::MockConversationRepository;
        use rim_core::conversation_member::repository::MockConversationMemberRepository;
        use rim_core::server_member::repository::MockServerMemberRepository;

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_add_member()
            .times(1)
            .returning(|sid, uid, role| {
                Ok(ServerMember {
                    id: ServerMemberId::new(),
                    server_id: sid,
                    user_id: uid,
                    member_role: role,
                    joined_at: chrono::Utc::now(),
                })
            });
        let mut conversation_repo = MockConversationRepository::new();
        conversation_repo.expect_list_public_channel_ids().never();
        let conversation_member_repo = MockConversationMemberRepository::new();

        let member = add_workspace_member(
            &server_member_repo,
            &conversation_repo,
            &conversation_member_repo,
            ServerId::new(),
            UserId::new(),
            ServerMemberRole::Guest,
        )
        .await
        .expect("add guest");
        assert_eq!(member.member_role, ServerMemberRole::Guest);
    }

    // ── add_workspace_member: auto-join ──

    #[tokio::test]
    async fn add_workspace_member_joins_public_channels_skipping_existing() {
        use rim_core::conversation::repository::MockConversationRepository;
        use rim_core::conversation_member::model::{ConversationMember, ConversationMemberId};
        use rim_core::conversation_member::repository::MockConversationMemberRepository;
        use rim_core::server_member::repository::MockServerMemberRepository;

        let server_id = ServerId::new();
        let user_id = UserId::new();
        let already = ConversationId::new();
        let fresh = ConversationId::new();

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_add_member()
            .withf(move |sid, uid, role| {
                *sid == server_id && *uid == user_id && *role == ServerMemberRole::Member
            })
            .times(1)
            .returning(|sid, uid, role| {
                Ok(ServerMember {
                    id: ServerMemberId::new(),
                    server_id: sid,
                    user_id: uid,
                    member_role: role,
                    joined_at: chrono::Utc::now(),
                })
            });
        let mut conversation_repo = MockConversationRepository::new();
        conversation_repo
            .expect_list_public_channel_ids()
            .withf(move |sid| *sid == server_id)
            .returning(move |_| Box::pin(async move { Ok(vec![already, fresh]) }));
        let mut conversation_member_repo = MockConversationMemberRepository::new();
        conversation_member_repo
            .expect_is_member()
            .returning(move |cid, _| Box::pin(async move { Ok(cid == already) }));
        conversation_member_repo
            .expect_add_member()
            .withf(move |cid, uid| *cid == fresh && *uid == user_id)
            .times(1)
            .returning(|cid, uid| {
                Box::pin(async move {
                    Ok(ConversationMember {
                        id: ConversationMemberId::new(),
                        user_id: uid,
                        conversation_id: cid,
                        joined_at: chrono::Utc::now(),
                    })
                })
            });

        let member = add_workspace_member(
            &server_member_repo,
            &conversation_repo,
            &conversation_member_repo,
            server_id,
            user_id,
            ServerMemberRole::Member,
        )
        .await
        .expect("add workspace member");
        assert_eq!(member.user_id, user_id);
    }

    #[tokio::test]
    async fn add_workspace_member_joins_nothing_when_membership_insert_fails() {
        use rim_core::conversation::repository::MockConversationRepository;
        use rim_core::conversation_member::repository::MockConversationMemberRepository;
        use rim_core::server_member::repository::MockServerMemberRepository;

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_add_member()
            .returning(|_, _, _| Err(RepoError::new(RepoErrorKind::Conflict)));
        let mut conversation_repo = MockConversationRepository::new();
        conversation_repo.expect_list_public_channel_ids().never();
        let conversation_member_repo = MockConversationMemberRepository::new();

        let err = add_workspace_member(
            &server_member_repo,
            &conversation_repo,
            &conversation_member_repo,
            ServerId::new(),
            UserId::new(),
            ServerMemberRole::Member,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, ServiceError::Conflict(_)));
    }
}
