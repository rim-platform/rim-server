//! Self-service account deletion: the request and its cancel, and the purge of accounts whose
//! grace period ended, one transaction per account.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Duration, SubsecRound, Utc};
use sea_orm::DatabaseConnection;
use tracing::{error, info, warn};

use rim_core::account_deletion::model::PurgeStep;
use rim_core::account_deletion::repository::AccountDeletionRepository;
use rim_core::auth::repository::SessionRepository;
use rim_core::user::avatar::avatar_dir;
use rim_core::user::model::UserId;
use rim_infra_db::begin_transaction;
use rim_infra_db::sea_orm_impl::account_deletion::SeaOrmAccountDeletionRepository;
use rim_services::account_deletion::dto::{
    AccountDeletionScheduleDto, AccountPurgeDto, AccountPurgeFailureDto, DEFAULT_GRACE_DAYS,
    PurgeRunDto, RequestAccountDeletionCmd,
};
use rim_services::account_deletion::service::AccountDeletionService;
use rim_services::events::{EventObserver, Observers, WorkspaceEvent};
use rim_services::remote_workspace::service::WorkspaceRelayService;
use rim_services::session_revocation::SessionRevocations;
use rim_services::storage::service::StorageService;
use rim_services::{Actor, ServiceError, ServiceResult};

/// Most accounts one purge run handles; the next run takes the rest.
pub const PURGE_BATCH: u64 = 100;

/// The clock the service reads; tests fix it.
pub type Clock = Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>;

#[derive(Clone)]
pub struct DefaultAccountDeletionService {
    repo: Arc<dyn AccountDeletionRepository>,
    session_repo: Arc<dyn SessionRepository>,
    /// Each account's purge runs in a transaction of its own; `None` in unit tests, which run
    /// the steps on `repo`.
    db: Option<DatabaseConnection>,
    storage: Arc<dyn StorageService>,
    observers: Observers,
    grace: Duration,
    clock: Clock,
    /// This server as its users' relay: cancels a purged account's subscriptions at the hosts
    /// holding it. `None` without server-to-server wiring.
    relay_departure: Option<Arc<dyn WorkspaceRelayService>>,
    /// Told of the sessions a request or a purge ends, so their live connections close.
    session_revocations: Arc<dyn SessionRevocations>,
}

impl DefaultAccountDeletionService {
    pub fn new(
        repo: Arc<dyn AccountDeletionRepository>,
        session_repo: Arc<dyn SessionRepository>,
        db: Option<DatabaseConnection>,
        storage: Arc<dyn StorageService>,
        observers: Observers,
    ) -> Self {
        Self {
            repo,
            session_repo,
            db,
            storage,
            observers,
            grace: Duration::days(i64::from(DEFAULT_GRACE_DAYS)),
            clock: Arc::new(Utc::now),
            relay_departure: None,
            session_revocations: crate::session_revocation::unshared(),
        }
    }

    /// Report the sessions a request or a purge ends to `revocations`.
    pub fn with_session_revocations(mut self, revocations: Arc<dyn SessionRevocations>) -> Self {
        self.session_revocations = revocations;
        self
    }

    /// Time between a request and the purge.
    pub fn with_grace(mut self, grace: Duration) -> Self {
        self.grace = grace;
        self
    }

    pub fn with_clock(mut self, clock: Clock) -> Self {
        self.clock = clock;
        self
    }

    pub fn with_relay_departure(mut self, relay: Arc<dyn WorkspaceRelayService>) -> Self {
        self.relay_departure = Some(relay);
        self
    }

    /// Cancel the account's subscriptions at every host holding it and drop its relay queue,
    /// unless its deletion was cancelled or it is purged already. A host that cannot be reached
    /// is logged and left; the purge goes on.
    async fn leave_relay(&self, user_id: UserId, now: DateTime<Utc>) -> ServiceResult<()> {
        let Some(relay) = &self.relay_departure else {
            return Ok(());
        };
        let due = self.repo.standing(user_id).await?.is_some_and(|s| {
            s.deleted_at.is_none() && s.deletion_scheduled_for.is_some_and(|at| at <= now)
        });
        if due && let Err(e) = relay.leave_relay(Actor::new(user_id)).await {
            warn!(user_id = %user_id, error = %e, "could not cancel a purged account's subscriptions");
        }
        Ok(())
    }

