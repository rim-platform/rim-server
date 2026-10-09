#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use chrono::{Duration, Utc};

    use rim_core::key_identity::model::{KeyLogEntry, KeyLogEntryType};
    use rim_core::key_identity::repository::MockKeyIdentityRepository;
    use rim_core::repository::error::{RepoError, RepoErrorKind, RepoResult};
    use rim_core::server::model::ServerId;
    use rim_core::server_member::model::{ServerMember, ServerMemberId, ServerMemberRole};
    use rim_core::server_member::repository::MockServerMemberRepository;
    use rim_core::user::model::UserId;
    use rim_core::workspace_plugin::model::{
        NewPluginBundle, PluginBundle, PluginRuntimeKind, PluginSource, WorkspacePlugin,
        WorkspacePluginId, WorkspacePluginStatus, WorkspacePluginWrite,
    };
    use rim_core::workspace_plugin::repository::WorkspacePluginRepository;
    use rim_identity::testing::{
        KNOWN_PLUGIN_MANIFEST_PAYLOAD, KNOWN_PLUGIN_MANIFEST_SIGNATURE, TestIdentity,
        bundle_source, plugin_manifest_payload, sign_plugin_manifest, signing_key, web_manifest,
    };
    use rim_identity::{KeyState, SignedStatement, StatementVersion};
    use rim_services::events::{Broadcaster, Observers, WorkspaceEvent, WorkspacePluginChange};
    use rim_services::key_auth::command::SignedPayload;
    use rim_services::workspace_plugin::command::{
        AddWorkspacePluginCmd, ListWorkspacePluginsQuery, RemoveWorkspacePluginCmd,
        UpdateWorkspacePluginCmd,
    };
    use rim_services::workspace_plugin::dto::PluginSignatureStatus;
    use rim_services::workspace_plugin::service::WorkspacePluginService;
    use rim_services::{Actor, ServiceError, ServiceResult};

    use crate::registration_rate_limiter::RegistrationRateLimiter;
    use crate::workspace_plugin::{DefaultWorkspacePluginService, WorkspacePluginRepos};

    const SHA: &str = "5b154b3eb45c3ebc44f6820ba30a389ce85b43dea34bcbf652b9b1a5416fbe85";
    const PLUGIN: &str = "dev.example.hello-web";

    // ── In-memory registry ───────────────────────────────────────────────────────────────

    #[derive(Default)]
    struct Store {
        entries: Vec<WorkspacePlugin>,
        bundles: Vec<PluginBundle>,
    }

    type Shared = Arc<Mutex<Store>>;

    fn to_entry(
        id: WorkspacePluginId,
        created: chrono::DateTime<Utc>,
        w: WorkspacePluginWrite,
    ) -> WorkspacePlugin {
        WorkspacePlugin {
            id,
            server_id: w.server_id,
            plugin_id: w.plugin_id,
            version: w.version,
            display_name: w.display_name,
            runtime_kind: w.runtime_kind,
            status: w.status,
            source: w.source,
            statement: w.statement,
            publisher_principal_id: w.publisher_principal_id,
            publisher_key_seq: w.publisher_key_seq,
            publisher_key_log: w.publisher_key_log,
            approved_by: Some(w.approved_by),
            approved_at: Utc::now(),
            created_at: created,
            updated_at: Utc::now(),
        }
    }

    struct FakePlugins(Shared);

    #[async_trait]
    impl WorkspacePluginRepository for FakePlugins {
        async fn lock_server(&self, _server_id: ServerId) -> RepoResult<bool> {
            Ok(true)
        }

        async fn list(&self, server_id: ServerId) -> RepoResult<Vec<WorkspacePlugin>> {
            let s = self.0.lock().unwrap();
            let mut entries: Vec<WorkspacePlugin> = s
                .entries
                .iter()
                .filter(|e| e.server_id == server_id)
                .cloned()
                .collect();
            entries.sort_by(|a, b| a.plugin_id.cmp(&b.plugin_id));
            Ok(entries)
        }

        async fn get(
            &self,
            server_id: ServerId,
            plugin_id: &str,
        ) -> RepoResult<Option<WorkspacePlugin>> {
            let s = self.0.lock().unwrap();
            Ok(s.entries
                .iter()
                .find(|e| e.server_id == server_id && e.plugin_id == plugin_id)
                .cloned())
        }

        async fn insert(
            &self,
            entry: WorkspacePluginWrite,
            max_entries: u64,
        ) -> RepoResult<Option<WorkspacePlugin>> {
            let mut s = self.0.lock().unwrap();
            if s.entries
                .iter()
                .any(|e| e.server_id == entry.server_id && e.plugin_id == entry.plugin_id)
            {
                return Err(RepoError::new(RepoErrorKind::Conflict));
            }
            let count = s
                .entries
                .iter()
                .filter(|e| e.server_id == entry.server_id)
                .count() as u64;
            if count >= max_entries {
                return Ok(None);
            }
            let stored = to_entry(WorkspacePluginId::new(), Utc::now(), entry);
            s.entries.push(stored.clone());
            Ok(Some(stored))
        }

        async fn update(
            &self,
            id: WorkspacePluginId,
            entry: WorkspacePluginWrite,
        ) -> RepoResult<Option<WorkspacePlugin>> {
            let mut s = self.0.lock().unwrap();
            let Some(row) = s.entries.iter_mut().find(|e| e.id == id) else {
                return Ok(None);
            };
            *row = to_entry(id, row.created_at, entry);
            Ok(Some(row.clone()))
        }

        async fn delete(&self, server_id: ServerId, plugin_id: &str) -> RepoResult<bool> {
            let mut s = self.0.lock().unwrap();
            let before = s.entries.len();
            s.entries
                .retain(|e| !(e.server_id == server_id && e.plugin_id == plugin_id));
            Ok(s.entries.len() < before)
        }

        async fn get_bundle(
            &self,
            server_id: ServerId,
            sha256: &str,
        ) -> RepoResult<Option<PluginBundle>> {
            let s = self.0.lock().unwrap();
            Ok(s.bundles
                .iter()
                .find(|b| b.server_id == server_id && b.sha256 == sha256)
                .cloned())
        }

        async fn insert_bundle(&self, bundle: NewPluginBundle) -> RepoResult<Option<PluginBundle>> {
            let mut s = self.0.lock().unwrap();
            if s.bundles
                .iter()
                .any(|b| b.server_id == bundle.server_id && b.sha256 == bundle.sha256)
            {
                return Ok(None);
            }
            let stored = PluginBundle {
                server_id: bundle.server_id,
                sha256: bundle.sha256,
                size_bytes: bundle.size_bytes,
                storage_key: bundle.storage_key,
                uploaded_by: Some(bundle.uploaded_by),
                created_at: Utc::now(),
            };
            s.bundles.push(stored.clone());
            Ok(Some(stored))
        }

        async fn bundle_bytes(&self, server_id: ServerId) -> RepoResult<u64> {
            let s = self.0.lock().unwrap();
            Ok(s.bundles
                .iter()
                .filter(|b| b.server_id == server_id)
                .map(|b| b.size_bytes)
                .sum())
        }

        async fn delete_unnamed_bundles(
            &self,
            _server_id: ServerId,
            _age: Duration,
        ) -> RepoResult<Vec<String>> {
            Ok(Vec::new())
        }
    }

    #[derive(Default)]
    struct RecordingBroadcaster(Mutex<Vec<WorkspaceEvent>>);

    impl Broadcaster for RecordingBroadcaster {
        fn broadcast(&self, event: WorkspaceEvent) {
            self.0.lock().unwrap().push(event);
        }
    }

    // ── Fixture ──────────────────────────────────────────────────────────────────────────

    /// The publisher's key log as this server stores it when they are enrolled here.
    type LocalLog = Arc<Mutex<Option<Vec<SignedStatement>>>>;

    struct Harness {
        server_id: ServerId,
        owner: UserId,
        moderator: UserId,
        member: UserId,
        guest: UserId,
        outsider: UserId,
        store: Shared,
        local_log: LocalLog,
        events: Arc<RecordingBroadcaster>,
        service: DefaultWorkspacePluginService,
    }

    fn harness() -> Harness {
        harness_with_limit(RegistrationRateLimiter::new(3600, 1000))
    }

    fn harness_with_limit(limiter: RegistrationRateLimiter) -> Harness {
        let server_id = ServerId::new();
        let (owner, moderator, member, guest, outsider) = (
            UserId::new(),
            UserId::new(),
            UserId::new(),
            UserId::new(),
            UserId::new(),
        );
        let roles: HashMap<UserId, ServerMemberRole> = [
            (owner, ServerMemberRole::Owner),
            (moderator, ServerMemberRole::Moderator),
            (member, ServerMemberRole::Member),
            (guest, ServerMemberRole::Guest),
        ]
        .into_iter()
        .collect();
        let mut members = MockServerMemberRepository::new();
        members.expect_get_member().returning(move |sid, uid| {
            Ok(roles.get(&uid).map(|role| ServerMember {
                id: ServerMemberId::new(),
                server_id: sid,
                user_id: uid,
                member_role: *role,
                joined_at: Utc::now(),
            }))
        });

        let local_log: LocalLog = Arc::new(Mutex::new(None));
        let enrolled_user = UserId::new();
        let mut keys = MockKeyIdentityRepository::new();
        let lookup = Arc::clone(&local_log);
        keys.expect_find_user_by_principal_id()
            .returning(move |principal_id| {
                let enrolled =
                    principal_id == publisher().principal_id() && lookup.lock().unwrap().is_some();
                Box::pin(async move { Ok(enrolled.then_some(enrolled_user)) })
            });
        let entries = Arc::clone(&local_log);
        keys.expect_list_key_log().returning(move |user_id| {
            let log: Vec<KeyLogEntry> = entries
                .lock()
                .unwrap()
                .clone()
                .unwrap_or_default()
                .into_iter()
                .enumerate()
                .map(|(seq, s)| KeyLogEntry {
                    user_id,
                    seq: seq as u32,
                    entry_type: if seq == 0 {
                        KeyLogEntryType::Inception
                    } else {
                        KeyLogEntryType::Rotate
                    },
                    payload: s.payload,
                    signature: s.signature,
                    created_at: Utc::now(),
                })
                .collect();
            Box::pin(async move { Ok(log) })
        });

        let store: Shared = Arc::new(Mutex::new(Store::default()));
        let events = Arc::new(RecordingBroadcaster::default());
        let service = DefaultWorkspacePluginService::new(
            WorkspacePluginRepos {
                server_member_repo: Arc::new(members),
                key_identity_repo: Arc::new(keys),
                workspace_plugin_repo: Arc::new(FakePlugins(Arc::clone(&store))),
            },
            None,
            Observers::new(Arc::clone(&events) as Arc<dyn Broadcaster>),
        )
        .with_rate_limiter(limiter);
        Harness {
            server_id,
            owner,
            moderator,
            member,
            guest,
            outsider,
            store,
            local_log,
            events,
            service,
        }
    }

    impl Harness {
        fn upload_bundle(&self, sha256: &str, size: u64) {
            self.store.lock().unwrap().bundles.push(PluginBundle {
                server_id: self.server_id,
                sha256: sha256.to_string(),
                size_bytes: size,
                storage_key: format!("plugin-bundles/{}/{sha256}", self.server_id),
                uploaded_by: Some(self.owner),
                created_at: Utc::now(),
            });
        }

        fn enroll_publisher(&self, log: Vec<SignedStatement>) {
            *self.local_log.lock().unwrap() = Some(log);
        }

        fn events(&self) -> Vec<(String, WorkspacePluginChange)> {
            self.events
                .0
                .lock()
                .unwrap()
                .iter()
                .filter_map(|e| match e {
                    WorkspaceEvent::WorkspacePluginChanged {
                        plugin_id, change, ..
                    } => Some((plugin_id.clone(), *change)),
                    _ => None,
                })
                .collect()
        }

        fn add_cmd(&self, by: UserId, statement: SignedPayload) -> AddWorkspacePluginCmd {
            AddWorkspacePluginCmd {
                actor: Actor::new(by),
                server_id: self.server_id,
                statement,
                status: WorkspacePluginStatus::Recommended,
                bundle_url: None,
                publisher_key_log: Some(vec![payload(&inception())]),
            }
        }

        fn update_cmd(&self, by: UserId) -> UpdateWorkspacePluginCmd {
            UpdateWorkspacePluginCmd {
                actor: Actor::new(by),
                server_id: self.server_id,
                plugin_id: PLUGIN.to_string(),
                statement: None,
                status: None,
                bundle_url: None,
                publisher_key_log: None,
            }
        }

        fn remove_cmd(&self, by: UserId) -> RemoveWorkspacePluginCmd {
            RemoveWorkspacePluginCmd {
                actor: Actor::new(by),
                server_id: self.server_id,
                plugin_id: PLUGIN.to_string(),
            }
        }

        async fn add_known(
            &self,
        ) -> ServiceResult<rim_services::workspace_plugin::dto::WorkspacePluginDto> {
            self.upload_bundle(SHA, 1605);
            self.service.add(self.add_cmd(self.owner, known())).await
        }

        async fn list(
            &self,
            by: UserId,
        ) -> ServiceResult<Vec<rim_services::workspace_plugin::dto::WorkspacePluginDto>> {
            self.service
                .list(ListWorkspacePluginsQuery {
                    actor: Actor::new(by),
                    server_id: self.server_id,
                })
                .await
        }
    }

    fn publisher() -> TestIdentity {
        TestIdentity::with_version(1, StatementVersion::V2)
    }

    fn inception() -> SignedStatement {
        publisher().inception()
    }

    fn payload(s: &SignedStatement) -> SignedPayload {
        SignedPayload {
            payload: s.payload.clone(),
            signature: s.signature.clone(),
        }
    }

    fn known() -> SignedPayload {
        SignedPayload {
            payload: KNOWN_PLUGIN_MANIFEST_PAYLOAD.to_string(),
            signature: KNOWN_PLUGIN_MANIFEST_SIGNATURE.to_string(),
        }
    }

    /// A web plugin statement by the publisher at `keySeq` 0 over the known bundle.
    fn web_statement(plugin_id: &str, version: &str) -> SignedPayload {
        let text = plugin_manifest_payload(
            &publisher().principal_id(),
            0,
            "Ada",
            &web_manifest(plugin_id, version),
            &bundle_source(SHA, 1605),
        );
        payload(&sign_plugin_manifest(&signing_key(1), text))
    }

    /// A desktop plugin statement from git by the publisher at `keySeq` 0.
    fn git_statement(plugin_id: &str) -> SignedPayload {
        let manifest =
            web_manifest(plugin_id, "1.0.0").replace("WebSandboxed", "DesktopLocalProcess");
        let source = format!(
            r#"{{"kind":"git","url":"https://github.com/acme/hello","commit":"{}","treeSha256":"{SHA}"}}"#,
            "c".repeat(40)
        );
        let text =
            plugin_manifest_payload(&publisher().principal_id(), 0, "Ada", &manifest, &source);
        payload(&sign_plugin_manifest(&signing_key(1), text))
    }

    /// The publisher's log after one identity rotation, and after a recovery rotation too.
    fn rotated_logs() -> (Vec<SignedStatement>, Vec<SignedStatement>) {
        let mut id = publisher();
        let inception = id.inception();
        let state = KeyState::from_inception(&inception).expect("inception");
        let r1 = id.rotate_with_identity(&state, 2);
        let (state1, _) = state.apply(&r1).expect("r1");
        let r2 = id.recover(&state1, 3);
        (vec![inception.clone(), r1.clone()], vec![inception, r1, r2])
    }

    fn assert_validation<T: std::fmt::Debug>(result: ServiceResult<T>, expected: &str) {
        match result {
            Err(ServiceError::Validation(message)) => assert_eq!(message, expected),
            other => panic!("expected Validation({expected:?}), got {other:?}"),
        }
    }

    fn assert_forbidden<T: std::fmt::Debug>(result: ServiceResult<T>) {
        assert!(
            matches!(result, Err(ServiceError::PermissionDenied)),
            "expected PermissionDenied, got {result:?}"
        );
    }

    // ── Roles ────────────────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn every_member_reads_the_registry_and_outsiders_do_not() {
        let h = harness();
        h.add_known().await.expect("add");
        for user in [h.owner, h.moderator, h.member, h.guest] {
            let entries = h.list(user).await.expect("list");
            assert_eq!(entries.len(), 1);
        }
        assert_forbidden(h.list(h.outsider).await);
    }

    #[tokio::test]
    async fn only_owners_and_moderators_change_the_registry() {
        let h = harness();
        h.upload_bundle(SHA, 1605);
        for user in [h.member, h.guest, h.outsider] {
            assert_forbidden(h.service.add(h.add_cmd(user, known())).await);
        }
        h.service
            .add(h.add_cmd(h.moderator, known()))
            .await
            .expect("moderator adds");
        for user in [h.member, h.guest, h.outsider] {
            let mut update = h.update_cmd(user);
            update.status = Some(WorkspacePluginStatus::Required);
            assert_forbidden(h.service.update(update).await);
            assert_forbidden(h.service.remove(h.remove_cmd(user)).await);
        }
        let mut update = h.update_cmd(h.owner);
        update.status = Some(WorkspacePluginStatus::Required);
        h.service.update(update).await.expect("owner updates");
        h.service
            .remove(h.remove_cmd(h.moderator))
            .await
            .expect("moderator removes");
    }

    // ── Add ──────────────────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn add_stores_the_verified_statement_with_this_servers_copy() {
        let h = harness();
        let entry = h.add_known().await.expect("add");
        assert_eq!(entry.plugin_id, PLUGIN);
        assert_eq!(entry.version, "0.1.0");
        assert_eq!(entry.display_name, "Hello Web");
        assert_eq!(entry.runtime_kind, PluginRuntimeKind::WebSandboxed);
        assert_eq!(entry.status, WorkspacePluginStatus::Recommended);
        assert_eq!(
            entry.source,
            PluginSource::Bundle {
                url: format!("/api/plugin-bundles/{}/{SHA}", h.server_id),
                sha256: SHA.to_string(),
                size: 1605,
            }
        );
        assert!(entry.manifest.starts_with(r#"{"manifest_version":1,"#));
        assert_eq!(entry.statement, known());
        assert_eq!(entry.publisher_principal_id, publisher().principal_id());
        assert_eq!(entry.publisher_name, "Ada");
        assert_eq!(entry.publisher_key_seq, 0);
        assert_eq!(entry.publisher_key_log, vec![payload(&inception())]);
        assert_eq!(entry.signature_status, PluginSignatureStatus::Valid);
        assert_eq!(entry.approved_by, Some(h.owner));

        let stored = h.store.lock().unwrap().entries[0].clone();
        assert_eq!(
            stored.publisher_key_log.map(|log| log.len()),
            Some(1),
            "a publisher not enrolled here keeps the supplied log"
        );
    }

    #[tokio::test]
    async fn add_takes_a_given_https_bundle_url_without_an_upload() {
        let h = harness();
        let mut cmd = h.add_cmd(h.owner, known());
        cmd.bundle_url = Some("https://cdn.example.com/hello.rimbundle".to_string());
        let entry = h.service.add(cmd).await.expect("add");
        assert!(matches!(
            entry.source,
            PluginSource::Bundle { ref url, .. } if url == "https://cdn.example.com/hello.rimbundle"
        ));
    }

    #[tokio::test]
    async fn add_of_a_plugin_id_the_registry_has_is_a_conflict() {
        let h = harness();
        h.add_known().await.expect("first");
        match h.service.add(h.add_cmd(h.owner, known())).await {
            Err(ServiceError::Conflict(message)) => {
                assert_eq!(
                    message,
                    "dev.example.hello-web is already in this workspace's plugins"
                )
            }
            other => panic!("expected Conflict, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn add_to_a_full_registry_is_a_conflict() {
        let h = harness();
        h.upload_bundle(SHA, 1605);
        for n in 0..100 {
            let id = format!("dev.example.p{n}");
            h.service
                .add(h.add_cmd(h.owner, web_statement(&id, "1.0.0")))
                .await
                .expect("under the cap");
        }
        match h.service.add(h.add_cmd(h.owner, known())).await {
            Err(ServiceError::Conflict(message)) => {
                assert_eq!(message, "this workspace already has 100 plugins")
            }
            other => panic!("expected Conflict, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn statements_that_fail_verification_are_refused_with_the_reason() {
        let h = harness();
        h.upload_bundle(SHA, 1605);

        let mut tampered = known();
        tampered.signature = web_statement(PLUGIN, "0.2.0").signature;
        assert_validation(
            h.service.add(h.add_cmd(h.owner, tampered)).await,
            "signature does not verify",
        );

        let mismatch = known().payload.replacen(
            r#""pluginId":"dev.example.hello-web""#,
            r#""pluginId":"dev.example.other""#,
            1,
        );
        let mismatch = payload(&sign_plugin_manifest(&signing_key(1), mismatch));
        assert_validation(
            h.service.add(h.add_cmd(h.owner, mismatch)).await,
            "pluginId does not match the manifest",
        );

        let (_, recovered) = rotated_logs();
        let mut cmd = h.add_cmd(h.owner, known());
        cmd.publisher_key_log = Some(recovered.iter().map(payload).collect());
        assert_validation(
            h.service.add(cmd).await,
            "the publisher recovered their key after keySeq 0",
        );
        assert!(h.store.lock().unwrap().entries.is_empty());
        assert!(h.events().is_empty());
    }

    #[tokio::test]
    async fn manifests_outside_the_registry_rules_are_refused() {
        let h = harness();
        h.upload_bundle(SHA, 1605);
        for (plugin_id, reason) in [
            (
                "dev.rim.hello",
                "plugin_id must not be under dev.rim. or com.rim.",
            ),
            (
                "example.hello",
                "plugin_id must be a reverse-domain name of at least three segments of ASCII \
                 letters, digits and hyphens",
            ),
            (
                "dev.example.hello_web",
                "plugin_id must be a reverse-domain name of at least three segments of ASCII \
                 letters, digits and hyphens",
            ),
        ] {
            assert_validation(
                h.service
                    .add(h.add_cmd(h.owner, web_statement(plugin_id, "1.0.0")))
                    .await,
                reason,
            );
        }
        let manifest = web_manifest(PLUGIN, "1.0.0").replacen(
            r#""manifest_version":1"#,
            r#""manifest_version":2"#,
            1,
        );
        let statement = plugin_manifest_payload(
            &publisher().principal_id(),
            0,
            "Ada",
            &manifest,
            &bundle_source(SHA, 1605),
        );
        let statement = payload(&sign_plugin_manifest(&signing_key(1), statement));
        assert_validation(
            h.service.add(h.add_cmd(h.owner, statement)).await,
            "the manifest's manifest_version must be 1",
        );
    }

    #[tokio::test]
    async fn a_publisher_not_enrolled_here_needs_a_log_that_replays() {
        let h = harness();
        h.upload_bundle(SHA, 1605);
        let mut cmd = h.add_cmd(h.owner, known());
        cmd.publisher_key_log = None;
        assert_validation(
            h.service.add(cmd).await,
            "the publisher is not enrolled on this server: publisherKeyLog is required",
        );

        let mut broken = payload(&inception());
        broken.signature = known().signature;
        let mut cmd = h.add_cmd(h.owner, known());
        cmd.publisher_key_log = Some(vec![broken]);
        assert_validation(
            h.service.add(cmd).await,
            "publisherKeyLog does not replay: signature does not verify",
        );

        let mut cmd = h.add_cmd(h.owner, known());
        cmd.publisher_key_log = Some(vec![payload(&inception()); 257]);
        assert_validation(
            h.service.add(cmd).await,
            "publisherKeyLog may have at most 256 entries",
        );
    }

    #[tokio::test]
    async fn this_servers_log_wins_over_a_supplied_one() {
        let h = harness();
        let (rotated, _) = rotated_logs();
        h.enroll_publisher(rotated.clone());
        h.upload_bundle(SHA, 1605);
        // A log that would not even replay is ignored for an enrolled publisher.
        let mut cmd = h.add_cmd(h.owner, known());
        cmd.publisher_key_log = Some(vec![known()]);
        let entry = h.service.add(cmd).await.expect("add");
        assert_eq!(
            entry.publisher_key_log,
            rotated.iter().map(payload).collect::<Vec<_>>()
        );
        assert_eq!(
            h.store.lock().unwrap().entries[0].publisher_key_log,
            None,
            "no snapshot for a publisher enrolled here"
        );
    }

    #[tokio::test]
    async fn the_default_url_needs_an_uploaded_bundle_of_that_size() {
        let h = harness();
        assert_validation(
            h.service.add(h.add_cmd(h.owner, known())).await,
            "bundle 5b154b3e… is not uploaded to this workspace",
        );
        h.upload_bundle(SHA, 1604);
        assert_validation(
            h.service.add(h.add_cmd(h.owner, known())).await,
            "bundle 5b154b3e… is 1604 bytes, the statement says 1605",
        );
    }

    #[tokio::test]
    async fn bad_bundle_urls_are_refused() {
        let h = harness();
        for url in [
            "http://cdn.example.com/b.rimbundle".to_string(),
            "https://".to_string(),
            "https://cdn.example.com/a b".to_string(),
            format!("https://cdn.example.com/{}", "a".repeat(2048)),
        ] {
            let mut cmd = h.add_cmd(h.owner, known());
            cmd.bundle_url = Some(url);
            assert_validation(
                h.service.add(cmd).await,
                "bundleUrl must be an https:// URL of at most 2048 bytes",
            );
        }
        let mut cmd = h.add_cmd(h.owner, git_statement("dev.example.desk"));
        cmd.bundle_url = Some("https://cdn.example.com/b.rimbundle".to_string());
        assert_validation(
            h.service.add(cmd).await,
            "bundleUrl is only for a bundle source",
        );
        let entry = h
            .service
            .add(h.add_cmd(h.owner, git_statement("dev.example.desk")))
            .await
            .expect("git source");
        assert!(matches!(entry.source, PluginSource::Git { .. }));
    }

    // ── Update ───────────────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn update_of_an_unknown_entry_is_not_found() {
        let h = harness();
        let mut update = h.update_cmd(h.owner);
        update.status = Some(WorkspacePluginStatus::Required);
        assert!(matches!(
            h.service.update(update).await,
            Err(ServiceError::NotFound)
        ));
        assert!(matches!(
            h.service.remove(h.remove_cmd(h.owner)).await,
            Err(ServiceError::NotFound)
        ));
    }

    #[tokio::test]
    async fn update_must_change_something_and_keep_the_plugin_id() {
        let h = harness();
        h.add_known().await.expect("add");
        let mut nothing = h.update_cmd(h.owner);
        nothing.publisher_key_log = Some(vec![payload(&inception())]);
        assert_validation(
            h.service.update(nothing).await,
            "nothing to update: give a statement, a status or a bundleUrl",
        );
        let mut other = h.update_cmd(h.owner);
        other.statement = Some(web_statement("dev.example.other", "0.1.0"));
        assert_validation(
            h.service.update(other).await,
            "the statement is for dev.example.other, not dev.example.hello-web",
        );
    }

    #[tokio::test]
    async fn update_replaces_the_statement_and_the_approval() {
        let h = harness();
        h.add_known().await.expect("add");
        let mut update = h.update_cmd(h.moderator);
        update.statement = Some(web_statement(PLUGIN, "0.0.9"));
        update.status = Some(WorkspacePluginStatus::Required);
        let entry = h.service.update(update).await.expect("update");
        // A lower version is an ordinary update.
        assert_eq!(entry.version, "0.0.9");
        assert_eq!(entry.status, WorkspacePluginStatus::Required);
        assert_eq!(entry.statement, web_statement(PLUGIN, "0.0.9"));
        assert_eq!(entry.approved_by, Some(h.moderator));

        // A status change alone keeps the statement and the source.
        let mut update = h.update_cmd(h.owner);
        update.status = Some(WorkspacePluginStatus::Recommended);
        let entry = h.service.update(update).await.expect("status");
        assert_eq!(entry.version, "0.0.9");
        assert_eq!(entry.status, WorkspacePluginStatus::Recommended);
        assert_eq!(entry.approved_by, Some(h.owner));

        // A URL change alone moves where the bundle is fetched from.
        let mut update = h.update_cmd(h.owner);
        update.bundle_url = Some("https://cdn.example.com/hello.rimbundle".to_string());
        let entry = h.service.update(update).await.expect("url");
        assert!(matches!(
            entry.source,
            PluginSource::Bundle { ref url, ref sha256, .. }
                if url == "https://cdn.example.com/hello.rimbundle" && sha256 == SHA
        ));
    }

    #[tokio::test]
    async fn the_kept_log_is_replaced_only_by_one_that_extends_it() {
        let h = harness();
        h.add_known().await.expect("add");
        let (rotated, recovered) = rotated_logs();

        // A diverging log: another person's inception.
        let stranger = TestIdentity::with_version(9, StatementVersion::V2).inception();
        let mut update = h.update_cmd(h.owner);
        update.status = Some(WorkspacePluginStatus::Required);
        update.publisher_key_log = Some(vec![payload(&stranger)]);
        assert_validation(
            h.service.update(update).await,
            "publisherKeyLog must extend the log this entry keeps",
        );

        // An extending log replaces the kept one.
        let mut update = h.update_cmd(h.owner);
        update.status = Some(WorkspacePluginStatus::Required);
        update.publisher_key_log = Some(rotated.iter().map(payload).collect());
        let entry = h.service.update(update).await.expect("extend");
        assert_eq!(entry.publisher_key_log.len(), 2);
        assert_eq!(
            h.store.lock().unwrap().entries[0]
                .publisher_key_log
                .as_ref()
                .map(Vec::len),
            Some(2)
        );

        // Without a log, an update reuses the kept one; a shorter log does not extend it.
        let mut update = h.update_cmd(h.owner);
        update.statement = Some(web_statement(PLUGIN, "0.2.0"));
        h.service.update(update).await.expect("reuses the kept log");
        let mut update = h.update_cmd(h.owner);
        update.status = Some(WorkspacePluginStatus::Recommended);
        update.publisher_key_log = Some(vec![payload(&inception())]);
        assert_validation(
            h.service.update(update).await,
            "publisherKeyLog must extend the log this entry keeps",
        );

        // A log that shows a recovery after the statement's keySeq is refused.
        let mut update = h.update_cmd(h.owner);
        update.status = Some(WorkspacePluginStatus::Recommended);
        update.publisher_key_log = Some(recovered.iter().map(payload).collect());
        assert_validation(
            h.service.update(update).await,
            "the publisher recovered their key after keySeq 0",
        );
    }

    #[tokio::test]
    async fn a_new_statement_without_a_url_points_at_this_servers_copy() {
        let h = harness();
        let mut cmd = h.add_cmd(h.owner, known());
        cmd.bundle_url = Some("https://cdn.example.com/hello.rimbundle".to_string());
        h.service.add(cmd).await.expect("add");
        let mut update = h.update_cmd(h.owner);
        update.statement = Some(web_statement(PLUGIN, "0.2.0"));
        assert_validation(
            h.service.update(update).await,
            "bundle 5b154b3e… is not uploaded to this workspace",
        );
        // The same statement again keeps its URL.
        let mut update = h.update_cmd(h.owner);
        update.status = Some(WorkspacePluginStatus::Required);
        update.publisher_key_log = Some(vec![payload(&inception())]);
        let entry = h.service.update(update).await.expect("same statement");
        assert!(matches!(
            entry.source,
            PluginSource::Bundle { ref url, .. } if url == "https://cdn.example.com/hello.rimbundle"
        ));
    }

    // ── Read time ────────────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn signature_status_follows_the_publishers_log_when_read() {
        let h = harness();
        let (rotated, recovered) = rotated_logs();
        h.enroll_publisher(rotated);
        h.add_known().await.expect("add");
        assert_eq!(
            h.list(h.member).await.unwrap()[0].signature_status,
            PluginSignatureStatus::Valid
        );
        // The publisher recovers their key on this server after the entry was approved.
        h.enroll_publisher(recovered);
        let entry = h.list(h.guest).await.unwrap().remove(0);
        assert_eq!(
            entry.signature_status,
            PluginSignatureStatus::PublisherRecovered
        );
        assert_eq!(entry.publisher_key_log.len(), 3);
    }

    #[tokio::test]
    async fn the_registry_lists_by_plugin_id() {
        let h = harness();
        h.upload_bundle(SHA, 1605);
        for id in ["dev.example.zeta", "dev.example.alpha", "dev.example.mid"] {
            h.service
                .add(h.add_cmd(h.owner, web_statement(id, "1.0.0")))
                .await
                .expect("add");
        }
        let ids: Vec<String> = h
            .list(h.member)
            .await
            .unwrap()
            .into_iter()
            .map(|e| e.plugin_id)
            .collect();
        assert_eq!(
            ids,
            ["dev.example.alpha", "dev.example.mid", "dev.example.zeta"]
        );
    }

    // ── Events and limits ────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn each_change_emits_one_event() {
        let h = harness();
        h.add_known().await.expect("add");
        let mut update = h.update_cmd(h.owner);
        update.status = Some(WorkspacePluginStatus::Required);
        h.service.update(update).await.expect("update");
        h.service
            .remove(h.remove_cmd(h.owner))
            .await
            .expect("remove");
        // Refused calls emit nothing.
        let _ = h.service.remove(h.remove_cmd(h.owner)).await;
        let _ = h.service.add(h.add_cmd(h.member, known())).await;
        assert_eq!(
            h.events(),
            [
                (PLUGIN.to_string(), WorkspacePluginChange::Added),
                (PLUGIN.to_string(), WorkspacePluginChange::Updated),
                (PLUGIN.to_string(), WorkspacePluginChange::Removed),
            ]
        );
        assert!(h.store.lock().unwrap().entries.is_empty());
    }

    #[tokio::test]
    async fn changes_are_rate_limited_per_user() {
        let h = harness_with_limit(RegistrationRateLimiter::new(3600, 2));
        h.add_known().await.expect("first");
        let mut update = h.update_cmd(h.owner);
        update.status = Some(WorkspacePluginStatus::Required);
        h.service.update(update.clone()).await.expect("second");
        assert!(matches!(
            h.service.update(update.clone()).await,
            Err(ServiceError::RateLimited { .. })
        ));
        assert!(matches!(
            h.service.remove(h.remove_cmd(h.owner)).await,
            Err(ServiceError::RateLimited { .. })
        ));
        // Another user has their own budget.
        update.actor = Actor::new(h.moderator);
        h.service.update(update).await.expect("moderator");
    }
}
