//! The owner claim of a defined default workspace and the start that offers it, against
//! repository mocks.

use std::sync::Mutex;

use rim_auth::session_token::hash_token;
use rim_core::auth::model::{InstanceInvite, InstanceInviteId, Invite, InviteId};
use rim_core::auth::repository::{MockInstanceInviteRepository, MockInviteRepository};
use rim_core::conversation::model::{Conversation, ConversationId};
use rim_core::conversation::repository::MockConversationRepository;
use rim_core::conversation_member::repository::MockConversationMemberRepository;
use rim_core::default_workspace::model::DefaultWorkspaceRecord;
use rim_core::default_workspace::repository::MockDefaultWorkspaceRepository;
use rim_core::dm_policy::model::{MemberDm, OutsideFloor};
use rim_core::dm_policy::repository::MockDmPolicyRepository;
use rim_core::moderation::repository::MockModerationRepository;
use rim_core::server::model::Server;
use rim_core::server::repository::MockServerRepository;
use rim_core::server_member::model::{ServerMember, ServerMemberId};
use rim_core::server_member::repository::MockServerMemberRepository;
use rim_core::user::model::User;
use rim_core::user::repository::MockUserRepository;
use rim_services::events::Broadcaster;

use super::*;

const DEFINITION: &str = r#"{
  "name": "RIM",
  "claimGrantsOperator": true,
  "dm": { "outsideFloor": "REQUESTS", "memberDefault": "DIRECT" },
  "channels": [
    { "name": "general", "topic": "Anything about RIM" },
    { "name": "staff", "private": true }
  ]
}"#;

fn defined(file: PathBuf) -> DefaultWorkspace {
    DefaultWorkspace::Defined(Arc::new(DefinedWorkspace {
        definition: WorkspaceDefinition::parse(DEFINITION).expect("definition"),
        claim_link_file: file,
    }))
}

fn instance_invite(id: InstanceInviteId) -> InstanceInvite {
    InstanceInvite {
        id,
        created_by_user_id: None,
        token_hash: hash_token("rim_iinv_claim"),
        created_at: Utc::now(),
        expires_at: Utc::now() + Duration::days(30),
        max_uses: 1,
        use_count: 1,
        revoked_at: None,
        email: None,
    }
}

fn workspace_invite() -> Invite {
    Invite {
        id: InviteId::new(),
        server_id: ServerId::new(),
        created_by_user_id: UserId::new(),
        token_hash: hash_token("rim_inv"),
        expires_at: None,
        max_uses: 1,
        use_count: 1,
        revoked_at: None,
        created_at: Utc::now(),
        role: ServerMemberRole::Member,
    }
}

/// Signup repositories that fail any call a test does not prime.
struct Repos {
    servers: MockServerRepository,
    members: MockServerMemberRepository,
    conversations: MockConversationRepository,
    conversation_members: MockConversationMemberRepository,
    users: MockUserRepository,
    dm_policy: MockDmPolicyRepository,
    record: MockDefaultWorkspaceRepository,
}

impl Repos {
    fn new() -> Self {
        Self {
            servers: MockServerRepository::new(),
            members: MockServerMemberRepository::new(),
            conversations: MockConversationRepository::new(),
            conversation_members: MockConversationMemberRepository::new(),
            users: MockUserRepository::new(),
            dm_policy: MockDmPolicyRepository::new(),
            record: MockDefaultWorkspaceRepository::new(),
        }
    }

    fn signup(self) -> SignupRepos {
        SignupRepos {
            instance_invite_repo: Arc::new(MockInstanceInviteRepository::new()),
            invite_repo: Arc::new(MockInviteRepository::new()),
            server_repo: Arc::new(self.servers),
            server_member_repo: Arc::new(self.members),
            conversation_repo: Arc::new(self.conversations),
            conversation_member_repo: Arc::new(self.conversation_members),
            moderation_repo: Arc::new(MockModerationRepository::new()),
            user_repo: Arc::new(self.users),
            dm_policy_repo: Arc::new(self.dm_policy),
            default_workspace_repo: Arc::new(self.record),
        }
    }
}

