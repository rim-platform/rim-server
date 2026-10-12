#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use chrono::{DateTime, Duration, Utc};
    use sha2::{Digest, Sha256};

    use rim_core::device_link::model::{
        DeviceLink, DeviceLinkId, DeviceLinkState, NewDeviceLink, SealedBytes,
    };
    use rim_core::device_link::repository::{DeviceLinkRepository, MockDeviceLinkRepository};
    use rim_core::repository::error::{RepoError, RepoErrorKind, RepoResult};
    use rim_core::user::model::{User, UserId};
    use rim_core::user::repository::MockUserRepository;
    use rim_identity::device_link::Direction;
    use rim_services::device_link::command::{
        CancelDeviceLinkCmd, CompleteDeviceLinkCmd, GetDeviceLinkAnswerQuery, GetDeviceLinkQuery,
        SealedPayload, SendDeviceLinkAnswerCmd, SendDeviceLinkOfferCmd, StartDeviceLinkCmd,
    };
    use rim_services::device_link::dto::DeviceLinkStateDto;
    use rim_services::device_link::service::DeviceLinkService;
    use rim_services::{Actor, ServiceError, ServiceResult};

    use crate::device_link::{
        DefaultDeviceLinkService, DeviceLinkRepos, MAX_ANSWER_CIPHERTEXT_LEN,
        MAX_OFFER_CIPHERTEXT_LEN,
    };
    use crate::registration_rate_limiter::RegistrationRateLimiter;

    const IP: &str = "203.0.113.7";
    const OTHER_IP: &str = "198.51.100.9";
    const TTL: Duration = Duration::minutes(10);

    // ── In-memory store with its own clock, standing in for the database's `now()` ────────

    struct Row {
        hash: Vec<u8>,
        id: DeviceLinkId,
        user_id: UserId,
        state: DeviceLinkState,
        offer: Option<SealedBytes>,
        answer: Option<SealedBytes>,
        created_at: DateTime<Utc>,
        expires_at: DateTime<Utc>,
        offered_at: Option<DateTime<Utc>>,
        answered_at: Option<DateTime<Utc>>,
        ended_at: Option<DateTime<Utc>>,
    }

    struct State {
        now: DateTime<Utc>,
        users: HashSet<UserId>,
        principals: HashSet<UserId>,
        rows: Vec<Row>,
        cleanup_graces: Vec<Duration>,
    }

    type Shared = Arc<Mutex<State>>;

    impl State {
        fn read(&self, row: &Row) -> DeviceLink {
            let expired = row.expires_at <= self.now;
            DeviceLink {
                id: row.id,
                user_id: row.user_id,
                state: row.state,
                expired,
                offer: if expired { None } else { row.offer.clone() },
                answer: if expired { None } else { row.answer.clone() },
                created_at: row.created_at,
                expires_at: row.expires_at,
                offered_at: row.offered_at,
                answered_at: row.answered_at,
                ended_at: row.ended_at,
            }
        }

        /// One conditional update: the row with `hash` in state `from`, unexpired, and the
        /// user's when `user_id` is given.
        fn transition(
            &mut self,
            hash: &[u8],
            from: &[DeviceLinkState],
            user_id: Option<UserId>,
            apply: impl FnOnce(&mut Row, DateTime<Utc>),
        ) -> Option<DeviceLink> {
            let now = self.now;
            let row = self.rows.iter_mut().find(|r| {
                r.hash == hash
                    && from.contains(&r.state)
                    && r.expires_at > now
                    && user_id.is_none_or(|u| u == r.user_id)
            })?;
            apply(row, now);
            let row = self.rows.iter().find(|r| r.hash == hash)?;
            Some(self.read(row))
        }
    }

    fn end(row: &mut Row, state: DeviceLinkState, now: DateTime<Utc>) {
        row.state = state;
        row.ended_at = Some(now);
        row.offer = None;
        row.answer = None;
    }

    struct FakeLinks(Shared);

    #[async_trait]
    impl DeviceLinkRepository for FakeLinks {
        async fn lock_user(&self, user_id: UserId) -> RepoResult<bool> {
            Ok(self.0.lock().unwrap().users.contains(&user_id))
        }

        async fn cancel_live_links(&self, user_id: UserId) -> RepoResult<Vec<DeviceLinkId>> {
            let mut s = self.0.lock().unwrap();
            let now = s.now;
            let mut ids = Vec::new();
            for row in s
                .rows
                .iter_mut()
                .filter(|r| r.user_id == user_id && r.state.is_live())
            {
                end(row, DeviceLinkState::Cancelled, now);
                ids.push(row.id);
            }
            Ok(ids)
        }

        async fn create(&self, link: NewDeviceLink) -> RepoResult<DeviceLink> {
            let mut s = self.0.lock().unwrap();
            let live = s
                .rows
                .iter()
                .any(|r| r.user_id == link.user_id && r.state.is_live());
            if live || s.rows.iter().any(|r| r.hash == link.link_hash) {
                return Err(RepoError::new(RepoErrorKind::Conflict));
            }
            let now = s.now;
            s.rows.push(Row {
                hash: link.link_hash.clone(),
                id: DeviceLinkId::new(),
                user_id: link.user_id,
                state: DeviceLinkState::Open,
                offer: None,
                answer: None,
                created_at: now,
                expires_at: now + link.ttl,
                offered_at: None,
                answered_at: None,
                ended_at: None,
            });
            let row = s.rows.last().unwrap();
            Ok(s.read(row))
        }

        async fn find(&self, link_hash: &[u8]) -> RepoResult<Option<DeviceLink>> {
            let s = self.0.lock().unwrap();
            Ok(s.rows
                .iter()
                .find(|r| r.hash == link_hash)
                .map(|r| s.read(r)))
        }

        async fn offer(
            &self,
            link_hash: &[u8],
            offer: SealedBytes,
        ) -> RepoResult<Option<DeviceLink>> {
            Ok(self.0.lock().unwrap().transition(
                link_hash,
                &[DeviceLinkState::Open],
                None,
                |row, now| {
                    row.state = DeviceLinkState::Offered;
                    row.offer = Some(offer);
                    row.offered_at = Some(now);
                },
            ))
        }

        async fn answer(
            &self,
            link_hash: &[u8],
            user_id: UserId,
            answer: SealedBytes,
            ttl: Duration,
        ) -> RepoResult<Option<DeviceLink>> {
            Ok(self.0.lock().unwrap().transition(
                link_hash,
                &[DeviceLinkState::Offered],
                Some(user_id),
                |row, now| {
                    row.state = DeviceLinkState::Answered;
                    row.answer = Some(answer);
                    row.answered_at = Some(now);
                    row.expires_at = now + ttl;
                },
            ))
        }

        async fn complete(
            &self,
            link_hash: &[u8],
            user_id: UserId,
        ) -> RepoResult<Option<DeviceLink>> {
            Ok(self.0.lock().unwrap().transition(
                link_hash,
                &[DeviceLinkState::Answered],
                Some(user_id),
                |row, now| end(row, DeviceLinkState::Consumed, now),
            ))
        }

        async fn cancel(
            &self,
            link_hash: &[u8],
            user_id: Option<UserId>,
        ) -> RepoResult<Option<DeviceLink>> {
            Ok(self.0.lock().unwrap().transition(
                link_hash,
                &DeviceLinkState::LIVE,
                user_id,
                |row, now| end(row, DeviceLinkState::Cancelled, now),
            ))
        }

        async fn delete_expired(&self, grace: Duration) -> RepoResult<u64> {
            let mut s = self.0.lock().unwrap();
            s.cleanup_graces.push(grace);
            let cutoff = s.now - grace;
            let before = s.rows.len();
            s.rows.retain(|r| r.expires_at >= cutoff);
            Ok((before - s.rows.len()) as u64)
        }
    }

    // ── Harness ───────────────────────────────────────────────────────────────────────────

    struct Harness {
        state: Shared,
        svc: DefaultDeviceLinkService,
    }

    fn user_row(id: UserId, principal_id: Option<String>) -> User {
        let now = Utc::now();
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
        }
    }

    fn user_repo(state: &Shared) -> MockUserRepository {
        let mut user_repo = MockUserRepository::new();
        let users = Arc::clone(state);
        user_repo.expect_get_by_id().returning(move |id| {
            let s = users.lock().unwrap();
            Ok(s.users.contains(&id).then(|| {
                user_row(
                    id,
                    s.principals.contains(&id).then(|| format!("rimp1_{id}")),
                )
            }))
        });
        user_repo
    }

    /// Limits: starts per user, offers and anonymous cancels per IP, answer polls per IP, and
    /// anonymous refusals per IP.
    fn harness_with_limits(start: u32, offer: u32, answer: u32, refusal: u32) -> Harness {
        let state: Shared = Arc::new(Mutex::new(State {
            now: Utc::now(),
            users: HashSet::new(),
            principals: HashSet::new(),
            rows: Vec::new(),
            cleanup_graces: Vec::new(),
        }));
        let repos = DeviceLinkRepos {
            user_repo: Arc::new(user_repo(&state)),
            device_link_repo: Arc::new(FakeLinks(Arc::clone(&state))),
        };
        let svc = DefaultDeviceLinkService::new(repos, None).with_rate_limiters(
            RegistrationRateLimiter::new(3600, start),
            RegistrationRateLimiter::new(3600, offer),
            RegistrationRateLimiter::new(3600, answer),
            RegistrationRateLimiter::new(3600, refusal),
        );
        Harness { state, svc }
    }

    fn harness() -> Harness {
        harness_with_limits(1000, 1000, 1000, 1000)
    }

    fn b64(bytes: &[u8]) -> String {
        URL_SAFE_NO_PAD.encode(bytes)
    }

    fn sealed(byte: u8, len: usize) -> SealedPayload {
        SealedPayload {
            nonce: b64(&[byte; 12]),
            ciphertext: b64(&vec![byte; len]),
        }
    }

    fn offer_bytes() -> SealedPayload {
        sealed(1, 217)
    }

    fn answer_bytes() -> SealedPayload {
        sealed(2, 476)
    }

    impl Harness {
        /// A user, with a key identity when `enrolled`.
        fn user(&self, enrolled: bool) -> Actor {
            let id = UserId::new();
            let mut s = self.state.lock().unwrap();
            s.users.insert(id);
            if enrolled {
                s.principals.insert(id);
            }
            Actor::new(id)
        }

        fn advance(&self, by: Duration) {
            self.state.lock().unwrap().now += by;
        }

        fn now(&self) -> DateTime<Utc> {
            self.state.lock().unwrap().now
        }

        /// Set the link's expiry relative to the store's clock.
        fn expire_in(&self, link_id: &str, by: Duration) {
            let hash = Sha256::digest(URL_SAFE_NO_PAD.decode(link_id).unwrap()).to_vec();
            let mut s = self.state.lock().unwrap();
            let now = s.now;
            let row = s.rows.iter_mut().find(|r| r.hash == hash).unwrap();
            row.expires_at = now + by;
        }

        fn state_of(&self, link_id: &str) -> Option<DeviceLinkState> {
            let hash = Sha256::digest(URL_SAFE_NO_PAD.decode(link_id).unwrap()).to_vec();
            let s = self.state.lock().unwrap();
            s.rows.iter().find(|r| r.hash == hash).map(|r| r.state)
        }

        async fn start(&self, actor: Actor) -> ServiceResult<String> {
            self.svc
                .start(StartDeviceLinkCmd { actor })
                .await
                .map(|started| started.link_id)
        }

        async fn offer(&self, link_id: &str, offer: SealedPayload) -> ServiceResult<()> {
            self.offer_from(IP, link_id, offer).await
        }

        async fn offer_from(
            &self,
            ip: &str,
            link_id: &str,
            offer: SealedPayload,
        ) -> ServiceResult<()> {
            self.svc
                .send_offer(SendDeviceLinkOfferCmd {
                    link_id: link_id.to_string(),
                    offer,
                    remote_ip: ip.to_string(),
                })
                .await
        }

        async fn get(
            &self,
            actor: Actor,
            link_id: &str,
        ) -> ServiceResult<rim_services::device_link::dto::DeviceLinkDto> {
            self.svc
                .get(GetDeviceLinkQuery {
                    actor,
                    link_id: link_id.to_string(),
                })
                .await
        }

        async fn answer(
            &self,
            actor: Actor,
            link_id: &str,
            answer: SealedPayload,
        ) -> ServiceResult<rim_services::device_link::dto::DeviceLinkDto> {
            self.svc
                .send_answer(SendDeviceLinkAnswerCmd {
                    actor,
                    link_id: link_id.to_string(),
                    answer,
                })
                .await
        }

        async fn poll(&self, link_id: &str) -> ServiceResult<Option<SealedPayload>> {
            self.poll_from(IP, link_id).await
        }

        async fn poll_from(&self, ip: &str, link_id: &str) -> ServiceResult<Option<SealedPayload>> {
            self.svc
                .get_answer(GetDeviceLinkAnswerQuery {
                    link_id: link_id.to_string(),
                    remote_ip: ip.to_string(),
                })
                .await
        }

        async fn complete(&self, actor: Actor, link_id: &str) -> ServiceResult<()> {
            self.svc
                .complete(CompleteDeviceLinkCmd {
                    actor,
                    link_id: link_id.to_string(),
                })
                .await
        }

        async fn cancel(&self, actor: Option<Actor>, link_id: &str) -> ServiceResult<()> {
            self.svc
                .cancel(CancelDeviceLinkCmd {
                    actor,
                    link_id: link_id.to_string(),
                    remote_ip: IP.to_string(),
                })
                .await
        }

        /// A started, offered and answered link of `actor`.
        async fn answered(&self, actor: Actor) -> String {
            let link_id = self.start(actor).await.unwrap();
            self.offer(&link_id, offer_bytes()).await.unwrap();
            self.answer(actor, &link_id, answer_bytes()).await.unwrap();
            link_id
        }

        /// A link of `actor` that the new device has completed.
        async fn consumed(&self, actor: Actor) -> String {
            let link_id = self.answered(actor).await;
            self.complete(actor, &link_id).await.unwrap();
            link_id
        }
    }

    fn assert_unauthenticated<T: std::fmt::Debug>(result: ServiceResult<T>, case: &str) {
        assert!(
            matches!(result, Err(ServiceError::Unauthenticated)),
            "{case}: {result:?}"
        );
    }

    fn assert_not_found<T: std::fmt::Debug>(result: ServiceResult<T>, case: &str) {
        assert!(
            matches!(result, Err(ServiceError::NotFound)),
            "{case}: {result:?}"
        );
    }

    fn assert_conflict<T: std::fmt::Debug>(result: ServiceResult<T>, case: &str) {
        assert!(
            matches!(result, Err(ServiceError::Conflict(_))),
            "{case}: {result:?}"
        );
    }

    fn assert_validation<T: std::fmt::Debug>(result: ServiceResult<T>, case: &str) {
        assert!(
            matches!(result, Err(ServiceError::Validation(_))),
            "{case}: {result:?}"
        );
    }

    fn assert_rate_limited<T: std::fmt::Debug>(result: ServiceResult<T>, case: &str) {
        assert!(
            matches!(result, Err(ServiceError::RateLimited { .. })),
            "{case}: {result:?}"
        );
    }

    /// A well-formed link id no link has.
    fn unknown_link_id() -> String {
        b64(&[0xab; 16])
    }

    const MALFORMED_LINK_IDS: &[&str] = &[
        "",
        "short",
        "AAECAwQFBgcICQoLDA0ODw==",
        "AAECAwQFBgcICQoLDA0OD",
        "AAECAwQFBgcICQoLDA0ODx",
        "AAECAwQFBgcICQoLDA0OD+",
    ];

    // ── startDeviceLink ───────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn start_opens_a_link_that_expires_in_ten_minutes_and_stores_only_its_hash() {
        let h = harness();
        let ada = h.user(true);
        let started = h
            .svc
            .start(StartDeviceLinkCmd { actor: ada })
            .await
            .unwrap();
        assert_eq!(started.link_id.len(), 22);
        assert_eq!(started.expires_at, h.now() + TTL);

        let bytes = URL_SAFE_NO_PAD.decode(&started.link_id).unwrap();
        assert_eq!(bytes.len(), 16);
        let s = h.state.lock().unwrap();
        assert_eq!(s.rows.len(), 1);
        assert_eq!(s.rows[0].hash, Sha256::digest(&bytes).to_vec());
        assert_eq!(s.rows[0].user_id, ada.user_id);
        assert_eq!(s.rows[0].state, DeviceLinkState::Open);
        assert_eq!(s.cleanup_graces, vec![Duration::hours(1)]);
    }

    #[tokio::test]
    async fn a_second_start_cancels_the_first_link() {
        let h = harness();
        let ada = h.user(true);
        let first = h.start(ada).await.unwrap();
        h.offer(&first, offer_bytes()).await.unwrap();
        let second = h.start(ada).await.unwrap();
        assert_ne!(first, second);
        assert_eq!(h.state_of(&first), Some(DeviceLinkState::Cancelled));
        assert_eq!(h.state_of(&second), Some(DeviceLinkState::Open));
        assert_eq!(
            h.get(ada, &first).await.unwrap().state,
            DeviceLinkStateDto::Cancelled
        );
        // The first link's offer is gone, and its id no longer works.
        assert_eq!(h.get(ada, &first).await.unwrap().offer, None);
        assert_unauthenticated(h.poll(&first).await, "poll cancelled");

        // An expired live link is cancelled too, so it never blocks the new one.
        h.expire_in(&second, -Duration::seconds(1));
        let third = h.start(ada).await.unwrap();
        assert_eq!(h.state_of(&second), Some(DeviceLinkState::Cancelled));
        assert_eq!(h.state_of(&third), Some(DeviceLinkState::Open));

        // Another user's link is untouched.
        let bob = h.user(true);
        let bobs = h.start(bob).await.unwrap();
        h.start(ada).await.unwrap();
        assert_eq!(h.state_of(&bobs), Some(DeviceLinkState::Open));
    }

    #[tokio::test]
    async fn start_needs_a_key_identity() {
        let h = harness();
        let grace = h.user(false);
        let err = h.start(grace).await.unwrap_err();
        assert!(
            matches!(&err, ServiceError::Validation(m) if m == "this account has no key identity yet"),
            "{err:?}"
        );
        assert!(h.state.lock().unwrap().rows.is_empty());
    }

    #[tokio::test]
    async fn start_for_a_user_that_does_not_exist_is_not_found() {
        let h = harness();
        assert_not_found(h.start(Actor::new(UserId::new())).await, "no such user");
        assert!(h.state.lock().unwrap().rows.is_empty());
    }

    #[tokio::test]
    async fn a_start_refused_for_no_key_identity_leaves_the_live_link_open() {
        let h = harness();
        let ada = h.user(true);
        let link_id = h.start(ada).await.unwrap();
        h.state.lock().unwrap().principals.remove(&ada.user_id);
        assert_validation(h.start(ada).await, "no key identity");
        assert_eq!(h.state_of(&link_id), Some(DeviceLinkState::Open));
        assert_eq!(h.state.lock().unwrap().rows.len(), 1);
    }

    #[tokio::test]
    async fn start_is_limited_per_user() {
        let h = harness_with_limits(2, 1000, 1000, 1000);
        let ada = h.user(true);
        let bob = h.user(true);
        h.start(ada).await.unwrap();
        h.start(ada).await.unwrap();
        assert_rate_limited(h.start(ada).await, "third start");
        h.start(bob).await.unwrap();
    }

    #[tokio::test]
    async fn start_deletes_links_expired_more_than_an_hour_ago() {
        let h = harness();
        let ada = h.user(true);
        let bob = h.user(true);
        let old = h.start(ada).await.unwrap();
        let recent = h.start(bob).await.unwrap();
        h.expire_in(&old, -Duration::minutes(61));
        h.expire_in(&recent, -Duration::minutes(59));
        h.start(h.user(true)).await.unwrap();
        assert_eq!(h.state_of(&old), None);
        assert_eq!(h.state_of(&recent), Some(DeviceLinkState::Open));
        // Once deleted, the link's user gets `NotFound` instead of `EXPIRED`.
        assert_not_found(h.get(ada, &old).await, "deleted link");
        assert_eq!(
            h.get(bob, &recent).await.unwrap().state,
            DeviceLinkStateDto::Expired
        );
    }

    /// The cleanup is best effort: a failure there does not stop the link from starting.
    #[tokio::test]
    async fn a_failed_cleanup_does_not_fail_start() {
        let ada = UserId::new();
        let mut repo = MockDeviceLinkRepository::new();
        repo.expect_delete_expired()
            .withf(|grace| *grace == Duration::hours(1))
            .returning(|_| Box::pin(async { Err(RepoError::new(RepoErrorKind::Unavailable)) }));
        repo.expect_lock_user()
            .returning(|_| Box::pin(async { Ok(true) }));
        repo.expect_cancel_live_links()
            .returning(|_| Box::pin(async { Ok(Vec::new()) }));
        repo.expect_create().returning(|link: NewDeviceLink| {
            let now = Utc::now();
            Box::pin(async move {
                Ok(DeviceLink {
                    id: DeviceLinkId::new(),
                    user_id: link.user_id,
                    state: DeviceLinkState::Open,
                    expired: false,
                    offer: None,
                    answer: None,
                    created_at: now,
                    expires_at: now + link.ttl,
                    offered_at: None,
                    answered_at: None,
                    ended_at: None,
                })
            })
        });
        let mut users = MockUserRepository::new();
        users
            .expect_get_by_id()
            .returning(|id| Ok(Some(user_row(id, Some("rimp1_ada".to_string())))));
        let svc = DefaultDeviceLinkService::new(
            DeviceLinkRepos {
                user_repo: Arc::new(users),
                device_link_repo: Arc::new(repo),
            },
            None,
        );
        let started = svc
            .start(StartDeviceLinkCmd {
                actor: Actor::new(ada),
            })
            .await
            .unwrap();
        assert_eq!(started.link_id.len(), 22);
    }

    /// A storage failure is an internal error, not a refusal, and refunds the refusal slot.
    #[tokio::test]
    async fn a_storage_failure_is_not_a_refusal() {
        let mut repo = MockDeviceLinkRepository::new();
        repo.expect_find()
            .returning(|_| Box::pin(async { Err(RepoError::new(RepoErrorKind::Internal)) }));
        let svc = DefaultDeviceLinkService::new(
            DeviceLinkRepos {
                user_repo: Arc::new(MockUserRepository::new()),
                device_link_repo: Arc::new(repo),
            },
            None,
        )
        .with_rate_limiters(
            RegistrationRateLimiter::new(3600, 10),
            RegistrationRateLimiter::new(3600, 10),
            RegistrationRateLimiter::new(3600, 10),
            RegistrationRateLimiter::new(3600, 1),
        );
        for _ in 0..3 {
            let result = svc
                .get_answer(GetDeviceLinkAnswerQuery {
                    link_id: unknown_link_id(),
                    remote_ip: IP.to_string(),
                })
                .await;
            assert!(
                matches!(result, Err(ServiceError::Internal(_))),
                "{result:?}"
            );
        }
    }

    // ── sendDeviceLinkOffer ───────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn an_offer_moves_an_open_link_to_offered_and_repeats_succeed() {
        let h = harness();
        let ada = h.user(true);
        let link_id = h.start(ada).await.unwrap();
        h.offer(&link_id, offer_bytes()).await.unwrap();
        assert_eq!(h.state_of(&link_id), Some(DeviceLinkState::Offered));
        // The same bytes again (the answer to the first send was lost): success.
        h.offer(&link_id, offer_bytes()).await.unwrap();
        // Other bytes: refused, and the stored offer stays.
        assert_unauthenticated(h.offer(&link_id, sealed(9, 217)).await, "other bytes");
        let link = h.get(ada, &link_id).await.unwrap();
        assert_eq!(link.state, DeviceLinkStateDto::Offered);
        assert_eq!(link.offer, Some(offer_bytes()));
    }

    #[tokio::test]
    async fn sealed_bytes_are_stored_and_returned_byte_exact() {
        let h = harness();
        let ada = h.user(true);
        let link_id = h.start(ada).await.unwrap();
        let ciphertext: Vec<u8> = (0..=255u8).cycle().take(16_400).collect();
        let offer = SealedPayload {
            nonce: b64(&[0xff, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 0xfe]),
            ciphertext: b64(&ciphertext),
        };
        h.offer(&link_id, offer.clone()).await.unwrap();
        assert_eq!(h.get(ada, &link_id).await.unwrap().offer, Some(offer));

        let ciphertext: Vec<u8> = (0..=255u8).rev().cycle().take(65_552).collect();
        let answer = SealedPayload {
            nonce: b64(&[7; 12]),
            ciphertext: b64(&ciphertext),
        };
        h.answer(ada, &link_id, answer.clone()).await.unwrap();
        assert_eq!(h.poll(&link_id).await.unwrap(), Some(answer));
    }

    #[tokio::test]
    async fn malformed_sealed_messages_are_validation_errors_before_the_link_is_looked_up() {
        let h = harness();
        let ada = h.user(true);
        let link_id = h.start(ada).await.unwrap();
        h.offer(&link_id, offer_bytes()).await.unwrap();
        let offers = [
            SealedPayload {
                nonce: b64(&[1; 11]),
                ciphertext: b64(&[1; 40]),
            },
            SealedPayload {
                nonce: b64(&[1; 13]),
                ciphertext: b64(&[1; 40]),
            },
            SealedPayload {
                nonce: format!("{}=", b64(&[1; 12])),
                ciphertext: b64(&[1; 40]),
            },
            SealedPayload {
                nonce: b64(&[1; 12]),
                ciphertext: b64(&[1; 16]),
            },
            SealedPayload {
                nonce: b64(&[1; 12]),
                ciphertext: format!("{}+", b64(&[1; 40])),
            },
            SealedPayload {
                nonce: b64(&[1; 12]),
                ciphertext: format!("{}=", b64(&[1; 40])),
            },
            SealedPayload {
                nonce: b64(&[1; 12]),
                ciphertext: "x".repeat(100_000),
            },
        ];
        for offer in &offers {
            // On a malformed link id too: the encoding is checked first.
            assert_validation(h.offer(&link_id, offer.clone()).await, "offer");
            assert_validation(h.offer("nope", offer.clone()).await, "offer, bad link");
            assert_validation(h.answer(ada, &link_id, offer.clone()).await, "answer");
            assert_validation(
                h.answer(ada, "nope", offer.clone()).await,
                "answer, bad link",
            );
        }
        // Each direction has its own cap; 17 bytes is the smallest accepted ciphertext.
        let link_id = h.start(ada).await.unwrap();
        assert_validation(h.offer(&link_id, sealed(1, 16_401)).await, "offer over cap");
        h.offer(&link_id, sealed(1, 17)).await.unwrap();
        assert_validation(
            h.answer(ada, &link_id, sealed(1, 65_553)).await,
            "answer over cap",
        );
        h.answer(ada, &link_id, sealed(1, 16_401)).await.unwrap();
    }

    #[tokio::test]
    async fn anonymous_offers_on_links_they_cannot_act_on_are_unauthenticated() {
        let h = harness();
        let ada = h.user(true);
        for link_id in MALFORMED_LINK_IDS {
            assert_unauthenticated(h.offer(link_id, offer_bytes()).await, link_id);
        }
        assert_unauthenticated(h.offer(&unknown_link_id(), offer_bytes()).await, "unknown");

        let expired = h.start(ada).await.unwrap();
        h.expire_in(&expired, Duration::zero());
        assert_unauthenticated(h.offer(&expired, offer_bytes()).await, "expired");

        let cancelled = h.start(ada).await.unwrap();
        h.cancel(Some(ada), &cancelled).await.unwrap();
        assert_unauthenticated(h.offer(&cancelled, offer_bytes()).await, "cancelled");

        let answered = h.answered(ada).await;
        assert_unauthenticated(h.offer(&answered, offer_bytes()).await, "answered");
        let consumed = h.consumed(ada).await;
        assert_unauthenticated(h.offer(&consumed, offer_bytes()).await, "consumed");

        // An offer repeated after the link expired is refused too.
        let offered = h.start(ada).await.unwrap();
        h.offer(&offered, offer_bytes()).await.unwrap();
        h.expire_in(&offered, -Duration::seconds(1));
        assert_unauthenticated(h.offer(&offered, offer_bytes()).await, "offered, expired");
    }

    // ── deviceLink ────────────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn device_link_reports_state_expiry_and_offer_to_the_links_user() {
        let h = harness();
        let ada = h.user(true);
        let link_id = h.start(ada).await.unwrap();
        let open = h.get(ada, &link_id).await.unwrap();
        assert_eq!(open.state, DeviceLinkStateDto::Open);
        assert_eq!(open.expires_at, h.now() + TTL);
        assert_eq!(open.offer, None);

        h.offer(&link_id, offer_bytes()).await.unwrap();
        let offered = h.get(ada, &link_id).await.unwrap();
        assert_eq!(offered.state, DeviceLinkStateDto::Offered);
        assert_eq!(offered.offer, Some(offer_bytes()));

        h.answer(ada, &link_id, answer_bytes()).await.unwrap();
        let answered = h.get(ada, &link_id).await.unwrap();
        assert_eq!(answered.state, DeviceLinkStateDto::Answered);
        assert_eq!(answered.offer, Some(offer_bytes()));

        h.complete(ada, &link_id).await.unwrap();
        let consumed = h.get(ada, &link_id).await.unwrap();
        assert_eq!(consumed.state, DeviceLinkStateDto::Consumed);
        assert_eq!(consumed.offer, None);
    }

    #[tokio::test]
    async fn device_link_is_not_found_for_other_users_and_bad_ids() {
        let h = harness();
        let ada = h.user(true);
        let bob = h.user(true);
        let link_id = h.start(ada).await.unwrap();
        assert_not_found(h.get(bob, &link_id).await, "other user");
        assert_not_found(h.get(ada, &unknown_link_id()).await, "unknown");
        for link_id in MALFORMED_LINK_IDS {
            assert_not_found(h.get(ada, link_id).await, link_id);
        }
    }

    #[tokio::test]
    async fn expiry_is_reached_exactly_at_expires_at() {
        let h = harness();
        let ada = h.user(true);
        let link_id = h.start(ada).await.unwrap();
        h.offer(&link_id, offer_bytes()).await.unwrap();

        h.expire_in(&link_id, Duration::milliseconds(1));
        let live = h.get(ada, &link_id).await.unwrap();
        assert_eq!(live.state, DeviceLinkStateDto::Offered);
        assert_eq!(live.offer, Some(offer_bytes()));

        h.advance(Duration::milliseconds(1));
        let expired = h.get(ada, &link_id).await.unwrap();
        assert_eq!(expired.state, DeviceLinkStateDto::Expired);
        // An expired link never comes back with its sealed bytes.
        assert_eq!(expired.offer, None);
        assert_conflict(
            h.answer(ada, &link_id, answer_bytes()).await,
            "answer at expiry",
        );
        assert_unauthenticated(h.poll(&link_id).await, "poll at expiry");
    }

    #[tokio::test]
    async fn a_new_link_expires_after_ten_minutes() {
        let h = harness();
        let ada = h.user(true);
        let link_id = h.start(ada).await.unwrap();
        h.advance(TTL - Duration::milliseconds(1));
        h.offer(&link_id, offer_bytes()).await.unwrap();
        h.advance(Duration::milliseconds(1));
        assert_eq!(
            h.get(ada, &link_id).await.unwrap().state,
            DeviceLinkStateDto::Expired
        );
    }

    // ── sendDeviceLinkAnswer ──────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn an_answer_moves_an_offered_link_to_answered_and_resets_expiry() {
        let h = harness();
        let ada = h.user(true);
        let link_id = h.start(ada).await.unwrap();
        h.offer(&link_id, offer_bytes()).await.unwrap();
        h.advance(Duration::minutes(9));
        let answered = h.answer(ada, &link_id, answer_bytes()).await.unwrap();
        assert_eq!(answered.state, DeviceLinkStateDto::Answered);
        assert_eq!(answered.expires_at, h.now() + TTL);
        assert_eq!(answered.offer, Some(offer_bytes()));

        // Past the original expiry, the answer is still there for the new device.
        h.advance(Duration::minutes(5));
        assert_eq!(h.poll(&link_id).await.unwrap(), Some(answer_bytes()));

        // The same bytes again: success, without moving the expiry again.
        let again = h.answer(ada, &link_id, answer_bytes()).await.unwrap();
        assert_eq!(again.state, DeviceLinkStateDto::Answered);
        assert_eq!(again.expires_at, answered.expires_at);
        // Other bytes: conflict.
        assert_conflict(h.answer(ada, &link_id, sealed(8, 476)).await, "other bytes");
        assert_eq!(h.poll(&link_id).await.unwrap(), Some(answer_bytes()));
    }

    #[tokio::test]
    async fn an_answer_in_the_wrong_state_or_after_expiry_is_a_conflict() {
        let h = harness();
        let ada = h.user(true);
        let open = h.start(ada).await.unwrap();
        assert_conflict(
            h.answer(ada, &open, answer_bytes()).await,
            "before the offer",
        );

        let expired = h.start(ada).await.unwrap();
        h.offer(&expired, offer_bytes()).await.unwrap();
        h.expire_in(&expired, Duration::zero());
        let err = h.answer(ada, &expired, answer_bytes()).await.unwrap_err();
        assert!(
            matches!(&err, ServiceError::Conflict(m) if m == "the link has expired"),
            "{err:?}"
        );

        let consumed = h.consumed(ada).await;
        assert_conflict(h.answer(ada, &consumed, answer_bytes()).await, "consumed");
        let cancelled = h.start(ada).await.unwrap();
        h.offer(&cancelled, offer_bytes()).await.unwrap();
        h.cancel(None, &cancelled).await.unwrap();
        assert_conflict(h.answer(ada, &cancelled, answer_bytes()).await, "cancelled");
    }

    #[tokio::test]
    async fn the_same_answer_after_expiry_is_a_conflict() {
        let h = harness();
        let ada = h.user(true);
        let link_id = h.answered(ada).await;
        h.expire_in(&link_id, Duration::zero());
        assert_conflict(
            h.answer(ada, &link_id, answer_bytes()).await,
            "same bytes, expired",
        );
    }

    #[tokio::test]
    async fn an_answer_to_another_users_link_or_a_bad_id_is_not_found() {
        let h = harness();
        let ada = h.user(true);
        let bob = h.user(true);
        let link_id = h.start(ada).await.unwrap();
        h.offer(&link_id, offer_bytes()).await.unwrap();
        assert_not_found(h.answer(bob, &link_id, answer_bytes()).await, "other user");
        assert_not_found(
            h.answer(ada, &unknown_link_id(), answer_bytes()).await,
            "unknown",
        );
        for bad in MALFORMED_LINK_IDS {
            assert_not_found(h.answer(ada, bad, answer_bytes()).await, bad);
        }
        assert_eq!(h.state_of(&link_id), Some(DeviceLinkState::Offered));
    }

    // ── deviceLinkAnswer ──────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn device_link_answer_is_null_until_answered() {
        let h = harness();
        let ada = h.user(true);
        let link_id = h.start(ada).await.unwrap();
        assert_eq!(h.poll(&link_id).await.unwrap(), None);
        h.offer(&link_id, offer_bytes()).await.unwrap();
        assert_eq!(h.poll(&link_id).await.unwrap(), None);
        h.answer(ada, &link_id, answer_bytes()).await.unwrap();
        assert_eq!(h.poll(&link_id).await.unwrap(), Some(answer_bytes()));
    }

    #[tokio::test]
    async fn device_link_answer_on_links_it_cannot_act_on_is_unauthenticated() {
        let h = harness();
        let ada = h.user(true);
        for link_id in MALFORMED_LINK_IDS {
            assert_unauthenticated(h.poll(link_id).await, link_id);
        }
        assert_unauthenticated(h.poll(&unknown_link_id()).await, "unknown");
        let expired = h.answered(ada).await;
        h.expire_in(&expired, -Duration::seconds(1));
        assert_unauthenticated(h.poll(&expired).await, "expired");
        let consumed = h.consumed(ada).await;
        assert_unauthenticated(h.poll(&consumed).await, "consumed");
        let cancelled = h.answered(ada).await;
        h.cancel(Some(ada), &cancelled).await.unwrap();
        assert_unauthenticated(h.poll(&cancelled).await, "cancelled");
    }

    // ── completeDeviceLink ────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn complete_consumes_an_answered_link_and_repeats_succeed() {
        let h = harness();
        let ada = h.user(true);
        let link_id = h.answered(ada).await;
        h.complete(ada, &link_id).await.unwrap();
        assert_eq!(h.state_of(&link_id), Some(DeviceLinkState::Consumed));
        h.complete(ada, &link_id).await.unwrap();
        // The sealed bytes are gone.
        let s = h.state.lock().unwrap();
        assert!(
            s.rows
                .iter()
                .all(|r| r.offer.is_none() && r.answer.is_none())
        );
        assert!(s.rows[0].ended_at.is_some());
    }

    #[tokio::test]
    async fn complete_in_the_wrong_state_or_after_expiry_is_a_conflict() {
        let h = harness();
        let ada = h.user(true);
        let bob = h.user(true);
        let open = h.start(ada).await.unwrap();
        assert_conflict(h.complete(ada, &open).await, "open");
        h.offer(&open, offer_bytes()).await.unwrap();
        assert_conflict(h.complete(ada, &open).await, "offered");

        let expired = h.answered(ada).await;
        h.expire_in(&expired, Duration::zero());
        assert_conflict(h.complete(ada, &expired).await, "expired");

        let cancelled = h.answered(ada).await;
        h.cancel(Some(ada), &cancelled).await.unwrap();
        assert_conflict(h.complete(ada, &cancelled).await, "cancelled");

        let answered = h.answered(ada).await;
        assert_not_found(h.complete(bob, &answered).await, "other user");
        assert_not_found(h.complete(ada, &unknown_link_id()).await, "unknown");
        assert_not_found(h.complete(ada, MALFORMED_LINK_IDS[2]).await, "malformed");
        assert_eq!(h.state_of(&answered), Some(DeviceLinkState::Answered));
    }

    // ── cancelDeviceLink ──────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn the_links_user_cancels_any_live_link_and_repeats_succeed() {
        let h = harness();
        let ada = h.user(true);
        for prepare in 0..3 {
            let link_id = h.start(ada).await.unwrap();
            if prepare >= 1 {
                h.offer(&link_id, offer_bytes()).await.unwrap();
            }
            if prepare >= 2 {
                h.answer(ada, &link_id, answer_bytes()).await.unwrap();
            }
            h.cancel(Some(ada), &link_id).await.unwrap();
            assert_eq!(h.state_of(&link_id), Some(DeviceLinkState::Cancelled));
            h.cancel(Some(ada), &link_id).await.unwrap();
        }
        // An expired link: `true`, and it stays as it was.
        let expired = h.start(ada).await.unwrap();
        h.expire_in(&expired, Duration::zero());
        h.cancel(Some(ada), &expired).await.unwrap();
        assert_eq!(h.state_of(&expired), Some(DeviceLinkState::Open));
        // A consumed link: conflict.
        let consumed = h.consumed(ada).await;
        assert_conflict(h.cancel(Some(ada), &consumed).await, "consumed");
    }

    #[tokio::test]
    async fn a_signed_in_user_cancelling_an_unknown_or_malformed_id_is_unauthenticated() {
        let h = harness();
        let ada = h.user(true);
        h.start(ada).await.unwrap();
        assert_unauthenticated(h.cancel(Some(ada), &unknown_link_id()).await, "unknown");
        for link_id in MALFORMED_LINK_IDS {
            assert_unauthenticated(h.cancel(Some(ada), link_id).await, link_id);
        }
    }

    #[tokio::test]
    async fn anyone_with_the_link_id_cancels_a_live_link() {
        let h = harness();
        let ada = h.user(true);
        let bob = h.user(true);
        let link_id = h.start(ada).await.unwrap();
        h.offer(&link_id, offer_bytes()).await.unwrap();
        h.cancel(None, &link_id).await.unwrap();
        assert_eq!(h.state_of(&link_id), Some(DeviceLinkState::Cancelled));
        // A session of another user follows the anonymous rules.
        let other = h.start(ada).await.unwrap();
        h.cancel(Some(bob), &other).await.unwrap();
        assert_eq!(h.state_of(&other), Some(DeviceLinkState::Cancelled));
    }

    #[tokio::test]
    async fn anonymous_cancels_of_links_they_cannot_act_on_are_unauthenticated() {
        let h = harness();
        let ada = h.user(true);
        let bob = h.user(true);
        for caller in [None, Some(bob)] {
            for link_id in MALFORMED_LINK_IDS {
                assert_unauthenticated(h.cancel(caller, link_id).await, link_id);
            }
            assert_unauthenticated(h.cancel(caller, &unknown_link_id()).await, "unknown");
            let cancelled = h.start(ada).await.unwrap();
            h.cancel(Some(ada), &cancelled).await.unwrap();
            assert_unauthenticated(h.cancel(caller, &cancelled).await, "cancelled");
            let expired = h.start(ada).await.unwrap();
            h.expire_in(&expired, -Duration::seconds(1));
            assert_unauthenticated(h.cancel(caller, &expired).await, "expired");
            let consumed = h.consumed(ada).await;
            assert_unauthenticated(h.cancel(caller, &consumed).await, "consumed");
        }
    }

    // ── Limits ────────────────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn offers_and_anonymous_cancels_share_a_per_ip_limit() {
        let h = harness_with_limits(1000, 3, 1000, 1000);
        let ada = h.user(true);
        let link_id = h.start(ada).await.unwrap();
        h.offer(&link_id, offer_bytes()).await.unwrap();
        h.offer(&link_id, offer_bytes()).await.unwrap();
        h.cancel(None, &link_id).await.unwrap();
        assert_rate_limited(h.offer(&link_id, offer_bytes()).await, "fourth call");
        assert_rate_limited(h.cancel(None, &link_id).await, "fourth call, cancel");
        // Another IP, the link's user cancelling, and answer polls are not counted here.
        let link_id = h.start(ada).await.unwrap();
        h.offer_from(OTHER_IP, &link_id, offer_bytes())
            .await
            .unwrap();
        h.poll(&link_id).await.unwrap();
        h.cancel(Some(ada), &link_id).await.unwrap();
    }

    #[tokio::test]
    async fn answer_polls_are_limited_per_ip() {
        let h = harness_with_limits(1000, 1000, 3, 1000);
        let ada = h.user(true);
        let link_id = h.start(ada).await.unwrap();
        for _ in 0..3 {
            assert_eq!(h.poll(&link_id).await.unwrap(), None);
        }
        assert_rate_limited(h.poll(&link_id).await, "fourth poll");
        assert_eq!(h.poll_from(OTHER_IP, &link_id).await.unwrap(), None);
        h.offer(&link_id, offer_bytes()).await.unwrap();
    }

    #[tokio::test]
    async fn anonymous_refusals_are_limited_per_ip_and_successes_are_refunded() {
        let h = harness_with_limits(1000, 1000, 1000, 2);
        let ada = h.user(true);
        let link_id = h.start(ada).await.unwrap();
        // Successes and validation errors never use up the refusal budget.
        for _ in 0..5 {
            assert_eq!(h.poll(&link_id).await.unwrap(), None);
        }
        for _ in 0..5 {
            assert_validation(h.offer(&link_id, sealed(1, 3)).await, "validation");
        }
        h.offer(&link_id, offer_bytes()).await.unwrap();
        h.offer(&link_id, offer_bytes()).await.unwrap();

        // Two refusals use it up...
        assert_unauthenticated(h.poll(&unknown_link_id()).await, "first refusal");
        assert_unauthenticated(h.cancel(None, &unknown_link_id()).await, "second refusal");
        // ...and then every anonymous call from that IP is refused, valid or not.
        assert_rate_limited(h.poll(&link_id).await, "poll after refusals");
        assert_rate_limited(
            h.offer(&link_id, offer_bytes()).await,
            "offer after refusals",
        );
        assert_rate_limited(h.cancel(None, &link_id).await, "cancel after refusals");
        // Other IPs and session calls are unaffected.
        assert_eq!(h.poll_from(OTHER_IP, &link_id).await.unwrap(), None);
        assert_eq!(
            h.get(ada, &link_id).await.unwrap().state,
            DeviceLinkStateDto::Offered
        );
    }

    #[test]
    fn sealed_message_caps_match_the_reference_implementation() {
        assert_eq!(
            MAX_OFFER_CIPHERTEXT_LEN,
            Direction::Offer.max_ciphertext_len()
        );
        assert_eq!(
            MAX_ANSWER_CIPHERTEXT_LEN,
            Direction::Answer.max_ciphertext_len()
        );
    }
}