    /// Purge one account in its own transaction. `None` when it is no longer due.
    async fn purge_account(
        &self,
        user_id: UserId,
        now: DateTime<Utc>,
        dry_run: bool,
    ) -> ServiceResult<Option<AccountPurgeDto>> {
        if !dry_run {
            self.leave_relay(user_id, now).await?;
        }
        let scope = match &self.db {
            Some(db) => Some(
                begin_transaction(db)
                    .await
                    .map_err(ServiceError::internal)?,
            ),
            None => None,
        };
        let repo: Arc<dyn AccountDeletionRepository> = match &scope {
            Some(scope) => Arc::new(SeaOrmAccountDeletionRepository::new(scope.handle())),
            None => Arc::clone(&self.repo),
        };
        if !repo.claim_due(user_id, now).await? {
            return Ok(None);
        }
        let mut done = PurgeStep::default();
        done.absorb(repo.purge_workspaces(user_id, now).await?);
        done.absorb(repo.purge_messages(user_id).await?);
        done.absorb(repo.purge_personal_data(user_id).await?);
        done.absorb(repo.purge_credentials(user_id, now).await?);
        done.absorb(repo.purge_relay_state(user_id).await?);
        done.absorb(repo.tombstone(user_id, now).await?);
        drop(repo);
        // Every file in the account's avatar directory goes, whatever its avatar URL named.
        done.storage_keys
            .extend(self.storage.list_files(&avatar_dir(user_id)).await?);

        let files = u64::try_from(done.storage_keys.len()).unwrap_or(u64::MAX);
        if dry_run {
            // Dropping the scope rolls the transaction back.
            drop(scope);
            info!(user_id = %user_id, counts = ?done.counts, files, "account purge dry run");
            return Ok(Some(AccountPurgeDto {
                user_id,
                counts: done.counts,
                files,
            }));
        }
        if let Some(scope) = scope {
            scope.commit().await.map_err(ServiceError::internal)?;
        }
        self.session_revocations.revoke_user(user_id);

        let files = self.delete_files(user_id, &done.storage_keys).await;
        for workspace_id in &done.left_workspaces {
            self.observers.emit(WorkspaceEvent::MemberLeft {
                workspace_id: *workspace_id,
                user_id,
                cause: Some("account_deleted".to_string()),
                until: None,
                reason: None,
            });
        }
        for left in done.left_conversations {
            self.observers.emit(WorkspaceEvent::MlsMembershipChanged {
                workspace_id: left.workspace_id,
                conversation_id: left.conversation_id,
                member_user_ids: left.member_user_ids,
            });
        }
        info!(
            target: "audit",
            event = "account.purged",
            user_id = %user_id,
            counts = ?done.counts,
            files,
            "an account was purged; its row is a tombstone"
        );
        Ok(Some(AccountPurgeDto {
            user_id,
            counts: done.counts,
            files,
        }))
    }

    /// Delete the purged account's stored files; how many went. A file already gone counts; one
    /// that cannot be deleted is logged and left.
    async fn delete_files(&self, user_id: UserId, keys: &[String]) -> u64 {
        let mut deleted = 0;
        for key in keys {
            match self.storage.delete_file(key).await {
                Ok(()) | Err(ServiceError::NotFound) => deleted += 1,
                Err(e) => {
                    warn!(user_id = %user_id, key = %key, error = %e, "could not delete a purged account's file")
                }
            }
        }
        deleted
    }
}

