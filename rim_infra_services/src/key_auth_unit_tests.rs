#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use chrono::{DateTime, Duration, Utc};
    use uuid::Uuid;

    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use rim_auth::session_token::hash_token;
    use rim_core::auth::model::{InstanceInvite, InstanceInviteId, Invite, InviteId};
    use rim_core::auth::model::{NewSession, Session, SessionId};
    use rim_core::auth::repository::SessionRepository;
    use rim_core::auth::repository::{MockInstanceInviteRepository, MockInviteRepository};
    use rim_core::conversation::repository::MockConversationRepository;
    use rim_core::conversation_member::repository::MockConversationMemberRepository;
    use rim_core::key_identity::model::{
        DeviceKey, DeviceKeyId, KeyChallenge, KeyChallengeId, KeyChallengePurpose, KeyLogEntry,
        KeyLogEntryType, NewDeviceKey, NewKeyLogEntry, StoredProfileCard,
    };
    use rim_core::key_identity::repository::{KeyChallengeRepository, KeyIdentityRepository};
    use rim_core::moderation::repository::MockModerationRepository;
    use rim_core::repository::error::{RepoError, RepoErrorKind, RepoResult};
    use rim_core::server::model::ServerId;
    use rim_core::server_member::model::{ServerMember, ServerMemberId, ServerMemberRole};
    use rim_core::server_member::repository::MockServerMemberRepository;
    use rim_core::user::model::{NewUser, User, UserId};
    use rim_core::user::repository::MockUserRepository;
    use rim_identity::testing::{
        SigningKey, TestIdentity, device_login, device_self_revoke, signing_key,
    };
    use rim_identity::{KeyState, PublicKey, SignedStatement, StatementVersion};
    use rim_services::auth::command::SignupToken;
    use rim_services::auth::dto::RegistrationMode;
    use rim_services::events::{Broadcaster, Observers, WorkspaceEvent};
    use rim_services::host::dto::IdentityRefresh;
    use rim_services::host::service::MockHostService;
    use rim_services::key_auth::command::{
        AddDeviceKeyCmd, EnrollIdentityKeyCmd, GetKeyLogQuery, GetMyKeyIdentityQuery,
        IssueKeyChallengeCmd, LoginWithKeyCmd, PublishProfileCardCmd, RevokeDeviceKeyCmd,
        RotateIdentityKeyCmd, SignedPayload, SignupWithKeyCmd,
    };
    use rim_services::key_auth::service::KeyAuthService;
    use rim_services::session_revocation::{SessionRevocations, SessionWatch, WatchedSession};
    use rim_services::{Actor, ServiceError};

    use crate::key_auth::{DefaultKeyAuthService, KeyAuthRepos};
    use crate::registration_rate_limiter::RegistrationRateLimiter;
    use crate::session_revocation::InProcessSessionRevocations;
    use crate::signup::SignupRepos;

    const AUDIENCE: &str = "rim.example.com";
    /// This server's id, also accepted as an audience.
    const SERVER_ID: &str = "rims1_OFPDNXTV1splCcKcgz1vWYj6Mg-lbUollHQwXTkYEBw";
    const IP: &str = "203.0.113.7";

    // ── In-memory store ───────────────────────────────────────────────────────────────────

    #[derive(Default)]
    struct State {
        users: HashSet<UserId>,
        principals: HashMap<UserId, String>,
        log: HashMap<UserId, Vec<KeyLogEntry>>,
        devices: Vec<DeviceKey>,
        cards: HashMap<UserId, StoredProfileCard>,
        challenges: Vec<KeyChallenge>,
        sessions: Vec<(Session, Option<DeviceKeyId>)>,
        /// `user.identity_key` as the service last wrote it.
        identity_keys: HashMap<UserId, String>,
        /// `device_key.key_hash` per stored device, in the same order as `devices`.
        device_hashes: Vec<Vec<u8>>,
        /// Users created through `UserRepository::create_user` (signups), as inserted.
        created: HashMap<UserId, NewUser>,
        /// Principals an operator blocked on this server.
        blocked: HashSet<String>,
        /// The terms URL each user accepted, as `record_terms_acceptance` stored it.
        terms: HashMap<UserId, String>,
        /// Principals of accounts deleted on this server.
        retired: HashSet<String>,
        /// Email addresses an operator blocked on this server.
        blocked_emails: HashSet<String>,
    }

    type Shared = Arc<Mutex<State>>;

    /// Keeps every event broadcast, so a test can check what was announced.
    #[derive(Default)]
    struct Recording(Mutex<Vec<WorkspaceEvent>>);

    impl Broadcaster for Recording {
        fn broadcast(&self, event: WorkspaceEvent) {
            self.0.lock().unwrap().push(event);
        }
    }

    fn conflict() -> RepoError {
        RepoError::new(RepoErrorKind::Conflict)
    }

    fn log_entry(user_id: UserId, e: NewKeyLogEntry) -> KeyLogEntry {
        KeyLogEntry {
            user_id,
            seq: e.seq,
            entry_type: e.entry_type,
            payload: e.payload,
            signature: e.signature,
            created_at: Utc::now(),
        }
    }

    struct FakeKeys(Shared);

    #[async_trait]
    impl KeyIdentityRepository for FakeKeys {
        async fn lock_user(&self, user_id: UserId) -> RepoResult<bool> {
            Ok(self.0.lock().unwrap().users.contains(&user_id))
        }

        async fn find_user_by_principal_id(
            &self,
            principal_id: &str,
        ) -> RepoResult<Option<UserId>> {
            Ok(self
                .0
                .lock()
                .unwrap()
                .principals
                .iter()
                .find(|(_, p)| p.as_str() == principal_id)
                .map(|(u, _)| *u))
        }

        async fn principal_retired(&self, principal_id: &str) -> RepoResult<bool> {
            Ok(self.0.lock().unwrap().retired.contains(principal_id))
        }

        async fn list_key_log(&self, user_id: UserId) -> RepoResult<Vec<KeyLogEntry>> {
            Ok(self
                .0
                .lock()
                .unwrap()
                .log
                .get(&user_id)
                .cloned()
                .unwrap_or_default())
        }

        async fn enroll(
            &self,
            user_id: UserId,
            principal_id: &str,
            identity_key: &str,
            entries: Vec<NewKeyLogEntry>,
        ) -> RepoResult<()> {
            let mut s = self.0.lock().unwrap();
            if s.principals.contains_key(&user_id)
                || s.principals.values().any(|p| p == principal_id)
            {
                return Err(conflict());
            }
            s.principals.insert(user_id, principal_id.to_string());
            s.identity_keys.insert(user_id, identity_key.to_string());
            let entries = entries.into_iter().map(|e| log_entry(user_id, e)).collect();
            s.log.insert(user_id, entries);
            Ok(())
        }

        async fn insert_key_log(
            &self,
            user_id: UserId,
            entries: Vec<NewKeyLogEntry>,
        ) -> RepoResult<()> {
            let mut s = self.0.lock().unwrap();
            if s.log.contains_key(&user_id) {
                return Err(conflict());
            }
            let entries = entries.into_iter().map(|e| log_entry(user_id, e)).collect();
            s.log.insert(user_id, entries);
            Ok(())
        }

        async fn append_rotation(
            &self,
            user_id: UserId,
            entry: NewKeyLogEntry,
            identity_key: &str,
        ) -> RepoResult<()> {
            let mut s = self.0.lock().unwrap();
            let log = s.log.entry(user_id).or_default();
            if log.iter().any(|e| e.seq == entry.seq) {
                return Err(conflict());
            }
            log.push(log_entry(user_id, entry));
            s.identity_keys.insert(user_id, identity_key.to_string());
            Ok(())
        }

        async fn add_device(&self, device: NewDeviceKey) -> RepoResult<DeviceKey> {
            let mut s = self.0.lock().unwrap();
            // Mirrors the unique (user_id, device_id) and (user_id, key_hash) constraints.
            if s.devices.iter().zip(&s.device_hashes).any(|(d, hash)| {
                d.user_id == device.user_id
                    && (d.device_id == device.device_id || *hash == device.key_hash)
            }) {
                return Err(conflict());
            }
            s.device_hashes.push(device.key_hash.clone());
            let stored = DeviceKey {
                id: DeviceKeyId::new(),
                user_id: device.user_id,
                device_id: device.device_id,
                public_key: device.public_key,
                name: device.name,
                cert_payload: device.cert_payload,
                cert_signature: device.cert_signature,
                key_seq: device.key_seq,
                added_at: Utc::now(),
                last_used_at: None,
                revoked_at: None,
                revocation_payload: None,
                revocation_signature: None,
            };
            s.devices.push(stored.clone());
            Ok(stored)
        }

        async fn get_device(
            &self,
            user_id: UserId,
            device_id: Uuid,
        ) -> RepoResult<Option<DeviceKey>> {
            Ok(self
                .0
                .lock()
                .unwrap()
                .devices
                .iter()
                .find(|d| d.user_id == user_id && d.device_id == device_id)
                .cloned())
        }

        async fn list_devices(&self, user_id: UserId) -> RepoResult<Vec<DeviceKey>> {
            Ok(self
                .0
                .lock()
                .unwrap()
                .devices
                .iter()
                .filter(|d| d.user_id == user_id)
                .cloned()
                .collect())
        }

        async fn revoke_device(
            &self,
            id: DeviceKeyId,
            revocation_payload: &str,
            revocation_signature: &str,
        ) -> RepoResult<bool> {
            let mut s = self.0.lock().unwrap();
            let Some(d) = s
                .devices
                .iter_mut()
                .find(|d| d.id == id && d.revoked_at.is_none())
            else {
                return Ok(false);
            };
            d.revoked_at = Some(Utc::now());
            d.revocation_payload = Some(revocation_payload.to_string());
            d.revocation_signature = Some(revocation_signature.to_string());
            Ok(true)
        }

        async fn revoke_all_devices(&self, user_id: UserId) -> RepoResult<u64> {
            let mut s = self.0.lock().unwrap();
            let mut n = 0;
            for d in s
                .devices
                .iter_mut()
                .filter(|d| d.user_id == user_id && d.revoked_at.is_none())
            {
                d.revoked_at = Some(Utc::now());
                n += 1;
            }
            Ok(n)
        }

        async fn touch_device(&self, id: DeviceKeyId) -> RepoResult<()> {
            let mut s = self.0.lock().unwrap();
            if let Some(d) = s.devices.iter_mut().find(|d| d.id == id) {
                d.last_used_at = Some(Utc::now());
            }
            Ok(())
        }

        async fn get_profile_card(&self, user_id: UserId) -> RepoResult<Option<StoredProfileCard>> {
            Ok(self.0.lock().unwrap().cards.get(&user_id).cloned())
        }

        async fn upsert_profile_card(
            &self,
            user_id: UserId,
            version: u32,
            payload: &str,
            signature: &str,
        ) -> RepoResult<bool> {
            let mut s = self.0.lock().unwrap();
            if s.cards.get(&user_id).is_some_and(|c| c.version >= version) {
                return Ok(false);
            }
            s.cards.insert(
                user_id,
                StoredProfileCard {
                    user_id,
                    version,
                    payload: payload.to_string(),
                    signature: signature.to_string(),
                    updated_at: Utc::now(),
                },
            );
            Ok(true)
        }
    }

    struct FakeChallenges(Shared);

    #[async_trait]
    impl KeyChallengeRepository for FakeChallenges {
        async fn create(
            &self,
            nonce: &str,
            expires_at: DateTime<Utc>,
            purpose: &KeyChallengePurpose,
        ) -> RepoResult<KeyChallenge> {
            let c = KeyChallenge {
                id: KeyChallengeId::new(),
                nonce: nonce.to_string(),
                purpose: purpose.clone(),
                created_at: Utc::now(),
                expires_at,
                used_at: None,
            };
            self.0.lock().unwrap().challenges.push(c.clone());
            Ok(c)
        }

        async fn consume(
            &self,
            id: KeyChallengeId,
            nonce: &str,
            purpose: &KeyChallengePurpose,
        ) -> RepoResult<bool> {
            let mut s = self.0.lock().unwrap();
            let now = Utc::now();
            let Some(c) = s.challenges.iter_mut().find(|c| {
                c.id == id
                    && c.nonce == nonce
                    && c.purpose == *purpose
                    && c.used_at.is_none()
                    && c.expires_at > now
            }) else {
                return Ok(false);
            };
            c.used_at = Some(now);
            Ok(true)
        }

        async fn delete_expired(&self, before: DateTime<Utc>) -> RepoResult<u64> {
            let mut s = self.0.lock().unwrap();
            let len = s.challenges.len();
            s.challenges.retain(|c| c.expires_at >= before);
            Ok((len - s.challenges.len()) as u64)
        }
    }

    struct FakeSessions(Shared);

    #[async_trait]
    impl SessionRepository for FakeSessions {
        async fn get_by_id(&self, id: SessionId) -> RepoResult<Option<Session>> {
            Ok(self
                .0
                .lock()
                .unwrap()
                .sessions
                .iter()
                .find(|(s, _)| s.id == id)
                .map(|(s, _)| s.clone()))
        }

        async fn get_by_token_hash(&self, token_hash: &str) -> RepoResult<Option<Session>> {
            Ok(self
                .0
                .lock()
                .unwrap()
                .sessions
                .iter()
                .find(|(s, _)| s.token_hash == token_hash)
                .map(|(s, _)| s.clone()))
        }

        async fn get_many(&self, ids: &[SessionId]) -> RepoResult<Vec<Session>> {
            Ok(self
                .0
                .lock()
                .unwrap()
                .sessions
                .iter()
                .filter(|(s, _)| ids.contains(&s.id))
                .map(|(s, _)| s.clone())
                .collect())
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
                user_agent: new.user_agent,
                ip: new.ip,
                device_key_id: None,
            };
            self.0
                .lock()
                .unwrap()
                .sessions
                .push((session.clone(), None));
            Ok(session)
        }

        async fn touch_last_seen_at(&self, _id: SessionId) -> RepoResult<()> {
            Ok(())
        }

        async fn revoke_session(&self, id: SessionId) -> RepoResult<()> {
            let mut s = self.0.lock().unwrap();
            for (session, _) in s.sessions.iter_mut().filter(|(s, _)| s.id == id) {
                session.revoked_at = Some(Utc::now());
            }
            Ok(())
        }

        async fn revoke_all_for_user(&self, user_id: UserId) -> RepoResult<u64> {
            let mut s = self.0.lock().unwrap();
            let mut n = 0;
            for (session, _) in s
                .sessions
                .iter_mut()
                .filter(|(s, _)| s.user_id == user_id && s.revoked_at.is_none())
            {
                session.revoked_at = Some(Utc::now());
                n += 1;
            }
            Ok(n)
        }

        async fn revoke_all_for_user_except(
            &self,
            user_id: UserId,
            keep: SessionId,
        ) -> RepoResult<u64> {
            let mut s = self.0.lock().unwrap();
            let mut n = 0;
            for (session, _) in s
                .sessions
                .iter_mut()
                .filter(|(s, _)| s.user_id == user_id && s.id != keep && s.revoked_at.is_none())
            {
                session.revoked_at = Some(Utc::now());
                n += 1;
            }
            Ok(n)
        }

        async fn bind_device(&self, id: SessionId, device_key_id: DeviceKeyId) -> RepoResult<()> {
            let mut s = self.0.lock().unwrap();
            for (_, device) in s.sessions.iter_mut().filter(|(s, _)| s.id == id) {
                *device = Some(device_key_id);
            }
            Ok(())
        }

        async fn revoke_all_for_device(&self, device_key_id: DeviceKeyId) -> RepoResult<u64> {
            let mut s = self.0.lock().unwrap();
            let mut n = 0;
            for (session, _) in s
                .sessions
                .iter_mut()
                .filter(|(s, d)| *d == Some(device_key_id) && s.revoked_at.is_none())
            {
                session.revoked_at = Some(Utc::now());
                n += 1;
            }
            Ok(n)
        }
    }

    // ── Harness ───────────────────────────────────────────────────────────────────────────

    struct Harness {
        state: Shared,
        svc: DefaultKeyAuthService,
        revocations: Arc<InProcessSessionRevocations>,
    }

    fn user_row(s: &State, id: UserId) -> User {
        let now = Utc::now();
        let principal_id = s.principals.get(&id).cloned();
        if let Some(new) = s.created.get(&id) {
            return User {
                id,
                username: new.username.clone(),
                password_hash: new.password.clone(),
                created_at: now,
                updated_at: now,
                last_login_at: None,
                display_name: new.display_name.clone(),
                avatar_url: None,
                status_text: None,
                bio: None,
                email: new.email.clone(),
                timezone: None,
                principal_id,
                deletion_scheduled_for: None,
                handle: rim_core::user::model::UserHandle::Local,
            };
        }
        User {
            id,
            username: format!("user-{id}"),
            password_hash: Some("hash".to_string()),
            created_at: now,
            updated_at: now,
            last_login_at: None,
            display_name: None,
            avatar_url: None,
            status_text: None,
            bio: None,
            email: Some(format!("{id}@example.com")),
            timezone: None,
            principal_id,
            deletion_scheduled_for: None,
            handle: rim_core::user::model::UserHandle::Local,
        }
    }

    /// Token and workspace repositories that refuse every call: a test that reaches one by
    /// mistake fails loudly.
    fn no_signup_repos() -> SignupRepos {
        SignupRepos {
            instance_invite_repo: Arc::new(MockInstanceInviteRepository::new()),
            invite_repo: Arc::new(MockInviteRepository::new()),
            server_repo: Arc::new(rim_core::server::repository::MockServerRepository::new()),
            server_member_repo: Arc::new(MockServerMemberRepository::new()),
            conversation_repo: Arc::new(MockConversationRepository::new()),
            conversation_member_repo: Arc::new(MockConversationMemberRepository::new()),
            moderation_repo: Arc::new(MockModerationRepository::new()),
            user_repo: Arc::new(MockUserRepository::new()),
            dm_policy_repo: Arc::new(
                rim_core::dm_policy::repository::MockDmPolicyRepository::new(),
            ),
            default_workspace_repo: Arc::new(
                rim_core::default_workspace::repository::MockDefaultWorkspaceRepository::new(),
            ),
        }
    }

    /// The user store behind the harness: lookups by id, username and email, blocked emails, and
    /// inserts that mirror the unique username and principal id indexes.
    fn user_repo(state: &Shared) -> MockUserRepository {
        let mut user_repo = MockUserRepository::new();
        let users = Arc::clone(state);
        user_repo.expect_get_by_id().returning(move |id| {
            let s = users.lock().unwrap();
            Ok(s.users.contains(&id).then(|| user_row(&s, id)))
        });
        let users = Arc::clone(state);
        user_repo.expect_find_by_username().returning(move |name| {
            let s = users.lock().unwrap();
            Ok(s.created
                .iter()
                .find(|(_, u)| u.username == name)
                .map(|(id, _)| user_row(&s, *id)))
        });
        let users = Arc::clone(state);
        user_repo.expect_find_by_email().returning(move |email| {
            let s = users.lock().unwrap();
            Ok(s.created
                .iter()
                .find(|(_, u)| u.email.as_deref() == Some(email))
                .map(|(id, _)| user_row(&s, *id)))
        });
        let users = Arc::clone(state);
        user_repo
            .expect_is_email_blocked()
            .returning(move |email| Ok(users.lock().unwrap().blocked_emails.contains(email)));
        let users = Arc::clone(state);
        user_repo
            .expect_create_user()
            .returning(move |new: NewUser| {
                let mut s = users.lock().unwrap();
                let taken = s.created.values().any(|u| u.username == new.username)
                    || new
                        .key_identity
                        .as_ref()
                        .is_some_and(|k| s.principals.values().any(|p| *p == k.principal_id));
                if taken {
                    return Err(conflict());
                }
                let id = UserId::new();
                s.users.insert(id);
                if let Some(k) = &new.key_identity {
                    s.principals.insert(id, k.principal_id.clone());
                    s.identity_keys.insert(id, k.identity_key.clone());
                }
                s.created.insert(id, new);
                Ok(user_row(&s, id))
            });
        let users = Arc::clone(state);
        user_repo
            .expect_record_terms_acceptance()
            .returning(move |id, url, _| {
                users.lock().unwrap().terms.insert(id, url);
                Ok(())
            });
        user_repo
    }

    /// Everything a harness can vary.
    struct Limits {
        challenge: u32,
        failure: u32,
        signup: u32,
    }

    fn harness_with_limits(challenge_limit: u32, failure_limit: u32) -> Harness {
        harness_with(
            Limits {
                challenge: challenge_limit,
                failure: failure_limit,
                signup: 100,
            },
            no_signup_repos(),
            Arc::new(Recording::default()),
        )
    }

    fn harness_with(
        limits: Limits,
        signup: SignupRepos,
        broadcaster: Arc<dyn Broadcaster>,
    ) -> Harness {
        harness_from(limits, signup, broadcaster, local_users_only())
    }

    /// [`harness_with`] with `host_repo` telling local users from those of other servers.
    fn harness_from(
        limits: Limits,
        signup: SignupRepos,
        broadcaster: Arc<dyn Broadcaster>,
        host_repo: rim_core::host::repository::MockHostRepository,
    ) -> Harness {
        let state: Shared = Arc::default();
        let repos = KeyAuthRepos {
            user_repo: Arc::new(user_repo(&state)),
            session_repo: Arc::new(FakeSessions(Arc::clone(&state))),
            key_identity_repo: Arc::new(FakeKeys(Arc::clone(&state))),
            key_challenge_repo: Arc::new(FakeChallenges(Arc::clone(&state))),
            signup,
            host_repo: Arc::new(host_repo),
            relay_abuse_repo: Arc::new(operator_blocks(&state)),
        };
        let svc = DefaultKeyAuthService::new(
            repos,
            None,
            Duration::days(30),
            vec![" RIM.example.com ".to_string(), SERVER_ID.to_string()],
            Observers::new(broadcaster),
            RegistrationRateLimiter::new(3600, limits.signup),
        )
        .with_rate_limiters(
            RegistrationRateLimiter::new(3600, limits.challenge),
            RegistrationRateLimiter::new(3600, limits.failure),
        );
        let revocations = Arc::new(InProcessSessionRevocations::default());
        let svc = svc.with_session_revocations(Arc::clone(&revocations) as _);
        Harness {
            state,
            svc,
            revocations,
        }
    }

    /// Operator blocks as `State::blocked` holds them.
    fn operator_blocks(state: &Shared) -> rim_core::relay::repository::MockRelayAbuseRepository {
        let state = Arc::clone(state);
        let mut abuse = rim_core::relay::repository::MockRelayAbuseRepository::new();
        abuse.expect_is_principal_blocked().returning(move |p| {
            let blocked = state.lock().unwrap().blocked.contains(p);
            Box::pin(async move { Ok(blocked) })
        });
        abuse
    }

    /// A host repository where every user is local.
    fn local_users_only() -> rim_core::host::repository::MockHostRepository {
        let mut host = rim_core::host::repository::MockHostRepository::new();
        host.expect_user_kind()
            .returning(|_| Box::pin(async { Ok(Some(rim_core::host::model::UserKind::Local)) }));
        host
    }

    fn harness() -> Harness {
        harness_with_limits(100, 100)
    }

    /// A harness where every user is from another server and refetching their identity before
    /// a direct sign-in comes to `refresh`.
    fn remote_harness(refresh: IdentityRefresh) -> Harness {
        let mut host = rim_core::host::repository::MockHostRepository::new();
        host.expect_user_kind()
            .returning(|_| Box::pin(async { Ok(Some(rim_core::host::model::UserKind::Remote)) }));
        let h = harness_from(
            Limits {
                challenge: 100,
                failure: 100,
                signup: 100,
            },
            no_signup_repos(),
            Arc::new(Recording::default()),
            host,
        );
        let mut host_service = MockHostService::new();
        host_service
            .expect_refresh_identity()
            .returning(move |_, _| Box::pin(async move { Ok(refresh) }));
        Harness {
            svc: h.svc.with_host_service(Arc::new(host_service)),
            ..h
        }
    }

    fn payload(s: SignedStatement) -> SignedPayload {
        SignedPayload {
            payload: s.payload,
            signature: s.signature,
        }
    }

    /// A user with a password session (`session`), enrolled with `id` and a first device.
    struct Enrolled {
        user_id: UserId,
        session: SessionId,
        id: TestIdentity,
        state: KeyState,
        device_id: Uuid,
        device: SigningKey,
    }

    impl Harness {
        async fn add_user_with_session(&self) -> (UserId, SessionId) {
            let user_id = UserId::new();
            self.state.lock().unwrap().users.insert(user_id);
            (user_id, self.add_session(user_id).await)
        }

        /// Another password session of `user_id`.
        async fn add_session(&self, user_id: UserId) -> SessionId {
            FakeSessions(Arc::clone(&self.state))
                .create_session(NewSession {
                    user_id,
                    token_hash: format!("password-session-{}", Uuid::now_v7()),
                    expires_at: Utc::now() + Duration::days(1),
                    user_agent: None,
                    ip: None,
                })
                .await
                .unwrap()
                .id
        }

        /// A `login` statement by `device` over a fresh challenge, as enrollment requires.
        async fn proof(
            &self,
            device: &SigningKey,
            state: &KeyState,
            device_id: Uuid,
        ) -> SignedPayload {
            self.proof_for(device, state, device_id, AUDIENCE).await
        }

        async fn proof_for(
            &self,
            device: &SigningKey,
            state: &KeyState,
            device_id: Uuid,
            audience: &str,
        ) -> SignedPayload {
            let challenge = self
                .svc
                .issue_challenge(IssueKeyChallengeCmd {
                    remote_ip: IP.to_string(),
                })
                .await
                .unwrap();
            payload(device_login(
                device,
                state,
                device_id,
                challenge.challenge_id.into(),
                &challenge.nonce,
                audience,
            ))
        }

        /// A `login` statement by `device`, naming this server's id, over a challenge this
        /// server issued for a join through `relay`, as a relay's join request carries it.
        async fn join_proof(
            &self,
            device: &SigningKey,
            state: &KeyState,
            device_id: Uuid,
            relay: &str,
        ) -> SignedPayload {
            let challenge = FakeChallenges(Arc::clone(&self.state))
                .create(
                    "am9pbi1ub25jZQ",
                    Utc::now() + Duration::minutes(5),
                    &KeyChallengePurpose::Join {
                        relay_server_id: relay.to_string(),
                    },
                )
                .await
                .unwrap();
            payload(device_login(
                device,
                state,
                device_id,
                challenge.id.into(),
                &challenge.nonce,
                SERVER_ID,
            ))
        }

        async fn enroll(&self, seed: u8) -> Enrolled {
            self.enroll_with(seed, StatementVersion::V1).await
        }

        /// Like [`Harness::enroll`], with every statement written at `version`.
        async fn enroll_with(&self, seed: u8, version: StatementVersion) -> Enrolled {
            let (user_id, session) = self.add_user_with_session().await;
            self.enroll_from(user_id, session, seed, version).await
        }

        /// Enroll `user_id` from its password session `session` with the identity from `seed`
        /// and a first device, every statement written at `version`.
        async fn enroll_from(
            &self,
            user_id: UserId,
            session: SessionId,
            seed: u8,
            version: StatementVersion,
        ) -> Enrolled {
            let id = TestIdentity::with_version(seed, version);
            let inception = id.inception();
            let state = KeyState::from_inception(&inception).unwrap();
            let device_id = Uuid::now_v7();
            let device = signing_key(seed.wrapping_add(50));
            let cert = id.device_cert(&state, device_id, &device, "Laptop");
            let proof = self.proof(&device, &state, device_id).await;
            let dto = self
                .svc
                .enroll(EnrollIdentityKeyCmd {
                    actor: Actor::new(user_id),
                    session_id: session,
                    key_log: vec![payload(inception)],
                    device: payload(cert),
                    proof,
                })
                .await
                .expect("enroll");
            assert_eq!(
                dto.principal_id.as_deref(),
                Some(state.principal_id.as_str())
            );
            Enrolled {
                user_id,
                session,
                id,
                state,
                device_id,
                device,
            }
        }

        async fn login(
            &self,
            e: &Enrolled,
            device_id: Uuid,
            device: &SigningKey,
            audience: &str,
        ) -> Result<SessionId, ServiceError> {
            let challenge = self
                .svc
                .issue_challenge(IssueKeyChallengeCmd {
                    remote_ip: IP.to_string(),
                })
                .await?;
            let proof = device_login(
                device,
                &e.state,
                device_id,
                challenge.challenge_id.into(),
                &challenge.nonce,
                audience,
            );
            let res = self
                .svc
                .login_with_key(LoginWithKeyCmd {
                    proof: payload(proof),
                    remote_ip: IP.to_string(),
                })
                .await?;
            assert_eq!(res.user_id, e.user_id);
            Ok(res.session.session_id)
        }

        fn session_device(&self, id: SessionId) -> Option<DeviceKeyId> {
            self.state
                .lock()
                .unwrap()
                .sessions
                .iter()
                .find(|(s, _)| s.id == id)
                .and_then(|(_, d)| *d)
        }

        fn session_revoked(&self, id: SessionId) -> bool {
            self.state
                .lock()
                .unwrap()
                .sessions
                .iter()
                .any(|(s, _)| s.id == id && s.revoked_at.is_some())
        }

        /// A live connection of session `id`, watching it as the WebSocket layer does.
        fn connection(&self, id: SessionId) -> SessionWatch {
            let state = self.state.lock().unwrap();
            let (session, device) = state
                .sessions
                .iter()
                .find(|(s, _)| s.id == id)
                .expect("a stored session");
            self.revocations.watch(WatchedSession {
                session_id: id,
                user_id: session.user_id,
                device_key_id: device.or(session.device_key_id),
            })
        }
    }

    // ── Enrollment ────────────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn enroll_binds_the_enrolling_session_to_the_device() {
        let h = harness();
        let e = h.enroll(1).await;

        let identity = h
            .svc
            .get_my_key_identity(GetMyKeyIdentityQuery {
                actor: Actor::new(e.user_id),
            })
            .await
            .unwrap();
        assert_eq!(identity.key_seq, Some(0));
        assert_eq!(identity.key_log.len(), 1);
        assert_eq!(identity.devices.len(), 1);
        assert_eq!(identity.devices[0].device_id, e.device_id);
        assert!(h.session_device(e.session).is_some());
    }

    #[tokio::test]
    async fn enroll_ends_every_other_session_of_the_account() {
        let h = harness();
        let (user_id, enrolling) = h.add_user_with_session().await;
        let other = h.add_session(user_id).await;
        let (_, stranger) = h.add_user_with_session().await;
        let mut connections = [h.connection(other), h.connection(stranger)];

        let e = h
            .enroll_from(user_id, enrolling, 1, StatementVersion::V2)
            .await;

        assert!(h.session_revoked(other));
        assert!(connections[0].has_ended());
        assert!(!h.session_revoked(e.session));
        assert!(h.session_device(e.session).is_some());
        assert!(!h.session_revoked(stranger));
        assert!(!connections[1].has_ended());
    }

    #[tokio::test]
    async fn enroll_accepts_a_log_with_rotations() {
        let h = harness();
        let (user_id, session) = h.add_user_with_session().await;
        let mut id = TestIdentity::new(1);
        let inception = id.inception();
        let s0 = KeyState::from_inception(&inception).unwrap();
        let rotation = id.rotate_with_identity(&s0, 2);
        let (s1, _) = s0.apply(&rotation).unwrap();
        let device = signing_key(30);
        let device_id = Uuid::now_v7();
        let cert = id.device_cert(&s1, device_id, &device, "Phone");

        let dto = h
            .svc
            .enroll(EnrollIdentityKeyCmd {
                actor: Actor::new(user_id),
                session_id: session,
                key_log: vec![payload(inception), payload(rotation)],
                device: payload(cert),
                proof: h.proof(&device, &s1, device_id).await,
            })
            .await
            .unwrap();
        assert_eq!(dto.key_seq, Some(1));
        assert_eq!(dto.devices[0].key_seq, 1);
    }

    #[tokio::test]
    async fn get_my_key_identity_is_empty_before_enrollment() {
        let h = harness();
        let (user_id, _) = h.add_user_with_session().await;
        let dto = h
            .svc
            .get_my_key_identity(GetMyKeyIdentityQuery {
                actor: Actor::new(user_id),
            })
            .await
            .unwrap();
        assert_eq!(dto.principal_id, None);
        assert!(dto.key_log.is_empty() && dto.devices.is_empty());
    }

    #[tokio::test]
    async fn key_log_is_public_by_principal_id() {
        let h = harness();
        let e = h.enroll(1).await;

        let log = h
            .svc
            .get_key_log(GetKeyLogQuery {
                principal_id: e.state.principal_id.clone(),
            })
            .await
            .unwrap();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].seq, 0);

        let unknown = h
            .svc
            .get_key_log(GetKeyLogQuery {
                principal_id: "rimp1_unknown".to_string(),
            })
            .await
            .unwrap();
        assert!(unknown.is_empty());
        // Too long to be a principal id: not looked up at all.
        let oversized = h
            .svc
            .get_key_log(GetKeyLogQuery {
                principal_id: format!("{}{}", e.state.principal_id, "x".repeat(64)),
            })
            .await
            .unwrap();
        assert!(oversized.is_empty());
    }

    #[tokio::test]
    async fn enroll_twice_or_with_a_taken_principal_conflicts() {
        let h = harness();
        let e = h.enroll(1).await;

        // Same account, a second identity.
        let other = TestIdentity::new(2);
        let inception = other.inception();
        let state = KeyState::from_inception(&inception).unwrap();
        let (device_id, device) = (Uuid::now_v7(), signing_key(60));
        let cert = other.device_cert(&state, device_id, &device, "Other");
        let err = h
            .svc
            .enroll(EnrollIdentityKeyCmd {
                actor: Actor::new(e.user_id),
                session_id: e.session,
                key_log: vec![payload(inception)],
                device: payload(cert),
                proof: h.proof(&device, &state, device_id).await,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Conflict(_)), "{err:?}");

        // Another account, the same identity.
        let (user_id, session) = h.add_user_with_session().await;
        let (device_id, device) = (Uuid::now_v7(), signing_key(61));
        let cert = e.id.device_cert(&e.state, device_id, &device, "Other");
        let err = h
            .svc
            .enroll(EnrollIdentityKeyCmd {
                actor: Actor::new(user_id),
                session_id: session,
                key_log: vec![payload(e.id.inception())],
                device: payload(cert),
                proof: h.proof(&device, &e.state, device_id).await,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Conflict(_)), "{err:?}");
    }

    #[tokio::test]
    async fn enroll_rejects_a_certificate_from_another_identity() {
        let h = harness();
        let (user_id, session) = h.add_user_with_session().await;
        let id = TestIdentity::new(1);
        let state = KeyState::from_inception(&id.inception()).unwrap();
        let stranger = TestIdentity::new(2);
        let (device_id, device) = (Uuid::now_v7(), signing_key(60));
        let cert = stranger.device_cert(&state, device_id, &device, "Laptop");

        let err = h
            .svc
            .enroll(EnrollIdentityKeyCmd {
                actor: Actor::new(user_id),
                session_id: session,
                key_log: vec![payload(id.inception())],
                device: payload(cert),
                proof: h.proof(&device, &state, device_id).await,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)), "{err:?}");
        assert!(h.state.lock().unwrap().principals.is_empty());
    }

    /// A copied log and certificate, as another server would hold them, without the device's
    /// own proof over a challenge from this server.
    #[tokio::test]
    async fn enroll_needs_a_fresh_proof_from_the_enrolling_device() {
        let h = harness();
        let (user_id, session) = h.add_user_with_session().await;
        let id = TestIdentity::new(1);
        let inception = id.inception();
        let state = KeyState::from_inception(&inception).unwrap();
        let (device_id, device) = (Uuid::now_v7(), signing_key(60));
        let cert = id.device_cert(&state, device_id, &device, "Laptop");
        let enroll = |proof: SignedPayload| EnrollIdentityKeyCmd {
            actor: Actor::new(user_id),
            session_id: session,
            key_log: vec![payload(inception.clone())],
            device: payload(cert.clone()),
            proof,
        };

        let other_key = h.proof(&signing_key(61), &state, device_id).await;
        let other_device = h.proof(&device, &state, Uuid::now_v7()).await;
        let other_server = h
            .proof_for(&device, &state, device_id, "other.example.com")
            .await;
        for bad in [other_key, other_device, other_server] {
            let err = h.svc.enroll(enroll(bad)).await.unwrap_err();
            assert!(matches!(err, ServiceError::Validation(_)), "{err:?}");
        }
        // A proof whose challenge was never issued here.
        let forged = payload(device_login(
            &device,
            &state,
            device_id,
            Uuid::now_v7(),
            "bm9uY2U",
            AUDIENCE,
        ));
        let err = h.svc.enroll(enroll(forged)).await.unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)), "{err:?}");
        assert!(h.state.lock().unwrap().principals.is_empty());

        // A proof over a challenge issued for joining through a relay.
        let joining = h
            .join_proof(&device, &state, device_id, "rims1_relay")
            .await;
        let err = h.svc.enroll(enroll(joining)).await.unwrap_err();
        assert!(
            matches!(&err, ServiceError::Validation(m) if m.contains("challenge")),
            "{err:?}"
        );
        assert!(h.state.lock().unwrap().principals.is_empty());

        let good = h.proof(&device, &state, device_id).await;
        h.svc.enroll(enroll(good.clone())).await.unwrap();
        // The challenge is spent.
        let (user_id, session) = h.add_user_with_session().await;
        let err = h
            .svc
            .enroll(EnrollIdentityKeyCmd {
                actor: Actor::new(user_id),
                session_id: session,
                ..enroll(good)
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)), "{err:?}");
    }

    #[tokio::test]
    async fn enroll_rejects_an_oversized_key_log() {
        let h = harness();
        let (user_id, session) = h.add_user_with_session().await;
        let id = TestIdentity::new(1);
        let state = KeyState::from_inception(&id.inception()).unwrap();
        let (device_id, device) = (Uuid::now_v7(), signing_key(60));
        let err = h
            .svc
            .enroll(EnrollIdentityKeyCmd {
                actor: Actor::new(user_id),
                session_id: session,
                key_log: vec![payload(id.inception()); 257],
                device: payload(id.device_cert(&state, device_id, &device, "Laptop")),
                proof: h.proof(&device, &state, device_id).await,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)), "{err:?}");
    }

    // ── Login ─────────────────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn key_login_issues_a_session_bound_to_the_device() {
        let h = harness();
        let e = h.enroll(1).await;

        let session = h
            .login(&e, e.device_id, &e.device, "rim.example.com")
            .await
            .unwrap();
        let device_key_id = h.session_device(session).expect("session bound to device");
        let s = h.state.lock().unwrap();
        let device = s.devices.iter().find(|d| d.id == device_key_id).unwrap();
        assert_eq!(device.device_id, e.device_id);
        assert!(device.last_used_at.is_some());
    }

    #[tokio::test]
    async fn key_login_audience_is_case_insensitive_and_must_match() {
        let h = harness();
        let e = h.enroll(1).await;
        assert!(
            h.login(&e, e.device_id, &e.device, "RIM.Example.COM")
                .await
                .is_ok()
        );

        let err = h
            .login(&e, e.device_id, &e.device, "evil.example.com")
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Unauthenticated), "{err:?}");
    }

    #[tokio::test]
    async fn key_login_audience_may_be_the_server_id_compared_exactly() {
        let h = harness();
        let e = h.enroll(1).await;
        assert!(h.login(&e, e.device_id, &e.device, SERVER_ID).await.is_ok());
        for other in [
            SERVER_ID.to_ascii_lowercase(),
            SERVER_ID.to_ascii_uppercase(),
            "rims1_T5LLFxkrobgyvlf3WVxuvLlfqYSZAssRFgmvCfI2R-c".to_string(),
        ] {
            let err = h
                .login(&e, e.device_id, &e.device, &other)
                .await
                .unwrap_err();
            assert!(
                matches!(err, ServiceError::Unauthenticated),
                "{other}: {err:?}"
            );
        }
    }

    #[tokio::test]
    async fn key_login_challenge_is_single_use() {
        let h = harness();
        let e = h.enroll(1).await;
        let challenge = h
            .svc
            .issue_challenge(IssueKeyChallengeCmd {
                remote_ip: IP.to_string(),
            })
            .await
            .unwrap();
        let proof = payload(device_login(
            &e.device,
            &e.state,
            e.device_id,
            challenge.challenge_id.into(),
            &challenge.nonce,
            AUDIENCE,
        ));
        let cmd = || LoginWithKeyCmd {
            proof: proof.clone(),
            remote_ip: IP.to_string(),
        };
        assert!(h.svc.login_with_key(cmd()).await.is_ok());
        let err = h.svc.login_with_key(cmd()).await.unwrap_err();
        assert!(matches!(err, ServiceError::Unauthenticated), "{err:?}");
    }

    #[tokio::test]
    async fn key_login_refuses_a_proof_over_a_join_challenge() {
        let h = harness();
        let e = h.enroll(1).await;
        let proof = h
            .join_proof(&e.device, &e.state, e.device_id, "rims1_relay")
            .await;
        let err = h
            .svc
            .login_with_key(LoginWithKeyCmd {
                proof,
                remote_ip: IP.to_string(),
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Unauthenticated), "{err:?}");
        assert!(
            h.state
                .lock()
                .unwrap()
                .challenges
                .iter()
                .all(|c| c.used_at.is_none() || c.purpose == KeyChallengePurpose::Login),
            "the join challenge stays unused"
        );
    }

    #[tokio::test]
    async fn key_login_with_a_wrong_nonce_fails() {
        let h = harness();
        let e = h.enroll(1).await;
        let challenge = h
            .svc
            .issue_challenge(IssueKeyChallengeCmd {
                remote_ip: IP.to_string(),
            })
            .await
            .unwrap();
        let proof = device_login(
            &e.device,
            &e.state,
            e.device_id,
            challenge.challenge_id.into(),
            "bm90LXRoZS1ub25jZQ",
            AUDIENCE,
        );
        let err = h
            .svc
            .login_with_key(LoginWithKeyCmd {
                proof: payload(proof),
                remote_ip: IP.to_string(),
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Unauthenticated), "{err:?}");
    }

    #[tokio::test]
    async fn key_login_signed_by_another_key_fails() {
        let h = harness();
        let e = h.enroll(1).await;
        // The enrolled device id, signed by a key that is not that device's.
        let err = h
            .login(&e, e.device_id, &signing_key(99), AUDIENCE)
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Unauthenticated), "{err:?}");
        // An unknown device id.
        let err = h
            .login(&e, Uuid::now_v7(), &e.device, AUDIENCE)
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Unauthenticated), "{err:?}");
    }

    #[tokio::test]
    async fn key_login_for_an_unknown_principal_fails() {
        let h = harness();
        let e = h.enroll(1).await;
        let stranger = TestIdentity::new(7);
        let state = KeyState::from_inception(&stranger.inception()).unwrap();
        let ghost = Enrolled {
            state,
            id: stranger,
            ..e
        };
        let err = h
            .login(&ghost, ghost.device_id, &ghost.device, AUDIENCE)
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Unauthenticated), "{err:?}");
    }

    #[tokio::test]
    async fn failed_key_logins_are_rate_limited_and_successes_are_not_counted() {
        let h = harness_with_limits(100, 2);
        let e = h.enroll(1).await;
        for _ in 0..3 {
            h.login(&e, e.device_id, &e.device, AUDIENCE).await.unwrap();
        }
        for _ in 0..2 {
            let err = h
                .login(&e, e.device_id, &e.device, "elsewhere")
                .await
                .unwrap_err();
            assert!(matches!(err, ServiceError::Unauthenticated), "{err:?}");
        }
        let err = h
            .login(&e, e.device_id, &e.device, AUDIENCE)
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::RateLimited { .. }), "{err:?}");
    }

    #[test]
    fn refused_challenges_and_proofs_of_work_are_counted() {
        use crate::metrics::test_support::{block_on, recorded};
        use crate::metrics::{POW_REJECTIONS, RATE_LIMIT_REJECTIONS};

        let ((), got) = recorded(|| {
            block_on(async {
                let h = harness_with_limits(1, 100);
                let cmd = || IssueKeyChallengeCmd {
                    remote_ip: IP.to_string(),
                };
                assert!(h.svc.issue_challenge(cmd()).await.is_ok());
                assert!(h.svc.issue_challenge(cmd()).await.is_err());

                let h = relay_harness(100, crate::quota::Quotas::default());
                let p = h.present(1).await;
                assert!(
                    h.svc
                        .signup_with_key(pow_cmd(&p, "ada", 16, "AAAAAAAAkaI"))
                        .await
                        .is_err()
                );
            })
        });
        assert_eq!(
            got.counter(RATE_LIMIT_REJECTIONS, &[("limiter", "key_challenge")]),
            1
        );
        assert_eq!(got.counter(POW_REJECTIONS, &[]), 1);
    }

    #[tokio::test]
    async fn challenges_are_rate_limited_per_ip() {
        let h = harness_with_limits(2, 100);
        let cmd = || IssueKeyChallengeCmd {
            remote_ip: IP.to_string(),
        };
        assert!(h.svc.issue_challenge(cmd()).await.is_ok());
        assert!(h.svc.issue_challenge(cmd()).await.is_ok());
        let err = h.svc.issue_challenge(cmd()).await.unwrap_err();
        assert!(matches!(err, ServiceError::RateLimited { .. }), "{err:?}");
    }

    // ── Devices ───────────────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn added_device_can_sign_in_and_duplicates_conflict() {
        let h = harness();
        let e = h.enroll(1).await;
        let phone_id = Uuid::now_v7();
        let phone = signing_key(70);
        let cert = e.id.device_cert(&e.state, phone_id, &phone, "Phone");
        let dto = h
            .svc
            .add_device(AddDeviceKeyCmd {
                actor: Actor::new(e.user_id),
                device: payload(cert.clone()),
            })
            .await
            .unwrap();
        assert_eq!(dto.name, "Phone");
        assert!(h.login(&e, phone_id, &phone, AUDIENCE).await.is_ok());

        let err = h
            .svc
            .add_device(AddDeviceKeyCmd {
                actor: Actor::new(e.user_id),
                device: payload(cert),
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Conflict(_)), "{err:?}");
    }

    #[tokio::test]
    async fn add_device_before_enrollment_is_refused() {
        let h = harness();
        let (user_id, _) = h.add_user_with_session().await;
        let id = TestIdentity::new(1);
        let state = KeyState::from_inception(&id.inception()).unwrap();
        let cert = id.device_cert(&state, Uuid::now_v7(), &signing_key(70), "Phone");
        let err = h
            .svc
            .add_device(AddDeviceKeyCmd {
                actor: Actor::new(user_id),
                device: payload(cert),
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)), "{err:?}");
    }

    #[tokio::test]
    async fn revoking_a_device_ends_its_sessions_and_its_logins() {
        let h = harness();
        let e = h.enroll(1).await;
        let key_session = h.login(&e, e.device_id, &e.device, AUDIENCE).await.unwrap();
        let mut connections = [h.connection(key_session), h.connection(e.session)];

        let revocation = e.id.revoke_device(&e.state, e.device_id);
        let dto = h
            .svc
            .revoke_device(RevokeDeviceKeyCmd {
                actor: Actor::new(e.user_id),
                revocation: payload(revocation.clone()),
            })
            .await
            .unwrap();
        assert!(dto.revoked_at.is_some());
        assert!(h.session_revoked(key_session));
        // The enrolling password session was bound to the device too.
        assert!(h.session_revoked(e.session));
        assert!(connections.iter_mut().all(SessionWatch::has_ended));

        let err = h
            .login(&e, e.device_id, &e.device, AUDIENCE)
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::PermissionDenied), "{err:?}");

        // Revoking again is idempotent and keeps the first revocation.
        let again = h
            .svc
            .revoke_device(RevokeDeviceKeyCmd {
                actor: Actor::new(e.user_id),
                revocation: payload(revocation),
            })
            .await
            .unwrap();
        assert_eq!(again.revoked_at, dto.revoked_at);
    }

    #[tokio::test]
    async fn revocation_is_disclosed_only_after_the_challenge_and_audience() {
        let h = harness();
        let e = h.enroll(1).await;
        let revocation = e.id.revoke_device(&e.state, e.device_id);
        h.svc
            .revoke_device(RevokeDeviceKeyCmd {
                actor: Actor::new(e.user_id),
                revocation: payload(revocation),
            })
            .await
            .unwrap();

        // Another server's audience.
        let err = h
            .login(&e, e.device_id, &e.device, "evil.example.com")
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Unauthenticated), "{err:?}");

        // A challenge that was never issued.
        let unknown = payload(device_login(
            &e.device,
            &e.state,
            e.device_id,
            Uuid::now_v7(),
            "bm90LXRoZS1ub25jZQ",
            AUDIENCE,
        ));
        let cmd = |proof: &SignedPayload| LoginWithKeyCmd {
            proof: proof.clone(),
            remote_ip: IP.to_string(),
        };
        let err = h.svc.login_with_key(cmd(&unknown)).await.unwrap_err();
        assert!(matches!(err, ServiceError::Unauthenticated), "{err:?}");

        // A valid proof is told about the revocation once; replaying it is not.
        let challenge = h
            .svc
            .issue_challenge(IssueKeyChallengeCmd {
                remote_ip: IP.to_string(),
            })
            .await
            .unwrap();
        let valid = payload(device_login(
            &e.device,
            &e.state,
            e.device_id,
            challenge.challenge_id.into(),
            &challenge.nonce,
            AUDIENCE,
        ));
        let err = h.svc.login_with_key(cmd(&valid)).await.unwrap_err();
        assert!(matches!(err, ServiceError::PermissionDenied), "{err:?}");
        let err = h.svc.login_with_key(cmd(&valid)).await.unwrap_err();
        assert!(matches!(err, ServiceError::Unauthenticated), "{err:?}");
    }

    #[tokio::test]
    async fn a_device_can_revoke_itself_but_not_another() {
        let h = harness();
        let e = h.enroll(1).await;
        let phone_id = Uuid::now_v7();
        let phone = signing_key(70);
        h.svc
            .add_device(AddDeviceKeyCmd {
                actor: Actor::new(e.user_id),
                device: payload(e.id.device_cert(&e.state, phone_id, &phone, "Phone")),
            })
            .await
            .unwrap();

        // The phone signs a self-revocation naming the laptop.
        let err = h
            .svc
            .revoke_device(RevokeDeviceKeyCmd {
                actor: Actor::new(e.user_id),
                revocation: payload(device_self_revoke(&phone, &e.state, e.device_id)),
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)), "{err:?}");

        let dto = h
            .svc
            .revoke_device(RevokeDeviceKeyCmd {
                actor: Actor::new(e.user_id),
                revocation: payload(device_self_revoke(&phone, &e.state, phone_id)),
            })
            .await
            .unwrap();
        assert!(dto.revoked_at.is_some());
    }

    #[tokio::test]
    async fn another_account_cannot_revoke_my_device() {
        let h = harness();
        let e = h.enroll(1).await;
        let other = h.enroll(2).await;
        let err = h
            .svc
            .revoke_device(RevokeDeviceKeyCmd {
                actor: Actor::new(other.user_id),
                revocation: payload(e.id.revoke_device(&e.state, e.device_id)),
            })
            .await
            .unwrap_err();
        // The other account has no device with this id.
        assert!(matches!(err, ServiceError::NotFound), "{err:?}");
    }

    // ── Rotation ──────────────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn identity_rotation_needs_the_session_and_keeps_devices() {
        let h = harness();
        let mut e = h.enroll(1).await;
        let rotation = e.id.rotate_with_identity(&e.state, 3);

        let err = h
            .svc
            .rotate(RotateIdentityKeyCmd {
                actor: None,
                entry: payload(rotation.clone()),
                device: None,
                remote_ip: IP.to_string(),
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Unauthenticated), "{err:?}");

        let other = h.enroll(2).await;
        let err = h
            .svc
            .rotate(RotateIdentityKeyCmd {
                actor: Some(Actor::new(other.user_id)),
                entry: payload(rotation.clone()),
                device: None,
                remote_ip: IP.to_string(),
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::PermissionDenied), "{err:?}");

        let dto = h
            .svc
            .rotate(RotateIdentityKeyCmd {
                actor: Some(Actor::new(e.user_id)),
                entry: payload(rotation.clone()),
                device: None,
                remote_ip: IP.to_string(),
            })
            .await
            .unwrap();
        assert_eq!(dto.key_seq, Some(1));
        assert!(dto.devices.iter().all(|d| d.revoked_at.is_none()));
        // The old device still signs in.
        e.state = e.state.apply(&rotation).unwrap().0;
        assert!(h.login(&e, e.device_id, &e.device, AUDIENCE).await.is_ok());

        // The same entry again: the log moved on.
        let err = h
            .svc
            .rotate(RotateIdentityKeyCmd {
                actor: Some(Actor::new(e.user_id)),
                entry: payload(rotation),
                device: None,
                remote_ip: IP.to_string(),
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)), "{err:?}");
    }

    #[tokio::test]
    async fn identity_rotations_stop_at_the_key_log_cap_and_recovery_does_not() {
        let h = harness();
        let mut e = h.enroll(1).await;
        let rotate = |entry: SignedStatement, actor: Option<Actor>| RotateIdentityKeyCmd {
            actor,
            entry: payload(entry),
            device: None,
            remote_ip: IP.to_string(),
        };
        // Entries 1 to 255, written straight to the store: the log reaches 256 entries.
        for n in 1..256u32 {
            let entry =
                e.id.rotate_with_identity(&e.state, if n % 2 == 0 { 3 } else { 4 });
            e.state = e.state.apply(&entry).unwrap().0;
            let mut s = h.state.lock().unwrap();
            let entry = log_entry(
                e.user_id,
                NewKeyLogEntry {
                    seq: n,
                    entry_type: KeyLogEntryType::Rotate,
                    payload: entry.payload,
                    signature: entry.signature,
                },
            );
            s.log.get_mut(&e.user_id).unwrap().push(entry);
        }
        let entry = e.id.rotate_with_identity(&e.state, 5);
        let err = h
            .svc
            .rotate(rotate(entry, Some(Actor::new(e.user_id))))
            .await
            .unwrap_err();
        assert!(
            matches!(&err, ServiceError::Validation(m) if m.contains("at most 256 entries")),
            "{err:?}"
        );

        // The person can still get back in with the recovery key.
        let recovery = e.id.recover(&e.state, 6);
        let next = e.state.apply(&recovery).unwrap().0;
        let (device_id, device) = (Uuid::now_v7(), signing_key(70));
        let cert = e.id.device_cert(&next, device_id, &device, "New");
        let dto = h
            .svc
            .rotate(RotateIdentityKeyCmd {
                device: Some(payload(cert)),
                ..rotate(recovery, None)
            })
            .await
            .unwrap();
        assert_eq!(dto.key_seq, Some(256));
    }

    #[tokio::test]
    async fn recovery_without_a_session_revokes_everything_and_enrolls_the_new_device() {
        let h = harness();
        let mut e = h.enroll(1).await;
        let key_session = h.login(&e, e.device_id, &e.device, AUDIENCE).await.unwrap();
        let mut connections = [h.connection(key_session), h.connection(e.session)];
        let recovery = e.id.recover(&e.state, 4);
        let (next, _) = e.state.apply(&recovery).unwrap();

        let err = h
            .svc
            .rotate(RotateIdentityKeyCmd {
                actor: None,
                entry: payload(recovery.clone()),
                device: None,
                remote_ip: IP.to_string(),
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)), "{err:?}");

        let new_device_id = Uuid::now_v7();
        let new_device = signing_key(80);
        let cert =
            e.id.device_cert(&next, new_device_id, &new_device, "New laptop");
        let dto = h
            .svc
            .rotate(RotateIdentityKeyCmd {
                actor: None,
                entry: payload(recovery),
                device: Some(payload(cert)),
                remote_ip: IP.to_string(),
            })
            .await
            .unwrap();
        assert_eq!(dto.key_seq, Some(1));
        assert!(h.session_revoked(key_session));
        assert!(h.session_revoked(e.session));
        assert!(connections.iter_mut().all(SessionWatch::has_ended));
        let old = dto
            .devices
            .iter()
            .find(|d| d.device_id == e.device_id)
            .unwrap();
        assert!(old.revoked_at.is_some());
        let new = dto
            .devices
            .iter()
            .find(|d| d.device_id == new_device_id)
            .unwrap();
        assert!(new.revoked_at.is_none());

        e.state = next;
        assert!(
            h.login(&e, new_device_id, &new_device, AUDIENCE)
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn rotations_and_recoveries_push_the_identity_to_hosts() {
        let mut h = harness();
        let mut e = h.enroll(1).await;
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut relay = rim_services::relay::service::MockRelayService::new();
        relay.expect_identity_changed().returning(move |user_id| {
            let _ = tx.send(user_id);
            Box::pin(async { Ok(()) })
        });
        h.svc = h.svc.with_relay_service(Arc::new(relay));

        let rotation = e.id.rotate_with_identity(&e.state, 3);
        e.state = e.state.apply(&rotation).unwrap().0;
        h.svc
            .rotate(RotateIdentityKeyCmd {
                actor: Some(Actor::new(e.user_id)),
                entry: payload(rotation),
                device: None,
                remote_ip: IP.to_string(),
            })
            .await
            .unwrap();
        let pushed = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv()).await;
        assert_eq!(pushed.expect("an identity push"), Some(e.user_id));

        let recovery = e.id.recover(&e.state, 4);
        let next = e.state.apply(&recovery).unwrap().0;
        let cert =
            e.id.device_cert(&next, Uuid::now_v7(), &signing_key(80), "New");
        h.svc
            .rotate(RotateIdentityKeyCmd {
                actor: None,
                entry: payload(recovery),
                device: Some(payload(cert)),
                remote_ip: IP.to_string(),
            })
            .await
            .unwrap();
        let pushed = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv()).await;
        assert_eq!(pushed.expect("an identity push"), Some(e.user_id));
    }

    #[tokio::test]
    async fn anonymous_recovery_for_an_unknown_principal_is_unauthenticated() {
        let h = harness();
        let mut stranger = TestIdentity::new(5);
        let state = KeyState::from_inception(&stranger.inception()).unwrap();
        let err = h
            .svc
            .rotate(RotateIdentityKeyCmd {
                actor: None,
                entry: payload(stranger.recover(&state, 6)),
                device: None,
                remote_ip: IP.to_string(),
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Unauthenticated), "{err:?}");
    }

    #[tokio::test]
    async fn anonymous_recovery_attempts_are_rate_limited() {
        let h = harness_with_limits(100, 2);
        let mut e = h.enroll(1).await;
        let recovery = e.id.recover(&e.state, 4);
        for _ in 0..2 {
            let err = h
                .svc
                .rotate(RotateIdentityKeyCmd {
                    actor: None,
                    entry: payload(recovery.clone()),
                    device: None,
                    remote_ip: IP.to_string(),
                })
                .await
                .unwrap_err();
            assert!(matches!(err, ServiceError::Validation(_)), "{err:?}");
        }
        let err = h
            .svc
            .rotate(RotateIdentityKeyCmd {
                actor: None,
                entry: payload(recovery),
                device: None,
                remote_ip: IP.to_string(),
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::RateLimited { .. }), "{err:?}");
    }

    // ── Profile card ──────────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn profile_card_replaces_only_older_versions() {
        let h = harness();
        let e = h.enroll(1).await;
        let publish = |version: u64, name: &str| PublishProfileCardCmd {
            actor: Actor::new(e.user_id),
            card: payload(e.id.profile_card(
                &e.state,
                version,
                name,
                &["https://relay.example.org"],
                &[(e.device_id, &e.device)],
            )),
        };

        let first = h.svc.publish_profile_card(publish(2, "Ada")).await.unwrap();
        assert_eq!(first.version, 2);

        for stale in [1, 2] {
            let err = h
                .svc
                .publish_profile_card(publish(stale, "Old"))
                .await
                .unwrap_err();
            assert!(matches!(err, ServiceError::Conflict(_)), "{err:?}");
        }

        let newer = h
            .svc
            .publish_profile_card(publish(3, "Ada L."))
            .await
            .unwrap();
        assert_eq!(newer.version, 3);
        assert!(newer.payload.contains("Ada L."));
    }

    #[tokio::test]
    async fn profile_card_signed_by_a_stranger_is_rejected() {
        let h = harness();
        let e = h.enroll(1).await;
        let stranger = TestIdentity::new(9);
        let card = stranger.profile_card(&e.state, 1, "Mallory", &[], &[]);
        let err = h
            .svc
            .publish_profile_card(PublishProfileCardCmd {
                actor: Actor::new(e.user_id),
                card: payload(card),
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)), "{err:?}");
    }

    // ── Statement versions and tagged keys ────────────────────────────────────────────────

    #[tokio::test]
    async fn version_2_identity_enrolls_signs_in_and_stores_tagged_keys() {
        let h = harness();
        let e = h.enroll_with(1, StatementVersion::V2).await;
        assert_eq!(e.state.version, StatementVersion::V2);
        assert!(h.login(&e, e.device_id, &e.device, AUDIENCE).await.is_ok());

        let identity = h
            .svc
            .get_my_key_identity(GetMyKeyIdentityQuery {
                actor: Actor::new(e.user_id),
            })
            .await
            .unwrap();
        let device_key = PublicKey::from(&e.device);
        assert_eq!(identity.devices[0].public_key, device_key.to_string());
        assert!(identity.devices[0].public_key.starts_with("ed25519:"));

        let s = h.state.lock().unwrap();
        assert_eq!(
            s.identity_keys.get(&e.user_id),
            Some(&PublicKey::from(&e.id.identity).to_string())
        );
        assert_eq!(s.device_hashes[0], device_key.key_hash().to_vec());
    }

    #[tokio::test]
    async fn version_1_identity_stores_tagged_keys_too() {
        let h = harness();
        let e = h.enroll(1).await;
        let s = h.state.lock().unwrap();
        assert_eq!(
            s.devices[0].public_key,
            PublicKey::from(&e.device).to_string()
        );
        assert!(s.identity_keys[&e.user_id].starts_with("ed25519:"));
    }

    #[tokio::test]
    async fn version_1_log_moves_to_version_2_and_never_back() {
        let h = harness();
        let mut e = h.enroll(1).await;

        e.id.version = StatementVersion::V2;
        let rotation = e.id.rotate_with_identity(&e.state, 3);
        let dto = h
            .svc
            .rotate(RotateIdentityKeyCmd {
                actor: Some(Actor::new(e.user_id)),
                entry: payload(rotation.clone()),
                device: None,
                remote_ip: IP.to_string(),
            })
            .await
            .expect("version 2 rotation of a version 1 log");
        assert_eq!(dto.key_seq, Some(1));
        e.state = e.state.apply(&rotation).unwrap().0;
        assert_eq!(
            h.state.lock().unwrap().identity_keys[&e.user_id],
            PublicKey::from(&e.id.identity).to_string()
        );

        // The device enrolled with a version 1 certificate signs in with a version 2 proof.
        assert!(h.login(&e, e.device_id, &e.device, AUDIENCE).await.is_ok());

        // A version 2 certificate under the rotated key adds a device.
        let phone_id = Uuid::now_v7();
        let phone = signing_key(70);
        let cert = e.id.device_cert(&e.state, phone_id, &phone, "Phone");
        h.svc
            .add_device(AddDeviceKeyCmd {
                actor: Actor::new(e.user_id),
                device: payload(cert),
            })
            .await
            .expect("version 2 certificate");
        assert!(h.login(&e, phone_id, &phone, AUDIENCE).await.is_ok());

        // A version 1 rotation after a version 2 entry is refused.
        e.id.version = StatementVersion::V1;
        let back = e.id.rotate_with_identity(&e.state, 4);
        let err = h
            .svc
            .rotate(RotateIdentityKeyCmd {
                actor: Some(Actor::new(e.user_id)),
                entry: payload(back),
                device: None,
                remote_ip: IP.to_string(),
            })
            .await
            .unwrap_err();
        assert!(
            matches!(&err, ServiceError::Validation(m) if m.contains("cannot follow")),
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn one_device_key_spelled_both_ways_is_one_key() {
        let h = harness();
        let mut e = h.enroll(1).await;
        // The enrolled device key again, certified with a version 2 certificate (tagged
        // spelling) under a new device id.
        e.id.version = StatementVersion::V2;
        let cert =
            e.id.device_cert(&e.state, Uuid::now_v7(), &e.device, "Again");
        let err = h
            .svc
            .add_device(AddDeviceKeyCmd {
                actor: Actor::new(e.user_id),
                device: payload(cert),
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Conflict(_)), "{err:?}");
    }

    // ── Key-only signup ───────────────────────────────────────────────────────────────────

    /// What a device presents to sign up: a new identity, its first device, and a proof over a
    /// fresh challenge.
    struct Presented {
        id: TestIdentity,
        state: KeyState,
        device_id: Uuid,
        device: SigningKey,
        key_log: Vec<SignedPayload>,
        cert: SignedPayload,
        proof: SignedPayload,
    }

    impl Harness {
        async fn present(&self, seed: u8) -> Presented {
            let id = TestIdentity::with_version(seed, StatementVersion::V2);
            let inception = id.inception();
            let state = KeyState::from_inception(&inception).unwrap();
            let device_id = Uuid::now_v7();
            let device = signing_key(seed.wrapping_add(50));
            let cert = payload(id.device_cert(&state, device_id, &device, "Laptop"));
            let proof = self.proof(&device, &state, device_id).await;
            Presented {
                key_log: vec![payload(inception)],
                id,
                state,
                device_id,
                device,
                cert,
                proof,
            }
        }

        fn user_count(&self) -> usize {
            self.state.lock().unwrap().users.len()
        }
    }

    fn signup_cmd(
        p: &Presented,
        username: &str,
        mode: RegistrationMode,
        token: Option<SignupToken>,
    ) -> SignupWithKeyCmd {
        SignupWithKeyCmd {
            username: username.to_string(),
            display_name: None,
            key_log: p.key_log.clone(),
            device: p.cert.clone(),
            proof: p.proof.clone(),
            registration_mode: mode,
            token,
            proof_of_work: None,
            remote_ip: IP.to_string(),
            terms_accepted_url: None,
            confirmed_minimum_age: None,
        }
    }

    fn open_cmd(p: &Presented, username: &str) -> SignupWithKeyCmd {
        signup_cmd(p, username, RegistrationMode::Open, None)
    }

    fn signup_harness(signup: SignupRepos, broadcaster: Arc<dyn Broadcaster>) -> Harness {
        harness_with(
            Limits {
                challenge: 100,
                failure: 100,
                signup: 100,
            },
            signup,
            broadcaster,
        )
    }

    #[tokio::test]
    async fn signup_with_key_in_open_mode_creates_a_key_only_account_on_the_device() {
        let recorder = metrics_util::debugging::DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _guard = metrics::set_default_local_recorder(&recorder);

        let h = harness();
        let p = h.present(1).await;
        let res = h.svc.signup_with_key(open_cmd(&p, "ada")).await.unwrap();
        assert_eq!(res.server_id, None);
        assert!(!res.session_token.is_empty() && !res.csrf_token.is_empty());

        {
            let s = h.state.lock().unwrap();
            let created = &s.created[&res.user_id];
            assert_eq!(created.password, None);
            assert_eq!(created.email, None);
            assert_eq!(created.display_name.as_deref(), Some("ada"));
            let key = created.key_identity.as_ref().expect("key identity");
            assert_eq!(key.principal_id, p.state.principal_id);
            assert_eq!(key.identity_key, p.state.identity_key.to_string());
            assert_eq!(s.log[&res.user_id].len(), 1);
            let device = s
                .devices
                .iter()
                .find(|d| d.user_id == res.user_id)
                .expect("device stored");
            assert_eq!(device.device_id, p.device_id);
            assert!(device.last_used_at.is_some());
        }
        // The new session belongs to the device that signed up.
        let device_key_id = h.state.lock().unwrap().devices[0].id;
        assert_eq!(
            h.session_device(res.session.session_id),
            Some(device_key_id)
        );

        // The device signs in again with a fresh challenge.
        let enrolled = Enrolled {
            user_id: res.user_id,
            session: res.session.session_id,
            id: p.id,
            state: p.state,
            device_id: p.device_id,
            device: p.device,
        };
        h.login(&enrolled, enrolled.device_id, &enrolled.device, AUDIENCE)
            .await
            .unwrap();
        let got = crate::metrics::test_support::Recorded::snapshot(&snapshotter);
        assert_eq!(got.counter(crate::metrics::REGISTRATIONS, &[]), 1);
    }

    #[tokio::test]
    async fn signup_with_key_refuses_the_key_identity_of_a_deleted_account() {
        let h = harness();
        let p = h.present(1).await;
        h.state
            .lock()
            .unwrap()
            .retired
            .insert(p.state.principal_id.clone());
        let err = h
            .svc
            .signup_with_key(open_cmd(&p, "ada"))
            .await
            .expect_err("refused");
        assert!(
            matches!(&err, ServiceError::Validation(m) if m.contains("deleted")),
            "{err:?}"
        );
        assert_eq!(h.user_count(), 0);
    }

    #[tokio::test]
    async fn signup_with_key_refuses_a_reserved_username_or_display_name() {
        use crate::reserved_names::{RESERVED_NAME_REFUSAL, ReservedNames};

        let mut h = harness();
        h.svc = h
            .svc
            .with_reserved_names(ReservedNames::parse("rim").unwrap());
        let p = h.present(1).await;
        for cmd in [
            open_cmd(&p, "rim-admin"),
            SignupWithKeyCmd {
                display_name: Some("RIM Team".to_string()),
                ..open_cmd(&p, "ada")
            },
        ] {
            let err = h.svc.signup_with_key(cmd).await.unwrap_err();
            assert!(
                matches!(&err, ServiceError::Validation(m) if m == RESERVED_NAME_REFUSAL),
                "{err:?}"
            );
        }
        assert_eq!(h.user_count(), 0);
        h.svc
            .signup_with_key(open_cmd(&p, "rimmer"))
            .await
            .expect("a name that only starts with the letters");
    }

    /// On an instance with terms, a key signup that did not accept them is refused before it
    /// creates an account, and one that did has them recorded. Without instance terms nothing
    /// is recorded, whatever the signup sent.
    #[tokio::test]
    async fn signup_with_key_requires_and_records_the_instance_terms() {
        use rim_services::auth::terms::TERMS_NOT_ACCEPTED;

        let terms = "https://rim.example.org/terms";
        let mut h = harness();
        h.svc = h.svc.with_terms_url(Some(terms.to_string()));
        let p = h.present(1).await;
        for sent in [None, Some("https://rim.example.org/old-terms")] {
            let err = h
                .svc
                .signup_with_key(SignupWithKeyCmd {
                    terms_accepted_url: sent.map(str::to_string),
                    confirmed_minimum_age: None,
                    ..open_cmd(&p, "ada")
                })
                .await
                .unwrap_err();
            assert!(
                matches!(&err, ServiceError::Validation(m) if m == TERMS_NOT_ACCEPTED),
                "{sent:?}: {err:?}"
            );
        }
        assert_eq!(h.user_count(), 0);
        let accepted = h
            .svc
            .signup_with_key(SignupWithKeyCmd {
                terms_accepted_url: Some(terms.to_string()),
                confirmed_minimum_age: None,
                ..open_cmd(&p, "ada")
            })
            .await
            .expect("signup");
        assert_eq!(
            h.state
                .lock()
                .unwrap()
                .terms
                .get(&accepted.user_id)
                .map(String::as_str),
            Some(terms)
        );

        let h = harness();
        let p = h.present(1).await;
        let ignored = h
            .svc
            .signup_with_key(SignupWithKeyCmd {
                terms_accepted_url: Some(terms.to_string()),
                confirmed_minimum_age: None,
                ..open_cmd(&p, "grace")
            })
            .await
            .expect("signup");
        assert!(!h.state.lock().unwrap().terms.contains_key(&ignored.user_id));
    }

    #[tokio::test]
    async fn signup_with_key_normalizes_the_display_name() {
        let h = harness();
        let p = h.present(1).await;
        let res = h
            .svc
            .signup_with_key(SignupWithKeyCmd {
                display_name: Some("  Ada Lovelace  ".to_string()),
                ..open_cmd(&p, "ada")
            })
            .await
            .unwrap();
        let s = h.state.lock().unwrap();
        assert_eq!(
            s.created[&res.user_id].display_name.as_deref(),
            Some("Ada Lovelace")
        );
    }

    #[tokio::test]
    async fn signup_with_key_requires_a_token_in_invite_only_mode() {
        let h = harness();
        let p = h.present(1).await;
        let err = h
            .svc
            .signup_with_key(signup_cmd(&p, "ada", RegistrationMode::InviteOnly, None))
            .await
            .unwrap_err();
        assert!(
            matches!(&err, ServiceError::Validation(m) if m.contains("exactly one token")),
            "{err:?}"
        );
        assert_eq!(h.user_count(), 0);
    }

    #[tokio::test]
    async fn signup_with_key_refuses_a_token_in_open_mode() {
        let h = harness();
        let p = h.present(1).await;
        let err = h
            .svc
            .signup_with_key(signup_cmd(
                &p,
                "ada",
                RegistrationMode::Open,
                Some(SignupToken::InstanceInvite("rim_iinv".to_string())),
            ))
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)), "{err:?}");
        assert_eq!(h.user_count(), 0);
    }

    #[tokio::test]
    async fn signup_with_key_refuses_a_proof_by_another_key() {
        let h = harness();
        let p = h.present(1).await;
        // Signed by a key that is not the certified device's.
        let proof = h.proof(&signing_key(99), &p.state, p.device_id).await;
        let err = h
            .svc
            .signup_with_key(SignupWithKeyCmd {
                proof,
                ..open_cmd(&p, "ada")
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)), "{err:?}");
        assert_eq!(h.user_count(), 0);
    }

    #[tokio::test]
    async fn signup_with_key_refuses_a_proof_for_another_server() {
        let h = harness();
        let p = h.present(1).await;
        let proof = h
            .proof_for(&p.device, &p.state, p.device_id, "evil.example.com")
            .await;
        let err = h
            .svc
            .signup_with_key(SignupWithKeyCmd {
                proof,
                ..open_cmd(&p, "ada")
            })
            .await
            .unwrap_err();
        assert!(
            matches!(&err, ServiceError::Validation(m) if m.contains("audience")),
            "{err:?}"
        );
        assert_eq!(h.user_count(), 0);
    }

    #[tokio::test]
    async fn signup_with_key_refuses_a_used_challenge() {
        let h = harness();
        let p = h.present(1).await;
        h.svc.signup_with_key(open_cmd(&p, "ada")).await.unwrap();
        // The same proof again, for another username: its challenge is spent.
        let err = h
            .svc
            .signup_with_key(open_cmd(&p, "ada2"))
            .await
            .unwrap_err();
        assert!(
            matches!(&err, ServiceError::Validation(m) if m.contains("challenge")),
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn signup_with_key_refuses_a_proof_over_a_join_challenge() {
        let h = harness();
        let p = h.present(1).await;
        let proof = h
            .join_proof(&p.device, &p.state, p.device_id, "rims1_relay")
            .await;
        let err = h
            .svc
            .signup_with_key(SignupWithKeyCmd {
                proof,
                ..open_cmd(&p, "ada")
            })
            .await
            .unwrap_err();
        assert!(
            matches!(&err, ServiceError::Validation(m) if m.contains("challenge")),
            "{err:?}"
        );
        assert_eq!(h.user_count(), 0);
    }

    #[tokio::test]
    async fn signup_with_key_refuses_a_principal_another_account_holds() {
        let h = harness();
        let p = h.present(1).await;
        h.svc.signup_with_key(open_cmd(&p, "ada")).await.unwrap();

        // The same identity again, from a new device over a new challenge.
        let device = signing_key(77);
        let device_id = Uuid::now_v7();
        let again = Presented {
            cert: payload(p.id.device_cert(&p.state, device_id, &device, "Phone")),
            proof: h.proof(&device, &p.state, device_id).await,
            device_id,
            device,
            ..p
        };
        let err = h
            .svc
            .signup_with_key(open_cmd(&again, "ada_again"))
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Conflict(_)), "{err:?}");
        assert_eq!(h.user_count(), 1);
    }

    #[tokio::test]
    async fn signup_with_key_refuses_a_principal_enrolled_on_a_password_account() {
        let h = harness();
        let e = h.enroll_with(1, StatementVersion::V2).await;
        let device = signing_key(77);
        let device_id = Uuid::now_v7();
        let again = Presented {
            key_log: vec![payload(e.id.inception())],
            cert: payload(e.id.device_cert(&e.state, device_id, &device, "Phone")),
            proof: h.proof(&device, &e.state, device_id).await,
            id: e.id,
            state: e.state,
            device_id,
            device,
        };
        let err = h
            .svc
            .signup_with_key(open_cmd(&again, "ada_again"))
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Conflict(_)), "{err:?}");
    }

    #[tokio::test]
    async fn signup_with_key_refuses_a_taken_username() {
        let recorder = metrics_util::debugging::DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _guard = metrics::set_default_local_recorder(&recorder);

        let h = harness();
        let first = h.present(1).await;
        h.svc
            .signup_with_key(open_cmd(&first, "ada"))
            .await
            .unwrap();
        let second = h.present(2).await;
        let err = h
            .svc
            .signup_with_key(open_cmd(&second, "ada"))
            .await
            .unwrap_err();
        assert!(
            matches!(&err, ServiceError::Conflict(m) if m.contains("username")),
            "{err:?}"
        );
        assert_eq!(h.user_count(), 1);
        let got = crate::metrics::test_support::Recorded::snapshot(&snapshotter);
        assert_eq!(got.counter(crate::metrics::REGISTRATIONS, &[]), 1);
    }

    #[tokio::test]
    async fn signup_with_key_refuses_an_invalid_username() {
        let h = harness();
        let p = h.present(1).await;
        let err = h
            .svc
            .signup_with_key(open_cmd(&p, "a b"))
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)), "{err:?}");
    }

    #[tokio::test]
    async fn signup_with_key_in_open_mode_is_rate_limited_per_ip() {
        let h = harness_with(
            Limits {
                challenge: 100,
                failure: 100,
                signup: 1,
            },
            no_signup_repos(),
            Arc::new(Recording::default()),
        );
        let first = h.present(1).await;
        h.svc
            .signup_with_key(open_cmd(&first, "ada"))
            .await
            .unwrap();
        let second = h.present(2).await;
        let err = h
            .svc
            .signup_with_key(open_cmd(&second, "grace"))
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::RateLimited { .. }), "{err:?}");
        assert_eq!(h.user_count(), 1);
    }

    fn instance_invite(expires_in: Duration, email: Option<&str>) -> InstanceInvite {
        InstanceInvite {
            id: InstanceInviteId::new(),
            created_by_user_id: None,
            token_hash: hash_token("rim_iinv"),
            created_at: Utc::now(),
            expires_at: Utc::now() + expires_in,
            max_uses: 1,
            use_count: 0,
            revoked_at: None,
            email: email.map(str::to_string),
        }
    }

    #[tokio::test]
    async fn signup_with_key_with_an_instance_invite_consumes_it_and_joins_nothing() {
        let invite = instance_invite(Duration::minutes(15), None);
        let invite_id = invite.id;
        let creator = invite.created_by_user_id;
        let mut invites = MockInstanceInviteRepository::new();
        invites
            .expect_get_by_token_hash()
            .withf(|hash| hash == hash_token("rim_iinv"))
            .returning(move |_| Ok(Some(invite.clone())));
        invites
            .expect_try_consume()
            .withf(move |id| *id == invite_id)
            .times(1)
            .returning(|_| Ok(true));
        let (users, chains) = crate::signup::tests::chain_recorder();
        let signup = SignupRepos {
            instance_invite_repo: Arc::new(invites),
            user_repo: Arc::new(users),
            ..no_signup_repos()
        };
        let events = Arc::new(Recording::default());
        let h = signup_harness(signup, Arc::clone(&events) as Arc<dyn Broadcaster>);
        let p = h.present(1).await;

        let res = h
            .svc
            .signup_with_key(signup_cmd(
                &p,
                "admin",
                RegistrationMode::InviteOnly,
                Some(SignupToken::InstanceInvite("rim_iinv".to_string())),
            ))
            .await
            .unwrap();
        assert_eq!(res.server_id, None);
        assert!(events.0.lock().unwrap().is_empty());
        assert_eq!(
            *chains.lock().unwrap(),
            vec![(res.user_id, invite_id.into(), creator)]
        );
        let s = h.state.lock().unwrap();
        assert_eq!(s.created[&res.user_id].password, None);
    }

    #[tokio::test]
    async fn signup_with_key_with_an_expired_instance_invite_creates_nothing() {
        let invite = instance_invite(Duration::minutes(-1), None);
        let mut invites = MockInstanceInviteRepository::new();
        invites
            .expect_get_by_token_hash()
            .returning(move |_| Ok(Some(invite.clone())));
        invites.expect_try_consume().never();
        let signup = SignupRepos {
            instance_invite_repo: Arc::new(invites),
            ..no_signup_repos()
        };
        let h = signup_harness(signup, Arc::new(Recording::default()));
        let p = h.present(1).await;

        let err = h
            .svc
            .signup_with_key(signup_cmd(
                &p,
                "admin",
                RegistrationMode::InviteOnly,
                Some(SignupToken::InstanceInvite("rim_iinv".to_string())),
            ))
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::ExpiredToken), "{err:?}");
        assert_eq!(h.user_count(), 0);
    }

    /// A signup harness whose one instance invite, `rim_iinv`, is bound to `ops@example.com`
    /// and expects to be consumed `uses` times.
    fn email_bound_invite_harness(uses: usize) -> Harness {
        let invite = instance_invite(Duration::minutes(15), Some("ops@example.com"));
        let invite_id = invite.id;
        let mut invites = MockInstanceInviteRepository::new();
        invites
            .expect_get_by_token_hash()
            .returning(move |_| Ok(Some(invite.clone())));
        invites
            .expect_try_consume()
            .withf(move |id| *id == invite_id)
            .times(uses)
            .returning(|_| Ok(true));
        let (users, _) = crate::signup::tests::chain_recorder();
        let signup = SignupRepos {
            instance_invite_repo: Arc::new(invites),
            user_repo: Arc::new(users),
            ..no_signup_repos()
        };
        signup_harness(signup, Arc::new(Recording::default()))
    }

    fn email_bound_signup_cmd(p: &Presented) -> SignupWithKeyCmd {
        signup_cmd(
            p,
            "ops",
            RegistrationMode::InviteOnly,
            Some(SignupToken::InstanceInvite("rim_iinv".to_string())),
        )
    }

    #[tokio::test]
    async fn signup_with_key_with_an_email_bound_instance_invite_takes_its_address() {
        let h = email_bound_invite_harness(1);
        let p = h.present(1).await;

        let res = h
            .svc
            .signup_with_key(email_bound_signup_cmd(&p))
            .await
            .unwrap();
        let s = h.state.lock().unwrap();
        let created = &s.created[&res.user_id];
        assert_eq!(created.email.as_deref(), Some("ops@example.com"));
        assert_eq!(created.password, None);
        assert!(created.key_identity.is_some());
    }

    #[tokio::test]
    async fn signup_with_key_refuses_an_email_bound_invite_whose_address_is_blocked_or_taken() {
        let h = email_bound_invite_harness(0);
        h.state
            .lock()
            .unwrap()
            .blocked_emails
            .insert("ops@example.com".to_string());
        let p = h.present(1).await;
        let err = h
            .svc
            .signup_with_key(email_bound_signup_cmd(&p))
            .await
            .unwrap_err();
        assert!(
            matches!(&err, ServiceError::Validation(m) if m == crate::signup::BLOCKED_EMAIL_REFUSAL),
            "{err:?}"
        );
        assert_eq!(h.user_count(), 0);

        let h = email_bound_invite_harness(0);
        {
            let mut s = h.state.lock().unwrap();
            let holder = UserId::new();
            s.users.insert(holder);
            s.created.insert(
                holder,
                NewUser {
                    username: "holder".to_string(),
                    password: Some("hash".to_string()),
                    email: Some("ops@example.com".to_string()),
                    display_name: None,
                    key_identity: None,
                },
            );
        }
        let p = h.present(2).await;
        let err = h
            .svc
            .signup_with_key(email_bound_signup_cmd(&p))
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::EmailTaken), "{err:?}");
        assert_eq!(h.user_count(), 1);
    }

    #[tokio::test]
    async fn signup_with_key_with_a_workspace_invite_joins_and_announces() {
        let server_id = ServerId::new();
        let invite = Invite {
            id: InviteId::new(),
            server_id,
            created_by_user_id: UserId::new(),
            token_hash: hash_token("rim_inv"),
            created_at: Utc::now(),
            expires_at: None,
            max_uses: 1,
            use_count: 0,
            revoked_at: None,
            role: ServerMemberRole::Member,
        };
        let invite_id = invite.id;
        let creator = invite.created_by_user_id;
        let mut invites = MockInviteRepository::new();
        invites
            .expect_get_by_token_hash()
            .returning(move |_| Ok(Some(invite.clone())));
        invites
            .expect_try_consume()
            .withf(move |id| *id == invite_id)
            .times(1)
            .returning(|_| Ok(true));
        let mut moderation = MockModerationRepository::new();
        moderation
            .expect_get_active_ban()
            .returning(|_, _, _| Box::pin(async { Ok(None) }));
        let mut members = MockServerMemberRepository::new();
        members
            .expect_add_member()
            .times(1)
            .returning(|server_id, user_id, role| {
                Ok(ServerMember {
                    id: ServerMemberId::new(),
                    server_id,
                    user_id,
                    member_role: role,
                    joined_at: Utc::now(),
                })
            });
        let mut conversations = MockConversationRepository::new();
        conversations
            .expect_list_public_channel_ids()
            .returning(|_| Box::pin(async { Ok(Vec::new()) }));
        let (users, chains) = crate::signup::tests::chain_recorder();
        let signup = SignupRepos {
            invite_repo: Arc::new(invites),
            moderation_repo: Arc::new(moderation),
            server_member_repo: Arc::new(members),
            conversation_repo: Arc::new(conversations),
            user_repo: Arc::new(users),
            ..no_signup_repos()
        };
        let events = Arc::new(Recording::default());
        let h = signup_harness(signup, Arc::clone(&events) as Arc<dyn Broadcaster>);
        let p = h.present(1).await;

        let res = h
            .svc
            .signup_with_key(signup_cmd(
                &p,
                "ada",
                RegistrationMode::InviteOnly,
                Some(SignupToken::WorkspaceInvite("rim_inv".to_string())),
            ))
            .await
            .unwrap();
        assert_eq!(res.server_id, Some(server_id));
        assert_eq!(
            *chains.lock().unwrap(),
            vec![(res.user_id, invite_id.into(), Some(creator))]
        );
        let events = events.0.lock().unwrap();
        assert!(matches!(
            events.as_slice(),
            [WorkspaceEvent::MemberJoined { workspace_id, user_id, username }]
                if *workspace_id == server_id && *user_id == res.user_id && username == "ada"
        ));
    }

    // ── Public relay: proof-of-work, operator blocks, quotas ─────────────────────────────

    /// The relay of known-answer vector A.1, which vector A.8's proof-of-work names.
    const A8_RELAY: &str = "rims1_T5LLFxkrobgyvlf3WVxuvLlfqYSZAssRFgmvCfI2R-c";
    const A8_INCEPTION: &str = r#"{"v":2,"type":"inception","seq":0,"identityKey":"ed25519:iojj3XQJ8ZX9UtstPLpdcspnCb8dlBIb83SIAbQPb1w","recoveryKey":"ed25519:1i8Bah79Hk_feT60LNhEceG6nwzwTRKHtcxx9hYofLg","createdAt":"2026-10-09T00:00:00Z"}"#;

    /// A harness for the A.8 relay, which also accepts its server id as an audience.
    fn relay_harness(signup_limit: u32, quotas: crate::quota::Quotas) -> Harness {
        let h = harness_with(
            Limits {
                challenge: 100,
                failure: 100,
                signup: signup_limit,
            },
            no_signup_repos(),
            Arc::new(Recording::default()),
        );
        Harness {
            state: h.state,
            svc: h
                .svc
                .with_server_id(A8_RELAY.to_string())
                .with_quotas(quotas),
            revocations: h.revocations,
        }
    }

    fn pow_cmd(p: &Presented, username: &str, bits: u32, nonce: &str) -> SignupWithKeyCmd {
        SignupWithKeyCmd {
            proof_of_work: Some(nonce.to_string()),
            ..signup_cmd(p, username, RegistrationMode::Pow { bits }, None)
        }
    }

    #[tokio::test]
    async fn a8_proof_of_work_signs_up_on_its_relay() {
        let h = relay_harness(100, crate::quota::Quotas::default());
        let p = h.present(1).await;
        assert_eq!(p.key_log[0].payload, A8_INCEPTION);
        let res = h
            .svc
            .signup_with_key(pow_cmd(&p, "ada", 16, "AAAAAAAAkaM"))
            .await
            .expect("A.8's nonce meets 16 bits");
        assert_eq!(res.server_id, None);
        assert_eq!(h.user_count(), 1);
    }

    #[tokio::test]
    async fn a_weak_malformed_or_misplaced_proof_of_work_creates_nothing() {
        let h = relay_harness(100, crate::quota::Quotas::default());
        let p = h.present(1).await;
        for (bits, nonce) in [
            (16, "AAAAAAAAkaI"),
            (19, "AAAAAAAAkaM"),
            (16, "AAAA"),
            (16, "!!"),
        ] {
            let err = h
                .svc
                .signup_with_key(pow_cmd(&p, "ada", bits, nonce))
                .await
                .unwrap_err();
            assert!(
                matches!(err, ServiceError::Validation(_)),
                "{nonce}: {err:?}"
            );
        }
        // Bound to the relay: the same nonce does not count at another server.
        let other = harness_with(
            Limits {
                challenge: 100,
                failure: 100,
                signup: 100,
            },
            no_signup_repos(),
            Arc::new(Recording::default()),
        );
        let other = Harness {
            state: other.state,
            svc: other.svc.with_server_id(SERVER_ID.to_string()),
            revocations: other.revocations,
        };
        let q = other.present(1).await;
        let err = other
            .svc
            .signup_with_key(pow_cmd(&q, "ada", 16, "AAAAAAAAkaM"))
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)), "{err:?}");
        assert_eq!(h.user_count() + other.user_count(), 0);
    }

    #[tokio::test]
    async fn pow_mode_takes_a_token_or_a_proof_of_work_and_limits_per_ip() {
        let h = relay_harness(1, crate::quota::Quotas::default());
        let p = h.present(1).await;
        let neither = h
            .svc
            .signup_with_key(signup_cmd(
                &p,
                "ada",
                RegistrationMode::Pow { bits: 16 },
                None,
            ))
            .await
            .unwrap_err();
        assert!(matches!(&neither, ServiceError::Validation(m) if m.contains("required")));
        let both = h
            .svc
            .signup_with_key(SignupWithKeyCmd {
                token: Some(SignupToken::InstanceInvite("rim_iinv".to_string())),
                ..pow_cmd(&p, "ada", 16, "AAAAAAAAkaM")
            })
            .await
            .unwrap_err();
        assert!(matches!(&both, ServiceError::Validation(m) if m.contains("not both")));
        let elsewhere = h
            .svc
            .signup_with_key(SignupWithKeyCmd {
                proof_of_work: Some("AAAAAAAAkaM".to_string()),
                ..open_cmd(&p, "ada")
            })
            .await
            .unwrap_err();
        assert!(matches!(&elsewhere, ServiceError::Validation(m) if m.contains("pow")));
        h.svc
            .signup_with_key(pow_cmd(&p, "ada", 16, "AAAAAAAAkaM"))
            .await
            .expect("first");
        // The open-signup limit per IP applies to proof-of-work signups too.
        let q = h.present(2).await;
        let nonce =
            URL_SAFE_NO_PAD.encode(rim_s2s::pow::solve_pow(A8_RELAY, &q.key_log[0].payload, 4));
        let err = h
            .svc
            .signup_with_key(pow_cmd(&q, "grace", 4, &nonce))
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::RateLimited { .. }), "{err:?}");
    }

    #[tokio::test]
    async fn a_principal_blocked_here_cannot_sign_up_enroll_or_sign_in() {
        let h = harness();
        let p = h.present(1).await;
        h.state
            .lock()
            .unwrap()
            .blocked
            .insert(p.state.principal_id.clone());
        let err = h
            .svc
            .signup_with_key(open_cmd(&p, "ada"))
            .await
            .unwrap_err();
        assert!(
            matches!(&err, ServiceError::Forbidden(m) if m.contains("blocked")),
            "{err:?}"
        );
        assert_eq!(h.user_count(), 0);

        let e = h.enroll(2).await;
        assert!(h.login(&e, e.device_id, &e.device, AUDIENCE).await.is_ok());
        h.state
            .lock()
            .unwrap()
            .blocked
            .insert(e.state.principal_id.clone());
        let err = h
            .login(&e, e.device_id, &e.device, AUDIENCE)
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Forbidden(_)), "{err:?}");

        let (user_id, session_id) = h.add_user_with_session().await;
        let id = TestIdentity::new(3);
        h.state.lock().unwrap().blocked.insert(
            KeyState::from_inception(&id.inception())
                .unwrap()
                .principal_id,
        );
        let state = KeyState::from_inception(&id.inception()).unwrap();
        let device_id = Uuid::now_v7();
        let device = signing_key(53);
        let err = h
            .svc
            .enroll(EnrollIdentityKeyCmd {
                actor: Actor::new(user_id),
                session_id,
                key_log: vec![payload(id.inception())],
                device: payload(id.device_cert(&state, device_id, &device, "Laptop")),
                proof: h.proof(&device, &state, device_id).await,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Forbidden(_)), "{err:?}");
    }

    #[tokio::test]
    async fn devices_and_card_size_follow_the_quotas() {
        let h = relay_harness(
            100,
            crate::quota::Quotas {
                devices_per_user: 2,
                card_max_bytes: 2048,
                ..crate::quota::Quotas::default()
            },
        );
        let e = h.enroll(1).await;
        let add = |n: u8| AddDeviceKeyCmd {
            actor: Actor::new(e.user_id),
            device: payload(e.id.device_cert(
                &e.state,
                Uuid::from_u128(u128::from(n)),
                &signing_key(n),
                "Phone",
            )),
        };
        h.svc.add_device(add(70)).await.expect("second device");
        let err = h.svc.add_device(add(71)).await.unwrap_err();
        assert!(
            matches!(&err, ServiceError::Validation(m) if m == "this instance allows 2 devices per user; revoke one first"),
            "{err:?}"
        );

        let card = |name: &str| PublishProfileCardCmd {
            actor: Actor::new(e.user_id),
            card: payload(e.id.profile_card(&e.state, 1, name, &[], &[])),
        };
        let err = h
            .svc
            .publish_profile_card(card(&"x".repeat(3000)))
            .await
            .unwrap_err();
        assert!(
            matches!(&err, ServiceError::Validation(m) if m.contains("2048 bytes")),
            "{err:?}"
        );
        h.svc
            .publish_profile_card(card("Ada"))
            .await
            .expect("a small card");
    }

    // ── Default workspace ────────────────────────────────────────────────────────────────

    /// Every workspace membership [`default_workspace_repos`] inserted.
    type MemberLog = Arc<Mutex<Vec<(ServerId, UserId, ServerMemberRole)>>>;

    /// Signup repositories holding the workspaces `default` and `invited_to`, one usable
    /// instance invite and one usable workspace invite to `invited_to`, with a membership store
    /// whose inserts are logged.
    fn default_workspace_repos(
        default: ServerId,
        invited_to: ServerId,
    ) -> (SignupRepos, MemberLog) {
        let log: MemberLog = Arc::default();
        let mut members = MockServerMemberRepository::new();
        let seen = Arc::clone(&log);
        members
            .expect_is_member()
            .returning(move |server_id, user_id| {
                Ok(seen
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|(s, u, _)| *s == server_id && *u == user_id))
            });
        let seen = Arc::clone(&log);
        members
            .expect_add_member()
            .returning(move |server_id, user_id, member_role| {
                seen.lock().unwrap().push((server_id, user_id, member_role));
                Ok(ServerMember {
                    id: ServerMemberId::new(),
                    server_id,
                    user_id,
                    member_role,
                    joined_at: Utc::now(),
                })
            });
        let mut servers = rim_core::server::repository::MockServerRepository::new();
        servers.expect_get_by_id().returning(move |id| {
            Ok([default, invited_to]
                .contains(&id)
                .then(|| rim_core::server::model::Server {
                    id,
                    name: "Lobby".to_string(),
                    created_at: Utc::now(),
                    updated_at: Utc::now(),
                }))
        });
        let mut conversations = MockConversationRepository::new();
        conversations
            .expect_list_public_channel_ids()
            .returning(|_| Box::pin(async { Ok(Vec::new()) }));
        let mut instance_invites = MockInstanceInviteRepository::new();
        instance_invites
            .expect_get_by_token_hash()
            .returning(|_| Ok(Some(instance_invite(Duration::minutes(15), None))));
        instance_invites
            .expect_try_consume()
            .returning(|_| Ok(true));
        let mut invites = MockInviteRepository::new();
        invites.expect_get_by_token_hash().returning(move |_| {
            Ok(Some(Invite {
                id: InviteId::new(),
                server_id: invited_to,
                created_by_user_id: UserId::new(),
                token_hash: hash_token("rim_inv"),
                created_at: Utc::now(),
                expires_at: None,
                max_uses: 1,
                use_count: 0,
                revoked_at: None,
                role: ServerMemberRole::Member,
            }))
        });
        invites.expect_try_consume().returning(|_| Ok(true));
        let (users, _) = crate::signup::tests::chain_recorder();
        let signup = SignupRepos {
            instance_invite_repo: Arc::new(instance_invites),
            invite_repo: Arc::new(invites),
            server_repo: Arc::new(servers),
            server_member_repo: Arc::new(members),
            conversation_repo: Arc::new(conversations),
            conversation_member_repo: Arc::new(MockConversationMemberRepository::new()),
            moderation_repo: Arc::new(MockModerationRepository::permissive()),
            user_repo: Arc::new(users),
            dm_policy_repo: Arc::new(
                rim_core::dm_policy::repository::MockDmPolicyRepository::new(),
            ),
            default_workspace_repo: Arc::new(
                rim_core::default_workspace::repository::MockDefaultWorkspaceRepository::new(),
            ),
        };
        (signup, log)
    }

    /// Every key-only signup, open, proof-of-work, with an instance invite and with a
    /// workspace invite, makes the new account a Member of the default workspace and announces
    /// it there; the workspace invite also joins its own workspace.
    #[tokio::test]
    async fn every_key_signup_joins_the_default_workspace() {
        let default = ServerId::new();
        let invited_to = ServerId::new();
        for path in ["open", "pow", "instance invite", "workspace invite"] {
            let (signup, log) = default_workspace_repos(default, invited_to);
            let events = Arc::new(Recording::default());
            let h = signup_harness(signup, Arc::clone(&events) as Arc<dyn Broadcaster>);
            let h = Harness {
                state: h.state,
                svc: h
                    .svc
                    .with_server_id(A8_RELAY.to_string())
                    .with_default_workspace(Some(default)),
                revocations: h.revocations,
            };
            let p = h.present(1).await;
            let cmd = match path {
                "open" => open_cmd(&p, "ada"),
                "pow" => pow_cmd(&p, "ada", 16, "AAAAAAAAkaM"),
                "instance invite" => signup_cmd(
                    &p,
                    "ada",
                    RegistrationMode::InviteOnly,
                    Some(SignupToken::InstanceInvite("rim_iinv".to_string())),
                ),
                _ => signup_cmd(
                    &p,
                    "ada",
                    RegistrationMode::InviteOnly,
                    Some(SignupToken::WorkspaceInvite("rim_inv".to_string())),
                ),
            };
            let res = h.svc.signup_with_key(cmd).await.expect(path);
            let mut expected = vec![(default, res.user_id, ServerMemberRole::Member)];
            let mut announced = vec![default];
            if path == "workspace invite" {
                expected.insert(0, (invited_to, res.user_id, ServerMemberRole::Member));
                announced.insert(0, invited_to);
                assert_eq!(res.server_id, Some(invited_to), "{path}");
            } else {
                assert_eq!(res.server_id, None, "{path}");
            }
            assert_eq!(*log.lock().unwrap(), expected, "{path}");
            let joined: Vec<ServerId> = events
                .0
                .lock()
                .unwrap()
                .iter()
                .filter_map(|e| match e {
                    WorkspaceEvent::MemberJoined {
                        workspace_id,
                        user_id,
                        ..
                    } if *user_id == res.user_id => Some(*workspace_id),
                    _ => None,
                })
                .collect();
            assert_eq!(joined, announced, "{path}");
        }
    }

    /// Without a default workspace a key-only signup joins nothing.
    #[tokio::test]
    async fn without_a_default_workspace_a_key_signup_joins_nothing() {
        let (signup, log) = default_workspace_repos(ServerId::new(), ServerId::new());
        let events = Arc::new(Recording::default());
        let h = signup_harness(signup, Arc::clone(&events) as Arc<dyn Broadcaster>);
        let p = h.present(1).await;
        h.svc
            .signup_with_key(open_cmd(&p, "ada"))
            .await
            .expect("open signup");
        assert!(log.lock().unwrap().is_empty());
        assert!(events.0.lock().unwrap().is_empty());
    }

    /// A direct sign-in of someone from another server is refused when their relays answered
    /// with an identity this server refused.
    #[tokio::test]
    async fn a_direct_sign_in_is_refused_when_the_relays_identity_is_refused() {
        let h = remote_harness(IdentityRefresh::Refused);
        let e = h.enroll(1).await;
        let err = h
            .login(&e, e.device_id, &e.device, AUDIENCE)
            .await
            .expect_err("a refused identity");
        assert!(matches!(err, ServiceError::Unauthenticated), "{err:?}");
    }

    /// A direct sign-in checks the stored identity when the relays do not answer, and the one
    /// they served when it was stored.
    #[tokio::test]
    async fn a_direct_sign_in_uses_the_stored_identity_when_the_relays_do_not_answer() {
        for refresh in [IdentityRefresh::Unreachable, IdentityRefresh::Applied(true)] {
            let h = remote_harness(refresh);
            let e = h.enroll(1).await;
            let session = h
                .login(&e, e.device_id, &e.device, AUDIENCE)
                .await
                .unwrap_or_else(|err| panic!("{refresh:?}: {err:?}"));
            assert!(!h.session_revoked(session), "{refresh:?}");
        }
    }
}