fn user(id: UserId, username: &str) -> User {
    User {
        id,
        username: username.to_string(),
        password_hash: None,
        created_at: Utc::now(),
        updated_at: Utc::now(),
        last_login_at: None,
        display_name: None,
        avatar_url: None,
        status_text: None,
        bio: None,
        email: None,
        timezone: None,
        principal_id: None,
        deletion_scheduled_for: None,
    }
}

fn channel(id: ConversationId, server_id: ServerId, title: &str, is_private: bool) -> Conversation {
    Conversation {
        id,
        conversation_type: ConversationType::Channel,
        title: Some(title.to_string()),
        topic: None,
        created_at: Utc::now(),
        last_activity_at: Utc::now(),
        server_id,
        is_private,
        archived_at: None,
        created_by: None,
        slow_mode_seconds: 0,
        encrypted_at: None,
    }
}

#[tokio::test]
async fn only_the_claim_invite_under_a_definition_claims() {
    let owner = UserId::new();
    let claim_invite = CheckedToken::InstanceInvite(instance_invite(InstanceInviteId::new()));

    let untouched = Repos::new().signup();
    for (setting, token) in [
        (DefaultWorkspace::None, Some(&claim_invite)),
        (
            DefaultWorkspace::Fixed(ServerId::new()),
            Some(&claim_invite),
        ),
        (defined(PathBuf::from("/nonexistent")), None),
        (
            defined(PathBuf::from("/nonexistent")),
            Some(&CheckedToken::WorkspaceInvite(workspace_invite())),
        ),
    ] {
        assert_eq!(
            claim(&untouched, &setting, token, owner).await.unwrap(),
            None
        );
    }

    // Another instance invite: the record keeps its claim and nothing is created.
    let mut repos = Repos::new();
    repos
        .record
        .expect_take_claim()
        .times(1)
        .returning(|_| Box::pin(async { Ok(false) }));
    repos.servers.expect_create_server().never();
    let signup = repos.signup();
    assert_eq!(
        claim(
            &signup,
            &defined(PathBuf::from("/nonexistent")),
            Some(&claim_invite),
            owner
        )
        .await
        .unwrap(),
        None
    );
}