#[async_trait]
impl AccountDeletionService for DefaultAccountDeletionService {
    async fn request_deletion(
        &self,
        cmd: RequestAccountDeletionCmd,
    ) -> ServiceResult<AccountDeletionScheduleDto> {
        let user_id = cmd.actor.user_id;
        let standing = self
            .repo
            .standing(user_id)
            .await?
            .filter(|s| s.deleted_at.is_none())
            .ok_or(ServiceError::NotFound)?;
        if standing.is_remote {
            return Err(ServiceError::Forbidden(
                "a person from another server deletes their account at their relay".to_string(),
            ));
        }
        if !cmd
            .confirm_username
            .trim()
            .eq_ignore_ascii_case(&standing.username)
        {
            return Err(ServiceError::Validation(
                "the username does not match this account".to_string(),
            ));
        }
        if let Some(at) = standing.deletion_scheduled_for {
            return Err(ServiceError::Conflict(format!(
                "the account is already scheduled for deletion at {}",
                at.to_rfc3339()
            )));
        }
        let owned = self.repo.owned_shared_workspaces(user_id).await?;
        if !owned.is_empty() {
            let names: Vec<&str> = owned.iter().map(|w| w.name.as_str()).collect();
            return Err(ServiceError::Validation(format!(
                "transfer or delete your workspaces first: you own {}, which other people are \
                 members of",
                names.join(", ")
            )));
        }

        // Microseconds, as the database stores them, so the answer matches `me` later.
        let now = (self.clock)().trunc_subsecs(6);
        let scheduled_for = now + self.grace;
        if !self
            .repo
            .record_request(user_id, now, scheduled_for)
            .await?
        {
            return Err(ServiceError::Conflict(
                "the account is already scheduled for deletion".to_string(),
            ));
        }
        let ended_sessions = self
            .session_repo
            .revoke_all_for_user_except(user_id, cmd.session_id)
            .await?;
        self.session_revocations
            .revoke_user_except(user_id, cmd.session_id);
        info!(
            target: "audit",
            event = "account.deletion_requested",
            user_id = %user_id,
            scheduled_for = %scheduled_for.to_rfc3339(),
            ended_sessions,
            "an account asked to be deleted"
        );
        Ok(AccountDeletionScheduleDto { scheduled_for })
    }

    async fn cancel_deletion(&self, actor: Actor) -> ServiceResult<bool> {
        let cancelled = self.repo.clear_request(actor.user_id).await?;
        if cancelled {
            info!(
                target: "audit",
                event = "account.deletion_cancelled",
                user_id = %actor.user_id,
                "an account cancelled its deletion"
            );
        }
        Ok(cancelled)
    }

    async fn purge_due(&self, now: DateTime<Utc>, dry_run: bool) -> ServiceResult<PurgeRunDto> {
        let mut run = PurgeRunDto {
            dry_run,
            ..PurgeRunDto::default()
        };
        for user_id in self.repo.due(now, PURGE_BATCH).await? {
            match self.purge_account(user_id, now, dry_run).await {
                Ok(Some(purged)) => run.purged.push(purged),
                Ok(None) => {}
                Err(e) => {
                    error!(user_id = %user_id, error = %e, "account purge failed; skipped until the next run");
                    run.failed.push(AccountPurgeFailureDto {
                        user_id,
                        error: e.to_string(),
                    });
                }
            }
        }
        Ok(run)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use rim_core::account_deletion::model::{
        AccountStanding, LeftConversation, OwnedSharedWorkspace,
    };
    use rim_core::account_deletion::repository::MockAccountDeletionRepository;
    use rim_core::auth::model::SessionId;
    use rim_core::auth::repository::MockSessionRepository;
    use rim_core::conversation::model::ConversationId;
    use rim_core::repository::error::{RepoError, RepoErrorKind};
    use rim_core::server::model::ServerId;
    use rim_services::events::Broadcaster;
    use rim_services::remote_workspace::service::MockWorkspaceRelayService;
    use rim_services::session_revocation::{SessionWatch, WatchedSession};
    use rim_services::storage::service::MockStorageService;

    use super::*;
    use crate::session_revocation::InProcessSessionRevocations;

    /// A registry with a live connection of `session_id` for `user_id`, and that connection.
    fn watching(
        user_id: UserId,
        session_id: SessionId,
    ) -> (Arc<InProcessSessionRevocations>, SessionWatch) {
        let registry = Arc::new(InProcessSessionRevocations::default());
        let watch = registry.watch(WatchedSession {
            session_id,
            user_id,
            device_key_id: None,
        });
        (registry, watch)
    }

    /// Every event broadcast through it, in order.
    #[derive(Default)]
    struct Recorder(Mutex<Vec<WorkspaceEvent>>);

    impl Broadcaster for Recorder {
        fn broadcast(&self, event: WorkspaceEvent) {
            self.0.lock().expect("lock").push(event);
        }
    }

    fn fixed_now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-10-12T12:00:00Z")
            .expect("time")
            .with_timezone(&Utc)
    }

