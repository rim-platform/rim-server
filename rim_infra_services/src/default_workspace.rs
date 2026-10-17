//! The server's default workspace: the workspace every account created here joins as a Member.

use chrono::Utc;

use rim_core::server::model::ServerId;
use rim_core::server::repository::ServerRepository;
use rim_core::server_member::model::ServerMemberRole;
use rim_core::user::model::UserId;
use rim_services::ServiceResult;
use rim_services::events::{EventObserver, Observers, WorkspaceEvent};

use crate::signup::SignupRepos;
use crate::util::add_workspace_member;

/// Check that `workspace` can be the default workspace: it exists on this server and is not the
/// direct space `direct_space_id`. Run once at start.
///
/// # Errors
/// A message naming the problem, or the storage error the lookup failed with.
pub async fn check(
    server_repo: &dyn ServerRepository,
    direct_space_id: ServerId,
    workspace: ServerId,
) -> Result<(), String> {
    if workspace == direct_space_id {
        return Err(format!("{workspace} is the direct space, not a workspace"));
    }
    match server_repo.get_by_id(workspace).await {
        Ok(Some(_)) => Ok(()),
        Ok(None) => Err(format!("no workspace {workspace} exists on this server")),
        Err(e) => Err(format!("could not look workspace {workspace} up: {e}")),
    }
}

/// Make the new account `user_id` a Member of `workspace` inside the signup's transaction, with
/// the public-channel auto-join every new member gets. The members-per-workspace cap does not
/// apply. Nothing happens without a default workspace, when the account already belongs to it
/// (it signed up with an invite to it, or was a member from another server), when it is banned
/// there, or when the workspace was deleted since the start. Returns the workspace when the
/// account joined it.
///
/// # Errors
/// Storage errors; the caller's transaction rolls back.
pub(crate) async fn join(
    repos: &SignupRepos,
    workspace: Option<ServerId>,
    user_id: UserId,
) -> ServiceResult<Option<ServerId>> {
    let Some(workspace) = workspace else {
        return Ok(None);
    };
    if repos
        .server_member_repo
        .is_member(workspace, user_id)
        .await?
    {
        return Ok(None);
    }
    if repos
        .moderation_repo
        .get_active_ban(workspace, user_id, Utc::now())
        .await?
        .is_some()
    {
        return Ok(None);
    }
    if repos.server_repo.get_by_id(workspace).await?.is_none() {
        tracing::warn!(
            server_id = %workspace,
            "RIM_DEFAULT_WORKSPACE names a workspace that no longer exists; the new account \
             joins no workspace"
        );
        return Ok(None);
    }
    add_workspace_member(
        repos.server_member_repo.as_ref(),
        repos.conversation_repo.as_ref(),
        repos.conversation_member_repo.as_ref(),
        workspace,
        user_id,
        ServerMemberRole::Member,
        None,
    )
    .await?;
    Ok(Some(workspace))
}

