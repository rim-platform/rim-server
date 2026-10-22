#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use chrono::{Duration, Utc};
    use uuid::Uuid;

    use rim_core::conversation::model::{Conversation, ConversationId, ConversationType};
    use rim_core::conversation::repository::MockConversationRepository;
    use rim_core::conversation_member::repository::MockConversationMemberRepository;
    use rim_core::e2ee::model::{E2eeDevice, MlsDevice, MlsGroup, MlsGroupDevice};
    use rim_core::e2ee::repository::E2eeRepository;
    use rim_core::key_identity::model::DeviceKeyId;
    use rim_core::message::model::MessageId;
    use rim_core::message::repository::MockMessageRepository;
    use rim_core::message_attachment::repository::MockMessageAttachmentRepository;
    use rim_core::message_mention::repository::MockMessageMentionRepository;
    use rim_core::message_thread::repository::MockMessageThreadRepository;
    use rim_core::server::model::ServerId;
    use rim_core::server_member::model::{MemberStanding, ServerMemberRole};
    use rim_core::server_member::repository::MockServerMemberRepository;
    use rim_core::user::model::UserId;
    use rim_core::user::repository::MockUserRepository;
    use rim_identity::StatementVersion;
    use rim_identity::testing::TestIdentity;
    use rim_services::e2ee::command::MlsCaller;
    use rim_services::e2ee::{MlsCommitRequiredReason, mls_group_id};
    use rim_services::events::{Broadcaster, Observers, WorkspaceEvent};
    use rim_services::message::command::{
        EditEncryptedMessageCmd, EditMessageCmd, SendEncryptedMessageCmd, SendMessageCmd,
    };
    use rim_services::message::service::MessageService;
    use rim_services::{Actor, ServiceError};

    use crate::MessageRepos;
    use crate::e2ee::franking::{ReceiptFields, receipt, sent_at_us};
    use crate::e2ee::openmls_wire::OpenMlsWire;
    use crate::e2ee_test_support::{EMPTY_COMMIT, FakeE2ee, FakeWire, SUITE, b64, frame};
    use crate::franking_keys::{FrankingKey, FrankingKeys, SharedFrankingKeys};
    use crate::message::DefaultMessageService;

    struct RecordingBroadcaster(Mutex<Vec<WorkspaceEvent>>);

    impl Broadcaster for RecordingBroadcaster {
        fn broadcast(&self, event: WorkspaceEvent) {
            self.0.lock().expect("events").push(event);
        }
    }

    type Mentions = Arc<Mutex<Vec<(MessageId, Vec<UserId>)>>>;

    struct Person {
        user: UserId,
        device: DeviceKeyId,
        device_id: Uuid,
        principal: String,
    }

    impl Person {
        fn new(seed: u8) -> Self {
            Self {
                user: UserId::new(),
                device: DeviceKeyId::new(),
                device_id: Uuid::now_v7(),
                principal: TestIdentity::with_version(seed, StatementVersion::V2).principal_id(),
            }
        }

        fn caller(&self) -> MlsCaller {
            MlsCaller {
                actor: Actor::new(self.user),
                device_key_id: Some(self.device),
            }
        }

        fn stored(&self) -> E2eeDevice {
            E2eeDevice {
                device_key_id: self.device,
                user_id: self.user,
                device_id: self.device_id,
                principal_id: Some(self.principal.clone()),
                public_key: "ed25519:unused".to_string(),
                revoked_at: None,
                last_key_package_upload_at: Some(Utc::now()),
            }
        }

        fn leaf(&self) -> MlsGroupDevice {
            MlsGroupDevice {
                device: MlsDevice {
                    device_key_id: self.device,
                    user_id: self.user,
                    principal_id: self.principal.clone(),
                    device_id: self.device_id,
                },
                added_epoch: 0,
            }
        }
    }

    struct Fixture {
        repo: Arc<FakeE2ee>,
        events: Arc<RecordingBroadcaster>,
        mentions: Mentions,
        service: DefaultMessageService,
        server: ServerId,
        conv: Conversation,
        alice: Person,
        bob: Person,
    }

    impl Fixture {
        /// A DM of alice and bob, encrypted or not, with an MLS group at incarnation 2, epoch 3
        /// holding both devices.
        fn new(encrypted: bool) -> Self {
            let server = ServerId::new();
            let (alice, bob) = (Person::new(1), Person::new(2));
            let conv = Conversation {
                id: ConversationId::new(),
                conversation_type: ConversationType::Direct,
                title: None,
                topic: None,
                created_at: Utc::now() - Duration::hours(1),
                last_activity_at: Utc::now(),
                server_id: server,
                is_private: false,
                archived_at: None,
                created_by: Some(alice.user),
                slow_mode_seconds: 0,
                encrypted_at: encrypted.then(Utc::now),
            };
            let repo = Arc::new(FakeE2ee::default());
            {
                let mut state = repo.state();
                state.devices = vec![alice.stored(), bob.stored()];
                state.members.insert(conv.id, vec![alice.user, bob.user]);
                state.principals.insert(alice.user, alice.principal.clone());
                state.principals.insert(bob.user, bob.principal.clone());
                state.groups.push(MlsGroup {
                    conversation_id: conv.id,
                    incarnation: 2,
                    group_id: mls_group_id(conv.id, 2),
                    cipher_suite: SUITE,
                    epoch: 3,
                    created_by_device_key_id: None,
                    created_at: Utc::now(),
                    retired_at: None,
                });
                for leaf in [alice.leaf(), bob.leaf()] {
                    state.group_devices.push((conv.id, 2, leaf));
                }
            }
            let events = Arc::new(RecordingBroadcaster(Mutex::new(Vec::new())));
            let mentions: Mentions = Arc::new(Mutex::new(Vec::new()));
            let service = Self::build(&repo, &events, &mentions, &conv);
            Self {
                repo,
                events,
                mentions,
                service,
                server,
                conv,
                alice,
                bob,
            }
        }

        fn build(
            repo: &Arc<FakeE2ee>,
            events: &Arc<RecordingBroadcaster>,
            mentions: &Mentions,
            conv: &Conversation,
        ) -> DefaultMessageService {
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
            conversations
                .expect_touch_last_activity_at()
                .returning(|_, _| Box::pin(async { Ok(()) }));
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
            let mut messages = MockMessageRepository::new();
            let shared = Arc::clone(repo);
            messages.expect_get_by_id().returning(move |id| {
                let found = shared.state().messages.get(&id).cloned();
                Box::pin(async move { Ok(found) })
            });
            let mut attachments = MockMessageAttachmentRepository::new();
            attachments
                .expect_get_attachments_for_message()
                .returning(|_| Box::pin(async { Ok(vec![]) }));
            let mut mention_repo = MockMessageMentionRepository::new();
            let recorded = Arc::clone(mentions);
            mention_repo
                .expect_create_mentions()
                .returning(move |id, users| {
                    recorded
                        .lock()
                        .expect("mentions")
                        .push((id, users.to_vec()));
                    Box::pin(async { Ok(()) })
                });
            let recorded = Arc::clone(mentions);
            mention_repo
                .expect_delete_mentions_for_message()
                .returning(move |id| {
                    recorded.lock().expect("mentions").retain(|(m, _)| *m != id);
                    Box::pin(async { Ok(()) })
                });
            let mut threads = MockMessageThreadRepository::new();
            threads
                .expect_get_by_root_message()
                .returning(|_| Box::pin(async { Ok(None) }));
            DefaultMessageService::new(
                MessageRepos {
                    conversation_repo: Arc::new(conversations),
                    conversation_member_repo: Arc::new(members),
                    message_repo: Arc::new(messages),
                    message_attachment_repo: Arc::new(attachments),
                    message_mention_repo: Arc::new(mention_repo),
                    message_thread_repo: Arc::new(threads),
                    server_member_repo: Arc::new(server_members),
                    user_repo: Arc::new(MockUserRepository::new()),
                },
                None,
                Observers::new(Arc::clone(events) as Arc<dyn Broadcaster>),
            )
            .with_e2ee(
                Arc::clone(repo) as Arc<dyn E2eeRepository>,
                Arc::new(FakeWire(OpenMlsWire::new())),
                SharedFrankingKeys::fixed(
                    FrankingKeys::new(vec![FrankingKey::new(4, [0x80; 32], Utc::now())])
                        .expect("keys"),
                ),
            )
        }

        /// An envelope in the group's current incarnation and epoch naming `id`.
        fn envelope(&self, id: MessageId, mentions: &[&str]) -> String {
            self.envelope_at(mls_group_id(self.conv.id, 2), 3, id, mentions, 0x71)
        }

        fn envelope_at(
            &self,
            group_id: Vec<u8>,
            epoch: u64,
            id: MessageId,
            mentions: &[&str],
            franking: u8,
        ) -> String {
            let aad = serde_json::json!({
                "v": 1,
                "type": "application",
                "messageId": Uuid::from(id),
                "franking": b64(&[franking; 32]),
                "mentions": mentions,
                "mentionAll": false,
            });
            frame(group_id, epoch, "application", &aad.to_string())
        }

        fn send(&self, by: &Person, id: MessageId, envelope: String) -> SendEncryptedMessageCmd {
            SendEncryptedMessageCmd {
                caller: by.caller(),
                server_id: self.server,
                conversation_id: self.conv.id,
                message_id: id,
                envelope,
                thread_root_message_id: None,
                forwarded_from_message_id: None,
            }
        }

        fn events(&self) -> Vec<WorkspaceEvent> {
            std::mem::take(&mut *self.events.0.lock().expect("events"))
        }
    }

    fn validation(err: ServiceError) -> String {
        match err {
            ServiceError::Validation(message) => message,
            other => panic!("expected a validation error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_message_is_stored_with_its_receipt_mentions_and_events() {
        let recorder = metrics_util::debugging::DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _guard = metrics::set_default_local_recorder(&recorder);

        let f = Fixture::new(true);
        let id = MessageId::new();
        let stranger = TestIdentity::with_version(9, StatementVersion::V2).principal_id();
        let mentions = [&*f.bob.principal, &*f.alice.principal, &*stranger];
        let envelope = f.envelope(id, &mentions);
        let dto = f
            .service
            .send_encrypted_message(f.send(&f.alice, id, envelope.clone()))
            .await
            .expect("sent");
        assert_eq!(dto.id, id);
        assert_eq!(dto.text, None);
        let encryption = dto.encryption.expect("encrypted");
        assert_eq!((encryption.incarnation, encryption.epoch), (2, 3));
        assert_eq!(encryption.sender_device_id, f.alice.device_id);
        assert_eq!(encryption.franking_key_id, 4);
        assert_eq!(
            encryption.ciphertext.as_deref().map(b64),
            Some(envelope.clone())
        );

        // The receipt covers the ids, the send time and the commitment.
        let stored = f.repo.state().messages.get(&id).cloned().expect("stored");
        assert_eq!(sent_at_us(stored.sent_at), encryption.sent_at_us);
        let fields = ReceiptFields {
            message_id: id.into(),
            conversation_id: f.conv.id.into(),
            sender_user_id: f.alice.user.into(),
            sender_device_id: f.alice.device_id,
            sent_at_us: encryption.sent_at_us,
        };
        assert_eq!(
            encryption.franking_receipt,
            receipt(&[0x80; 32], &fields, &[0x71; 32]).to_vec()
        );

        // Mentions: members only, the author skipped.
        assert_eq!(*f.mentions.lock().unwrap(), vec![(id, vec![f.bob.user])]);
        let events = f.events();
        assert!(events.iter().any(|e| matches!(
            e,
            WorkspaceEvent::NewMessage { message, .. }
                if message.encryption.is_some() && message.text.is_none()
        )));
        assert!(events.iter().any(|e| matches!(
            e,
            WorkspaceEvent::UserMentioned { mentioned_user_ids, .. }
                if *mentioned_user_ids == vec![f.bob.user]
        )));

        // The same id and bytes answer the stored message; other bytes conflict.
        let again = f
            .service
            .send_encrypted_message(f.send(&f.alice, id, envelope))
            .await
            .expect("repeat");
        assert_eq!(
            again.encryption.map(|e| e.franking_receipt),
            Some(encryption.franking_receipt)
        );
        let other = f.envelope(id, &[]);
        let err = f
            .service
            .send_encrypted_message(f.send(&f.alice, id, other))
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Conflict(_)));
        let got = crate::metrics::test_support::Recorded::snapshot(&snapshotter);
        assert_eq!(
            got.counter(
                crate::metrics::MESSAGES_CREATED,
                &[("encryption", "encrypted")]
            ),
            1
        );
    }

    #[tokio::test]
    async fn the_checks_table_for_application_messages() {
        let f = Fixture::new(true);
        let id = MessageId::new();
        let send = |by: &Person, envelope: String| f.send(by, id, envelope);

        let mut unbound = send(&f.alice, f.envelope(id, &[]));
        unbound.caller.device_key_id = None;
        let err = f.service.send_encrypted_message(unbound).await.unwrap_err();
        assert!(
            matches!(err, ServiceError::Forbidden(m) if m == "this session is not bound to a device")
        );

        let v4 = MessageId::from(Uuid::new_v4());
        let err = f
            .service
            .send_encrypted_message(f.send(&f.alice, v4, f.envelope(v4, &[])))
            .await
            .unwrap_err();
        assert_eq!(validation(err), "messageId must be a UUID v7");

        let commit = frame(mls_group_id(f.conv.id, 2), 3, "commit", EMPTY_COMMIT);
        let err = f
            .service
            .send_encrypted_message(send(&f.alice, commit))
            .await
            .unwrap_err();
        assert_eq!(
            validation(err),
            "the PrivateMessage is not an application message"
        );

        let err = f
            .service
            .send_encrypted_message(send(&f.alice, f.envelope(MessageId::new(), &[])))
            .await
            .unwrap_err();
        assert_eq!(
            validation(err),
            "the authenticated data names another message"
        );

        let err = f
            .service
            .send_encrypted_message(send(&f.alice, "A".repeat(200 * 1024)))
            .await
            .unwrap_err();
        assert!(validation(err).contains("98304 bytes"));

        let old = f.envelope_at(mls_group_id(f.conv.id, 1), 3, id, &[], 0x71);
        let err = f
            .service
            .send_encrypted_message(send(&f.alice, old))
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            ServiceError::StaleEpoch {
                incarnation: 2,
                epoch: 3
            }
        ));
        let foreign = f.envelope_at(mls_group_id(ConversationId::new(), 2), 3, id, &[], 0x71);
        let err = f
            .service
            .send_encrypted_message(send(&f.alice, foreign))
            .await
            .unwrap_err();
        assert!(validation(err).contains("group id"));
        let behind = f.envelope_at(mls_group_id(f.conv.id, 2), 2, id, &[], 0x71);
        let err = f
            .service
            .send_encrypted_message(send(&f.alice, behind))
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            ServiceError::StaleEpoch {
                incarnation: 2,
                epoch: 3
            }
        ));

        // A device outside the group, and a device due for removal, are refused.
        f.repo
            .state()
            .group_devices
            .retain(|(_, _, d)| d.device.device_key_id != f.bob.device);
        let err = f
            .service
            .send_encrypted_message(send(&f.bob, f.envelope(id, &[])))
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Forbidden(_)));
        f.repo
            .state()
            .group_devices
            .push((f.conv.id, 2, f.bob.leaf()));
        f.repo.state().devices[1].last_key_package_upload_at =
            Some(Utc::now() - Duration::days(61));
        let err = f
            .service
            .send_encrypted_message(send(&f.bob, f.envelope(id, &[])))
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Forbidden(_)));

        // Bob's stale leaf blocks alice's message until it is committed away.
        let err = f
            .service
            .send_encrypted_message(send(&f.alice, f.envelope(id, &[])))
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            ServiceError::MlsCommitRequired {
                reason: MlsCommitRequiredReason::PendingRemovals
            }
        ));

        // Without a group, a Commit comes first.
        f.repo.state().groups.clear();
        let err = f
            .service
            .send_encrypted_message(send(&f.alice, f.envelope(id, &[])))
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            ServiceError::MlsCommitRequired {
                reason: MlsCommitRequiredReason::NoGroup
            }
        ));
    }

    #[tokio::test]
    async fn only_the_author_edits_and_an_edit_replaces_the_mentions() {
        let recorder = metrics_util::debugging::DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _guard = metrics::set_default_local_recorder(&recorder);

        let f = Fixture::new(true);
        let id = MessageId::new();
        f.service
            .send_encrypted_message(f.send(&f.alice, id, f.envelope(id, &[&f.bob.principal])))
            .await
            .expect("sent");
        f.events();
        let edit = |by: &Person, envelope: String| EditEncryptedMessageCmd {
            caller: by.caller(),
            server_id: f.server,
            conversation_id: f.conv.id,
            message_id: id,
            envelope,
        };
        let new_envelope = f.envelope_at(mls_group_id(f.conv.id, 2), 3, id, &[], 0x72);
        let err = f
            .service
            .edit_encrypted_message(edit(&f.bob, new_envelope.clone()))
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::PermissionDenied));
        let dto = f
            .service
            .edit_encrypted_message(edit(&f.alice, new_envelope.clone()))
            .await
            .expect("edited");
        let encryption = dto.encryption.expect("encrypted");
        assert_eq!(
            encryption.ciphertext.as_deref().map(b64),
            Some(new_envelope)
        );
        let stored = f.repo.state().messages.get(&id).cloned().expect("stored");
        assert_eq!(
            stored.encryption.expect("envelope").franking_commitment,
            vec![0x72; 32]
        );
        // The sent version and the edit are both recorded, with the same send time.
        let revisions: Vec<_> = f
            .repo
            .state()
            .revisions
            .iter()
            .map(|r| {
                (
                    r.revision,
                    r.franking_commitment.clone(),
                    r.sent_at_us,
                    r.sender_device_id,
                )
            })
            .collect();
        let sent_at = sent_at_us(stored.sent_at);
        assert_eq!(
            revisions,
            vec![
                (0, vec![0x71; 32], sent_at, f.alice.device_id),
                (1, vec![0x72; 32], sent_at, f.alice.device_id),
            ]
        );
        assert!(
            f.mentions.lock().unwrap().is_empty(),
            "the edit mentions no one"
        );
        let events = f.events();
        assert!(
            events
                .iter()
                .any(|e| matches!(e, WorkspaceEvent::MessageUpdated { .. }))
        );
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, WorkspaceEvent::UserMentioned { .. }))
        );
        let got = crate::metrics::test_support::Recorded::snapshot(&snapshotter);
        assert_eq!(
            got.counter(
                crate::metrics::MESSAGES_CREATED,
                &[("encryption", "encrypted")]
            ),
            1
        );
    }

    #[tokio::test]
    async fn encrypted_edits_spend_the_message_budget_and_retries_are_free() {
        let mut f = Fixture::new(true);
        let rate = crate::token_bucket::BucketRate::new(1, 2).expect("a rate");
        f.service = f.service.clone().with_message_budget(rate);
        let id = MessageId::new();
        f.service
            .send_encrypted_message(f.send(&f.alice, id, f.envelope(id, &[])))
            .await
            .expect("sent");
        let edit = |franking: u8| EditEncryptedMessageCmd {
            caller: f.alice.caller(),
            server_id: f.server,
            conversation_id: f.conv.id,
            message_id: id,
            envelope: f.envelope_at(mls_group_id(f.conv.id, 2), 3, id, &[], franking),
        };
        f.service
            .edit_encrypted_message(edit(0x72))
            .await
            .expect("within the burst");
        f.service
            .edit_encrypted_message(edit(0x72))
            .await
            .expect("a retry of the stored edit is free");
        f.events();

        let err = f
            .service
            .edit_encrypted_message(edit(0x73))
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::RateLimited { .. }), "{err:?}");
        let stored = f.repo.state().messages.get(&id).cloned().expect("stored");
        assert_eq!(
            stored.encryption.expect("envelope").franking_commitment,
            vec![0x72; 32]
        );
        assert_eq!(f.repo.state().revisions.len(), 2);
        assert!(f.events().is_empty());
    }

    #[tokio::test]
    async fn plaintext_and_encrypted_writes_stay_in_their_own_conversations() {
        let f = Fixture::new(true);
        let err = f
            .service
            .send_message(SendMessageCmd {
                actor: Actor::new(f.alice.user),
                server_id: f.server,
                conversation_id: f.conv.id,
                text: Some("plain".to_string()),
                parent_message_id: None,
                forwarded_from_message_id: None,
            })
            .await
            .unwrap_err();
        assert_eq!(validation(err), "this conversation is encrypted");

        let id = MessageId::new();
        f.service
            .send_encrypted_message(f.send(&f.alice, id, f.envelope(id, &[])))
            .await
            .expect("sent");
        let err = f
            .service
            .edit_message(EditMessageCmd {
                actor: Actor::new(f.alice.user),
                server_id: f.server,
                conversation_id: f.conv.id,
                message_id: id,
                text: "plain".to_string(),
            })
            .await
            .unwrap_err();
        assert_eq!(validation(err), "this conversation is encrypted");

        let plaintext = Fixture::new(false);
        let id = MessageId::new();
        let cmd = plaintext.send(&plaintext.alice, id, plaintext.envelope(id, &[]));
        let err = plaintext
            .service
            .send_encrypted_message(cmd)
            .await
            .unwrap_err();
        assert_eq!(validation(err), "this conversation is not encrypted");
    }
}