    fn standing(user_id: UserId) -> AccountStanding {
        AccountStanding {
            user_id,
            username: "alice".to_string(),
            is_remote: false,
            deletion_scheduled_for: None,
            deleted_at: None,
        }
    }

    /// The service over `repo` and `sessions`, with storage that deletes anything and holds
    /// two avatar files for every account, and a clock fixed at [`fixed_now`].
    fn service(
        repo: MockAccountDeletionRepository,
        sessions: MockSessionRepository,
    ) -> DefaultAccountDeletionService {
        let mut storage = holding_avatars();
        storage
            .expect_delete_file()
            .returning(|_| Box::pin(async { Ok(()) }));
        service_with(repo, sessions, storage, Arc::new(Recorder::default()))
    }

    /// Storage whose avatar directory of each account holds a png and a gif.
    fn holding_avatars() -> MockStorageService {
        let mut storage = MockStorageService::new();
        storage.expect_list_files().returning(|dir| {
            let keys = vec![format!("{dir}/avatar.gif"), format!("{dir}/avatar.png")];
            Box::pin(async move { Ok(keys) })
        });
        storage
    }

    fn service_with(
        repo: MockAccountDeletionRepository,
        sessions: MockSessionRepository,
        storage: MockStorageService,
        events: Arc<Recorder>,
    ) -> DefaultAccountDeletionService {
        DefaultAccountDeletionService::new(
            Arc::new(repo),
            Arc::new(sessions),
            None,
            Arc::new(storage),
            Observers::new(events),
        )
        .with_clock(Arc::new(fixed_now))
    }

    fn request(user_id: UserId, session_id: SessionId, typed: &str) -> RequestAccountDeletionCmd {
        RequestAccountDeletionCmd {
            actor: Actor::new(user_id),
            session_id,
            confirm_username: typed.to_string(),
        }
    }

    /// A repository whose `standing` answers `s` and that finds `owned` blocking workspaces.
    fn repo_for(
        s: AccountStanding,
        owned: Vec<OwnedSharedWorkspace>,
    ) -> MockAccountDeletionRepository {
        let mut repo = MockAccountDeletionRepository::new();
        repo.expect_standing().returning(move |_| {
            let s = s.clone();
            Box::pin(async move { Ok(Some(s)) })
        });
        repo.expect_owned_shared_workspaces().returning(move |_| {
            let owned = owned.clone();
            Box::pin(async move { Ok(owned) })
        });
        repo
    }

    /// [`repo_for`] with no blocking workspace, expecting nothing to be recorded.
    fn refusing(s: AccountStanding) -> MockAccountDeletionRepository {
        let mut repo = repo_for(s, vec![]);
        repo.expect_record_request().never();
        repo
    }

    fn no_sessions_ended() -> MockSessionRepository {
        let mut sessions = MockSessionRepository::new();
        sessions.expect_revoke_all_for_user_except().never();
        sessions
    }