/// The claim creates the workspace with the owner, its channels with topics and privacy and
/// its DM policy, records it, adds the other local account and grants the operator role.
#[tokio::test]
async fn the_claim_builds_the_workspace_from_the_definition() {
    let owner = UserId::new();
    let other = UserId::new();
    let invite_id = InstanceInviteId::new();
    let workspace = ServerId::new();
    let mut repos = Repos::new();
    repos
        .record
        .expect_take_claim()
        .withf(move |invite| *invite == invite_id)
        .times(1)
        .returning(|_| Box::pin(async { Ok(true) }));
    repos
        .servers
        .expect_create_server()
        .withf(|name| name == "RIM")
        .times(1)
        .returning(move |name| {
            Ok(Server {
                id: workspace,
                name: name.to_string(),
                created_at: Utc::now(),
                updated_at: Utc::now(),
            })
        });
    let roles = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&roles);
    repos
        .members
        .expect_add_member()
        .times(2)
        .returning(move |server_id, user_id, member_role| {
            seen.lock().unwrap().push((user_id, member_role));
            Ok(ServerMember {
                id: ServerMemberId::new(),
                server_id,
                user_id,
                member_role,
                joined_at: Utc::now(),
            })
        });
    repos
        .conversations
        .expect_list_public_channel_ids()
        .returning(|_| Box::pin(async { Ok(vec![]) }));
    let created = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&created);
    repos
        .conversations
        .expect_create_conversation_with_members()
        .times(2)
        .returning(move |new, members, pending| {
            assert_eq!(new.server_id, workspace);
            assert_eq!(new.created_by, Some(owner));
            assert!(!new.encrypted);
            assert_eq!(members, [owner]);
            assert!(pending.is_empty());
            let title = new.title.clone().expect("a title");
            seen.lock().unwrap().push((title.clone(), new.is_private));
            let conversation = channel(ConversationId::new(), workspace, &title, new.is_private);
            Box::pin(async move { Ok(conversation) })
        });
    repos
        .conversations
        .expect_update_topic()
        .withf(|_, topic| topic.as_deref() == Some("Anything about RIM"))
        .times(1)
        .returning(move |id, topic| {
            let mut conversation = channel(id, workspace, "general", false);
            conversation.topic = topic;
            Box::pin(async move { Ok(conversation) })
        });
    repos
        .dm_policy
        .expect_set_workspace_policy()
        .withf(move |server_id, floor, members| {
            *server_id == workspace
                && *floor == OutsideFloor::Requests
                && *members == MemberDm::Direct
        })
        .times(1)
        .returning(|_, _, _| Box::pin(async { Ok(None) }));
    repos
        .record
        .expect_record_claim()
        .withf(move |w, o| *w == workspace && *o == owner)
        .times(1)
        .returning(|_, _| Box::pin(async { Ok(()) }));
    repos
        .record
        .expect_other_local_accounts()
        .withf(move |except| *except == owner)
        .returning(move |_| Box::pin(async move { Ok(vec![other]) }));
    repos
        .users
        .expect_get_by_ids()
        .returning(move |_| Ok(vec![user(other, "earlier")]));
    repos
        .users
        .expect_set_instance_operator()
        .withf(move |id, operator| *id == owner && *operator)
        .times(1)
        .returning(|_, _| Ok(true));

    let token = CheckedToken::InstanceInvite(instance_invite(invite_id));
    let file = PathBuf::from("/data/owner-claim.txt");
    let claimed = claim(&repos.signup(), &defined(file.clone()), Some(&token), owner)
        .await
        .unwrap();
    assert_eq!(
        claimed,
        Some(Claimed {
            workspace_id: workspace,
            members: vec![(other, "earlier".to_string())],
            operator_granted: true,
            claim_link_file: file,
        })
    );
    assert_eq!(
        *roles.lock().unwrap(),
        [
            (owner, ServerMemberRole::Owner),
            (other, ServerMemberRole::Member)
        ]
    );
    assert_eq!(
        *created.lock().unwrap(),
        [("general".to_string(), false), ("staff".to_string(), true)]
    );
}

#[derive(Default)]
struct Recording(Mutex<Vec<WorkspaceEvent>>);

impl Broadcaster for Recording {
    fn broadcast(&self, event: WorkspaceEvent) {
        self.0.lock().unwrap().push(event);
    }
}

/// A fresh directory for claim link files.
fn temp_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rim-claim-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

#[test]
fn announcing_the_claim_deletes_the_link_and_tells_of_the_added_members() {
    let dir = temp_dir();
    let file = dir.join("owner-claim.txt");
    std::fs::write(&file, "https://rim.example.org/join/rim_iinv_x\n").expect("link file");
    let seen = Arc::new(Recording::default());
    let observers = Observers::new(Arc::clone(&seen) as Arc<dyn Broadcaster>);
    let workspace = ServerId::new();
    let other = UserId::new();

    announce_claim(None, UserId::new(), &observers);
    assert!(file.exists());
    announce_claim(
        Some(Claimed {
            workspace_id: workspace,
            members: vec![(other, "earlier".to_string())],
            operator_granted: true,
            claim_link_file: file.clone(),
        }),
        UserId::new(),
        &observers,
    );
    assert!(!file.exists());
    assert!(matches!(
        seen.0.lock().unwrap().as_slice(),
        [WorkspaceEvent::MemberJoined { workspace_id, user_id, username }]
            if *workspace_id == workspace && *user_id == other && username == "earlier"
    ));
    std::fs::remove_dir_all(dir).expect("clean up");
}