/// After the signup committed: when the account joined the default workspace `joined`, the
/// `MemberJoined` event other members see the newcomer by, and its audit line.
pub(crate) fn announce(
    joined: Option<ServerId>,
    user_id: UserId,
    username: &str,
    observers: &Observers,
) {
    let Some(workspace_id) = joined else {
        return;
    };
    observers.emit(WorkspaceEvent::MemberJoined {
        workspace_id,
        user_id,
        username: username.to_string(),
    });
    tracing::info!(
        target: "audit",
        event = "default_workspace_joined",
        user_id = %user_id,
        server_id = %workspace_id,
        "new account joined the default workspace"
    );
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use rim_core::auth::repository::{MockInstanceInviteRepository, MockInviteRepository};
    use rim_core::conversation::repository::MockConversationRepository;
    use rim_core::conversation_member::model::{ConversationMember, ConversationMemberId};
    use rim_core::conversation_member::repository::MockConversationMemberRepository;
    use rim_core::moderation::model::ServerBan;
    use rim_core::moderation::repository::MockModerationRepository;
    use rim_core::repository::error::{RepoError, RepoErrorKind};
    use rim_core::server::model::Server;
    use rim_core::server::repository::MockServerRepository;
    use rim_core::server_member::model::{ServerMember, ServerMemberId};
    use rim_core::server_member::repository::MockServerMemberRepository;
    use rim_core::user::repository::MockUserRepository;
    use rim_services::events::Broadcaster;

    use super::*;

    fn server(id: ServerId) -> Server {
        Server {
            id,
            name: "Lobby".to_string(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn servers_with(id: ServerId) -> MockServerRepository {
        let mut repo = MockServerRepository::new();
        repo.expect_get_by_id()
            .returning(move |asked| Ok((asked == id).then(|| server(id))));
        repo
    }

    fn not_banned() -> MockModerationRepository {
        let mut repo = MockModerationRepository::new();
        repo.expect_get_active_ban()
            .returning(|_, _, _| Box::pin(async { Ok(None) }));
        repo
    }

    fn repos(
        servers: MockServerRepository,
        members: MockServerMemberRepository,
        moderation: MockModerationRepository,
        conversations: MockConversationRepository,
        conversation_members: MockConversationMemberRepository,
    ) -> SignupRepos {
        SignupRepos {
            instance_invite_repo: Arc::new(MockInstanceInviteRepository::new()),
            invite_repo: Arc::new(MockInviteRepository::new()),
            server_repo: Arc::new(servers),
            server_member_repo: Arc::new(members),
            conversation_repo: Arc::new(conversations),
            conversation_member_repo: Arc::new(conversation_members),
            moderation_repo: Arc::new(moderation),
            user_repo: Arc::new(MockUserRepository::new()),
        }
    }

    /// Server membership that answers `is_member` with `member` and expects `adds` inserts.
    fn members(member: bool, adds: usize) -> MockServerMemberRepository {
        let mut repo = MockServerMemberRepository::new();
        repo.expect_is_member().returning(move |_, _| Ok(member));
        repo.expect_add_member()
            .times(adds)
            .returning(|server_id, user_id, member_role| {
                Ok(ServerMember {
                    id: ServerMemberId::new(),
                    server_id,
                    user_id,
                    member_role,
                    joined_at: Utc::now(),
                })
            });
        repo
    }

    #[tokio::test]
    async fn check_accepts_an_ordinary_workspace_of_this_server() {
        let workspace = ServerId::new();
        let res = check(&servers_with(workspace), ServerId::new(), workspace).await;
        assert_eq!(res, Ok(()));
    }

    #[tokio::test]
    async fn check_refuses_an_unknown_workspace_naming_it() {
        let workspace = ServerId::new();
        let err = check(&servers_with(ServerId::new()), ServerId::new(), workspace)
            .await
            .unwrap_err();
        assert_eq!(
            err,
            format!("no workspace {workspace} exists on this server")
        );
    }

    #[tokio::test]
    async fn check_refuses_the_direct_space_without_a_lookup() {
        let direct = ServerId::new();
        let mut servers = MockServerRepository::new();
        servers.expect_get_by_id().never();
        let err = check(&servers, direct, direct).await.unwrap_err();
        assert_eq!(
            err,
            format!("{direct} is the direct space, not a workspace")
        );
    }

    #[tokio::test]
    async fn check_reports_a_failed_lookup() {
        let mut servers = MockServerRepository::new();
        servers
            .expect_get_by_id()
            .returning(|_| Err(RepoError::new(RepoErrorKind::Internal)));
        let err = check(&servers, ServerId::new(), ServerId::new())
            .await
            .unwrap_err();
        assert!(err.starts_with("could not look workspace"), "{err}");
    }

    /// The account becomes a Member and joins the public channels it is not in yet.
    #[tokio::test]
    async fn join_adds_a_member_with_the_public_channels() {
        let workspace = ServerId::new();
        let user_id = UserId::new();
        let lobby = rim_core::conversation::model::ConversationId::new();

        let mut server_members = MockServerMemberRepository::new();
        server_members
            .expect_is_member()
            .withf(move |s, u| *s == workspace && *u == user_id)
            .returning(|_, _| Ok(false));
        server_members
            .expect_add_member()
            .withf(move |s, u, role| {
                *s == workspace && *u == user_id && *role == ServerMemberRole::Member
            })
            .times(1)
            .returning(|server_id, user_id, member_role| {
                Ok(ServerMember {
                    id: ServerMemberId::new(),
                    server_id,
                    user_id,
                    member_role,
                    joined_at: Utc::now(),
                })
            });
        let mut conversations = MockConversationRepository::new();
        conversations
            .expect_list_public_channel_ids()
            .withf(move |s| *s == workspace)
            .returning(move |_| Box::pin(async move { Ok(vec![lobby]) }));
        let mut conversation_members = MockConversationMemberRepository::new();
        conversation_members
            .expect_is_member()
            .returning(|_, _| Box::pin(async { Ok(false) }));
        conversation_members
            .expect_add_member()
            .withf(move |c, u| *c == lobby && *u == user_id)
            .times(1)
            .returning(|conversation_id, user_id| {
                Box::pin(async move {
                    Ok(ConversationMember {
                        id: ConversationMemberId::new(),
                        user_id,
                        conversation_id,
                        joined_at: Utc::now(),
                        pending_since: None,
                    })
                })
            });

        let r = repos(
            servers_with(workspace),
            server_members,
            not_banned(),
            conversations,
            conversation_members,
        );
        assert_eq!(
            join(&r, Some(workspace), user_id).await.unwrap(),
            Some(workspace)
        );
    }

    #[tokio::test]
    async fn join_without_a_default_workspace_touches_nothing() {
        let mut server_members = MockServerMemberRepository::new();
        server_members.expect_is_member().never();
        server_members.expect_add_member().never();
        let r = repos(
            MockServerRepository::new(),
            server_members,
            MockModerationRepository::new(),
            MockConversationRepository::new(),
            MockConversationMemberRepository::new(),
        );
        assert_eq!(join(&r, None, UserId::new()).await.unwrap(), None);
    }

    /// An account already in the workspace keeps its membership and role.
    #[tokio::test]
    async fn join_skips_an_account_that_is_already_a_member() {
        let workspace = ServerId::new();
        let r = repos(
            servers_with(workspace),
            members(true, 0),
            MockModerationRepository::new(),
            MockConversationRepository::new(),
            MockConversationMemberRepository::new(),
        );
        assert_eq!(
            join(&r, Some(workspace), UserId::new()).await.unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn join_skips_an_account_banned_from_the_workspace() {
        let workspace = ServerId::new();
        let mut moderation = MockModerationRepository::new();
        moderation
            .expect_get_active_ban()
            .returning(move |server_id, user_id, _| {
                Box::pin(async move {
                    Ok(Some(ServerBan {
                        server_id,
                        user_id,
                        reason: None,
                        banned_by: None,
                        created_at: Utc::now(),
                        expires_at: None,
                    }))
                })
            });
        let r = repos(
            servers_with(workspace),
            members(false, 0),
            moderation,
            MockConversationRepository::new(),
            MockConversationMemberRepository::new(),
        );
        assert_eq!(
            join(&r, Some(workspace), UserId::new()).await.unwrap(),
            None
        );
    }

    /// A workspace deleted after the start does not fail the signup.
    #[tokio::test]
    async fn join_skips_a_workspace_deleted_since_the_start() {
        let r = repos(
            servers_with(ServerId::new()),
            members(false, 0),
            not_banned(),
            MockConversationRepository::new(),
            MockConversationMemberRepository::new(),
        );
        assert_eq!(
            join(&r, Some(ServerId::new()), UserId::new())
                .await
                .unwrap(),
            None
        );
    }

    /// A failed insert fails the signup, so its transaction rolls back.
    #[tokio::test]
    async fn join_propagates_a_failed_insert() {
        let workspace = ServerId::new();
        let mut server_members = MockServerMemberRepository::new();
        server_members
            .expect_is_member()
            .returning(|_, _| Ok(false));
        server_members
            .expect_add_member()
            .returning(|_, _, _| Err(RepoError::new(RepoErrorKind::Internal)));
        let r = repos(
            servers_with(workspace),
            server_members,
            not_banned(),
            MockConversationRepository::new(),
            MockConversationMemberRepository::new(),
        );
        assert!(join(&r, Some(workspace), UserId::new()).await.is_err());
    }

    #[derive(Default)]
    struct Recording(Mutex<Vec<WorkspaceEvent>>);

    impl Broadcaster for Recording {
        fn broadcast(&self, event: WorkspaceEvent) {
            self.0.lock().unwrap().push(event);
        }
    }

    #[test]
    fn announce_emits_member_joined_only_after_a_join() {
        let seen = Arc::new(Recording::default());
        let observers = Observers::new(Arc::clone(&seen) as Arc<dyn Broadcaster>);
        let user_id = UserId::new();
        announce(None, user_id, "alice", &observers);
        assert!(seen.0.lock().unwrap().is_empty());
        let workspace = ServerId::new();
        announce(Some(workspace), user_id, "alice", &observers);
        let seen = seen.0.lock().unwrap();
        assert!(matches!(
            seen.as_slice(),
            [WorkspaceEvent::MemberJoined { workspace_id, user_id: u, username }]
                if *workspace_id == workspace && *u == user_id && username == "alice"
        ));
    }
}