    #[tokio::test]
    async fn a_request_schedules_the_purge_after_the_grace_period_and_ends_other_sessions() {
        let user_id = UserId::new();
        let session_id = SessionId::new();
        let mut repo = repo_for(standing(user_id), vec![]);
        repo.expect_record_request()
            .withf(move |id, at, scheduled| {
                *id == user_id
                    && *at == fixed_now()
                    && *scheduled == fixed_now() + Duration::days(14)
            })
            .times(1)
            .returning(|_, _, _| Box::pin(async { Ok(true) }));
        let mut sessions = MockSessionRepository::new();
        sessions
            .expect_revoke_all_for_user_except()
            .withf(move |id, keep| *id == user_id && *keep == session_id)
            .times(1)
            .returning(|_, _| Ok(2));
        let (registry, mut caller) = watching(user_id, session_id);
        let mut other = registry.watch(WatchedSession {
            session_id: SessionId::new(),
            user_id,
            device_key_id: None,
        });

        let scheduled = service(repo, sessions)
            .with_session_revocations(registry)
            .request_deletion(request(user_id, session_id, " alice "))
            .await
            .expect("scheduled");
        assert_eq!(scheduled.scheduled_for, fixed_now() + Duration::days(14));
        assert!(other.has_ended(), "the other session's connection closes");
        assert!(!caller.has_ended(), "the requesting connection stays");
    }

    #[tokio::test]
    async fn the_grace_period_comes_from_the_setting_and_zero_schedules_now() {
        for days in [0, 3, 30] {
            let user_id = UserId::new();
            let mut repo = repo_for(standing(user_id), vec![]);
            repo.expect_record_request()
                .returning(|_, _, _| Box::pin(async { Ok(true) }));
            let mut sessions = MockSessionRepository::new();
            sessions
                .expect_revoke_all_for_user_except()
                .returning(|_, _| Ok(0));
            let scheduled = service(repo, sessions)
                .with_grace(Duration::days(days))
                .request_deletion(request(user_id, SessionId::new(), "alice"))
                .await
                .expect("scheduled");
            assert_eq!(scheduled.scheduled_for, fixed_now() + Duration::days(days));
        }
    }

    #[tokio::test]
    async fn a_request_with_another_username_is_refused() {
        let user_id = UserId::new();
        let err = service(refusing(standing(user_id)), no_sessions_ended())
            .request_deletion(request(user_id, SessionId::new(), "alicia"))
            .await
            .expect_err("refused");
        assert!(matches!(err, ServiceError::Validation(m) if m.contains("username")));
    }

    #[tokio::test]
    async fn the_typed_username_is_matched_without_regard_to_case() {
        let user_id = UserId::new();
        let mut repo = repo_for(standing(user_id), vec![]);
        repo.expect_record_request()
            .times(1)
            .returning(|_, _, _| Box::pin(async { Ok(true) }));
        let mut sessions = MockSessionRepository::new();
        sessions
            .expect_revoke_all_for_user_except()
            .returning(|_, _| Ok(0));
        service(repo, sessions)
            .request_deletion(request(user_id, SessionId::new(), " ALICE "))
            .await
            .expect("scheduled");
    }