/// Start repositories that fail any call a test does not prime.
struct Start {
    record: MockDefaultWorkspaceRepository,
    invites: MockInstanceInviteRepository,
    servers: MockServerRepository,
}

impl Start {
    fn new(record: DefaultWorkspaceRecord) -> Self {
        let mut repo = MockDefaultWorkspaceRepository::new();
        repo.expect_record().returning(move || {
            let record = record.clone();
            Box::pin(async move { Ok(record) })
        });
        Self {
            record: repo,
            invites: MockInstanceInviteRepository::new(),
            servers: MockServerRepository::new(),
        }
    }

    fn repos(self) -> StartRepos {
        StartRepos {
            default_workspace_repo: Arc::new(self.record),
            instance_invite_repo: Arc::new(self.invites),
            server_repo: Arc::new(self.servers),
            conversation_repo: Arc::new(MockConversationRepository::new()),
            dm_policy_repo: Arc::new(MockDmPolicyRepository::new()),
        }
    }
}

fn context(signups_take_tokens: bool) -> StartContext<'static> {
    StartContext {
        direct_space_id: ServerId::new(),
        signups_take_tokens,
        link_base: "https://rim.example.org",
        now: Utc::now(),
    }
}

/// Unclaimed: the previous claim invite is revoked, a single-use one for the longest lifetime
/// is issued and recorded, and its link replaces the claim link file, readable by its owner
/// only.
#[tokio::test]
async fn an_unclaimed_start_replaces_the_claim_link() {
    use std::os::unix::fs::PermissionsExt;

    let dir = temp_dir();
    let file = dir.join("owner-claim.txt");
    std::fs::write(&file, "the previous link\n").expect("previous link");
    let previous = InstanceInviteId::new();
    let ctx = context(true);
    let mut start_repos = Start::new(DefaultWorkspaceRecord {
        claim_invite_id: Some(previous),
        ..Default::default()
    });
    start_repos
        .invites
        .expect_revoke()
        .withf(move |id| *id == previous)
        .times(1)
        .returning(|_| Ok(()));
    let issued = Arc::new(Mutex::new(None));
    let seen = Arc::clone(&issued);
    start_repos
        .invites
        .expect_create()
        .times(1)
        .returning(move |new| {
            assert_eq!(new.max_uses, 1);
            assert_eq!(new.created_by_user_id, None);
            assert_eq!(new.email, None);
            let mut invite = instance_invite(InstanceInviteId::new());
            invite.token_hash = new.token_hash;
            invite.expires_at = new.expires_at;
            *seen.lock().unwrap() = Some(invite.clone());
            Ok(invite)
        });
    let recorded = Arc::clone(&issued);
    start_repos
        .record
        .expect_set_claim_invite()
        .times(1)
        .returning(move |id| {
            let issued = recorded.lock().unwrap().clone().expect("issued first");
            assert_eq!(id, issued.id);
            Box::pin(async { Ok(()) })
        });

    let started = start(&start_repos.repos(), &defined(file.clone()), ctx)
        .await
        .expect("start");
    let issued = issued.lock().unwrap().clone().expect("an invite");
    assert_eq!(
        started,
        Started::ClaimWaiting {
            file: file.clone(),
            expires_at: issued.expires_at,
            workspace_deleted: false,
        }
    );
    assert_eq!(issued.expires_at, ctx.now + Duration::days(MAX_TTL_DAYS));
    let link = std::fs::read_to_string(&file).expect("the link");
    let token = link
        .trim_end()
        .strip_prefix("https://rim.example.org/join/")
        .expect("a join link");
    assert_eq!(hash_token(token), issued.token_hash);
    let mode = std::fs::metadata(&file).expect("file").permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
    std::fs::remove_dir_all(dir).expect("clean up");
}

