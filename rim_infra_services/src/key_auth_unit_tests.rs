#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use chrono::{DateTime, Duration, Utc};
    use uuid::Uuid;

    use rim_core::auth::model::{NewSession, Session, SessionId};
    use rim_core::auth::repository::SessionRepository;
    use rim_core::key_identity::model::{
        DeviceKey, DeviceKeyId, KeyChallenge, KeyChallengeId, KeyLogEntry, KeyLogEntryType,
        NewDeviceKey, NewKeyLogEntry, StoredProfileCard,
    };
    use rim_core::key_identity::repository::{KeyChallengeRepository, KeyIdentityRepository};
    use rim_core::repository::error::{RepoError, RepoErrorKind, RepoResult};
    use rim_core::user::model::{User, UserId};
    use rim_core::user::repository::MockUserRepository;
    use rim_identity::testing::{
        SigningKey, TestIdentity, device_login, device_self_revoke, signing_key,
    };
    use rim_identity::{KeyState, PublicKey, SignedStatement, StatementVersion};
    use rim_services::key_auth::command::{
        AddDeviceKeyCmd, EnrollIdentityKeyCmd, GetKeyLogQuery, GetMyKeyIdentityQuery,
        IssueKeyChallengeCmd, LoginWithKeyCmd, PublishProfileCardCmd, RevokeDeviceKeyCmd,
        RotateIdentityKeyCmd, SignedPayload,
    };
    use rim_services::key_auth::service::KeyAuthService;
    use rim_services::{Actor, ServiceError};

    use crate::key_auth::{DefaultKeyAuthService, KeyAuthRepos};
    use crate::registration_rate_limiter::RegistrationRateLimiter;

    const AUDIENCE: &str = "rim.example.com";
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
    }

    type Shared = Arc<Mutex<State>>;

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
        async fn create(&self, nonce: &str, expires_at: DateTime<Utc>) -> RepoResult<KeyChallenge> {
            let c = KeyChallenge {
                id: KeyChallengeId::new(),
                nonce: nonce.to_string(),
                created_at: Utc::now(),
                expires_at,
                used_at: None,
            };
            self.0.lock().unwrap().challenges.push(c.clone());
            Ok(c)
        }

        async fn consume(&self, id: KeyChallengeId, nonce: &str) -> RepoResult<bool> {
            let mut s = self.0.lock().unwrap();
            let now = Utc::now();
            let Some(c) = s.challenges.iter_mut().find(|c| {
                c.id == id && c.nonce == nonce && c.used_at.is_none() && c.expires_at > now
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
    }

    fn user_row(id: UserId, principal_id: Option<String>) -> User {
        let now = Utc::now();
        User {
            id,
            username: format!("user-{id}"),
            password_hash: "hash".to_string(),
            created_at: now,
            updated_at: now,
            last_login_at: None,
            display_name: None,
            avatar_url: None,
            status_text: None,
            bio: None,
            email: format!("{id}@example.com"),
            timezone: None,
            principal_id,
        }
    }

    fn harness_with_limits(challenge_limit: u32, failure_limit: u32) -> Harness {
        let state: Shared = Arc::default();
        let mut user_repo = MockUserRepository::new();
        let users = Arc::clone(&state);
        user_repo.expect_get_by_id().returning(move |id| {
            let s = users.lock().unwrap();
            Ok(s.users
                .contains(&id)
                .then(|| user_row(id, s.principals.get(&id).cloned())))
        });
        let repos = KeyAuthRepos {
            user_repo: Arc::new(user_repo),
            session_repo: Arc::new(FakeSessions(Arc::clone(&state))),
            key_identity_repo: Arc::new(FakeKeys(Arc::clone(&state))),
            key_challenge_repo: Arc::new(FakeChallenges(Arc::clone(&state))),
        };
        let svc = DefaultKeyAuthService::new(
            repos,
            None,
            Duration::days(30),
            vec![" RIM.example.com ".to_string()],
        )
        .with_rate_limiters(
            RegistrationRateLimiter::new(3600, challenge_limit),
            RegistrationRateLimiter::new(3600, failure_limit),
        );
        Harness { state, svc }
    }

    fn harness() -> Harness {
        harness_with_limits(100, 100)
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
            let session = FakeSessions(Arc::clone(&self.state))
                .create_session(NewSession {
                    user_id,
                    token_hash: format!("password-session-{user_id}"),
                    expires_at: Utc::now() + Duration::days(1),
                    user_agent: None,
                    ip: None,
                })
                .await
                .unwrap();
            (user_id, session.id)
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

        async fn enroll(&self, seed: u8) -> Enrolled {
            self.enroll_with(seed, StatementVersion::V1).await
        }

        /// Like [`Harness::enroll`], with every statement written at `version`.
        async fn enroll_with(&self, seed: u8, version: StatementVersion) -> Enrolled {
            let (user_id, session) = self.add_user_with_session().await;
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
                &["https://relay.rim.chat"],
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
}