    #[tokio::test]
    async fn the_only_owner_of_a_shared_workspace_is_told_to_transfer_or_delete_it() {
        let user_id = UserId::new();
        let mut repo = repo_for(
            standing(user_id),
            vec![OwnedSharedWorkspace {
                id: ServerId::new(),
                name: "Team".to_string(),
            }],
        );
        repo.expect_record_request().never();
        let err = service(repo, no_sessions_ended())
            .request_deletion(request(user_id, SessionId::new(), "alice"))
            .await
            .expect_err("refused");
        assert!(
            matches!(&err, ServiceError::Validation(m) if m.contains("transfer or delete") && m.contains("Team")),
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn a_remote_member_is_sent_to_their_relay() {
        let user_id = UserId::new();
        let err = service(
            refusing(AccountStanding {
                is_remote: true,
                ..standing(user_id)
            }),
            no_sessions_ended(),
        )
        .request_deletion(request(user_id, SessionId::new(), "alice"))
        .await
        .expect_err("refused");
        assert!(matches!(err, ServiceError::Forbidden(_)));
    }

    #[tokio::test]
    async fn a_second_request_while_one_is_pending_conflicts() {
        let user_id = UserId::new();
        let err = service(
            refusing(AccountStanding {
                deletion_scheduled_for: Some(fixed_now()),
                ..standing(user_id)
            }),
            no_sessions_ended(),
        )
        .request_deletion(request(user_id, SessionId::new(), "alice"))
        .await
        .expect_err("refused");
        assert!(matches!(err, ServiceError::Conflict(_)));
    }

    #[tokio::test]
    async fn a_purged_account_cannot_ask_again() {
        let user_id = UserId::new();
        let err = service(
            refusing(AccountStanding {
                deleted_at: Some(fixed_now()),
                ..standing(user_id)
            }),
            no_sessions_ended(),
        )
        .request_deletion(request(user_id, SessionId::new(), "alice"))
        .await
        .expect_err("refused");
        assert!(matches!(err, ServiceError::NotFound));
    }

    #[tokio::test]
    async fn cancel_reports_whether_a_deletion_was_pending() {
        for pending in [true, false] {
            let mut repo = MockAccountDeletionRepository::new();
            repo.expect_clear_request()
                .times(1)
                .returning(move |_| Box::pin(async move { Ok(pending) }));
            let cancelled = service(repo, MockSessionRepository::new())
                .cancel_deletion(Actor::new(UserId::new()))
                .await
                .expect("cancel");
            assert_eq!(cancelled, pending);
        }
    }

    /// A repository that has `due` accounts and runs every purge step for them, in order,
    /// recording each step's name; the account in `fails` fails at its messages step.
    fn purging(
        due: Vec<UserId>,
        fails: Option<UserId>,
        calls: Arc<Mutex<Vec<(UserId, &'static str)>>>,
    ) -> MockAccountDeletionRepository {
        let mut repo = MockAccountDeletionRepository::new();
        repo.expect_due()
            .withf(|now, limit| *now == fixed_now() && *limit == PURGE_BATCH)
            .returning(move |_, _| {
                let due = due.clone();
                Box::pin(async move { Ok(due) })
            });
        let record = |calls: &Arc<Mutex<Vec<(UserId, &'static str)>>>, id, step| {
            calls.lock().expect("lock").push((id, step));
        };
        let c = Arc::clone(&calls);
        repo.expect_claim_due().returning(move |id, _| {
            record(&c, id, "claim");
            Box::pin(async { Ok(true) })
        });
        let c = Arc::clone(&calls);
        repo.expect_purge_workspaces().returning(move |id, _| {
            record(&c, id, "workspaces");
            Box::pin(async {
                Ok(PurgeStep {
                    counts: vec![("workspace_memberships", 1)],
                    storage_keys: vec!["custom-emoji/a.png".to_string()],
                    left_workspaces: vec![ServerId::new()],
                    left_conversations: vec![left_conversation()],
                })
            })
        });
        let c = Arc::clone(&calls);
        repo.expect_purge_messages().returning(move |id| {
            record(&c, id, "messages");
            let failed = Some(id) == fails;
            Box::pin(async move {
                if failed {
                    Err(RepoError::new(RepoErrorKind::Internal))
                } else {
                    Ok(PurgeStep {
                        counts: vec![("messages_deleted", 3)],
                        storage_keys: vec!["uploads/a".to_string()],
                        ..PurgeStep::default()
                    })
                }
            })
        });
        let c = Arc::clone(&calls);
        repo.expect_purge_personal_data().returning(move |id| {
            record(&c, id, "personal_data");
            Box::pin(async { Ok(PurgeStep::counted(vec![("keybinds", 2)])) })
        });
        let c = Arc::clone(&calls);
        repo.expect_purge_credentials().returning(move |id, _| {
            record(&c, id, "credentials");
            Box::pin(async { Ok(PurgeStep::counted(vec![("sessions", 1)])) })
        });
        let c = Arc::clone(&calls);
        repo.expect_purge_relay_state().returning(move |id| {
            record(&c, id, "relay_state");
            Box::pin(async { Ok(PurgeStep::counted(vec![("relay_items", 0)])) })
        });
        let c = Arc::clone(&calls);
        repo.expect_tombstone().returning(move |id, _| {
            record(&c, id, "tombstone");
            Box::pin(async { Ok(PurgeStep::counted(vec![("tombstone", 1)])) })
        });
        repo
    }

    fn left_conversation() -> LeftConversation {
        LeftConversation {
            workspace_id: ServerId::new(),
            conversation_id: ConversationId::new(),
            member_user_ids: vec![UserId::new()],
        }
    }

    const STEPS: [&str; 7] = [
        "claim",
        "workspaces",
        "messages",
        "personal_data",
        "credentials",
        "relay_state",
        "tombstone",
    ];

    #[tokio::test]
    async fn the_purge_runs_every_step_in_order_then_deletes_files_and_tells_the_workspaces() {
        let user_id = UserId::new();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let repo = purging(vec![user_id], None, Arc::clone(&calls));
        let mut storage = holding_avatars();
        let deleted = Arc::new(Mutex::new(Vec::new()));
        let d = Arc::clone(&deleted);
        storage.expect_delete_file().returning(move |key| {
            d.lock().expect("lock").push(key.to_string());
            Box::pin(async { Ok(()) })
        });
        let events = Arc::new(Recorder::default());
        let (registry, mut connection) = watching(user_id, SessionId::new());
        let run = service_with(
            repo,
            MockSessionRepository::new(),
            storage,
            Arc::clone(&events),
        )
        .with_session_revocations(registry)
        .purge_due(fixed_now(), false)
        .await
        .expect("run");
        assert!(
            connection.has_ended(),
            "the purge closes the account's connections"
        );

        let steps: Vec<&str> = calls.lock().expect("lock").iter().map(|c| c.1).collect();
        assert_eq!(steps, STEPS);
        assert_eq!(run.failed, vec![]);
        assert_eq!(run.purged.len(), 1);
        let purged = &run.purged[0];
        assert_eq!(purged.user_id, user_id);
        assert_eq!(
            purged.counts,
            vec![
                ("workspace_memberships", 1),
                ("messages_deleted", 3),
                ("keybinds", 2),
                ("sessions", 1),
                ("relay_items", 0),
                ("tombstone", 1),
            ]
        );
        assert_eq!(purged.files, 4);
        assert_eq!(
            *deleted.lock().expect("lock"),
            vec![
                "custom-emoji/a.png".to_string(),
                "uploads/a".to_string(),
                format!("avatars/{user_id}/avatar.gif"),
                format!("avatars/{user_id}/avatar.png"),
            ]
        );
        let events = events.0.lock().expect("lock");
        assert!(matches!(
            events.as_slice(),
            [
                WorkspaceEvent::MemberLeft { user_id: left, .. },
                WorkspaceEvent::MlsMembershipChanged { .. },
            ] if *left == user_id
        ));
    }

    #[tokio::test]
    async fn a_failed_account_is_skipped_and_the_next_one_is_purged() {
        let (broken, fine) = (UserId::new(), UserId::new());
        let calls = Arc::new(Mutex::new(Vec::new()));
        let repo = purging(vec![broken, fine], Some(broken), Arc::clone(&calls));
        let run = service(repo, MockSessionRepository::new())
            .purge_due(fixed_now(), false)
            .await
            .expect("run");

        assert_eq!(run.failed.len(), 1);
        assert_eq!(run.failed[0].user_id, broken);
        assert_eq!(run.purged.len(), 1);
        assert_eq!(run.purged[0].user_id, fine);
        let fine_steps: Vec<&str> = calls
            .lock()
            .expect("lock")
            .iter()
            .filter(|c| c.0 == fine)
            .map(|c| c.1)
            .collect();
        assert_eq!(fine_steps, STEPS);
        let broken_steps: Vec<&str> = calls
            .lock()
            .expect("lock")
            .iter()
            .filter(|c| c.0 == broken)
            .map(|c| c.1)
            .collect();
        assert_eq!(broken_steps, ["claim", "workspaces", "messages"]);
    }

    #[tokio::test]
    async fn an_account_no_longer_due_is_left_alone() {
        let user_id = UserId::new();
        let mut repo = MockAccountDeletionRepository::new();
        repo.expect_due()
            .returning(move |_, _| Box::pin(async move { Ok(vec![user_id]) }));
        repo.expect_claim_due()
            .returning(|_, _| Box::pin(async { Ok(false) }));
        repo.expect_purge_workspaces().never();
        repo.expect_tombstone().never();
        let run = service(repo, MockSessionRepository::new())
            .purge_due(fixed_now(), false)
            .await
            .expect("run");
        assert_eq!(run, PurgeRunDto::default());
    }

    #[tokio::test]
    async fn a_dry_run_counts_but_deletes_no_file_tells_nobody_and_leaves_no_relay() {
        let user_id = UserId::new();
        let repo = purging(vec![user_id], None, Arc::new(Mutex::new(Vec::new())));
        let mut storage = holding_avatars();
        storage.expect_delete_file().never();
        let mut relay = MockWorkspaceRelayService::new();
        relay.expect_leave_relay().never();
        let events = Arc::new(Recorder::default());
        let (registry, mut connection) = watching(user_id, SessionId::new());
        let run = service_with(
            repo,
            MockSessionRepository::new(),
            storage,
            Arc::clone(&events),
        )
        .with_relay_departure(Arc::new(relay))
        .with_session_revocations(registry)
        .purge_due(fixed_now(), true)
        .await
        .expect("run");
        assert!(!connection.has_ended(), "a dry run closes no connection");
        assert!(run.dry_run);
        assert_eq!(run.purged.len(), 1);
        assert_eq!(run.purged[0].files, 4);
        assert!(events.0.lock().expect("lock").is_empty());
    }

    /// When the account's avatar directory cannot be listed, the account fails and no file is
    /// deleted, so the next run purges it again.
    #[tokio::test]
    async fn an_unlistable_avatar_directory_fails_the_account() {
        let user_id = UserId::new();
        let repo = purging(vec![user_id], None, Arc::new(Mutex::new(Vec::new())));
        let mut storage = MockStorageService::new();
        storage
            .expect_list_files()
            .returning(|_| Box::pin(async { Err(ServiceError::Unavailable) }));
        storage.expect_delete_file().never();
        let run = service_with(
            repo,
            MockSessionRepository::new(),
            storage,
            Arc::new(Recorder::default()),
        )
        .purge_due(fixed_now(), false)
        .await
        .expect("run");
        assert_eq!(run.purged, vec![]);
        assert_eq!(run.failed.len(), 1);
        assert_eq!(run.failed[0].user_id, user_id);
    }

    #[tokio::test]
    async fn the_purge_cancels_the_accounts_subscriptions_at_its_hosts_first() {
        let user_id = UserId::new();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut repo = purging(vec![user_id], None, Arc::clone(&calls));
        repo.expect_standing().returning(move |id| {
            Box::pin(async move {
                Ok(Some(AccountStanding {
                    deletion_scheduled_for: Some(fixed_now()),
                    ..standing(id)
                }))
            })
        });
        let mut relay = MockWorkspaceRelayService::new();
        let c = Arc::clone(&calls);
        relay
            .expect_leave_relay()
            .withf(move |actor| actor.user_id == user_id)
            .times(1)
            .returning(move |actor| {
                c.lock().expect("lock").push((actor.user_id, "leave_relay"));
                Box::pin(async { Ok(true) })
            });
        service(repo, MockSessionRepository::new())
            .with_relay_departure(Arc::new(relay))
            .purge_due(fixed_now(), false)
            .await
            .expect("run");
        assert_eq!(calls.lock().expect("lock")[0], (user_id, "leave_relay"));
    }
}