#[tokio::test]
async fn a_start_says_when_the_claimed_workspace_was_deleted() {
    let dir = temp_dir();
    let file = dir.join("owner-claim.txt");
    let mut start_repos = Start::new(DefaultWorkspaceRecord {
        claimed_by: Some(UserId::new()),
        claimed_at: Some(Utc::now()),
        ..Default::default()
    });
    start_repos.invites.expect_revoke().never();
    start_repos.invites.expect_create().returning(|new| {
        let mut invite = instance_invite(InstanceInviteId::new());
        invite.expires_at = new.expires_at;
        Ok(invite)
    });
    start_repos
        .record
        .expect_set_claim_invite()
        .returning(|_| Box::pin(async { Ok(()) }));
    let started = start(&start_repos.repos(), &defined(file.clone()), context(true))
        .await
        .expect("start");
    assert!(
        matches!(
            started,
            Started::ClaimWaiting {
                workspace_deleted: true,
                ..
            }
        ),
        "{started:?}"
    );
    std::fs::remove_dir_all(dir).expect("clean up");
}

#[tokio::test]
async fn open_registration_refuses_a_waiting_claim_before_issuing_it() {
    let mut start_repos = Start::new(DefaultWorkspaceRecord::default());
    start_repos.invites.expect_create().never();
    let err = start(
        &start_repos.repos(),
        &defined(PathBuf::from("/nonexistent/owner-claim.txt")),
        context(false),
    )
    .await
    .unwrap_err();
    assert!(err.contains("turn RIM_OPEN_REGISTRATION off"), "{err}");
}

/// Without a definition, a claim invite left from an earlier start is revoked, and a workspace
/// id is checked.
#[tokio::test]
async fn without_a_definition_a_left_claim_is_revoked() {
    let left = InstanceInviteId::new();
    let workspace = ServerId::new();
    let record = DefaultWorkspaceRecord {
        claim_invite_id: Some(left),
        ..Default::default()
    };

    let mut start_repos = Start::new(record.clone());
    start_repos
        .invites
        .expect_revoke()
        .withf(move |id| *id == left)
        .times(1)
        .returning(|_| Ok(()));
    assert_eq!(
        start(&start_repos.repos(), &DefaultWorkspace::None, context(true)).await,
        Ok(Started::None)
    );

    let mut start_repos = Start::new(record);
    start_repos
        .invites
        .expect_revoke()
        .times(1)
        .returning(|_| Ok(()));
    start_repos.servers.expect_get_by_id().returning(move |id| {
        Ok(Some(Server {
            id,
            name: "Lobby".to_string(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }))
    });
    assert_eq!(
        start(
            &start_repos.repos(),
            &DefaultWorkspace::Fixed(workspace),
            context(true)
        )
        .await,
        Ok(Started::Fixed(workspace))
    );
}

#[tokio::test]
async fn the_service_reads_the_claimed_workspace_only_under_a_definition() {
    let workspace = ServerId::new();
    let fixed = ServerId::new();
    let record = || {
        let mut repo = MockDefaultWorkspaceRepository::new();
        repo.expect_record().returning(move || {
            Box::pin(async move {
                Ok(DefaultWorkspaceRecord {
                    workspace_id: Some(workspace),
                    ..Default::default()
                })
            })
        });
        Arc::new(repo)
    };
    let current = |setting: DefaultWorkspace| {
        let service = DefaultDefaultWorkspaceService::new(setting, record());
        async move { service.current().await.unwrap() }
    };
    assert_eq!(current(DefaultWorkspace::None).await, None);
    assert_eq!(current(DefaultWorkspace::Fixed(fixed)).await, Some(fixed));
    assert_eq!(
        current(defined(PathBuf::from("/nonexistent"))).await,
        Some(workspace)
    );
}
