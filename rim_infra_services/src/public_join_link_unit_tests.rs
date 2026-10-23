#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use chrono::{DateTime, Duration, Utc};

    use rim_core::conversation::repository::MockConversationRepository;
    use rim_core::conversation_member::repository::MockConversationMemberRepository;
    use rim_core::moderation::model::ServerBan;
    use rim_core::moderation::repository::MockModerationRepository;
    use rim_core::public_join_link::model::{
        JoinRequest, JoinRequestId, JoinRequestStatus, NewJoinRequest, PublicJoinLink,
        PublicLinkMode,
    };
    use rim_core::public_join_link::repository::{
        JoinRequestCursor, ListJoinRequestsParams, PublicJoinLinkRepository,
    };
    use rim_core::repository::error::{RepoError, RepoErrorKind, RepoResult};
    use rim_core::repository::pagination::{Page, PageDir};
    use rim_core::server::model::{Server, ServerId};
    use rim_core::server::repository::MockServerRepository;
    use rim_core::server_member::model::{
        MemberStanding, ServerMember, ServerMemberId, ServerMemberRole,
    };
    use rim_core::server_member::repository::ServerMemberRepository;
    use rim_core::user::model::UserId;
    use rim_core::user::repository::MockUserRepository;
    use rim_services::audit::dto::{AuditLogEntryDto, AuditLogPage, ListAuditLogQuery};
    use rim_services::audit::service::AuditLogService;
    use rim_services::default_workspace::MockDefaultWorkspaceService;
    use rim_services::events::{Broadcaster, Observers, WorkspaceEvent};
    use rim_services::limits::InstanceLimits;
    use rim_services::public_join_link::dto::{
        JoinByPublicLinkOutcome, PublicJoinLinkPatch, PublicLinkViewer,
    };
    use rim_services::public_join_link::service::{ListJoinRequestsQuery, PublicJoinLinkService};
    use rim_services::{Actor, ServiceError, ServiceResult};

    use crate::clock::Clock;
    use crate::link_key::LinkKey;
    use crate::public_join_link::{
        DefaultPublicJoinLinkService, PublicJoinLinkRepos, cancel_requests_on_ban,
    };

    // ── In-memory storage ───────────────────────────────────────────────

    #[derive(Default)]
    struct State {
        links: HashMap<ServerId, PublicJoinLink>,
        requests: Vec<JoinRequest>,
        joins: Vec<(ServerId, UserId, DateTime<Utc>)>,
        members: Vec<ServerMember>,
        bans: Vec<(ServerId, UserId)>,
    }

    type Shared = Arc<Mutex<State>>;

    fn lock(state: &Shared) -> std::sync::MutexGuard<'_, State> {
        state.lock().expect("state lock")
    }

    struct FakeLinks(Shared);

    #[async_trait]
    impl PublicJoinLinkRepository for FakeLinks {
        async fn link(&self, server_id: ServerId) -> RepoResult<Option<PublicJoinLink>> {
            Ok(lock(&self.0).links.get(&server_id).cloned())
        }

        async fn link_by_token_hash(&self, token_hash: &str) -> RepoResult<Option<PublicJoinLink>> {
            Ok(lock(&self.0)
                .links
                .values()
                .find(|l| l.token_hash == token_hash)
                .cloned())
        }

        async fn save_link(&self, link: PublicJoinLink) -> RepoResult<PublicJoinLink> {
            lock(&self.0).links.insert(link.server_id, link.clone());
            Ok(link)
        }

        async fn count_links(&self) -> RepoResult<u64> {
            Ok(lock(&self.0).links.len() as u64)
        }

        async fn record_join(
            &self,
            server_id: ServerId,
            user_id: UserId,
            at: DateTime<Utc>,
        ) -> RepoResult<()> {
            lock(&self.0).joins.push((server_id, user_id, at));
            Ok(())
        }

        async fn count_joins_since(
            &self,
            server_id: ServerId,
            since: DateTime<Utc>,
        ) -> RepoResult<u64> {
            Ok(lock(&self.0)
                .joins
                .iter()
                .filter(|(s, _, at)| *s == server_id && *at >= since)
                .count() as u64)
        }

        async fn create_request(&self, new: NewJoinRequest) -> RepoResult<JoinRequest> {
            let mut state = lock(&self.0);
            for r in state.requests.iter_mut() {
                if r.server_id == new.server_id
                    && r.user_id == new.user_id
                    && r.status == JoinRequestStatus::Open
                    && r.expires_at <= new.created_at
                {
                    r.status = JoinRequestStatus::Expired;
                }
            }
            if state.requests.iter().any(|r| {
                r.server_id == new.server_id
                    && r.user_id == new.user_id
                    && r.status == JoinRequestStatus::Open
            }) {
                return Err(RepoError::new(RepoErrorKind::Conflict));
            }
            let request = JoinRequest {
                id: JoinRequestId::new(),
                server_id: new.server_id,
                user_id: new.user_id,
                message: new.message,
                status: JoinRequestStatus::Open,
                created_at: new.created_at,
                expires_at: new.expires_at,
                decided_at: None,
                decided_by: None,
            };
            state.requests.push(request.clone());
            Ok(request)
        }

        async fn request(&self, id: JoinRequestId) -> RepoResult<Option<JoinRequest>> {
            Ok(lock(&self.0).requests.iter().find(|r| r.id == id).cloned())
        }

        async fn open_request(
            &self,
            server_id: ServerId,
            user_id: UserId,
            now: DateTime<Utc>,
        ) -> RepoResult<Option<JoinRequest>> {
            Ok(lock(&self.0)
                .requests
                .iter()
                .find(|r| r.server_id == server_id && r.user_id == user_id && r.is_open_at(now))
                .cloned())
        }

        async fn last_declined_at(
            &self,
            server_id: ServerId,
            user_id: UserId,
        ) -> RepoResult<Option<DateTime<Utc>>> {
            Ok(lock(&self.0)
                .requests
                .iter()
                .filter(|r| {
                    r.server_id == server_id
                        && r.user_id == user_id
                        && r.status == JoinRequestStatus::Declined
                })
                .filter_map(|r| r.decided_at)
                .max())
        }

        async fn count_open_for_user(
            &self,
            user_id: UserId,
            now: DateTime<Utc>,
        ) -> RepoResult<u64> {
            Ok(lock(&self.0)
                .requests
                .iter()
                .filter(|r| r.user_id == user_id && r.is_open_at(now))
                .count() as u64)
        }

        async fn count_open_for_server(
            &self,
            server_id: ServerId,
            now: DateTime<Utc>,
        ) -> RepoResult<u64> {
            Ok(lock(&self.0)
                .requests
                .iter()
                .filter(|r| r.server_id == server_id && r.is_open_at(now))
                .count() as u64)
        }

        async fn list_open_requests(
            &self,
            params: ListJoinRequestsParams,
        ) -> RepoResult<Page<JoinRequest, JoinRequestCursor>> {
            let mut items: Vec<JoinRequest> = lock(&self.0)
                .requests
                .iter()
                .filter(|r| r.server_id == params.server_id && r.is_open_at(params.now))
                .cloned()
                .collect();
            items.sort_by(|a, b| {
                (b.created_at, b.id.to_string()).cmp(&(a.created_at, a.id.to_string()))
            });
            Ok(Page::new(items, None, None))
        }

        async fn list_open_for_user(
            &self,
            user_id: UserId,
            now: DateTime<Utc>,
        ) -> RepoResult<Vec<JoinRequest>> {
            Ok(lock(&self.0)
                .requests
                .iter()
                .filter(|r| r.user_id == user_id && r.is_open_at(now))
                .cloned()
                .collect())
        }

        async fn close_request(
            &self,
            id: JoinRequestId,
            status: JoinRequestStatus,
            decided_by: Option<UserId>,
            now: DateTime<Utc>,
        ) -> RepoResult<Option<JoinRequest>> {
            let mut state = lock(&self.0);
            let Some(r) = state
                .requests
                .iter_mut()
                .find(|r| r.id == id && r.is_open_at(now))
            else {
                return Ok(None);
            };
            r.status = status;
            r.decided_by = decided_by;
            r.decided_at = Some(now);
            Ok(Some(r.clone()))
        }

        async fn cancel_open_requests(
            &self,
            server_id: ServerId,
            user_id: UserId,
            now: DateTime<Utc>,
        ) -> RepoResult<Vec<JoinRequest>> {
            let mut state = lock(&self.0);
            let mut cancelled = Vec::new();
            for r in state.requests.iter_mut() {
                if r.server_id == server_id && r.user_id == user_id && r.is_open_at(now) {
                    r.status = JoinRequestStatus::Cancelled;
                    r.decided_at = Some(now);
                    cancelled.push(r.clone());
                }
            }
            Ok(cancelled)
        }
    }

    struct FakeMembers(Shared);

    #[async_trait]
    impl ServerMemberRepository for FakeMembers {
        async fn get_member(
            &self,
            server_id: ServerId,
            user_id: UserId,
        ) -> RepoResult<Option<ServerMember>> {
            Ok(lock(&self.0)
                .members
                .iter()
                .find(|m| m.server_id == server_id && m.user_id == user_id)
                .cloned())
        }

        async fn add_member(
            &self,
            server_id: ServerId,
            user_id: UserId,
            role: ServerMemberRole,
        ) -> RepoResult<ServerMember> {
            let member = ServerMember {
                id: ServerMemberId::new(),
                server_id,
                user_id,
                member_role: role,
                joined_at: Utc::now(),
            };
            lock(&self.0).members.push(member.clone());
            Ok(member)
        }

        async fn change_member_role(
            &self,
            _server_id: ServerId,
            _user_id: UserId,
            _role: ServerMemberRole,
        ) -> RepoResult<()> {
            Ok(())
        }

        async fn remove_member(&self, server_id: ServerId, user_id: UserId) -> RepoResult<()> {
            lock(&self.0)
                .members
                .retain(|m| !(m.server_id == server_id && m.user_id == user_id));
            Ok(())
        }

        async fn get_members_for_server(
            &self,
            server_id: ServerId,
        ) -> RepoResult<Vec<ServerMember>> {
            Ok(lock(&self.0)
                .members
                .iter()
                .filter(|m| m.server_id == server_id)
                .cloned()
                .collect())
        }

        async fn get_members(
            &self,
            server_id: ServerId,
            user_ids: &[UserId],
        ) -> RepoResult<Vec<ServerMember>> {
            Ok(lock(&self.0)
                .members
                .iter()
                .filter(|m| m.server_id == server_id && user_ids.contains(&m.user_id))
                .cloned()
                .collect())
        }

        async fn is_member(&self, server_id: ServerId, user_id: UserId) -> RepoResult<bool> {
            Ok(self.get_member(server_id, user_id).await?.is_some())
        }

        async fn join_direct_space(
            &self,
            _server_id: ServerId,
            _user_id: UserId,
        ) -> RepoResult<bool> {
            Ok(false)
        }

        async fn get_owner_id(&self, server_id: ServerId) -> RepoResult<Option<UserId>> {
            Ok(lock(&self.0)
                .members
                .iter()
                .find(|m| m.server_id == server_id && m.member_role == ServerMemberRole::Owner)
                .map(|m| m.user_id))
        }

        async fn get_member_standing(
            &self,
            server_id: ServerId,
            user_id: UserId,
        ) -> RepoResult<Option<MemberStanding>> {
            Ok(self
                .get_member(server_id, user_id)
                .await?
                .map(|m| MemberStanding {
                    role: m.member_role,
                    timed_out_until: None,
                }))
        }

        async fn count_members(&self, server_id: ServerId) -> RepoResult<u64> {
            Ok(lock(&self.0)
                .members
                .iter()
                .filter(|m| m.server_id == server_id)
                .count() as u64)
        }
    }

    #[derive(Default)]
    struct RecordingAudit(Mutex<Vec<AuditLogEntryDto>>);

    #[async_trait]
    impl AuditLogService for RecordingAudit {
        async fn write_entry(&self, entry: AuditLogEntryDto) -> ServiceResult<()> {
            self.0.lock().expect("lock").push(entry);
            Ok(())
        }

        async fn list_entries(
            &self,
            _caller: Actor,
            _query: ListAuditLogQuery,
        ) -> ServiceResult<AuditLogPage> {
            Err(ServiceError::NotFound)
        }
    }

    #[derive(Default)]
    struct RecordingBroadcaster(Mutex<Vec<WorkspaceEvent>>);

    impl Broadcaster for RecordingBroadcaster {
        fn broadcast(&self, event: WorkspaceEvent) {
            self.0.lock().expect("lock").push(event);
        }
    }

    // ── Fixture ─────────────────────────────────────────────────────────

    /// One workspace with an Owner, a Moderator and a Member, the server's default workspace,
    /// the direct space, and a clock the test moves.
    struct World {
        state: Shared,
        events: Arc<RecordingBroadcaster>,
        audit: Arc<RecordingAudit>,
        now: Arc<Mutex<DateTime<Utc>>>,
        workspace: ServerId,
        default_workspace: ServerId,
        direct_space: ServerId,
        owner: Actor,
        moderator: Actor,
        member: Actor,
        limits: InstanceLimits,
        link_key: Option<LinkKey>,
    }

    const IP: &str = "198.51.100.7";

    impl World {
        fn new() -> Self {
            let world = Self {
                state: Arc::default(),
                events: Arc::default(),
                audit: Arc::default(),
                now: Arc::new(Mutex::new(Utc::now())),
                workspace: ServerId::new(),
                default_workspace: ServerId::new(),
                direct_space: ServerId::new(),
                owner: Actor::new(UserId::new()),
                moderator: Actor::new(UserId::new()),
                member: Actor::new(UserId::new()),
                limits: InstanceLimits::default(),
                link_key: Some(LinkKey::new([3; 32])),
            };
            for (actor, role) in [
                (world.owner, ServerMemberRole::Owner),
                (world.moderator, ServerMemberRole::Moderator),
                (world.member, ServerMemberRole::Member),
            ] {
                world.add(world.workspace, actor.user_id, role);
            }
            world.add(
                world.default_workspace,
                world.owner.user_id,
                ServerMemberRole::Owner,
            );
            world
        }

        fn add(&self, server_id: ServerId, user_id: UserId, role: ServerMemberRole) {
            lock(&self.state).members.push(ServerMember {
                id: ServerMemberId::new(),
                server_id,
                user_id,
                member_role: role,
                joined_at: Utc::now(),
            });
        }

        fn ban(&self, user_id: UserId) {
            lock(&self.state).bans.push((self.workspace, user_id));
        }

        fn advance(&self, by: Duration) {
            *self.now.lock().expect("clock") += by;
        }

        fn service(&self) -> DefaultPublicJoinLinkService {
            let names = HashMap::from([
                (self.workspace, "Lobby".to_string()),
                (self.default_workspace, "Town square".to_string()),
            ]);
            let mut server_repo = MockServerRepository::new();
            server_repo.expect_get_by_id().returning(move |id| {
                Ok(names.get(&id).map(|name| Server {
                    id,
                    name: name.clone(),
                    created_at: Utc::now(),
                    updated_at: Utc::now(),
                }))
            });
            let bans = Arc::clone(&self.state);
            let mut moderation_repo = MockModerationRepository::new();
            moderation_repo
                .expect_get_active_ban()
                .returning(move |server_id, user_id, now| {
                    let banned = lock(&bans).bans.contains(&(server_id, user_id));
                    Box::pin(async move {
                        Ok(banned.then_some(ServerBan {
                            server_id,
                            user_id,
                            reason: None,
                            banned_by: None,
                            created_at: now,
                            expires_at: None,
                        }))
                    })
                });
            let mut conversation_repo = MockConversationRepository::new();
            conversation_repo
                .expect_list_public_channel_ids()
                .returning(|_| Box::pin(async { Ok(Vec::new()) }));
            let mut user_repo = MockUserRepository::new();
            user_repo.expect_get_by_id().returning(|id| {
                Ok(Some(crate::probation_fixtures::account(
                    id,
                    Utc::now(),
                    None,
                )))
            });
            let default_workspace = self.default_workspace;
            let mut default_workspace_service = MockDefaultWorkspaceService::new();
            default_workspace_service
                .expect_current()
                .returning(move || Box::pin(async move { Ok(Some(default_workspace)) }));
            let now = Arc::clone(&self.now);
            DefaultPublicJoinLinkService::new(
                PublicJoinLinkRepos {
                    link_repo: Arc::new(FakeLinks(Arc::clone(&self.state))),
                    server_repo: Arc::new(server_repo),
                    server_member_repo: Arc::new(FakeMembers(Arc::clone(&self.state))),
                    conversation_repo: Arc::new(conversation_repo),
                    conversation_member_repo: Arc::new(MockConversationMemberRepository::new()),
                    moderation_repo: Arc::new(moderation_repo),
                    user_repo: Arc::new(user_repo),
                },
                Observers::new(Arc::clone(&self.events) as Arc<dyn Broadcaster>),
                self.link_key.clone(),
                "https://rim.example.org/".to_string(),
            )
            .with_audit_log_service(Arc::clone(&self.audit) as Arc<dyn AuditLogService>)
            .with_direct_space(Some(self.direct_space))
            .with_default_workspace(Arc::new(default_workspace_service))
            .with_limits(self.limits)
            .with_clock(Clock::from_fn(move || *now.lock().expect("clock")))
        }

        /// Turn the workspace's link on in `mode`; returns its token.
        async fn link_on(
            &self,
            svc: &DefaultPublicJoinLinkService,
            mode: PublicLinkMode,
        ) -> String {
            svc.update_public_join_link(
                self.owner,
                self.workspace,
                PublicJoinLinkPatch {
                    enabled: Some(true),
                    mode: Some(mode),
                    ..Default::default()
                },
            )
            .await
            .expect("link on")
            .token
            .expect("token")
        }

        fn events(&self) -> Vec<WorkspaceEvent> {
            self.events.0.lock().expect("lock").clone()
        }

        fn audit_ids(&self) -> Vec<String> {
            self.audit
                .0
                .lock()
                .expect("lock")
                .iter()
                .map(|e| e.command_id.clone())
                .collect()
        }

        fn role_of(&self, user_id: UserId) -> Option<ServerMemberRole> {
            lock(&self.state)
                .members
                .iter()
                .find(|m| m.server_id == self.workspace && m.user_id == user_id)
                .map(|m| m.member_role)
        }

        fn audit_entries(&self) -> Vec<AuditLogEntryDto> {
            self.audit.0.lock().expect("lock").clone()
        }
    }

    fn outsider() -> Actor {
        Actor::new(UserId::new())
    }

    // ── The Owner's settings ────────────────────────────────────────────

    #[tokio::test]
    async fn only_the_owner_reads_or_changes_the_link() {
        let world = World::new();
        let svc = world.service();
        for actor in [world.moderator, world.member, outsider()] {
            assert!(matches!(
                svc.public_join_link(actor, world.workspace).await,
                Err(ServiceError::PermissionDenied)
            ));
            assert!(matches!(
                svc.update_public_join_link(
                    actor,
                    world.workspace,
                    PublicJoinLinkPatch {
                        enabled: Some(true),
                        ..Default::default()
                    }
                )
                .await,
                Err(ServiceError::PermissionDenied)
            ));
            assert!(matches!(
                svc.reissue_public_join_link(actor, world.workspace).await,
                Err(ServiceError::PermissionDenied)
            ));
        }
        assert!(lock(&world.state).links.is_empty());
    }

    #[tokio::test]
    async fn the_direct_space_and_the_default_workspace_have_no_link() {
        let world = World::new();
        let svc = world.service();
        for server_id in [world.direct_space, world.default_workspace] {
            assert!(matches!(
                svc.public_join_link(world.owner, server_id).await,
                Err(ServiceError::Validation(_))
            ));
            assert!(matches!(
                svc.reissue_public_join_link(world.owner, server_id).await,
                Err(ServiceError::Validation(_))
            ));
        }
    }

    #[tokio::test]
    async fn a_link_is_off_until_turned_on_and_shows_the_same_token_again() {
        let world = World::new();
        let svc = world.service();
        let before = svc
            .public_join_link(world.owner, world.workspace)
            .await
            .unwrap();
        assert!(!before.enabled);
        assert_eq!(before.token, None);
        assert_eq!(before.role, ServerMemberRole::Member);
        assert_eq!(before.mode, PublicLinkMode::Direct);
        assert!(before.allow_remote);

        let token = world.link_on(&svc, PublicLinkMode::Direct).await;
        assert_eq!(token.len(), 22);
        let again = svc
            .public_join_link(world.owner, world.workspace)
            .await
            .unwrap();
        assert!(again.enabled);
        assert_eq!(again.token.as_deref(), Some(token.as_str()));
        assert_eq!(
            again.url.as_deref(),
            Some(format!("https://rim.example.org/join/{token}").as_str())
        );
        assert_eq!(again.issued_by, Some(world.owner.user_id));
        assert!(again.enabled_at.is_some());

        let stored = lock(&world.state).links[&world.workspace].clone();
        assert_eq!(
            stored.token_hash,
            rim_auth::session_token::hash_token(&token)
        );
        assert!(
            !stored
                .sealed_token
                .windows(token.len())
                .any(|w| w == token.as_bytes()),
            "the stored copy is sealed"
        );
        assert_eq!(world.audit_ids(), vec!["public_link.enabled"]);
    }

    #[tokio::test]
    async fn settings_changes_and_turning_off_are_audited_without_the_token() {
        let world = World::new();
        let svc = world.service();
        let token = world.link_on(&svc, PublicLinkMode::Direct).await;
        let changed = svc
            .update_public_join_link(
                world.owner,
                world.workspace,
                PublicJoinLinkPatch {
                    mode: Some(PublicLinkMode::Requests),
                    role: Some(ServerMemberRole::Guest),
                    allow_remote: Some(false),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(changed.mode, PublicLinkMode::Requests);
        assert_eq!(changed.role, ServerMemberRole::Guest);
        assert!(!changed.allow_remote);
        assert_eq!(changed.token.as_deref(), Some(token.as_str()));
        svc.update_public_join_link(
            world.owner,
            world.workspace,
            PublicJoinLinkPatch {
                enabled: Some(false),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(
            world.audit_ids(),
            vec![
                "public_link.enabled",
                "public_link.settings_changed",
                "public_link.disabled"
            ]
        );
        for entry in world.audit_entries() {
            assert_eq!(entry.initiator, "public_link");
            assert!(!entry.args_redacted.to_string().contains(&token));
        }
    }

    #[tokio::test]
    async fn a_link_grants_member_or_guest_only() {
        let world = World::new();
        let svc = world.service();
        for role in [ServerMemberRole::Owner, ServerMemberRole::Moderator] {
            assert!(matches!(
                svc.update_public_join_link(
                    world.owner,
                    world.workspace,
                    PublicJoinLinkPatch {
                        role: Some(role),
                        ..Default::default()
                    }
                )
                .await,
                Err(ServiceError::Validation(_))
            ));
        }
    }

    #[tokio::test]
    async fn a_new_token_ends_the_old_one_at_once() {
        let world = World::new();
        let svc = world.service();
        let old = world.link_on(&svc, PublicLinkMode::Direct).await;
        let new = svc
            .reissue_public_join_link(world.owner, world.workspace)
            .await
            .unwrap()
            .token
            .unwrap();
        assert_ne!(old, new);
        assert!(matches!(
            svc.preview(None, old, IP.to_string()).await,
            Err(ServiceError::NotFound)
        ));
        assert!(svc.preview(None, new, IP.to_string()).await.is_ok());
        assert_eq!(
            world.audit_ids(),
            vec!["public_link.enabled", "public_link.reissued"]
        );
    }

    // ── Looking a link up ───────────────────────────────────────────────

    #[tokio::test]
    async fn every_invalid_link_gets_the_same_answer() {
        let world = World::new();
        let svc = world.service();
        let replaced = world.link_on(&svc, PublicLinkMode::Direct).await;
        let off = svc
            .reissue_public_join_link(world.owner, world.workspace)
            .await
            .unwrap()
            .token
            .unwrap();
        svc.update_public_join_link(
            world.owner,
            world.workspace,
            PublicJoinLinkPatch {
                enabled: Some(false),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        for token in [
            "AAAAAAAAAAAAAAAAAAAAAA".to_string(),
            replaced,
            off,
            String::new(),
            "rim_inv_x".to_string(),
        ] {
            let preview = svc
                .preview(Some(world.member), token.clone(), IP.to_string())
                .await;
            assert!(matches!(preview, Err(ServiceError::NotFound)), "{token}");
            let join = svc
                .join(outsider(), token.clone(), None, IP.to_string())
                .await;
            assert!(matches!(join, Err(ServiceError::NotFound)), "{token}");
        }
    }

    #[tokio::test]
    async fn the_preview_names_the_workspace_and_where_the_caller_stands() {
        let world = World::new();
        let svc = world.service();
        let token = world.link_on(&svc, PublicLinkMode::Requests).await;
        let banned = outsider();
        world.ban(banned.user_id);
        let asking = outsider();
        svc.join(asking, token.clone(), None, IP.to_string())
            .await
            .unwrap();
        for (actor, viewer) in [
            (None, PublicLinkViewer::SignedOut),
            (Some(outsider()), PublicLinkViewer::NotMember),
            (Some(world.member), PublicLinkViewer::Member),
            (Some(banned), PublicLinkViewer::Banned),
            (Some(asking), PublicLinkViewer::RequestOpen),
        ] {
            let preview = svc
                .preview(actor, token.clone(), IP.to_string())
                .await
                .unwrap();
            assert_eq!(preview.name, "Lobby");
            assert_eq!(preview.server_id, world.workspace);
            assert_eq!(preview.mode, PublicLinkMode::Requests);
            assert_eq!(preview.viewer, viewer);
            assert_eq!(
                preview.join_request_id.is_some(),
                viewer == PublicLinkViewer::RequestOpen
            );
        }
    }

    #[tokio::test]
    async fn twenty_misses_cut_the_address_off_for_an_hour_and_successes_do_not_count() {
        let world = World::new();
        let svc = world.service();
        let token = world.link_on(&svc, PublicLinkMode::Direct).await;
        for _ in 0..30 {
            svc.preview(None, token.clone(), IP.to_string())
                .await
                .unwrap();
        }
        for i in 0..20 {
            let miss = svc
                .preview(None, format!("wrong-{i}"), IP.to_string())
                .await;
            assert!(matches!(miss, Err(ServiceError::NotFound)), "miss {i}");
        }
        assert!(matches!(
            svc.preview(None, token.clone(), IP.to_string()).await,
            Err(ServiceError::RateLimited { retry_after_seconds }) if retry_after_seconds == 3600
        ));
        assert!(matches!(
            svc.join(outsider(), token.clone(), None, IP.to_string())
                .await,
            Err(ServiceError::RateLimited { .. })
        ));
        assert!(
            svc.preview(None, token.clone(), "203.0.113.9".to_string())
                .await
                .is_ok(),
            "another address is not affected"
        );
        world.advance(Duration::minutes(59));
        assert!(matches!(
            svc.preview(None, token.clone(), IP.to_string()).await,
            Err(ServiceError::RateLimited { .. })
        ));
        world.advance(Duration::minutes(1));
        assert!(svc.preview(None, token, IP.to_string()).await.is_ok());
    }

    #[tokio::test]
    async fn misses_older_than_ten_minutes_are_forgotten() {
        let world = World::new();
        let svc = world.service();
        let token = world.link_on(&svc, PublicLinkMode::Direct).await;
        for _ in 0..19 {
            let _ = svc.preview(None, "wrong".to_string(), IP.to_string()).await;
        }
        world.advance(Duration::minutes(10));
        for _ in 0..19 {
            let _ = svc
                .join(outsider(), "wrong".to_string(), None, IP.to_string())
                .await;
        }
        assert!(svc.preview(None, token, IP.to_string()).await.is_ok());
    }

    // ── Joining directly ────────────────────────────────────────────────

    #[tokio::test]
    async fn a_direct_join_adds_the_member_with_the_links_role_and_announces_it() {
        let world = World::new();
        let svc = world.service();
        let token = world.link_on(&svc, PublicLinkMode::Direct).await;
        let alice = outsider();
        let joined = svc
            .join(
                alice,
                token.clone(),
                Some("ignored".to_string()),
                IP.to_string(),
            )
            .await
            .unwrap();
        assert_eq!(joined.outcome, JoinByPublicLinkOutcome::Joined);
        assert_eq!(joined.server_id, world.workspace);
        assert_eq!(world.role_of(alice.user_id), Some(ServerMemberRole::Member));
        assert!(world.events().iter().any(|e| matches!(
            e,
            WorkspaceEvent::MemberJoined { workspace_id, user_id, source: Some(source), .. }
                if *workspace_id == world.workspace && *user_id == alice.user_id
                    && source == "public_link"
        )));
        let joined_entry = world
            .audit_entries()
            .into_iter()
            .find(|e| e.command_id == "workspace.member_joined")
            .expect("join audited");
        assert_eq!(joined_entry.args_redacted["source"], "public_link");
        assert_eq!(
            joined_entry.args_redacted["user_id"],
            alice.user_id.to_string()
        );

        svc.update_public_join_link(
            world.owner,
            world.workspace,
            PublicJoinLinkPatch {
                role: Some(ServerMemberRole::Guest),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let bob = outsider();
        svc.join(bob, token, None, IP.to_string()).await.unwrap();
        assert_eq!(world.role_of(bob.user_id), Some(ServerMemberRole::Guest));

        let stats = svc
            .public_join_link(world.owner, world.workspace)
            .await
            .unwrap();
        assert_eq!((stats.joins_last_day, stats.joins_last_week), (2, 2));
        world.advance(Duration::hours(25));
        let stats = svc
            .public_join_link(world.owner, world.workspace)
            .await
            .unwrap();
        assert_eq!((stats.joins_last_day, stats.joins_last_week), (0, 2));
    }

    #[tokio::test]
    async fn a_join_is_refused_for_a_banned_person_a_member_and_a_full_workspace() {
        let mut world = World::new();
        world.limits.members_per_workspace = Some(4);
        let svc = world.service();
        let token = world.link_on(&svc, PublicLinkMode::Direct).await;

        let banned = outsider();
        world.ban(banned.user_id);
        assert!(matches!(
            svc.join(banned, token.clone(), None, IP.to_string()).await,
            Err(ServiceError::Forbidden(_))
        ));
        assert!(matches!(
            svc.join(world.member, token.clone(), None, IP.to_string())
                .await,
            Err(ServiceError::Conflict(_))
        ));
        svc.join(outsider(), token.clone(), None, IP.to_string())
            .await
            .unwrap();
        let late = outsider();
        assert!(matches!(
            svc.join(late, token.clone(), None, IP.to_string()).await,
            Err(ServiceError::Validation(m)) if m == "this instance allows 4 members per workspace"
        ));
        assert_eq!(world.role_of(late.user_id), None);
        assert_eq!(world.role_of(banned.user_id), None);
        // Refusals on a valid link are no misses.
        for _ in 0..25 {
            let _ = svc
                .join(world.member, token.clone(), None, IP.to_string())
                .await;
        }
        assert!(svc.preview(None, token, IP.to_string()).await.is_ok());
    }

    // ── Join requests ───────────────────────────────────────────────────

    #[tokio::test]
    async fn asking_to_join_files_one_request_and_tells_the_moderators() {
        let world = World::new();
        let svc = world.service();
        let token = world.link_on(&svc, PublicLinkMode::Requests).await;
        let alice = outsider();
        let filed = svc
            .join(
                alice,
                token.clone(),
                Some("  hi there  ".to_string()),
                IP.to_string(),
            )
            .await
            .unwrap();
        assert_eq!(filed.outcome, JoinByPublicLinkOutcome::Requested);
        let request = filed.join_request.unwrap();
        assert_eq!(request.message.as_deref(), Some("hi there"));
        assert_eq!(request.status, JoinRequestStatus::Open);
        assert_eq!(request.workspace_name, "Lobby");
        assert_eq!(request.expires_at - request.created_at, Duration::days(30));
        assert_eq!(world.role_of(alice.user_id), None);

        let created = world
            .events()
            .into_iter()
            .find_map(|e| match e {
                WorkspaceEvent::JoinRequestCreated {
                    request_id,
                    user_id,
                    member_user_ids,
                    ..
                } => Some((request_id, user_id, member_user_ids)),
                _ => None,
            })
            .expect("announced");
        assert_eq!(created.0, request.id);
        assert_eq!(created.1, alice.user_id);
        let mut audience = created.2;
        audience.sort_by_key(|id| id.to_string());
        let mut expected = vec![world.owner.user_id, world.moderator.user_id, alice.user_id];
        expected.sort_by_key(|id| id.to_string());
        assert_eq!(audience, expected, "moderators and the person, not members");

        assert!(matches!(
            svc.join(alice, token, None, IP.to_string()).await,
            Err(ServiceError::Conflict(_))
        ));
        let mine = svc.my_join_requests(alice).await.unwrap();
        assert_eq!(mine.len(), 1);
    }

    #[tokio::test]
    async fn a_request_message_is_at_most_500_characters() {
        let world = World::new();
        let svc = world.service();
        let token = world.link_on(&svc, PublicLinkMode::Requests).await;
        assert!(matches!(
            svc.join(
                outsider(),
                token.clone(),
                Some("é".repeat(501)),
                IP.to_string()
            )
            .await,
            Err(ServiceError::Validation(_))
        ));
        assert!(
            svc.join(outsider(), token, Some("é".repeat(500)), IP.to_string())
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn a_person_holds_at_most_twenty_open_requests() {
        let world = World::new();
        let svc = world.service();
        let token = world.link_on(&svc, PublicLinkMode::Requests).await;
        let alice = outsider();
        let now = *world.now.lock().unwrap();
        for _ in 0..20 {
            FakeLinks(Arc::clone(&world.state))
                .create_request(NewJoinRequest {
                    server_id: ServerId::new(),
                    user_id: alice.user_id,
                    message: None,
                    created_at: now,
                    expires_at: now + Duration::days(30),
                })
                .await
                .unwrap();
        }
        assert!(matches!(
            svc.join(alice, token, None, IP.to_string()).await,
            Err(ServiceError::Validation(m)) if m.contains("at most 20 open join requests")
        ));
    }

    #[tokio::test]
    async fn banned_people_and_members_cannot_ask_and_a_full_workspace_refuses_requests() {
        let mut world = World::new();
        world.limits.members_per_workspace = Some(3);
        let svc = world.service();
        let token = world.link_on(&svc, PublicLinkMode::Requests).await;
        let banned = outsider();
        world.ban(banned.user_id);
        assert!(matches!(
            svc.join(banned, token.clone(), None, IP.to_string()).await,
            Err(ServiceError::Forbidden(_))
        ));
        assert!(matches!(
            svc.join(world.member, token.clone(), None, IP.to_string())
                .await,
            Err(ServiceError::Conflict(_))
        ));
        assert!(matches!(
            svc.join(outsider(), token, None, IP.to_string()).await,
            Err(ServiceError::Validation(_))
        ));
        assert!(lock(&world.state).requests.is_empty());
    }

    #[tokio::test]
    async fn accepting_a_request_joins_the_person_with_the_links_role() {
        let world = World::new();
        let svc = world.service();
        let token = world.link_on(&svc, PublicLinkMode::Requests).await;
        svc.update_public_join_link(
            world.owner,
            world.workspace,
            PublicJoinLinkPatch {
                role: Some(ServerMemberRole::Guest),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let alice = outsider();
        let request = svc
            .join(alice, token, None, IP.to_string())
            .await
            .unwrap()
            .join_request
            .unwrap();
        assert!(matches!(
            svc.accept_join_request(world.member, world.workspace, request.id)
                .await,
            Err(ServiceError::PermissionDenied)
        ));
        assert!(matches!(
            svc.accept_join_request(world.moderator, world.default_workspace, request.id)
                .await,
            Err(ServiceError::PermissionDenied | ServiceError::NotFound)
        ));
        let accepted = svc
            .accept_join_request(world.moderator, world.workspace, request.id)
            .await
            .unwrap();
        assert_eq!(accepted.status, JoinRequestStatus::Accepted);
        assert_eq!(accepted.decided_by, Some(world.moderator.user_id));
        assert_eq!(world.role_of(alice.user_id), Some(ServerMemberRole::Guest));
        assert!(world.events().iter().any(|e| matches!(
            e,
            WorkspaceEvent::JoinRequestDecided { user_id, outcome, member_user_ids, .. }
                if *user_id == alice.user_id && outcome == "accepted"
                    && member_user_ids.contains(&alice.user_id)
        )));
        assert!(world.events().iter().any(|e| matches!(
            e,
            WorkspaceEvent::MemberJoined { user_id, source: Some(_), .. } if *user_id == alice.user_id
        )));
        let ids = world.audit_ids();
        assert!(ids.contains(&"join_request.accepted".to_string()));
        assert!(ids.contains(&"workspace.member_joined".to_string()));
        assert!(matches!(
            svc.accept_join_request(world.owner, world.workspace, request.id)
                .await,
            Err(ServiceError::Conflict(_))
        ));
        let stats = svc
            .public_join_link(world.owner, world.workspace)
            .await
            .unwrap();
        assert_eq!(stats.joins_last_day, 1);
        assert_eq!(stats.open_requests, 0);
    }

    #[tokio::test]
    async fn accepting_runs_the_join_checks() {
        let mut world = World::new();
        world.limits.members_per_workspace = Some(4);
        let svc = world.service();
        let token = world.link_on(&svc, PublicLinkMode::Requests).await;
        let (alice, bob) = (outsider(), outsider());
        let first = svc
            .join(alice, token.clone(), None, IP.to_string())
            .await
            .unwrap()
            .join_request
            .unwrap();
        let second = svc
            .join(bob, token, None, IP.to_string())
            .await
            .unwrap()
            .join_request
            .unwrap();
        svc.accept_join_request(world.owner, world.workspace, first.id)
            .await
            .unwrap();
        assert!(matches!(
            svc.accept_join_request(world.owner, world.workspace, second.id)
                .await,
            Err(ServiceError::Validation(_))
        ));
        assert_eq!(world.role_of(bob.user_id), None);
    }

    #[tokio::test]
    async fn a_declined_person_may_ask_again_after_a_day() {
        let world = World::new();
        let svc = world.service();
        let token = world.link_on(&svc, PublicLinkMode::Requests).await;
        let alice = outsider();
        let request = svc
            .join(alice, token.clone(), None, IP.to_string())
            .await
            .unwrap()
            .join_request
            .unwrap();
        assert!(matches!(
            svc.decline_join_request(world.member, world.workspace, request.id)
                .await,
            Err(ServiceError::PermissionDenied)
        ));
        let declined = svc
            .decline_join_request(world.owner, world.workspace, request.id)
            .await
            .unwrap();
        assert_eq!(declined.status, JoinRequestStatus::Declined);
        assert!(
            world
                .audit_ids()
                .contains(&"join_request.declined".to_string())
        );
        assert!(world.events().iter().any(|e| matches!(
            e,
            WorkspaceEvent::JoinRequestDecided { outcome, .. } if outcome == "declined"
        )));
        world.advance(Duration::hours(23));
        assert!(matches!(
            svc.join(alice, token.clone(), None, IP.to_string()).await,
            Err(ServiceError::Validation(m)) if m.ends_with("in 1 hour")
        ));
        world.advance(Duration::hours(1));
        assert!(svc.join(alice, token, None, IP.to_string()).await.is_ok());
    }

    #[tokio::test]
    async fn the_person_withdraws_their_own_request_only() {
        let world = World::new();
        let svc = world.service();
        let token = world.link_on(&svc, PublicLinkMode::Requests).await;
        let alice = outsider();
        let request = svc
            .join(alice, token.clone(), None, IP.to_string())
            .await
            .unwrap()
            .join_request
            .unwrap();
        assert!(matches!(
            svc.withdraw_join_request(world.owner, request.id).await,
            Err(ServiceError::NotFound)
        ));
        let withdrawn = svc.withdraw_join_request(alice, request.id).await.unwrap();
        assert_eq!(withdrawn.status, JoinRequestStatus::Withdrawn);
        assert!(matches!(
            svc.withdraw_join_request(alice, request.id).await,
            Err(ServiceError::Conflict(_))
        ));
        assert!(svc.my_join_requests(alice).await.unwrap().is_empty());
        // A withdrawal is no decline: the person may ask again at once.
        assert!(svc.join(alice, token, None, IP.to_string()).await.is_ok());
    }

    #[tokio::test]
    async fn a_request_expires_after_thirty_days() {
        let world = World::new();
        let svc = world.service();
        let token = world.link_on(&svc, PublicLinkMode::Requests).await;
        let alice = outsider();
        let request = svc
            .join(alice, token.clone(), None, IP.to_string())
            .await
            .unwrap()
            .join_request
            .unwrap();
        world.advance(Duration::days(30));
        let page = svc
            .list_join_requests(ListJoinRequestsQuery {
                actor: world.moderator,
                server_id: world.workspace,
                dir: PageDir::After,
                cursor: None,
                limit: 50,
            })
            .await
            .unwrap();
        assert!(page.items.is_empty());
        assert!(svc.my_join_requests(alice).await.unwrap().is_empty());
        assert!(matches!(
            svc.accept_join_request(world.owner, world.workspace, request.id)
                .await,
            Err(ServiceError::Conflict(_))
        ));
        assert!(svc.join(alice, token, None, IP.to_string()).await.is_ok());
        assert_eq!(
            lock(&world.state)
                .requests
                .iter()
                .find(|r| r.id == request.id)
                .unwrap()
                .status,
            JoinRequestStatus::Expired
        );
    }

    #[tokio::test]
    async fn the_owner_and_moderators_list_open_requests() {
        let world = World::new();
        let svc = world.service();
        let token = world.link_on(&svc, PublicLinkMode::Requests).await;
        svc.join(outsider(), token.clone(), None, IP.to_string())
            .await
            .unwrap();
        svc.join(outsider(), token, None, IP.to_string())
            .await
            .unwrap();
        let query = |actor| ListJoinRequestsQuery {
            actor,
            server_id: world.workspace,
            dir: PageDir::After,
            cursor: None,
            limit: 50,
        };
        for actor in [world.owner, world.moderator] {
            assert_eq!(
                svc.list_join_requests(query(actor))
                    .await
                    .unwrap()
                    .items
                    .len(),
                2
            );
        }
        for actor in [world.member, outsider()] {
            assert!(matches!(
                svc.list_join_requests(query(actor)).await,
                Err(ServiceError::PermissionDenied)
            ));
        }
        let stats = svc
            .public_join_link(world.owner, world.workspace)
            .await
            .unwrap();
        assert_eq!(stats.open_requests, 2);
    }

    #[tokio::test]
    async fn a_ban_cancels_the_persons_open_requests() {
        let world = World::new();
        let svc = world.service();
        let token = world.link_on(&svc, PublicLinkMode::Requests).await;
        let alice = outsider();
        let request = svc
            .join(alice, token, None, IP.to_string())
            .await
            .unwrap()
            .join_request
            .unwrap();
        let observers = Observers::new(Arc::clone(&world.events) as Arc<dyn Broadcaster>);
        cancel_requests_on_ban(
            &FakeLinks(Arc::clone(&world.state)),
            &FakeMembers(Arc::clone(&world.state)),
            &observers,
            world.workspace,
            alice.user_id,
        )
        .await
        .unwrap();
        assert!(svc.my_join_requests(alice).await.unwrap().is_empty());
        assert_eq!(
            lock(&world.state).requests[0].status,
            JoinRequestStatus::Cancelled
        );
        assert!(world.events().iter().any(|e| matches!(
            e,
            WorkspaceEvent::JoinRequestDecided { request_id, outcome, .. }
                if *request_id == request.id && outcome == "cancelled"
        )));
    }

    #[tokio::test]
    async fn without_a_link_key_no_link_is_issued() {
        let mut world = World::new();
        world.link_key = None;
        let svc = world.service();
        assert!(matches!(
            svc.reissue_public_join_link(world.owner, world.workspace)
                .await,
            Err(ServiceError::Unavailable)
        ));
    }
}
