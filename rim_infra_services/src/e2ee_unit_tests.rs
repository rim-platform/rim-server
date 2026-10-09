#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};
    use std::sync::{Arc, Mutex};

    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use chrono::{DateTime, Duration, Utc};

    use uuid::Uuid;

    use rim_core::conversation::model::{Conversation, ConversationId, ConversationType};
    use rim_core::conversation::repository::MockConversationRepository;
    use rim_core::conversation_member::repository::MockConversationMemberRepository;
    use rim_core::e2ee::model::{
        E2eeDevice, MlsDevice, MlsGroup, MlsGroupDevice, MlsKeyPackage, MlsKeyPackageId,
        MlsReaddRequest, MlsReaddRequestId, MlsWelcomeId,
    };
    use rim_core::e2ee::repository::E2eeRepository;
    use rim_core::key_identity::model::{DeviceKey, DeviceKeyId};
    use rim_core::key_identity::repository::MockKeyIdentityRepository;
    use rim_core::message::model::{Message, MessageEncryption, MessageId};
    use rim_core::message::repository::MockMessageRepository;
    use rim_core::moderation::model::{MessageReport, MessageReportId, MessageReportStatus};
    use rim_core::moderation::repository::MockModerationRepository;
    use rim_core::repository::error::{RepoError, RepoErrorKind};
    use rim_core::server::model::ServerId;
    use rim_core::server_member::model::{MemberStanding, ServerMemberRole};
    use rim_core::server_member::repository::MockServerMemberRepository;
    use rim_core::user::model::UserId;
    use rim_identity::StatementVersion;
    use rim_identity::testing::{
        SigningKey, TestIdentity, device_mls_key, device_mls_readd, signing_key,
    };
    use rim_services::e2ee::command::{
        AckMlsWelcomesCmd, ClaimMlsKeyPackagesCmd, GetMlsGroupQuery, ListMlsCommitsQuery,
        MlsCaller, MlsDeviceRef, PrincipalDevicesQuery, ReportEncryptedMessageCmd,
        RequestMlsReaddCmd, SubmitMlsCommitCmd, UploadMlsKeyPackagesCmd,
    };
    use rim_services::e2ee::service::E2eeService;

    use rim_services::e2ee::{MlsCommitRequiredReason, mls_group_id};
    use rim_services::events::{Broadcaster, Observers, WorkspaceEvent};
    use rim_services::{Actor, ServiceError};

    use crate::e2ee::franking::{ReceiptFields, commitment, receipt, sent_at_us};
    use crate::e2ee::openmls_wire::OpenMlsWire;
    use crate::e2ee::{DefaultE2eeService, E2eeRepos};
    use crate::registration_rate_limiter::RegistrationRateLimiter;

    use crate::e2ee_test_support::{
        EMPTY_COMMIT, FakeE2ee, FakeWire, SUITE, StoredPackage, b64, frame, welcome_bytes,
    };

    // ── Fixture ────────────────────────────────────────────────────────────────────────

    struct RecordingBroadcaster(Mutex<Vec<WorkspaceEvent>>);

    impl Broadcaster for RecordingBroadcaster {
        fn broadcast(&self, event: WorkspaceEvent) {
            self.0.lock().expect("events").push(event);
        }
    }

    #[derive(Clone)]
    struct Device {
        user: UserId,
        id: DeviceKeyId,
        device_id: Uuid,
        principal: String,
        key: SigningKey,
        mls: SigningKey,
    }

    impl Device {
        fn caller(&self) -> MlsCaller {
            MlsCaller {
                actor: Actor::new(self.user),
                device_key_id: Some(self.id),
            }
        }

        fn as_ref(&self) -> MlsDeviceRef {
            MlsDeviceRef {
                principal_id: self.principal.clone(),
                device_id: self.device_id,
            }
        }

        fn mls_device(&self) -> MlsDevice {
            MlsDevice {
                device_key_id: self.id,
                user_id: self.user,
                principal_id: self.principal.clone(),
                device_id: self.device_id,
            }
        }
    }

    type Messages = Arc<Mutex<HashMap<MessageId, Message>>>;
    type Reports = Arc<Mutex<Vec<(MessageId, UserId, Option<Vec<u8>>)>>>;

    struct Fixture {
        repo: Arc<FakeE2ee>,
        events: Arc<RecordingBroadcaster>,
        service: DefaultE2eeService,
        server: ServerId,
        conv: Conversation,
        alice: Device,
        bob: Device,
        bob2: Device,
        messages: Messages,
        reports: Reports,
    }

    fn principal(seed: u8) -> String {
        TestIdentity::with_version(seed, StatementVersion::V2).principal_id()
    }

    fn device(user: UserId, principal_id: &str, seed: u8) -> Device {
        Device {
            user,
            id: DeviceKeyId::new(),
            device_id: Uuid::now_v7(),
            principal: principal_id.to_string(),
            key: signing_key(seed),
            mls: signing_key(seed.wrapping_add(100)),
        }
    }

    fn stored_device(d: &Device) -> E2eeDevice {
        E2eeDevice {
            device_key_id: d.id,
            user_id: d.user,
            device_id: d.device_id,
            principal_id: Some(d.principal.clone()),
            public_key: rim_identity::PublicKey::from(&d.key).to_string(),
            revoked_at: None,
            last_key_package_upload_at: Some(Utc::now()),
        }
    }

    fn readd_request(f: &Fixture, d: &Device, age: Duration) -> MlsReaddRequest {
        MlsReaddRequest {
            id: MlsReaddRequestId::new(),
            conversation_id: f.conv.id,
            incarnation: 1,
            device: d.mls_device(),
            payload: "{}".to_string(),
            signature: "sig".to_string(),
            created_at: Utc::now() - age,
            fulfilled_epoch: None,
        }
    }

    impl Fixture {
        /// A group conversation of alice (its creator, one device) and bob (two devices),
        /// created encrypted an hour ago, with no MLS group yet.
        fn new() -> Self {
            Self::with_conversation(ConversationType::Group, true)
        }

        fn with_conversation(kind: ConversationType, encrypted: bool) -> Self {
            let server = ServerId::new();
            let (alice_id, bob_id) = (UserId::new(), UserId::new());
            let (pa, pb) = (principal(1), principal(2));
            let alice = device(alice_id, &pa, 21);
            let bob = device(bob_id, &pb, 22);
            let bob2 = device(bob_id, &pb, 23);
            let created_at = Utc::now() - Duration::hours(1);
            let conv = Conversation {
                id: ConversationId::new(),
                conversation_type: kind,
                title: None,
                topic: None,
                created_at,
                last_activity_at: created_at,
                server_id: server,
                is_private: false,
                archived_at: None,
                created_by: Some(alice_id),
                slow_mode_seconds: 0,
                encrypted_at: encrypted.then_some(created_at),
            };
            let repo = Arc::new(FakeE2ee::default());
            {
                let mut state = repo.state();
                state.devices = vec![
                    stored_device(&alice),
                    stored_device(&bob),
                    stored_device(&bob2),
                ];
                state.members.insert(conv.id, vec![alice_id, bob_id]);
                state.principals.insert(alice_id, pa);
                state.principals.insert(bob_id, pb);
            }
            let events = Arc::new(RecordingBroadcaster(Mutex::new(Vec::new())));
            let messages: Messages = Arc::new(Mutex::new(HashMap::new()));
            let reports: Reports = Arc::new(Mutex::new(Vec::new()));
            let service = Self::build(&repo, &events, &conv, &messages, &reports);
            Self {
                repo,
                events,
                service,
                server,
                conv,
                alice,
                bob,
                bob2,
                messages,
                reports,
            }
        }

        fn build(
            repo: &Arc<FakeE2ee>,
            events: &Arc<RecordingBroadcaster>,
            conv: &Conversation,
            messages: &Messages,
            reports: &Reports,
        ) -> DefaultE2eeService {
            let mut server_members = MockServerMemberRepository::new();
            server_members
                .expect_get_member_standing()
                .returning(|_, _| Ok(Some(MemberStanding::new(ServerMemberRole::Member))));
            let mut conversations = MockConversationRepository::new();
            let stored = conv.clone();
            conversations.expect_get_by_id().returning(move |id| {
                let found = (id == stored.id).then(|| stored.clone());
                Box::pin(async move { Ok(found) })
            });
            let stored = conv.clone();
            conversations.expect_get_by_ids().returning(move |ids| {
                let found: Vec<_> = ids
                    .contains(&stored.id)
                    .then(|| stored.clone())
                    .into_iter()
                    .collect();
                Box::pin(async move { Ok(found) })
            });
            let mut members = MockConversationMemberRepository::new();
            let shared = Arc::clone(repo);
            members.expect_is_member().returning(move |c, u| {
                let found = shared
                    .state()
                    .members
                    .get(&c)
                    .is_some_and(|m| m.contains(&u));
                Box::pin(async move { Ok(found) })
            });
            let shared = Arc::clone(repo);
            members.expect_list_member_ids().returning(move |c| {
                let found = shared.state().members.get(&c).cloned().unwrap_or_default();
                Box::pin(async move { Ok(found) })
            });
            let mut keys = MockKeyIdentityRepository::new();
            let shared = Arc::clone(repo);
            keys.expect_list_devices().returning(move |user| {
                let devices: Vec<DeviceKey> = shared
                    .state()
                    .devices
                    .iter()
                    .filter(|d| d.user_id == user)
                    .map(|d| DeviceKey {
                        id: d.device_key_id,
                        user_id: d.user_id,
                        device_id: d.device_id,
                        public_key: d.public_key.clone(),
                        name: "Laptop".to_string(),
                        cert_payload: "{}".to_string(),
                        cert_signature: "sig".to_string(),
                        key_seq: 0,
                        added_at: Utc::now(),
                        last_used_at: None,
                        revoked_at: d.revoked_at,
                        revocation_payload: None,
                        revocation_signature: None,
                    })
                    .collect();
                Box::pin(async move { Ok(devices) })
            });
            let mut message_repo = MockMessageRepository::new();
            let shared_messages = Arc::clone(messages);
            message_repo.expect_get_by_id().returning(move |id| {
                let found = shared_messages.lock().expect("messages").get(&id).cloned();
                Box::pin(async move { Ok(found) })
            });
            let mut moderation = MockModerationRepository::new();
            let shared_reports = Arc::clone(reports);
            moderation.expect_create_report().returning(move |report| {
                let mut reports = shared_reports.lock().expect("reports");
                let duplicate = reports
                    .iter()
                    .any(|(m, u, _)| *m == report.message_id && *u == report.reporter_user_id);
                let result = if duplicate {
                    Err(RepoError::new(RepoErrorKind::Conflict))
                } else {
                    reports.push((
                        report.message_id,
                        report.reporter_user_id,
                        report.franked_content.clone(),
                    ));
                    Ok(MessageReport {
                        id: MessageReportId::new(),
                        server_id: report.server_id,
                        conversation_id: report.conversation_id,
                        message_id: report.message_id,
                        reporter_user_id: report.reporter_user_id,
                        reason: report.reason,
                        status: MessageReportStatus::Open,
                        resolved_by: None,
                        resolved_at: None,
                        created_at: Utc::now(),
                    })
                };
                Box::pin(async move { result })
            });
            DefaultE2eeService::new(
                E2eeRepos {
                    e2ee_repo: Arc::clone(repo) as Arc<dyn E2eeRepository>,
                    conversation_repo: Arc::new(conversations),
                    conversation_member_repo: Arc::new(members),
                    server_member_repo: Arc::new(server_members),
                    key_identity_repo: Arc::new(keys),
                    message_repo: Arc::new(message_repo),
                    moderation_repo: Arc::new(moderation),
                },
                None,
                Arc::new(FakeWire(OpenMlsWire::new())),
                Observers::new(Arc::clone(events) as Arc<dyn Broadcaster>),
            )
        }

        /// Rebuild the service, e.g. after changing the conversation.
        fn rebuild(&mut self) {
            self.service = Self::build(
                &self.repo,
                &self.events,
                &self.conv,
                &self.messages,
                &self.reports,
            );
        }

        /// Rebuild the service with other limits: devices claimed per user, one-time claims
        /// per target device, re-add requests per device and conversation.
        fn with_limits(mut self, claims: u32, one_time: u32, readds: u32) -> Self {
            self.rebuild();
            self.service = std::mem::replace(
                &mut self.service,
                Self::build(
                    &self.repo,
                    &self.events,
                    &self.conv,
                    &self.messages,
                    &self.reports,
                ),
            )
            .with_rate_limiters(
                RegistrationRateLimiter::new(3600, claims),
                RegistrationRateLimiter::new(86_400, one_time),
                RegistrationRateLimiter::new(86_400, readds),
            );
            self
        }

        fn events(&self) -> Vec<WorkspaceEvent> {
            std::mem::take(&mut *self.events.0.lock().expect("events"))
        }

        /// Store a key package of `d`, claimed by `claimer` that long ago when given, for the
        /// current incarnation (1 without a group).
        fn package(
            &self,
            d: &Device,
            tag: u8,
            last_resort: bool,
            claimed: Option<(&Device, Duration)>,
        ) -> Vec<u8> {
            let key_package_ref = vec![tag; 32];
            let mut state = self.repo.state();
            let incarnation = state
                .groups
                .iter()
                .filter(|g| g.conversation_id == self.conv.id && g.retired_at.is_none())
                .map(|g| g.incarnation)
                .max()
                .unwrap_or(1);
            state.packages.push(StoredPackage {
                meta: MlsKeyPackage {
                    id: MlsKeyPackageId::new(),
                    user_id: d.user,
                    device_key_id: d.id,
                    cipher_suite: SUITE,
                    key_package_ref: key_package_ref.clone(),
                    last_resort,
                    not_after: Utc::now() + Duration::days(30),
                    created_at: Utc::now(),
                    claimed_at: claimed.map(|(_, ago)| Utc::now() - ago),
                    claimed_by_device_key_id: claimed.map(|(by, _)| by.id),
                    claimed_for_conversation_id: claimed.map(|_| self.conv.id),
                    claimed_for_incarnation: claimed.map(|_| incarnation),
                },
                bytes: vec![tag; 64],
                claims: 0,
            });
            key_package_ref
        }

        /// A current group at `epoch` whose leaves are `devices`.
        fn group(&self, incarnation: i32, epoch: i64, devices: &[&Device]) {
            let mut state = self.repo.state();
            state.groups.push(MlsGroup {
                conversation_id: self.conv.id,
                incarnation,
                group_id: mls_group_id(self.conv.id, incarnation.unsigned_abs()),
                cipher_suite: SUITE,
                epoch,
                created_by_device_key_id: None,
                created_at: Utc::now() - Duration::hours(1),
                retired_at: None,
            });
            for d in devices {
                state.group_devices.push((
                    self.conv.id,
                    incarnation,
                    MlsGroupDevice {
                        device: d.mls_device(),
                        added_epoch: 0,
                    },
                ));
            }
        }

        fn commit_cmd(
            &self,
            by: &Device,
            incarnation: i32,
            epoch: i64,
            adds: &[(&Device, &[u8])],
            removes: &[&Device],
        ) -> SubmitMlsCommitCmd {
            let aad = serde_json::json!({
                "v": 1,
                "type": "commit",
                "adds": adds.iter().map(|(d, r)| serde_json::json!({
                    "principalId": d.principal,
                    "deviceId": d.device_id,
                    "keyPackageRef": b64(r),
                })).collect::<Vec<_>>(),
                "removes": removes.iter().map(|d| serde_json::json!({
                    "principalId": d.principal,
                    "deviceId": d.device_id,
                })).collect::<Vec<_>>(),
            });
            SubmitMlsCommitCmd {
                caller: by.caller(),
                server_id: self.server,
                conversation_id: self.conv.id,
                incarnation,
                epoch,
                cipher_suite: (epoch == 0).then_some(SUITE),
                commit: frame(
                    mls_group_id(self.conv.id, incarnation.unsigned_abs()),
                    epoch.unsigned_abs(),
                    "commit",
                    &aad.to_string(),
                ),
                welcome: (!adds.is_empty())
                    .then(|| welcome_bytes(adds.iter().map(|(_, r)| r.to_vec()).collect())),
            }
        }

        async fn submit(&self, cmd: SubmitMlsCommitCmd) -> Result<(), ServiceError> {
            self.service.submit_commit(cmd).await.map(|_| ())
        }

        fn group_devices(&self, incarnation: i32) -> Vec<DeviceKeyId> {
            self.repo
                .state()
                .group_devices
                .iter()
                .filter(|(c, i, _)| *c == self.conv.id && *i == incarnation)
                .map(|(_, _, d)| d.device.device_key_id)
                .collect()
        }

        fn leave(&self, user: UserId) {
            self.repo
                .state()
                .members
                .get_mut(&self.conv.id)
                .expect("members")
                .retain(|u| *u != user);
        }

        fn group_query(&self, user: UserId) -> GetMlsGroupQuery {
            GetMlsGroupQuery {
                actor: Actor::new(user),
                server_id: self.server,
                conversation_id: self.conv.id,
            }
        }
    }

    fn validation(err: ServiceError) -> String {
        match err {
            ServiceError::Validation(message) => message,
            other => panic!("expected a validation error, got {other:?}"),
        }
    }

    fn forbidden(err: ServiceError) -> String {
        match err {
            ServiceError::Forbidden(message) => message,
            other => panic!("expected forbidden, got {other:?}"),
        }
    }

    // ── Sessions and devices ───────────────────────────────────────────────────────────

    #[tokio::test]
    async fn every_mls_call_needs_a_device_bound_session() {
        let f = Fixture::new();
        let unbound = MlsCaller {
            actor: Actor::new(f.alice.user),
            device_key_id: None,
        };
        let not_bound = "this session is not bound to a device";
        let status = f.service.key_package_status(unbound).await.unwrap_err();
        assert_eq!(forbidden(status), not_bound);
        let welcomes = f.service.pending_welcomes(unbound).await.unwrap_err();
        assert_eq!(forbidden(welcomes), not_bound);
        let err = f
            .service
            .upload_key_packages(UploadMlsKeyPackagesCmd {
                caller: unbound,
                key_packages: vec!["AA".to_string()],
            })
            .await
            .unwrap_err();
        assert_eq!(forbidden(err), not_bound);
        let mut cmd = f.commit_cmd(&f.alice, 1, 0, &[], &[]);
        cmd.caller = unbound;
        assert_eq!(forbidden(f.submit(cmd).await.unwrap_err()), not_bound);
        let err = f
            .service
            .claim_key_packages(ClaimMlsKeyPackagesCmd {
                caller: unbound,
                server_id: f.server,
                conversation_id: f.conv.id,
                devices: vec![f.bob.as_ref()],
            })
            .await
            .unwrap_err();
        assert_eq!(forbidden(err), not_bound);
        let err = f
            .service
            .ack_welcomes(AckMlsWelcomesCmd {
                caller: unbound,
                ids: vec![],
            })
            .await
            .unwrap_err();
        assert_eq!(forbidden(err), not_bound);

        // Another user's device, or a revoked one, is no better.
        let mut theirs = f.alice.caller();
        theirs.device_key_id = Some(f.bob.id);
        let err = f.service.key_package_status(theirs).await.unwrap_err();
        assert_eq!(forbidden(err), not_bound);
        f.repo.state().devices[0].revoked_at = Some(Utc::now());
        let err = f
            .service
            .key_package_status(f.alice.caller())
            .await
            .unwrap_err();
        assert_eq!(forbidden(err), not_bound);
    }

    #[tokio::test]
    async fn channels_are_never_encrypted() {
        let f = Fixture::with_conversation(ConversationType::Channel, false);
        let err = f
            .submit(f.commit_cmd(&f.alice, 1, 0, &[], &[]))
            .await
            .unwrap_err();
        assert_eq!(
            validation(err),
            "only direct messages and groups are encrypted"
        );
    }

    // ── Creating and migrating a group ─────────────────────────────────────────────────

    #[tokio::test]
    async fn the_first_commit_creates_the_group_and_delivers_welcomes() {
        let f = Fixture::with_conversation(ConversationType::Group, false);
        let r1 = f.package(&f.bob, 1, false, Some((&f.alice, Duration::minutes(5))));
        let r2 = f.package(&f.bob2, 2, true, Some((&f.alice, Duration::minutes(5))));
        let cmd = f.commit_cmd(&f.alice, 1, 0, &[(&f.bob, &r1), (&f.bob2, &r2)], &[]);
        let commit = f.service.submit_commit(cmd).await.expect("accepted");
        assert_eq!((commit.incarnation, commit.epoch), (1, 0));
        assert_eq!(commit.sender_device_id, Some(f.alice.device_id));

        let group = f
            .repo
            .current_group(f.conv.id)
            .await
            .unwrap()
            .expect("group");
        assert_eq!(group.epoch, 1);
        assert_eq!(group.group_id, mls_group_id(f.conv.id, 1));
        assert_eq!(f.group_devices(1).len(), 3);
        {
            let state = f.repo.state();
            assert!(
                state.encrypted.contains(&f.conv.id),
                "the first Commit marks a plaintext conversation encrypted"
            );
            assert_eq!(state.welcomes.len(), 1);
            assert_eq!(state.welcomes[0].1.epoch, 1);
            assert_eq!(state.welcomes[0].1.recipients.len(), 2);
            assert_eq!(
                state.packages.len(),
                1,
                "the one-time package is consumed, the last-resort one stays"
            );
        }

        let events = f.events();
        assert!(events.iter().any(|e| matches!(
            e,
            WorkspaceEvent::MlsCommitAccepted {
                incarnation: 1,
                epoch: 0,
                ..
            }
        )));
        let welcomed: HashSet<Uuid> = events
            .iter()
            .filter_map(|e| match e {
                WorkspaceEvent::MlsWelcomeAvailable {
                    user_id,
                    device_id,
                    member_user_ids,
                    ..
                } => {
                    assert_eq!(member_user_ids, &vec![*user_id]);
                    Some(*device_id)
                }
                _ => None,
            })
            .collect();
        assert_eq!(welcomed, HashSet::from([f.bob.device_id, f.bob2.device_id]));
        assert!(
            events
                .iter()
                .any(|e| matches!(e, WorkspaceEvent::MlsMembershipChanged { .. }))
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, WorkspaceEvent::ConversationUpdated { .. }))
        );

        let page = f
            .service
            .list_commits(ListMlsCommitsQuery {
                actor: Actor::new(f.bob.user),
                server_id: f.server,
                conversation_id: f.conv.id,
                incarnation: 1,
                after_epoch: None,
                limit: None,
            })
            .await
            .unwrap();
        assert_eq!(page.commits.len(), 1);
        assert!(!page.has_more);
        let err = f
            .service
            .list_commits(ListMlsCommitsQuery {
                actor: Actor::new(f.bob.user),
                server_id: f.server,
                conversation_id: f.conv.id,
                incarnation: 1,
                after_epoch: None,
                limit: Some(101),
            })
            .await
            .unwrap_err();
        assert!(validation(err).contains("limit"));
    }

    #[tokio::test]
    async fn the_first_commit_needs_every_member_and_an_active_committer() {
        let f = Fixture::new();
        let err = f
            .submit(f.commit_cmd(&f.alice, 1, 0, &[], &[]))
            .await
            .unwrap_err();
        assert_eq!(
            validation(err),
            "every member needs at least one device in the group"
        );

        f.repo.state().devices[0].last_key_package_upload_at = None;
        let r = f.package(&f.bob, 1, false, Some((&f.alice, Duration::minutes(1))));
        let err = f
            .submit(f.commit_cmd(&f.alice, 1, 0, &[(&f.bob, &r)], &[]))
            .await
            .unwrap_err();
        assert!(validation(err).contains("must upload key packages"));
    }

    #[tokio::test]
    async fn the_creator_creates_the_group_first() {
        // A plaintext group is upgraded by its creator.
        let f = Fixture::with_conversation(ConversationType::Group, false);
        let r = f.package(&f.alice, 1, false, Some((&f.bob, Duration::minutes(1))));
        let err = f
            .submit(f.commit_cmd(&f.bob, 1, 0, &[(&f.alice, &r)], &[]))
            .await
            .unwrap_err();
        assert!(forbidden(err).contains("creator"));

        // In a DM either member may.
        let f = Fixture::with_conversation(ConversationType::Direct, false);
        let r = f.package(&f.alice, 1, false, Some((&f.bob, Duration::minutes(1))));
        f.submit(f.commit_cmd(&f.bob, 1, 0, &[(&f.alice, &r)], &[]))
            .await
            .expect("a DM member upgrades");

        // A group created encrypted a moment ago: only the creator, for now.
        let mut f = Fixture::new();
        f.conv.created_at = Utc::now();
        f.conv.encrypted_at = Some(f.conv.created_at);
        f.rebuild();
        let r = f.package(&f.alice, 1, false, Some((&f.bob, Duration::minutes(1))));
        let err = f
            .submit(f.commit_cmd(&f.bob, 1, 0, &[(&f.alice, &r)], &[]))
            .await
            .unwrap_err();
        assert!(forbidden(err).contains("creator"));

        // An hour later, any member.
        let f = Fixture::new();
        let r = f.package(&f.alice, 1, false, Some((&f.bob, Duration::minutes(1))));
        f.submit(f.commit_cmd(&f.bob, 1, 0, &[(&f.alice, &r)], &[]))
            .await
            .expect("after the head start");
    }

    #[tokio::test]
    async fn the_first_commit_needs_an_accepted_cipher_suite() {
        let f = Fixture::new();
        let r = f.package(&f.bob, 1, false, Some((&f.alice, Duration::minutes(1))));
        let mut cmd = f.commit_cmd(&f.alice, 1, 0, &[(&f.bob, &r)], &[]);
        cmd.cipher_suite = None;
        assert!(validation(f.submit(cmd).await.unwrap_err()).contains("cipherSuite"));
        let mut cmd = f.commit_cmd(&f.alice, 1, 0, &[(&f.bob, &r)], &[]);
        cmd.cipher_suite = Some(1);
        assert!(validation(f.submit(cmd).await.unwrap_err()).contains("not accepted"));
        let cmd = f.commit_cmd(&f.alice, 2, 0, &[(&f.bob, &r)], &[]);
        assert!(validation(f.submit(cmd).await.unwrap_err()).contains("next incarnation is 1"));
    }

    // ── The checks table ───────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn the_commit_must_be_a_private_message_commit() {
        let f = Fixture::new();
        f.group(1, 3, &[&f.alice, &f.bob, &f.bob2]);
        let group_id = mls_group_id(f.conv.id, 1);
        let mut cmd = f.commit_cmd(&f.alice, 1, 3, &[], &[]);
        cmd.commit = frame(group_id.clone(), 3, "application", EMPTY_COMMIT);
        assert_eq!(
            validation(f.submit(cmd).await.unwrap_err()),
            "the PrivateMessage is not a Commit"
        );
        let mut cmd = f.commit_cmd(&f.alice, 1, 3, &[], &[]);
        cmd.commit = b64(b"garbage");
        assert_eq!(
            validation(f.submit(cmd).await.unwrap_err()),
            "not a valid MLSMessage"
        );
        let mut cmd = f.commit_cmd(&f.alice, 1, 3, &[], &[]);
        cmd.commit = "not base64!".to_string();
        assert!(validation(f.submit(cmd).await.unwrap_err()).contains("base64url"));
        let mut cmd = f.commit_cmd(&f.alice, 1, 3, &[], &[]);
        cmd.commit = frame(
            group_id,
            3,
            "commit",
            r#"{"v":1,"type":"commit","adds":[]}"#,
        );
        assert!(validation(f.submit(cmd).await.unwrap_err()).contains("authenticated data"));
    }

    #[tokio::test]
    async fn group_ids_and_epochs_must_be_current() {
        let f = Fixture::new();
        f.group(2, 5, &[&f.alice, &f.bob, &f.bob2]);
        // The Commit names incarnation 1: stale.
        let mut cmd = f.commit_cmd(&f.alice, 2, 5, &[], &[]);
        cmd.commit = frame(mls_group_id(f.conv.id, 1), 5, "commit", EMPTY_COMMIT);
        assert!(matches!(
            f.submit(cmd).await.unwrap_err(),
            ServiceError::StaleEpoch {
                incarnation: 2,
                epoch: 5
            }
        ));
        // Another conversation's group: validation.
        let mut cmd = f.commit_cmd(&f.alice, 2, 5, &[], &[]);
        cmd.commit = frame(
            mls_group_id(ConversationId::new(), 2),
            5,
            "commit",
            EMPTY_COMMIT,
        );
        assert!(validation(f.submit(cmd).await.unwrap_err()).contains("group id"));
        // An old epoch or incarnation: stale, naming the current ones.
        for (incarnation, epoch) in [(2, 4), (1, 4)] {
            let err = f
                .submit(f.commit_cmd(&f.alice, incarnation, epoch, &[], &[]))
                .await
                .unwrap_err();
            assert!(matches!(
                err,
                ServiceError::StaleEpoch {
                    incarnation: 2,
                    epoch: 5
                }
            ));
        }
        // The header's epoch must be the request's.
        let mut cmd = f.commit_cmd(&f.alice, 2, 5, &[], &[]);
        cmd.commit = frame(mls_group_id(f.conv.id, 2), 6, "commit", EMPTY_COMMIT);
        assert!(validation(f.submit(cmd).await.unwrap_err()).contains("epoch differs"));
        // A self-update in the current epoch passes.
        f.submit(f.commit_cmd(&f.alice, 2, 5, &[], &[]))
            .await
            .expect("self-update");
        let events = f.events();
        assert!(
            events
                .iter()
                .any(|e| matches!(e, WorkspaceEvent::MlsCommitAccepted { epoch: 5, .. }))
        );
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, WorkspaceEvent::MlsMembershipChanged { .. }))
        );
    }

    #[tokio::test]
    async fn the_committer_must_be_a_leaf_that_is_not_leaving() {
        let f = Fixture::new();
        f.group(1, 1, &[&f.alice, &f.bob]);
        let err = f
            .submit(f.commit_cmd(&f.bob2, 1, 1, &[], &[]))
            .await
            .unwrap_err();
        assert!(forbidden(err).contains("not in the conversation's group"));
        // Two months without a key package upload: the device is due for removal.
        f.repo.state().devices[1].last_key_package_upload_at =
            Some(Utc::now() - Duration::days(61));
        let err = f
            .submit(f.commit_cmd(&f.bob, 1, 1, &[], &[]))
            .await
            .unwrap_err();
        assert!(forbidden(err).contains("waiting to be removed"));
        // A member who left cannot commit at all.
        f.leave(f.bob.user);
        let err = f
            .submit(f.commit_cmd(&f.bob, 1, 1, &[], &[]))
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::PermissionDenied));
    }

    #[tokio::test]
    async fn a_lost_race_is_stale() {
        let f = Fixture::new();
        f.group(1, 1, &[&f.alice, &f.bob, &f.bob2]);
        f.repo.state().fail_commit_insert = true;
        assert!(matches!(
            f.submit(f.commit_cmd(&f.alice, 1, 1, &[], &[]))
                .await
                .unwrap_err(),
            ServiceError::StaleEpoch {
                incarnation: 1,
                epoch: 2
            }
        ));
    }

    // ── The summary ────────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn adds_need_a_key_package_claimed_for_this_commit() {
        let f = Fixture::new();
        f.group(1, 1, &[&f.alice, &f.bob]);
        let refused = |r: &[u8], d: &Device| f.commit_cmd(&f.alice, 1, 1, &[(d, r)], &[]);

        let in_group = f.package(&f.bob, 1, false, Some((&f.alice, Duration::minutes(1))));
        let err = f.submit(refused(&in_group, &f.bob)).await.unwrap_err();
        assert!(validation(err).contains("already in the group"));

        let unclaimed = f.package(&f.bob2, 2, false, None);
        let by_other = f.package(&f.bob2, 3, false, Some((&f.bob, Duration::minutes(1))));
        let stale = f.package(&f.bob2, 4, false, Some((&f.alice, Duration::minutes(61))));
        for r in [&unclaimed, &by_other, &stale] {
            let err = f.submit(refused(r, &f.bob2)).await.unwrap_err();
            assert!(validation(err).contains("was not claimed"));
        }
        let alices = f.package(&f.alice, 5, false, Some((&f.alice, Duration::minutes(1))));
        let err = f.submit(refused(&alices, &f.bob2)).await.unwrap_err();
        assert!(validation(err).contains("another device"));
        let err = f.submit(refused(&[9; 32], &f.bob2)).await.unwrap_err();
        assert!(validation(err).contains("unknown"));

        let good = f.package(&f.bob2, 6, false, Some((&f.alice, Duration::minutes(1))));
        let mut cmd = refused(&good, &f.bob2);
        cmd.welcome = None;
        assert!(validation(f.submit(cmd).await.unwrap_err()).contains("needs a Welcome"));
        let mut cmd = refused(&good, &f.bob2);
        cmd.welcome = Some(welcome_bytes(vec![vec![1; 32]]));
        assert!(validation(f.submit(cmd).await.unwrap_err()).contains("exactly the added devices"));
        let mut cmd = f.commit_cmd(&f.alice, 1, 1, &[], &[]);
        cmd.welcome = Some(welcome_bytes(vec![]));
        assert!(validation(f.submit(cmd).await.unwrap_err()).contains("no Welcome"));

        // A device that is not a member's cannot be added.
        let stranger = device(UserId::new(), &principal(9), 29);
        f.repo.state().devices.push(stored_device(&stranger));
        let r = f.package(&stranger, 7, false, Some((&f.alice, Duration::minutes(1))));
        let err = f.submit(refused(&r, &stranger)).await.unwrap_err();
        assert!(validation(err).contains("may be added"));

        f.submit(refused(&good, &f.bob2)).await.expect("accepted");
        let added = f
            .repo
            .state()
            .group_devices
            .iter()
            .find(|(_, _, d)| d.device.device_key_id == f.bob2.id)
            .map(|(_, _, d)| d.added_epoch);
        assert_eq!(added, Some(2), "added at the epoch its Welcome joins");
    }

    #[tokio::test]
    async fn removes_must_be_pending_and_never_the_committer() {
        let f = Fixture::new();
        f.group(1, 1, &[&f.alice, &f.bob, &f.bob2]);
        let err = f
            .submit(f.commit_cmd(&f.alice, 1, 1, &[], &[&f.bob]))
            .await
            .unwrap_err();
        assert!(validation(err).contains("not waiting to be removed"));
        let err = f
            .submit(f.commit_cmd(&f.alice, 1, 1, &[], &[&f.alice]))
            .await
            .unwrap_err();
        assert!(validation(err).contains("cannot remove itself"));

        // Bob's second device is revoked: its removal is accepted.
        f.repo.state().devices[2].revoked_at = Some(Utc::now());
        f.submit(f.commit_cmd(&f.alice, 1, 1, &[], &[&f.bob2]))
            .await
            .expect("accepted");
        assert_eq!(f.group_devices(1), vec![f.alice.id, f.bob.id]);
        assert!(
            f.events()
                .iter()
                .any(|e| matches!(e, WorkspaceEvent::MlsMembershipChanged { .. }))
        );

        // Bob leaves: his last device becomes removable.
        f.leave(f.bob.user);
        let gone = device(UserId::new(), &principal(9), 29);
        let err = f
            .submit(f.commit_cmd(&f.alice, 1, 2, &[], &[&gone]))
            .await
            .unwrap_err();
        assert!(validation(err).contains("is not in the group"));
        f.submit(f.commit_cmd(&f.alice, 1, 2, &[], &[&f.bob]))
            .await
            .expect("accepted");
        assert_eq!(f.group_devices(1), vec![f.alice.id]);
    }

    #[tokio::test]
    async fn a_readd_removes_and_adds_with_a_one_time_package() {
        let f = Fixture::new();
        f.group(1, 4, &[&f.alice, &f.bob, &f.bob2]);
        let request = readd_request(&f, &f.bob, Duration::zero());
        f.repo.state().readds.push(request);
        let last_resort = f.package(&f.bob, 1, true, Some((&f.alice, Duration::minutes(1))));
        let err = f
            .submit(f.commit_cmd(&f.alice, 1, 4, &[(&f.bob, &last_resort)], &[&f.bob]))
            .await
            .unwrap_err();
        assert!(validation(err).contains("one-time package for a re-add"));
        let one_time = f.package(&f.bob, 2, false, Some((&f.alice, Duration::minutes(1))));
        let err = f
            .submit(f.commit_cmd(&f.alice, 1, 4, &[(&f.bob, &one_time)], &[]))
            .await
            .unwrap_err();
        assert!(validation(err).contains("already in the group"));
        f.submit(f.commit_cmd(&f.alice, 1, 4, &[(&f.bob, &one_time)], &[&f.bob]))
            .await
            .expect("re-added");
        assert_eq!(f.repo.state().readds[0].fulfilled_epoch, Some(5));
        assert_eq!(f.group_devices(1).len(), 3);
    }

    // ── Incarnations ───────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn a_new_incarnation_needs_a_ground() {
        let f = Fixture::new();
        f.group(1, 7, &[&f.alice, &f.bob, &f.bob2]);
        let r = f.package(&f.bob, 1, false, Some((&f.alice, Duration::minutes(1))));
        let err = f
            .submit(f.commit_cmd(&f.alice, 2, 0, &[(&f.bob, &r)], &[]))
            .await
            .unwrap_err();
        assert!(validation(err).contains("a new incarnation needs"));
        let err = f
            .submit(f.commit_cmd(&f.alice, 1, 0, &[(&f.bob, &r)], &[]))
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            ServiceError::StaleEpoch {
                incarnation: 1,
                epoch: 7
            }
        ));
        let err = f
            .submit(f.commit_cmd(&f.alice, 3, 0, &[(&f.bob, &r)], &[]))
            .await
            .unwrap_err();
        assert!(validation(err).contains("next incarnation is 2"));

        // A fresh re-add request is no ground yet.
        let fresh = readd_request(&f, &f.alice, Duration::minutes(1));
        f.repo.state().readds.push(fresh);
        let err = f
            .submit(f.commit_cmd(&f.alice, 2, 0, &[(&f.bob, &r)], &[]))
            .await
            .unwrap_err();
        assert!(validation(err).contains("a new incarnation needs"));

        // Recovery: alice asked over ten minutes ago and nobody committed since.
        let waited = readd_request(&f, &f.alice, Duration::minutes(11));
        f.repo.state().readds.push(waited);
        let err = f
            .submit(f.commit_cmd(&f.alice, 2, 0, &[(&f.bob, &r)], &[]))
            .await
            .unwrap_err();
        assert!(validation(err).contains("claimed for another incarnation"));
        f.repo.state().packages[0].meta.claimed_for_incarnation = Some(2);
        let commit = f
            .service
            .submit_commit(f.commit_cmd(&f.alice, 2, 0, &[(&f.bob, &r)], &[]))
            .await
            .expect("recovery incarnation");
        assert_eq!(commit.incarnation, 2);
        let state = f.repo.state();
        assert!(
            state
                .groups
                .iter()
                .any(|g| g.incarnation == 1 && g.retired_at.is_some())
        );
        assert!(
            state
                .groups
                .iter()
                .any(|g| g.incarnation == 2 && g.retired_at.is_none() && g.epoch == 1)
        );
    }

    #[tokio::test]
    async fn a_group_with_no_devices_left_starts_over() {
        let f = Fixture::new();
        // The only leaf is bob's revoked device; alice is a member outside the group.
        f.group(1, 3, &[&f.bob2]);
        f.repo.state().devices[2].revoked_at = Some(Utc::now());
        let r = f.package(&f.bob, 1, false, Some((&f.alice, Duration::minutes(1))));
        f.repo.state().packages[0].meta.claimed_for_incarnation = Some(2);
        f.submit(f.commit_cmd(&f.alice, 2, 0, &[(&f.bob, &r)], &[]))
            .await
            .expect("no devices left");
        assert_eq!(f.group_devices(2).len(), 2);
    }

    // ── Claims ─────────────────────────────────────────────────────────────────────────

    fn claim(f: &Fixture, by: &Device, targets: &[&Device]) -> ClaimMlsKeyPackagesCmd {
        ClaimMlsKeyPackagesCmd {
            caller: by.caller(),
            server_id: f.server,
            conversation_id: f.conv.id,
            devices: targets.iter().map(|d| d.as_ref()).collect(),
        }
    }

    #[tokio::test]
    async fn claims_hand_out_pending_adds_only() {
        let f = Fixture::new();
        // Without a group, a member's device claims for the other desired devices.
        f.package(&f.bob, 1, false, None);
        f.package(&f.bob2, 2, true, None);
        let claimed = f
            .service
            .claim_key_packages(claim(&f, &f.alice, &[&f.bob, &f.bob2]))
            .await
            .unwrap();
        assert_eq!(claimed.len(), 2);
        assert!(!claimed[0].last_resort);
        assert!(claimed[1].last_resort);
        assert_eq!(claimed[0].principal_id, f.bob.principal);
        let err = f
            .service
            .claim_key_packages(claim(&f, &f.alice, &[&f.alice]))
            .await
            .unwrap_err();
        assert!(validation(err).contains("not waiting to be added"));

        f.group(1, 1, &[&f.alice, &f.bob]);
        let err = f
            .service
            .claim_key_packages(claim(&f, &f.bob2, &[&f.bob]))
            .await
            .unwrap_err();
        assert!(forbidden(err).contains("not in the conversation's group"));
        let err = f
            .service
            .claim_key_packages(claim(&f, &f.alice, &[&f.bob]))
            .await
            .unwrap_err();
        assert!(validation(err).contains("not waiting to be added"));
        f.package(&f.bob2, 3, false, None);
        let claimed = f
            .service
            .claim_key_packages(claim(&f, &f.alice, &[&f.bob2]))
            .await
            .unwrap();
        assert_eq!(claimed.len(), 1);
        assert_eq!(claimed[0].key_package_ref, vec![3; 32]);
        let err = f
            .service
            .claim_key_packages(claim(&f, &f.alice, &[&f.bob2, &f.bob2]))
            .await
            .unwrap_err();
        assert!(validation(err).contains("named twice"));
    }

    #[tokio::test]
    async fn past_the_daily_limit_only_the_last_resort_is_handed_out() {
        let f = Fixture::new().with_limits(600, 0, 3);
        f.package(&f.bob, 1, false, None);
        f.package(&f.bob, 2, true, None);
        let claimed = f
            .service
            .claim_key_packages(claim(&f, &f.alice, &[&f.bob]))
            .await
            .unwrap();
        assert!(claimed[0].last_resort);

        let f = Fixture::new().with_limits(1, 100, 3);
        let err = f
            .service
            .claim_key_packages(claim(&f, &f.alice, &[&f.bob, &f.bob2]))
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::RateLimited { .. }));
    }

    #[tokio::test]
    async fn a_readd_target_never_gets_a_last_resort_package() {
        let f = Fixture::new();
        f.group(1, 1, &[&f.alice, &f.bob, &f.bob2]);
        // A one-time package from before the request, from the lost state.
        f.package(&f.bob, 3, false, None);
        f.repo.state().packages[0].meta.created_at = Utc::now() - Duration::minutes(5);
        let request = readd_request(&f, &f.bob, Duration::minutes(1));
        f.repo.state().readds.push(request);
        f.package(&f.bob, 1, true, None);
        let err = f
            .service
            .claim_key_packages(claim(&f, &f.alice, &[&f.bob]))
            .await
            .unwrap_err();
        assert_eq!(
            validation(err),
            "the device has not uploaded key packages since its re-add request"
        );
        // The newest package uploaded since is handed out, for the current incarnation.
        f.package(&f.bob, 2, false, None);
        f.package(&f.bob, 4, false, None);
        f.repo.state().packages[2].meta.created_at = Utc::now() - Duration::seconds(10);
        let claimed = f
            .service
            .claim_key_packages(claim(&f, &f.alice, &[&f.bob]))
            .await
            .unwrap();
        assert_eq!(claimed[0].key_package_ref, vec![4; 32]);
        let state = f.repo.state();
        let used = state
            .packages
            .iter()
            .find(|p| p.meta.key_package_ref == vec![4; 32])
            .unwrap();
        assert_eq!(used.meta.claimed_for_incarnation, Some(1));
    }

    #[tokio::test]
    async fn packages_close_to_expiry_count_as_absent_and_last_resort_claims_are_counted() {
        let f = Fixture::new();
        f.package(&f.bob, 1, false, None);
        f.repo.state().packages[0].meta.not_after = Utc::now() + Duration::days(13);
        f.package(&f.bob, 2, true, None);
        for _ in 0..2 {
            let claimed = f
                .service
                .claim_key_packages(claim(&f, &f.alice, &[&f.bob]))
                .await
                .unwrap();
            assert_eq!(claimed[0].key_package_ref, vec![2; 32]);
        }
        let status = f.service.key_package_status(f.bob.caller()).await.unwrap();
        assert_eq!(status[0].available, 0);
        assert_eq!(status[0].last_resort_claims, 2);
        f.repo.state().packages[1].meta.not_after = Utc::now() + Duration::days(13);
        let claimed = f
            .service
            .claim_key_packages(claim(&f, &f.alice, &[&f.bob]))
            .await
            .unwrap();
        assert!(claimed.is_empty());
    }

    // ── Key package uploads, with real OpenMLS key packages ────────────────────────────

    mod packages {
        use ed25519_dalek::Signer as _;
        use openmls::prelude::*;
        use openmls_rust_crypto::OpenMlsRustCrypto;
        use openmls_traits::signatures::{Signer, SignerError};

        struct Key(super::SigningKey);

        impl Signer for Key {
            fn sign(&self, payload: &[u8]) -> Result<Vec<u8>, SignerError> {
                Ok(self.0.sign(payload).to_bytes().to_vec())
            }

            fn signature_scheme(&self) -> SignatureScheme {
                SignatureScheme::ED25519
            }
        }

        pub(super) const SUITE: Ciphersuite =
            Ciphersuite::MLS_128_MLKEM768X25519_AES128GCM_SHA256_Ed25519;
        pub(super) const CLASSIC: Ciphersuite =
            Ciphersuite::MLS_128_DHKEMX25519_AES128GCM_SHA256_Ed25519;

        /// An `MLSMessage` carrying a key package with `identity` as its basic credential, its
        /// leaf signed by `leaf`.
        pub(super) fn build(
            suite: Ciphersuite,
            leaf: &super::SigningKey,
            identity: Vec<u8>,
            last_resort: bool,
            lifetime: Lifetime,
        ) -> Vec<u8> {
            let provider = OpenMlsRustCrypto::default();
            let key = Key(leaf.clone());
            let capabilities = Capabilities::new(
                None,
                Some(&[suite]),
                Some(&[ExtensionType::LastResort]),
                None,
                Some(&[CredentialType::Basic]),
            );
            let mut builder = KeyPackage::builder()
                .key_package_lifetime(lifetime)
                .leaf_node_capabilities(capabilities);
            if last_resort {
                builder = builder.mark_as_last_resort();
            }
            let credential = CredentialWithKey {
                credential: BasicCredential::new(identity).into(),
                signature_key: leaf.verifying_key().to_bytes().to_vec().into(),
            };
            let bundle = builder
                .build(suite, &provider, &key, credential)
                .expect("key package");
            MlsMessageOut::from(bundle.key_package().clone())
                .to_bytes()
                .expect("encode")
        }

        pub(super) fn days(days: u64) -> Lifetime {
            Lifetime::new(days * 86_400)
        }

        pub(super) fn expired() -> Lifetime {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_secs();
            Lifetime::init(now - 7200, now - 3600)
        }
    }

    fn identity_of(d: &Device, device_id: Uuid, signer: &SigningKey) -> Vec<u8> {
        let statement = device_mls_key(signer, &d.principal, device_id, &d.mls);
        rim_identity::mls_credential_identity(&statement)
    }

    fn real_package(d: &Device, last_resort: bool) -> String {
        b64(&packages::build(
            packages::SUITE,
            &d.mls,
            identity_of(d, d.device_id, &d.key),
            last_resort,
            packages::days(30),
        ))
    }

    async fn upload(
        f: &Fixture,
        d: &Device,
        key_packages: Vec<String>,
    ) -> Result<i64, ServiceError> {
        f.service
            .upload_key_packages(UploadMlsKeyPackagesCmd {
                caller: d.caller(),
                key_packages,
            })
            .await
            .map(|s| s.available)
    }

    #[tokio::test]
    async fn uploads_validate_every_key_package() {
        let f = Fixture::new();
        let a = &f.alice;
        let first = upload(&f, a, vec![real_package(a, false), real_package(a, true)]).await;
        assert_eq!(first.unwrap(), 1);
        let status = f.service.key_package_status(a.caller()).await.unwrap();
        assert_eq!(status[0].cipher_suite, SUITE);
        assert!(status[0].last_resort_not_after.is_some());

        // A flipped byte in the signed body.
        let mut tampered = URL_SAFE_NO_PAD.decode(real_package(a, false)).unwrap();
        let at = tampered.len() - 10;
        tampered[at] ^= 1;
        let err = upload(&f, a, vec![b64(&tampered)]).await.unwrap_err();
        assert!(validation(err).contains("key package 0"));

        // An mls-key statement signed by another device's key, or naming another device.
        let other_signer = packages::build(
            packages::SUITE,
            &a.mls,
            identity_of(a, a.device_id, &f.bob.key),
            false,
            packages::days(30),
        );
        let err = upload(&f, a, vec![b64(&other_signer)]).await.unwrap_err();
        assert!(validation(err).contains("mls-key statement"));
        let other_device = packages::build(
            packages::SUITE,
            &a.mls,
            identity_of(a, Uuid::now_v7(), &a.key),
            false,
            packages::days(30),
        );
        let err = upload(&f, a, vec![b64(&other_device)]).await.unwrap_err();
        assert!(validation(err).contains("names another device"));

        // The leaf is signed by a key other than the certified mlsKey.
        let wrong_key = packages::build(
            packages::SUITE,
            &f.bob.mls,
            identity_of(a, a.device_id, &a.key),
            false,
            packages::days(30),
        );
        let err = upload(&f, a, vec![b64(&wrong_key)]).await.unwrap_err();
        assert!(validation(err).contains("signature key"));

        let expired = packages::build(
            packages::SUITE,
            &a.mls,
            identity_of(a, a.device_id, &a.key),
            false,
            packages::expired(),
        );
        let err = upload(&f, a, vec![b64(&expired)]).await.unwrap_err();
        assert!(validation(err).contains("key package 0"));

        let classic = packages::build(
            packages::CLASSIC,
            &a.mls,
            identity_of(a, a.device_id, &a.key),
            false,
            packages::days(30),
        );
        let err = upload(&f, a, vec![b64(&classic)]).await.unwrap_err();
        assert!(validation(err).contains("cipher suite 1 is not accepted"));

        let reused = real_package(a, false);
        assert_eq!(upload(&f, a, vec![reused.clone()]).await.unwrap(), 2);
        let err = upload(&f, a, vec![reused.clone()]).await.unwrap_err();
        assert!(validation(err).contains("already uploaded"));
        let batch = vec![real_package(a, false), real_package(a, false), reused];
        let err = upload(&f, a, batch).await.unwrap_err();
        assert!(validation(err).contains("key package 2"));
        let two = vec![real_package(a, true), real_package(a, true)];
        let err = upload(&f, a, two).await.unwrap_err();
        assert!(validation(err).contains("one last-resort"));
        let err = upload(&f, a, vec![]).await.unwrap_err();
        assert!(validation(err).contains("1 to 50"));
        let err = upload(&f, a, vec!["x".to_string(); 51]).await.unwrap_err();
        assert!(validation(err).contains("1 to 50"));
    }

    #[tokio::test]
    async fn a_new_last_resort_package_replaces_the_old_one() {
        let f = Fixture::new();
        f.group(1, 1, &[&f.alice, &f.bob]);
        upload(&f, &f.bob2, vec![real_package(&f.bob2, true)])
            .await
            .unwrap();
        upload(&f, &f.bob2, vec![real_package(&f.bob2, true)])
            .await
            .unwrap();
        let last_resorts = f
            .repo
            .state()
            .packages
            .iter()
            .filter(|p| p.meta.device_key_id == f.bob2.id && p.meta.last_resort)
            .count();
        assert_eq!(last_resorts, 1);
        // The device waits to join the group, so the members hear about it.
        assert!(
            f.events()
                .iter()
                .any(|e| matches!(e, WorkspaceEvent::MlsMembershipChanged { .. }))
        );
    }

    // ── Welcomes, re-adds, devices, the group view ─────────────────────────────────────

    #[tokio::test]
    async fn welcomes_are_per_device() {
        let f = Fixture::new();
        let r = f.package(&f.bob, 1, false, Some((&f.alice, Duration::minutes(1))));
        let r2 = f.package(&f.bob2, 2, false, Some((&f.alice, Duration::minutes(1))));
        f.submit(f.commit_cmd(&f.alice, 1, 0, &[(&f.bob, &r), (&f.bob2, &r2)], &[]))
            .await
            .unwrap();
        let mine = f.service.pending_welcomes(f.bob.caller()).await.unwrap();
        assert_eq!(mine.len(), 1);
        assert!(
            f.service
                .pending_welcomes(f.alice.caller())
                .await
                .unwrap()
                .is_empty()
        );
        let ack = |caller: MlsCaller, ids: Vec<MlsWelcomeId>| AckMlsWelcomesCmd { caller, ids };
        let acked = f
            .service
            .ack_welcomes(ack(f.bob.caller(), vec![mine[0].id]))
            .await
            .unwrap();
        assert_eq!(acked, 1);
        assert!(
            f.service
                .pending_welcomes(f.bob.caller())
                .await
                .unwrap()
                .is_empty()
        );
        let theirs = f.service.pending_welcomes(f.bob2.caller()).await.unwrap();
        assert_eq!(theirs.len(), 1);
        let err = f
            .service
            .ack_welcomes(ack(f.bob.caller(), vec![mine[0].id; 101]))
            .await
            .unwrap_err();
        assert!(validation(err).contains("at most 100"));
    }

    fn readd_cmd(
        f: &Fixture,
        d: &Device,
        signer: &SigningKey,
        device_id: Uuid,
        conversation: Uuid,
        incarnation: u32,
    ) -> RequestMlsReaddCmd {
        let statement = device_mls_readd(
            signer,
            &d.principal,
            device_id,
            conversation,
            incarnation,
            Some(1),
            &[7; 16],
        );
        RequestMlsReaddCmd {
            caller: d.caller(),
            server_id: f.server,
            conversation_id: f.conv.id,
            payload: statement.payload,
            signature: statement.signature,
        }
    }

    #[tokio::test]
    async fn readd_requests_are_verified_and_limited() {
        let f = Fixture::new().with_limits(600, 100, 2);
        let conv: Uuid = f.conv.id.into();
        let (bob, bob2) = (&f.bob, &f.bob2);
        let request = |d: &Device, signer: &SigningKey, device_id: Uuid, c: Uuid, i: u32| {
            readd_cmd(&f, d, signer, device_id, c, i)
        };
        let err = f
            .service
            .request_readd(request(bob, &bob.key, bob.device_id, conv, 1))
            .await
            .unwrap_err();
        assert!(validation(err).contains("no MLS group"));

        f.group(2, 3, &[&f.alice, bob]);
        let refusals = [
            (request(bob, &bob2.key, bob.device_id, conv, 2), "signature"),
            (
                request(bob, &bob.key, bob2.device_id, conv, 2),
                "another device",
            ),
            (
                request(bob, &bob.key, bob.device_id, Uuid::now_v7(), 2),
                "another conversation",
            ),
            (
                request(bob2, &bob2.key, bob2.device_id, conv, 2),
                "not in the group",
            ),
        ];
        for (cmd, why) in refusals {
            let err = f.service.request_readd(cmd).await.unwrap_err();
            assert!(validation(err).contains(why), "{why}");
        }
        let err = f
            .service
            .request_readd(request(bob, &bob.key, bob.device_id, conv, 1))
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            ServiceError::StaleEpoch {
                incarnation: 2,
                epoch: 3
            }
        ));

        for _ in 0..2 {
            f.service
                .request_readd(request(bob, &bob.key, bob.device_id, conv, 2))
                .await
                .expect("stored");
        }
        assert!(
            f.events()
                .iter()
                .any(|e| matches!(e, WorkspaceEvent::MlsMembershipChanged { .. }))
        );
        let err = f
            .service
            .request_readd(request(bob, &bob.key, bob.device_id, conv, 2))
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::RateLimited { .. }));

        let group = f
            .service
            .group(f.group_query(f.alice.user))
            .await
            .unwrap()
            .expect("group");
        assert_eq!(group.readd_requests.len(), 2);
        assert_eq!(group.readd_requests[0].device_id, bob.device_id);
    }

    #[tokio::test]
    async fn the_group_view_lists_pending_changes() {
        let f = Fixture::new();
        let none = f.service.group(f.group_query(f.alice.user)).await.unwrap();
        assert!(none.is_none());
        f.group(1, 2, &[&f.alice, &f.bob]);
        f.package(&f.bob2, 1, true, None);
        f.repo.state().devices[1].last_key_package_upload_at =
            Some(Utc::now() - Duration::days(61));
        let group = f
            .service
            .group(f.group_query(f.alice.user))
            .await
            .unwrap()
            .expect("group");
        assert_eq!(group.epoch, 2);
        assert_eq!(group.devices.len(), 2);
        assert_eq!(group.pending_adds.len(), 1);
        assert_eq!(group.pending_adds[0].device_id, f.bob2.device_id);
        assert!(group.pending_adds[0].has_key_package);
        assert_eq!(group.pending_removes.len(), 1, "60 days without an upload");
        assert_eq!(group.pending_removes[0].device_id, f.bob.device_id);
    }

    #[tokio::test]
    async fn principal_devices_are_shown_to_workspace_peers_only() {
        let f = Fixture::new();
        let stranger = UserId::new();
        f.repo.state().principals.insert(stranger, principal(9));
        f.repo
            .state()
            .visible
            .insert(f.alice.user, HashSet::from([f.bob.user]));
        let answer = f
            .service
            .principal_devices(PrincipalDevicesQuery {
                actor: Actor::new(f.alice.user),
                principal_ids: vec![
                    f.bob.principal.clone(),
                    principal(9),
                    f.bob.principal.clone(),
                ],
            })
            .await
            .unwrap();
        assert_eq!(answer.len(), 2);
        assert_eq!(answer[0].devices.len(), 2);
        assert!(answer[1].devices.is_empty());
        let err = f
            .service
            .principal_devices(PrincipalDevicesQuery {
                actor: Actor::new(f.alice.user),
                principal_ids: vec!["x".to_string(); 101],
            })
            .await
            .unwrap_err();
        assert!(validation(err).contains("at most 100"));
    }

    #[tokio::test]
    async fn a_device_change_reaches_the_users_groups() {
        let f = Fixture::new();
        f.service.devices_changed(f.bob.user).await.unwrap();
        assert!(f.events().is_empty(), "no group yet");
        f.group(1, 1, &[&f.alice, &f.bob]);
        f.service.devices_changed(f.bob.user).await.unwrap();
        assert!(matches!(
            f.events().as_slice(),
            [WorkspaceEvent::MlsMembershipChanged { .. }]
        ));
    }

    // ── Franked reports ────────────────────────────────────────────────────────────────

    fn encrypted_message(f: &Fixture, sent_at: DateTime<Utc>, receipt: &[u8]) -> Message {
        Message {
            id: MessageId::new(),
            conversation_id: f.conv.id,
            thread_id: None,
            parent_message_id: None,
            forwarded_from_message_id: None,
            sent_at,
            sent_by: f.bob.user,
            text: None,
            is_deleted: false,
            has_attachments: false,
            encryption: Some(MessageEncryption {
                incarnation: 1,
                epoch: 1,
                sender_device_key_id: Some(f.bob.id),
                ciphertext: Some(vec![1]),
                franking_commitment: vec![0; 32],
                franking_receipt: receipt.to_vec(),
                franking_key_id: 1,
            }),
        }
    }

    #[tokio::test]
    async fn a_franked_report_proves_the_message() {
        let f = Fixture::new();
        let server_key = f.repo.current_franking_key(vec![0x42; 32]).await.unwrap();
        let content = br#"{"v":1,"type":"message","body":"hi"}"#.to_vec();
        let franking_key = [0x60u8; 32];
        let sent_at = Utc::now();
        let mut message = encrypted_message(&f, sent_at, &[]);
        let c = commitment(&franking_key, &content);
        let fields = ReceiptFields {
            message_id: message.id.into(),
            conversation_id: f.conv.id.into(),
            sender_user_id: f.bob.user.into(),
            sender_device_id: f.bob.device_id,
            sent_at_us: sent_at_us(sent_at),
        };
        let r = receipt(&server_key.secret, &fields, &c);
        // The sender deleted it first: it stays reportable.
        message.is_deleted = true;
        let message_id = message.id;
        f.messages.lock().unwrap().insert(message_id, message);

        let report = |content: &[u8], key: &[u8], receipt_key_id: i32| ReportEncryptedMessageCmd {
            actor: Actor::new(f.alice.user),
            server_id: f.server,
            conversation_id: f.conv.id,
            message_id,
            reason: "abuse".to_string(),
            content: b64(content),
            franking_key: b64(key),
            receipt: b64(&r),
            receipt_key_id,
            sender_user_id: f.bob.user,
            sender_device_id: f.bob.device_id,
            sent_at_us: sent_at_us(sent_at).to_string(),
        };
        let mismatch = "the report does not match the message";
        let tampered = br#"{"v":1,"type":"message","body":"ho"}"#;
        for cmd in [
            report(tampered, &franking_key, 1),
            report(&content, &[0x61; 32], 1),
            report(&content, &franking_key, 2),
        ] {
            let err = f.service.report_encrypted_message(cmd).await.unwrap_err();
            assert_eq!(validation(err), mismatch);
        }
        let mut later = report(&content, &franking_key, 1);
        later.sent_at_us = (sent_at_us(sent_at) + 1).to_string();
        let err = f.service.report_encrypted_message(later).await.unwrap_err();
        assert_eq!(validation(err), mismatch);
        let short = report(&content, &[0x60; 31], 1);
        let err = f.service.report_encrypted_message(short).await.unwrap_err();
        assert!(validation(err).contains("32 bytes"));

        f.service
            .report_encrypted_message(report(&content, &franking_key, 1))
            .await
            .expect("verified");
        assert_eq!(
            f.reports.lock().unwrap()[0].2.as_deref(),
            Some(content.as_slice())
        );
        let second = f
            .service
            .report_encrypted_message(report(&content, &franking_key, 1))
            .await
            .unwrap_err();
        assert!(matches!(second, ServiceError::Conflict(_)));

        // A plaintext message is reported the usual way.
        let mut plain = encrypted_message(&f, sent_at, &r);
        plain.encryption = None;
        plain.text = Some("plain".to_string());
        let plain_id = plain.id;
        f.messages.lock().unwrap().insert(plain_id, plain);
        let mut cmd = report(&content, &franking_key, 1);
        cmd.message_id = plain_id;
        let err = f.service.report_encrypted_message(cmd).await.unwrap_err();
        assert!(validation(err).contains("not encrypted"));
    }

    #[test]
    fn commit_required_reasons_are_named() {
        assert_eq!(
            MlsCommitRequiredReason::PendingRemovals.as_str(),
            "pendingRemovals"
        );
        assert_eq!(MlsCommitRequiredReason::NoGroup.as_str(), "noGroup");
    }
}
