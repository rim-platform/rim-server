#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use chrono::{DateTime, Duration, Utc};
    use uuid::Uuid;

    use rim_auth::opaque::client::{Login, Registration};
    use rim_auth::opaque::{LABEL_SERVER_SHARE, transport_open};
    use rim_auth::session_token::hash_token;
    use rim_core::account_password::model::PasswordLoginId;
    use rim_core::account_password::repository::AccountPasswordRepository;
    use rim_core::auth::model::{NewSession, Session, SessionId, SessionKeep};
    use rim_core::auth::repository::SessionRepository;
    use rim_core::host::model::UserKind;
    use rim_core::host::repository::MockHostRepository;
    use rim_core::key_identity::model::{
        DeviceKeyId, KeyChallenge, KeyChallengeId, KeyChallengePurpose, KeyLogEntry,
        KeyLogEntryType,
    };
    use rim_core::key_identity::repository::{KeyChallengeRepository, MockKeyIdentityRepository};
    use rim_core::repository::error::RepoResult;
    use rim_core::user::model::{User, UserHandle, UserId};
    use rim_core::user::repository::MockUserRepository;
    use rim_identity::testing::TestIdentity;
    use rim_identity::{KeyState, StatementVersion};
    use rim_services::account_password::dto::{
        PasswordChangeFinishCmd, PasswordChangeStartCmd, PasswordLoginFinishCmd,
        PasswordLoginStartCmd, PasswordRegistrationStartCmd, PasswordResetChallengeCmd,
        PasswordResetFinishCmd, PasswordResetStartCmd,
    };
    use rim_services::account_password::service::AccountPasswordService;
    use rim_services::key_auth::command::SignedPayload;
    use rim_services::{Actor, ServiceError};

    use crate::account_password::{AccountPasswordRepos, DefaultAccountPasswordService};
    use crate::account_password_fakes::{FakePasswords, TEST_PASSWORD_KEY, store_password};
    use crate::clock::Clock;
    use crate::registration_rate_limiter::RegistrationRateLimiter;

    const AUDIENCE: &str = "rim.example.com";
    const IP: &str = "203.0.113.7";

    // ── In-memory storage ───────────────────────────────────────────────

    /// A local account with a version 2 key identity.
    struct Account {
        user_id: UserId,
        identity: TestIdentity,
        state: KeyState,
    }

    impl Account {
        fn principal(&self) -> &str {
            &self.state.principal_id
        }

        fn actor(&self) -> Actor {
            Actor::new(self.user_id)
        }
    }

    #[derive(Default)]
    struct Directory {
        principals: HashMap<String, UserId>,
        inceptions: HashMap<UserId, rim_identity::SignedStatement>,
        remote: HashSet<UserId>,
    }

    struct FakeChallenges(Mutex<Vec<KeyChallenge>>);

    #[async_trait]
    impl KeyChallengeRepository for FakeChallenges {
        async fn create(
            &self,
            nonce: &str,
            expires_at: DateTime<Utc>,
            purpose: &KeyChallengePurpose,
        ) -> RepoResult<KeyChallenge> {
            let challenge = KeyChallenge {
                id: KeyChallengeId::new(),
                nonce: nonce.to_string(),
                purpose: purpose.clone(),
                created_at: Utc::now(),
                expires_at,
                used_at: None,
            };
            self.0.lock().unwrap().push(challenge.clone());
            Ok(challenge)
        }

        async fn consume(
            &self,
            id: KeyChallengeId,
            nonce: &str,
            purpose: &KeyChallengePurpose,
        ) -> RepoResult<bool> {
            let mut challenges = self.0.lock().unwrap();
            let Some(c) = challenges.iter_mut().find(|c| {
                c.id == id && c.nonce == nonce && &c.purpose == purpose && c.used_at.is_none()
            }) else {
                return Ok(false);
            };
            c.used_at = Some(Utc::now());
            Ok(true)
        }

        async fn delete_expired(&self, _before: DateTime<Utc>) -> RepoResult<u64> {
            Ok(0)
        }
    }

    #[derive(Default)]
    struct FakeSessions(Mutex<Vec<Session>>);

    impl FakeSessions {
        fn live(&self, user_id: UserId) -> Vec<SessionId> {
            self.0
                .lock()
                .unwrap()
                .iter()
                .filter(|s| s.user_id == user_id && s.revoked_at.is_none())
                .map(|s| s.id)
                .collect()
        }
    }

    #[async_trait]
    impl SessionRepository for FakeSessions {
        async fn get_by_id(&self, id: SessionId) -> RepoResult<Option<Session>> {
            Ok(self.0.lock().unwrap().iter().find(|s| s.id == id).cloned())
        }

        async fn get_by_token_hash(&self, _token_hash: &str) -> RepoResult<Option<Session>> {
            Ok(None)
        }

        async fn get_many(&self, _ids: &[SessionId]) -> RepoResult<Vec<Session>> {
            Ok(Vec::new())
        }

        async fn create_session(&self, new: NewSession) -> RepoResult<Session> {
            let session = Session {
                id: SessionId::new(),
                user_id: new.user_id,
                token_hash: new.token_hash,
                created_at: Utc::now(),
                expires_at: new.expires_at,
                last_seen_at: None,
                revoked_at: None,
                user_agent: None,
                ip: None,
                device_key_id: None,
                kept: true,
                secret_hash: None,
                keep_key_sealed: None,
                reload_key_sealed: None,
                ws_closed_at: None,
            };
            self.0.lock().unwrap().push(session.clone());
            Ok(session)
        }

        async fn touch_last_seen_at(&self, _id: SessionId) -> RepoResult<()> {
            Ok(())
        }

        async fn revoke_session(&self, id: SessionId) -> RepoResult<()> {
            for s in self.0.lock().unwrap().iter_mut().filter(|s| s.id == id) {
                s.revoked_at = Some(Utc::now());
            }
            Ok(())
        }

        async fn revoke_all_for_user(&self, user_id: UserId) -> RepoResult<u64> {
            let mut n = 0;
            for s in self
                .0
                .lock()
                .unwrap()
                .iter_mut()
                .filter(|s| s.user_id == user_id && s.revoked_at.is_none())
            {
                s.revoked_at = Some(Utc::now());
                n += 1;
            }
            Ok(n)
        }

        async fn revoke_all_for_user_except(
            &self,
            user_id: UserId,
            keep: SessionId,
        ) -> RepoResult<u64> {
            let mut n = 0;
            for s in self
                .0
                .lock()
                .unwrap()
                .iter_mut()
                .filter(|s| s.user_id == user_id && s.id != keep && s.revoked_at.is_none())
            {
                s.revoked_at = Some(Utc::now());
                n += 1;
            }
            Ok(n)
        }

        async fn bind_device(&self, _id: SessionId, _device: DeviceKeyId) -> RepoResult<()> {
            Ok(())
        }

        async fn revoke_all_for_device(&self, _device: DeviceKeyId) -> RepoResult<u64> {
            Ok(0)
        }

        async fn delete_ended(&self, _now: DateTime<Utc>) -> RepoResult<u64> {
            Ok(0)
        }

        async fn set_keep(&self, _id: SessionId, _keep: SessionKeep) -> RepoResult<()> {
            unimplemented!("the password service keeps no session")
        }

        async fn set_reload_key(&self, _id: SessionId, _sealed: Vec<u8>) -> RepoResult<bool> {
            unimplemented!("the password service stores no reload key")
        }

        async fn take_reload_key(
            &self,
            _id: SessionId,
            _next_sealed: Vec<u8>,
            _now: DateTime<Utc>,
            _closed_since: DateTime<Utc>,
        ) -> RepoResult<Option<Vec<u8>>> {
            unimplemented!("the password service takes no reload key")
        }

        async fn socket_opened(&self, _id: SessionId) -> RepoResult<()> {
            unimplemented!("the password service opens no WebSocket")
        }

        async fn socket_closed(&self, _id: SessionId, _at: DateTime<Utc>) -> RepoResult<()> {
            unimplemented!("the password service opens no WebSocket")
        }
    }

    fn user(id: UserId, principal_id: Option<String>) -> User {
        let now = Utc::now();
        User {
            id,
            username: format!("user-{id}"),
            password_hash: None,
            created_at: now,
            updated_at: now,
            last_login_at: None,
            display_name: None,
            avatar_url: None,
            status_text: None,
            bio: None,
            email: None,
            timezone: None,
            principal_id,
            deletion_scheduled_for: None,
            handle: UserHandle::Local,
        }
    }

    struct World {
        svc: DefaultAccountPasswordService,
        passwords: FakePasswords,
        sessions: Arc<FakeSessions>,
        directory: Arc<Mutex<Directory>>,
        now: Arc<Mutex<DateTime<Utc>>>,
    }

    fn world() -> World {
        let directory: Arc<Mutex<Directory>> = Arc::default();
        let now = Arc::new(Mutex::new(Utc::now()));

        let mut users = MockUserRepository::new();
        let d = Arc::clone(&directory);
        users.expect_get_by_id().returning(move |id| {
            let d = d.lock().unwrap();
            let principal = d
                .principals
                .iter()
                .find(|(_, u)| **u == id)
                .map(|(p, _)| p.clone());
            Ok(Some(user(id, principal)))
        });

        let mut keys = MockKeyIdentityRepository::new();
        let d = Arc::clone(&directory);
        keys.expect_find_user_by_principal_id().returning(move |p| {
            let found = d.lock().unwrap().principals.get(p).copied();
            Box::pin(async move { Ok(found) })
        });
        let d = Arc::clone(&directory);
        keys.expect_list_key_log().returning(move |user_id| {
            let entries = d
                .lock()
                .unwrap()
                .inceptions
                .get(&user_id)
                .map(|s| {
                    vec![KeyLogEntry {
                        user_id,
                        seq: 0,
                        entry_type: KeyLogEntryType::Inception,
                        payload: s.payload.clone(),
                        signature: s.signature.clone(),
                        created_at: Utc::now(),
                    }]
                })
                .unwrap_or_default();
            Box::pin(async move { Ok(entries) })
        });
        keys.expect_lock_user()
            .returning(|_| Box::pin(async { Ok(true) }));

        let mut host = MockHostRepository::new();
        let d = Arc::clone(&directory);
        host.expect_user_kind().returning(move |id| {
            let remote = d.lock().unwrap().remote.contains(&id);
            Box::pin(async move {
                Ok(Some(if remote {
                    UserKind::Remote
                } else {
                    UserKind::Local
                }))
            })
        });

        let passwords = FakePasswords::default();
        let sessions = Arc::new(FakeSessions::default());
        let repos = AccountPasswordRepos {
            password_repo: Arc::new(passwords.clone()),
            user_repo: Arc::new(users),
            key_identity_repo: Arc::new(keys),
            key_challenge_repo: Arc::new(FakeChallenges(Mutex::new(Vec::new()))),
            session_repo: Arc::clone(&sessions) as Arc<dyn SessionRepository>,
            host_repo: Arc::new(host),
        };
        let clock_now = Arc::clone(&now);
        let svc = DefaultAccountPasswordService::new(
            repos,
            Some(TEST_PASSWORD_KEY.clone()),
            vec![AUDIENCE.to_string()],
        )
        .with_clock(Clock::from_fn(move || *clock_now.lock().unwrap()))
        .with_rate_limiters(
            RegistrationRateLimiter::new(3600, 1000),
            RegistrationRateLimiter::new(3600, 1000),
        );
        World {
            svc,
            passwords,
            sessions,
            directory,
            now,
        }
    }

    impl World {
        fn advance(&self, by: Duration) {
            *self.now.lock().unwrap() += by;
        }

        fn account(&self, seed: u8) -> Account {
            let identity = TestIdentity::with_version(seed, StatementVersion::V2);
            let inception = identity.inception();
            let state = KeyState::from_inception(&inception).unwrap();
            let user_id = UserId::new();
            let mut d = self.directory.lock().unwrap();
            d.principals.insert(state.principal_id.clone(), user_id);
            d.inceptions.insert(user_id, inception);
            Account {
                user_id,
                identity,
                state,
            }
        }

        /// An account with the password `password`, as a signup leaves it.
        async fn account_with_password(&self, seed: u8, password: &[u8]) -> Account {
            let account = self.account(seed);
            store_password(
                &self.passwords,
                account.principal(),
                account.user_id,
                password,
            )
            .await;
            account
        }

        async fn session(&self, account: &Account) -> SessionId {
            self.sessions
                .create_session(NewSession {
                    user_id: account.user_id,
                    token_hash: Uuid::now_v7().to_string(),
                    expires_at: Utc::now() + Duration::days(1),
                    user_agent: None,
                    ip: None,
                })
                .await
                .unwrap()
                .id
        }

        async fn login_start(&self, principal: &str) -> Result<PasswordLoginId, ServiceError> {
            let (_, request) = Login::start(b"guess");
            self.svc
                .login_start(PasswordLoginStartCmd {
                    principal_id: principal.to_string(),
                    request,
                    remote_ip: IP.to_string(),
                })
                .await
                .map(|dto| dto.login_id)
        }

        fn tries(&self, account: &Account) -> u32 {
            self.passwords.state().passwords[&account.user_id].failed_attempts
        }

        /// Change `account`'s password from `old` to `new` with `rewrap`, from `session`.
        async fn change(
            &self,
            account: &Account,
            session: SessionId,
            old: &[u8],
            new: &[u8],
            rewrap: &[u8],
        ) -> Result<(), ServiceError> {
            let (login, credential_request) = Login::start(old);
            let (registration, registration_request) = Registration::start(new);
            let started = self
                .svc
                .change_start(PasswordChangeStartCmd {
                    actor: account.actor(),
                    credential_request,
                    registration_request,
                })
                .await?;
            let finalization = login
                .finish(&started.credential_response)
                .map_or_else(|| vec![0; 64], |done| done.finalization);
            let (upload, _) = registration.finish(&started.registration_response).unwrap();
            self.svc
                .change_finish(PasswordChangeFinishCmd {
                    actor: account.actor(),
                    session_id: session,
                    login_id: started.login_id,
                    credential_finalization: finalization,
                    registration_upload: upload,
                    rewrap: rewrap.to_vec(),
                })
                .await
                .map(|_| ())
        }

        /// Whether `password` opens a login for `account` on the client side.
        async fn password_works(&self, account: &Account, password: &[u8]) -> bool {
            let (flow, request) = Login::start(password);
            let started = self
                .svc
                .login_start(PasswordLoginStartCmd {
                    principal_id: account.principal().to_string(),
                    request,
                    remote_ip: IP.to_string(),
                })
                .await
                .unwrap();
            let works = flow.finish(&started.response).is_some();
            self.passwords
                .clear_attempts(account.user_id, Utc::now())
                .await
                .unwrap();
            works
        }

        /// A reset statement over a fresh challenge, signed by `signer`'s recovery key or, with
        /// `by_identity`, its identity key.
        async fn reset_statement(&self, account: &Account, by_identity: bool) -> SignedPayload {
            let challenge = self
                .svc
                .reset_challenge(PasswordResetChallengeCmd {
                    principal_id: account.principal().to_string(),
                    remote_ip: IP.to_string(),
                })
                .await
                .unwrap();
            let statement = if by_identity {
                rim_identity::sign(
                    rim_identity::Purpose::PasswordReset,
                    StatementVersion::V2,
                    &account.identity.identity,
                    rim_identity::testing::password_reset_payload(
                        &account.state,
                        challenge.challenge_id.into(),
                        &challenge.nonce,
                        AUDIENCE,
                    ),
                )
            } else {
                account.identity.password_reset(
                    &account.state,
                    challenge.challenge_id.into(),
                    &challenge.nonce,
                    AUDIENCE,
                )
            };
            SignedPayload {
                payload: statement.payload,
                signature: statement.signature,
            }
        }

        /// Reset `account`'s password to `new` with `statement`.
        async fn reset(&self, statement: SignedPayload, new: &[u8]) -> Result<(), ServiceError> {
            let (registration, registration_request) = Registration::start(new);
            let started = self
                .svc
                .reset_start(PasswordResetStartCmd {
                    statement,
                    registration_request,
                    remote_ip: IP.to_string(),
                })
                .await?;
            let (upload, _) = registration.finish(&started.registration_response).unwrap();
            self.svc
                .reset_finish(PasswordResetFinishCmd {
                    reset_id: started.reset_id,
                    registration_upload: upload,
                })
                .await
        }
    }

    // ── Wrong tries ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn three_tries_are_free_then_each_waits_longer_and_the_tenth_locks() {
        let w = world();
        let a = w.account_with_password(1, b"pw").await;
        for _ in 0..3 {
            w.login_start(a.principal()).await.unwrap();
        }
        // The 4th try is allowed and starts the first wait.
        w.login_start(a.principal()).await.unwrap();
        for (try_number, wait) in [(5u32, 60u64), (6, 300), (7, 900), (8, 3600), (9, 14_400)] {
            let err = w.login_start(a.principal()).await.unwrap_err();
            match err {
                ServiceError::RateLimited {
                    retry_after_seconds,
                } => assert_eq!(retry_after_seconds, wait, "before try {try_number}"),
                other => panic!("before try {try_number}: {other:?}"),
            }
            let wait_seconds = i64::try_from(wait).unwrap();
            w.advance(Duration::seconds(wait_seconds - 1));
            assert!(w.login_start(a.principal()).await.is_err());
            w.advance(Duration::seconds(1));
            w.login_start(a.principal()).await.unwrap();
            assert_eq!(w.tries(&a), try_number);
        }
        let err = w.login_start(a.principal()).await.unwrap_err();
        assert!(
            matches!(
                err,
                ServiceError::RateLimited {
                    retry_after_seconds: 43_200
                }
            ),
            "{err:?}"
        );
        w.advance(Duration::hours(12));
        // The 10th try still gets an answer, and locks the password.
        w.login_start(a.principal()).await.unwrap();
        assert!(
            w.passwords.state().passwords[&a.user_id]
                .locked_at
                .is_some()
        );
        w.advance(Duration::days(30));
        let err = w.login_start(a.principal()).await.unwrap_err();
        assert!(matches!(err, ServiceError::PasswordLocked), "{err:?}");
        assert_eq!(w.tries(&a), 10);
    }

    #[tokio::test]
    async fn an_abandoned_start_counts_and_a_success_clears_the_count() {
        let w = world();
        let a = w.account_with_password(1, b"pw").await;
        w.login_start(a.principal()).await.unwrap();
        w.login_start(a.principal()).await.unwrap();
        assert_eq!(w.tries(&a), 2);
        // A change proves the password; its success clears the count.
        let session = w.session(&a).await;
        w.change(&a, session, b"pw", b"pw2", b"rewrap")
            .await
            .unwrap();
        assert_eq!(w.tries(&a), 0);
    }

    #[tokio::test]
    async fn an_unknown_principal_gets_an_answer_no_password_finishes_and_nothing_is_stored() {
        let w = world();
        let stranger = TestIdentity::with_version(9, StatementVersion::V2);
        let principal = KeyState::from_inception(&stranger.inception())
            .unwrap()
            .principal_id;
        let (flow, request) = Login::start(b"pw");
        let started = w
            .svc
            .login_start(PasswordLoginStartCmd {
                principal_id: principal,
                request,
                remote_ip: IP.to_string(),
            })
            .await
            .unwrap();
        assert!(!started.response.is_empty());
        assert!(flow.finish(&started.response).is_none());
        assert!(w.passwords.state().logins.is_empty());
        // An account without a password answers the same way and counts nothing.
        let without = w.account(2);
        w.login_start(without.principal()).await.unwrap();
        assert!(w.passwords.state().logins.is_empty());
    }

    #[tokio::test]
    async fn a_malformed_request_or_principal_is_a_validation_error() {
        let w = world();
        let a = w.account_with_password(1, b"pw").await;
        let err = w
            .svc
            .login_start(PasswordLoginStartCmd {
                principal_id: a.principal().to_string(),
                request: vec![1, 2, 3],
                remote_ip: IP.to_string(),
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)), "{err:?}");
        let err = w.login_start("not-a-principal").await.unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)), "{err:?}");
    }

    // ── Registration ────────────────────────────────────────────────────

    #[tokio::test]
    async fn registration_is_refused_for_a_principal_a_local_account_holds() {
        let w = world();
        let local = w.account(1);
        let (_, request) = Registration::start(b"pw");
        let cmd = |principal: &str| PasswordRegistrationStartCmd {
            principal_id: principal.to_string(),
            request: request.clone(),
            remote_ip: IP.to_string(),
        };
        let err = w
            .svc
            .registration_start(cmd(local.principal()))
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Conflict(_)), "{err:?}");

        // Someone from another server may sign up here and become local.
        let remote = w.account(2);
        w.directory.lock().unwrap().remote.insert(remote.user_id);
        assert!(
            w.svc
                .registration_start(cmd(remote.principal()))
                .await
                .is_ok()
        );
        let nobody = TestIdentity::with_version(3, StatementVersion::V2);
        let nobody = KeyState::from_inception(&nobody.inception()).unwrap();
        assert!(
            w.svc
                .registration_start(cmd(&nobody.principal_id))
                .await
                .is_ok()
        );
    }

    // ── Change ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn a_change_keeps_the_share_stores_the_rewrap_and_ends_other_sessions() {
        let w = world();
        let a = w.account_with_password(1, b"old").await;
        let share = TEST_PASSWORD_KEY
            .open_share(
                a.user_id,
                &w.passwords.state().passwords[&a.user_id].share_sealed,
            )
            .unwrap();
        let current = w.session(&a).await;
        let other = w.session(&a).await;

        w.change(&a, current, b"old", b"new", b"rewrap-1")
            .await
            .unwrap();
        assert_eq!(w.sessions.live(a.user_id), vec![current]);
        assert!(!w.sessions.live(a.user_id).contains(&other));
        assert!(w.password_works(&a, b"new").await);
        assert!(!w.password_works(&a, b"old").await);
        assert_eq!(
            TEST_PASSWORD_KEY
                .open_share(
                    a.user_id,
                    &w.passwords.state().passwords[&a.user_id].share_sealed
                )
                .unwrap(),
            share,
            "the server share stays"
        );

        // The old password does not change it again, and counts as a try.
        let err = w
            .change(&a, current, b"old", b"newer", b"rewrap-x")
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Unauthenticated), "{err:?}");
        assert_eq!(w.tries(&a), 1);

        // Only the newest three rewraps stay.
        w.change(&a, current, b"new", b"p2", b"rewrap-2")
            .await
            .unwrap();
        w.change(&a, current, b"p2", b"p3", b"rewrap-3")
            .await
            .unwrap();
        w.change(&a, current, b"p3", b"p4", b"rewrap-4")
            .await
            .unwrap();
        assert_eq!(
            w.passwords.list_rewraps(a.user_id).await.unwrap(),
            vec![
                b"rewrap-4".to_vec(),
                b"rewrap-3".to_vec(),
                b"rewrap-2".to_vec()
            ]
        );
    }

    #[tokio::test]
    async fn a_change_without_a_password_or_of_a_remote_account_is_refused() {
        let w = world();
        let without = w.account(1);
        let session = w.session(&without).await;
        let err = w
            .change(&without, session, b"a", b"b", b"r")
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::NotFound), "{err:?}");
        let remote = w.account(2);
        w.directory.lock().unwrap().remote.insert(remote.user_id);
        let session = w.session(&remote).await;
        let err = w
            .change(&remote, session, b"a", b"b", b"r")
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)), "{err:?}");
    }

    // ── Reset ───────────────────────────────────────────────────────────

    #[tokio::test]
    async fn a_reset_signed_by_the_recovery_key_keeps_the_share_clears_the_lock_and_ends_sessions()
    {
        let w = world();
        let a = w.account_with_password(1, b"forgotten").await;
        let share_before = TEST_PASSWORD_KEY
            .open_share(
                a.user_id,
                &w.passwords.state().passwords[&a.user_id].share_sealed,
            )
            .unwrap();
        {
            let mut state = w.passwords.state();
            let password = state.passwords.get_mut(&a.user_id).unwrap();
            password.failed_attempts = 10;
            password.locked_at = Some(Utc::now());
            state.rewraps.insert(a.user_id, vec![b"old".to_vec()]);
        }
        w.session(&a).await;
        w.session(&a).await;

        let statement = w.reset_statement(&a, false).await;
        w.reset(statement.clone(), b"fresh").await.unwrap();

        let stored = w.passwords.state().passwords[&a.user_id].clone();
        assert_eq!(stored.failed_attempts, 0);
        assert!(stored.locked_at.is_none());
        assert_eq!(
            TEST_PASSWORD_KEY
                .open_share(a.user_id, &stored.share_sealed)
                .unwrap(),
            share_before
        );
        assert!(
            w.passwords
                .list_rewraps(a.user_id)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(w.sessions.live(a.user_id).is_empty());
        assert!(w.password_works(&a, b"fresh").await);
        assert!(!w.password_works(&a, b"forgotten").await);

        // The challenge is used.
        let err = w.reset(statement, b"again").await.unwrap_err();
        assert!(matches!(err, ServiceError::Unauthenticated), "{err:?}");
    }

    #[tokio::test]
    async fn a_reset_signed_by_the_identity_key_is_refused() {
        let w = world();
        let a = w.account_with_password(1, b"pw").await;
        let statement = w.reset_statement(&a, true).await;
        let err = w.reset(statement, b"new").await.unwrap_err();
        assert!(matches!(err, ServiceError::Unauthenticated), "{err:?}");
        assert!(w.password_works(&a, b"pw").await);
    }

    #[tokio::test]
    async fn a_reset_finish_works_once_and_not_after_it_expires() {
        let w = world();
        let a = w.account_with_password(1, b"pw").await;
        let statement = w.reset_statement(&a, false).await;
        let (registration, registration_request) = Registration::start(b"new");
        let started = w
            .svc
            .reset_start(PasswordResetStartCmd {
                statement,
                registration_request,
                remote_ip: IP.to_string(),
            })
            .await
            .unwrap();
        let (upload, _) = registration.finish(&started.registration_response).unwrap();
        w.advance(Duration::seconds(121));
        let err = w
            .svc
            .reset_finish(PasswordResetFinishCmd {
                reset_id: started.reset_id,
                registration_upload: upload,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Unauthenticated), "{err:?}");
        assert!(w.password_works(&a, b"pw").await);
    }

    // ── Login finish ────────────────────────────────────────────────────

    /// Start a login for `account` with `password` and finish it on the client; the login id
    /// and the client's finalization and session key (garbage and `None` for a wrong password).
    async fn client_login(
        w: &World,
        account: &Account,
        password: &[u8],
    ) -> (PasswordLoginId, Vec<u8>, Option<Vec<u8>>) {
        let (flow, request) = Login::start(password);
        let started = w
            .svc
            .login_start(PasswordLoginStartCmd {
                principal_id: account.principal().to_string(),
                request,
                remote_ip: IP.to_string(),
            })
            .await
            .unwrap();
        match flow.finish(&started.response) {
            Some(done) => (started.login_id, done.finalization, Some(done.session_key)),
            None => (started.login_id, vec![0; 64], None),
        }
    }

    #[tokio::test]
    async fn a_finished_login_clears_the_tries_and_issues_a_ticket_and_the_sealed_share() {
        let w = world();
        let a = w.account_with_password(1, b"pw").await;
        w.passwords
            .insert_rewrap(a.user_id, b"older".to_vec(), Utc::now(), 3)
            .await
            .unwrap();
        w.passwords
            .insert_rewrap(a.user_id, b"newer".to_vec(), Utc::now(), 3)
            .await
            .unwrap();
        w.login_start(a.principal()).await.unwrap();
        let (login_id, finalization, session_key) = client_login(&w, &a, b"pw").await;
        assert_eq!(w.tries(&a), 2);
        let done = w
            .svc
            .login_finish(PasswordLoginFinishCmd {
                login_id,
                credential_finalization: finalization.clone(),
            })
            .await
            .unwrap();
        assert_eq!(w.tries(&a), 0);
        assert_eq!(done.rewraps, vec![b"newer".to_vec(), b"older".to_vec()]);
        let share = TEST_PASSWORD_KEY
            .open_share(
                a.user_id,
                &w.passwords.state().passwords[&a.user_id].share_sealed,
            )
            .unwrap();
        let opened = transport_open(
            &session_key.unwrap(),
            LABEL_SERVER_SHARE,
            &done.server_share,
        )
        .expect("the share opens under the login's transport key");
        assert_eq!(opened.as_slice(), share.as_slice());
        let ticket = w.passwords.state().tickets[&hash_token(&done.ticket)];
        assert_eq!(ticket.0, a.user_id);
        assert!(ticket.1 > Utc::now() + Duration::seconds(100));
        assert_eq!(done.ticket.len(), 43, "32 bytes, unpadded base64url");

        // The login id works once.
        let err = w
            .svc
            .login_finish(PasswordLoginFinishCmd {
                login_id,
                credential_finalization: finalization,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Unauthenticated), "{err:?}");
    }

    #[tokio::test]
    async fn a_wrong_expired_or_made_up_login_is_refused_without_a_ticket() {
        let w = world();
        let a = w.account_with_password(1, b"pw").await;
        let finish = |login_id, credential_finalization| {
            w.svc.login_finish(PasswordLoginFinishCmd {
                login_id,
                credential_finalization,
            })
        };

        let (login_id, finalization, _) = client_login(&w, &a, b"wrong").await;
        let err = finish(login_id, finalization).await.unwrap_err();
        assert!(matches!(err, ServiceError::Unauthenticated), "{err:?}");
        assert_eq!(w.tries(&a), 1, "the try stays counted");

        let (login_id, finalization, _) = client_login(&w, &a, b"pw").await;
        w.advance(Duration::seconds(121));
        let err = finish(login_id, finalization).await.unwrap_err();
        assert!(matches!(err, ServiceError::Unauthenticated), "{err:?}");

        // The id an unknown principal gets names no login.
        let stranger = TestIdentity::with_version(9, StatementVersion::V2);
        let principal = KeyState::from_inception(&stranger.inception())
            .unwrap()
            .principal_id;
        let (flow, request) = Login::start(b"pw");
        let started = w
            .svc
            .login_start(PasswordLoginStartCmd {
                principal_id: principal,
                request,
                remote_ip: IP.to_string(),
            })
            .await
            .unwrap();
        assert!(flow.finish(&started.response).is_none());
        let err = finish(started.login_id, vec![0; 64]).await.unwrap_err();
        assert!(matches!(err, ServiceError::Unauthenticated), "{err:?}");
        assert!(w.passwords.state().tickets.is_empty());
    }
}
