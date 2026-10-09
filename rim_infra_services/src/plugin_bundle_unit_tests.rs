#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    use chrono::Utc;

    use rim_core::server::model::ServerId;
    use rim_core::server_member::model::{ServerMember, ServerMemberId, ServerMemberRole};
    use rim_core::server_member::repository::MockServerMemberRepository;
    use rim_core::storage::StoredFile;
    use rim_core::user::model::UserId;
    use rim_core::workspace_plugin::model::PluginBundle;
    use rim_core::workspace_plugin::repository::MockWorkspacePluginRepository;
    use rim_identity::testing::KNOWN_PLUGIN_BUNDLE;
    use rim_services::storage::service::{MockStorageService, QuotaUsage};
    use rim_services::workspace_plugin::command::{
        DownloadPluginBundleQuery, UploadPluginBundleCmd,
    };
    use rim_services::workspace_plugin::service::PluginBundleService;
    use rim_services::{Actor, ServiceError};

    use crate::plugin_bundle::{DefaultPluginBundleService, PluginBundleRepos, check_bundle};
    use crate::registration_rate_limiter::RegistrationRateLimiter;

    const SHA: &str = "5b154b3eb45c3ebc44f6820ba30a389ce85b43dea34bcbf652b9b1a5416fbe85";

    // ── Structure ────────────────────────────────────────────────────────────────────────

    fn bundle(files: &str) -> Vec<u8> {
        format!(r#"{{"format":"rim-plugin-bundle","v":1,"files":[{files}]}}"#).into_bytes()
    }

    const MANIFEST: &str = r#"{"path":"rim-plugin.json","text":"{}"}"#;

    #[test]
    fn the_known_bundle_is_a_bundle() {
        assert_eq!(check_bundle(KNOWN_PLUGIN_BUNDLE), Ok(()));
        let with_panel = bundle(&format!(
            r#"{{"path":"main.mjs","text":""}},{{"path":"panels/prs/icon.png","base64":"iVBORw=="}},{MANIFEST}"#
        ));
        assert_eq!(check_bundle(&with_panel), Ok(()));
    }

    #[test]
    fn malformed_bundles_are_refused_with_the_reason() {
        let cases: Vec<(Vec<u8>, &str)> = vec![
            (b"[]".to_vec(), "a bundle is a JSON object"),
            (b"{".to_vec(), "not a bundle"),
            (
                br#"{"format":"zip","v":1,"files":[]}"#.to_vec(),
                "format must be rim-plugin-bundle",
            ),
            (
                br#"{"format":"rim-plugin-bundle","v":2,"files":[]}"#.to_vec(),
                "unsupported bundle version 2",
            ),
            (bundle(""), "a bundle has 1 to 256 files"),
            (
                br#"{"format":"rim-plugin-bundle","v":1,"files":[],"x":1}"#.to_vec(),
                "not a bundle",
            ),
            (
                bundle(r#"{"path":"rim-plugin.json","text":"{}","mode":"0755"}"#),
                "not a bundle",
            ),
            (
                bundle(&format!(r#"{MANIFEST},{{"path":"a.js","text":""}}"#)),
                "files must be sorted by path, each path once",
            ),
            (
                bundle(&format!(r#"{MANIFEST},{MANIFEST}"#)),
                "files must be sorted by path, each path once",
            ),
            (
                bundle(&format!(r#"{{"path":"../x","text":""}},{MANIFEST}"#)),
                "is not a bundle path",
            ),
            (
                bundle(&format!(r#"{{"path":".env","text":""}},{MANIFEST}"#)),
                "is not a bundle path",
            ),
            (
                bundle(&format!(r#"{{"path":"a//b","text":""}},{MANIFEST}"#)),
                "is not a bundle path",
            ),
            (
                bundle(&format!(r#"{{"path":"a b","text":""}},{MANIFEST}"#)),
                "is not a bundle path",
            ),
            (
                bundle(&format!(
                    r#"{{"path":"{}","text":""}},{MANIFEST}"#,
                    "a".repeat(256)
                )),
                "is not a bundle path",
            ),
            (
                bundle(&format!(
                    r#"{{"path":"a.js","text":"","base64":""}},{MANIFEST}"#
                )),
                "must have exactly one of text and base64",
            ),
            (
                bundle(&format!(r#"{{"path":"a.js"}},{MANIFEST}"#)),
                "must have exactly one of text and base64",
            ),
            (
                bundle(&format!(
                    r#"{{"path":"a.png","base64":"iVBORw"}},{MANIFEST}"#
                )),
                "is not valid base64",
            ),
            (
                bundle(r#"{"path":"main.mjs","text":""}"#),
                "rim-plugin.json is missing",
            ),
        ];
        for (bytes, reason) in cases {
            let err = check_bundle(&bytes).expect_err(&String::from_utf8_lossy(&bytes));
            assert!(err.contains(reason), "{err:?} should say {reason:?}");
        }

        let many: Vec<String> = (0..257)
            .map(|n| format!(r#"{{"path":"f{n:03}","text":""}}"#))
            .collect();
        assert_eq!(
            check_bundle(&bundle(&many.join(","))),
            Err("a bundle has 1 to 256 files".to_string())
        );
    }

    // ── Service ──────────────────────────────────────────────────────────────────────────

    struct Fixture {
        server_id: ServerId,
        owner: UserId,
        moderator: UserId,
        member: UserId,
        guest: UserId,
        outsider: UserId,
        plugins: MockWorkspacePluginRepository,
        storage: MockStorageService,
    }

    fn fixture() -> Fixture {
        Fixture {
            server_id: ServerId::new(),
            owner: UserId::new(),
            moderator: UserId::new(),
            member: UserId::new(),
            guest: UserId::new(),
            outsider: UserId::new(),
            plugins: MockWorkspacePluginRepository::new(),
            storage: MockStorageService::new(),
        }
    }

    impl Fixture {
        fn build(self, limiter: RegistrationRateLimiter) -> Built {
            let roles: HashMap<UserId, ServerMemberRole> = [
                (self.owner, ServerMemberRole::Owner),
                (self.moderator, ServerMemberRole::Moderator),
                (self.member, ServerMemberRole::Member),
                (self.guest, ServerMemberRole::Guest),
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
            let service = DefaultPluginBundleService::new(
                PluginBundleRepos {
                    server_member_repo: Arc::new(members),
                    workspace_plugin_repo: Arc::new(self.plugins),
                },
                Arc::new(self.storage),
            )
            .with_rate_limiter(limiter);
            Built {
                server_id: self.server_id,
                owner: self.owner,
                moderator: self.moderator,
                member: self.member,
                guest: self.guest,
                outsider: self.outsider,
                service,
            }
        }

        /// The cleanup finds nothing to delete.
        fn nothing_to_clean(&mut self) {
            self.plugins
                .expect_delete_unnamed_bundles()
                .returning(|_, _| Box::pin(async { Ok(Vec::new()) }));
        }

        fn has_bundle(&mut self, present: bool) {
            let server_id = self.server_id;
            self.plugins
                .expect_get_bundle()
                .returning(move |_, sha256| {
                    let bundle = present.then(|| PluginBundle {
                        server_id,
                        sha256: sha256.to_string(),
                        size_bytes: 1605,
                        storage_key: format!("plugin-bundles/{server_id}/{sha256}"),
                        uploaded_by: None,
                        created_at: Utc::now(),
                    });
                    Box::pin(async move { Ok(bundle) })
                });
        }

        fn quota(&mut self, used: u64, max: u64) {
            self.storage.expect_get_quota_usage().returning(move |_| {
                Box::pin(async move {
                    Ok(QuotaUsage {
                        used_bytes: used,
                        max_bytes: Some(max),
                        file_count: 0,
                    })
                })
            });
        }
    }

    struct Built {
        server_id: ServerId,
        owner: UserId,
        moderator: UserId,
        member: UserId,
        guest: UserId,
        outsider: UserId,
        service: DefaultPluginBundleService,
    }

    impl Built {
        fn upload(&self, by: UserId, bytes: &[u8]) -> UploadPluginBundleCmd {
            UploadPluginBundleCmd {
                actor: Actor::new(by),
                server_id: self.server_id,
                bytes: bytes.to_vec(),
                sha256: SHA.to_string(),
            }
        }

        fn download(&self, by: UserId, sha256: &str) -> DownloadPluginBundleQuery {
            DownloadPluginBundleQuery {
                actor: Actor::new(by),
                server_id: self.server_id,
                sha256: sha256.to_string(),
            }
        }
    }

    fn generous() -> RegistrationRateLimiter {
        RegistrationRateLimiter::new(3600, 1000)
    }

    #[tokio::test]
    async fn an_owner_or_moderator_stores_a_new_bundle() {
        let mut f = fixture();
        let server_id = f.server_id;
        f.nothing_to_clean();
        f.has_bundle(false);
        f.quota(0, 1_000_000);
        let stored: Arc<Mutex<Vec<(String, usize, String)>>> = Arc::default();
        let record = Arc::clone(&stored);
        f.storage
            .expect_upload_file()
            .returning(move |key, data, content_type| {
                record.lock().unwrap().push((
                    key.to_string(),
                    data.len(),
                    content_type.to_string(),
                ));
                let file = StoredFile {
                    key: key.to_string(),
                    size_bytes: data.len() as u64,
                };
                Box::pin(async move { Ok(file) })
            });
        f.plugins.expect_insert_bundle().returning(|new| {
            let bundle = PluginBundle {
                server_id: new.server_id,
                sha256: new.sha256,
                size_bytes: new.size_bytes,
                storage_key: new.storage_key,
                uploaded_by: Some(new.uploaded_by),
                created_at: Utc::now(),
            };
            Box::pin(async move { Ok(Some(bundle)) })
        });
        let b = f.build(generous());

        for by in [b.owner, b.moderator] {
            let uploaded = b
                .service
                .upload(b.upload(by, KNOWN_PLUGIN_BUNDLE))
                .await
                .expect("upload");
            assert!(uploaded.created);
            assert_eq!(uploaded.sha256, SHA);
            assert_eq!(uploaded.size, 1605);
            assert_eq!(
                uploaded.url,
                format!("/api/plugin-bundles/{server_id}/{SHA}")
            );
        }
        assert_eq!(
            stored.lock().unwrap()[0],
            (
                format!("plugin-bundles/{server_id}/{SHA}"),
                1605,
                "application/vnd.rim.plugin-bundle+json".to_string()
            )
        );
    }

    #[tokio::test]
    async fn a_hash_the_workspace_has_is_not_stored_again() {
        let mut f = fixture();
        f.nothing_to_clean();
        f.has_bundle(true);
        f.storage.expect_upload_file().never();
        let b = f.build(generous());
        let uploaded = b
            .service
            .upload(b.upload(b.owner, KNOWN_PLUGIN_BUNDLE))
            .await
            .expect("upload");
        assert!(!uploaded.created);
    }

    #[tokio::test]
    async fn members_guests_and_outsiders_cannot_upload() {
        let mut f = fixture();
        f.storage.expect_upload_file().never();
        let b = f.build(generous());
        for by in [b.member, b.guest, b.outsider] {
            assert!(matches!(
                b.service.upload(b.upload(by, KNOWN_PLUGIN_BUNDLE)).await,
                Err(ServiceError::PermissionDenied)
            ));
        }
    }

    #[tokio::test]
    async fn bytes_that_are_not_a_bundle_are_refused() {
        let f = fixture();
        let b = f.build(generous());
        match b.service.upload(b.upload(b.owner, b"[]")).await {
            Err(ServiceError::Validation(reason)) => {
                assert_eq!(reason, "a bundle is a JSON object")
            }
            other => panic!("expected Validation, got {other:?}"),
        }
        let too_big = vec![b' '; 8 * 1024 * 1024 + 1];
        assert!(matches!(
            b.service.upload(b.upload(b.owner, &too_big)).await,
            Err(ServiceError::Validation(_))
        ));
    }

    #[tokio::test]
    async fn an_upload_past_the_storage_quota_is_refused() {
        let mut f = fixture();
        f.nothing_to_clean();
        f.has_bundle(false);
        f.quota(999_000, 1_000_000);
        f.storage.expect_upload_file().never();
        let b = f.build(generous());
        match b
            .service
            .upload(b.upload(b.owner, KNOWN_PLUGIN_BUNDLE))
            .await
        {
            Err(ServiceError::Validation(reason)) => assert_eq!(
                reason,
                "storage quota exceeded: 999000 of 1000000 bytes used"
            ),
            other => panic!("expected Validation, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn uploads_are_rate_limited_per_user() {
        let mut f = fixture();
        f.nothing_to_clean();
        f.has_bundle(true);
        let b = f.build(RegistrationRateLimiter::new(3600, 1));
        b.service
            .upload(b.upload(b.owner, KNOWN_PLUGIN_BUNDLE))
            .await
            .expect("first");
        assert!(matches!(
            b.service
                .upload(b.upload(b.owner, KNOWN_PLUGIN_BUNDLE))
                .await,
            Err(ServiceError::RateLimited { .. })
        ));
        b.service
            .upload(b.upload(b.moderator, KNOWN_PLUGIN_BUNDLE))
            .await
            .expect("another user");
    }

    #[tokio::test]
    async fn each_upload_deletes_the_files_of_aged_unnamed_bundles() {
        let mut f = fixture();
        let server_id = f.server_id;
        f.plugins
            .expect_delete_unnamed_bundles()
            .withf(move |sid, age| *sid == server_id && *age == chrono::Duration::hours(24))
            .returning(|_, _| {
                Box::pin(async {
                    Ok(vec![
                        "plugin-bundles/a".to_string(),
                        "plugin-bundles/b".to_string(),
                    ])
                })
            });
        let deleted: Arc<Mutex<Vec<String>>> = Arc::default();
        let record = Arc::clone(&deleted);
        f.storage.expect_delete_file().returning(move |key| {
            record.lock().unwrap().push(key.to_string());
            // A file that cannot be deleted does not fail the upload.
            let result = if key.ends_with('a') {
                Err(ServiceError::NotFound)
            } else {
                Ok(())
            };
            Box::pin(async move { result })
        });
        f.has_bundle(true);
        let b = f.build(generous());
        b.service
            .upload(b.upload(b.owner, KNOWN_PLUGIN_BUNDLE))
            .await
            .expect("upload");
        assert_eq!(
            *deleted.lock().unwrap(),
            ["plugin-bundles/a", "plugin-bundles/b"]
        );
    }

    #[tokio::test]
    async fn members_download_and_others_get_not_found() {
        let mut f = fixture();
        f.has_bundle(true);
        f.storage
            .expect_download_file()
            .returning(|_| Box::pin(async { Ok(KNOWN_PLUGIN_BUNDLE.to_vec()) }));
        let b = f.build(generous());
        for by in [b.owner, b.member, b.guest] {
            let bytes = b
                .service
                .download(b.download(by, SHA))
                .await
                .expect("download");
            assert_eq!(bytes, KNOWN_PLUGIN_BUNDLE);
        }
        assert!(matches!(
            b.service.download(b.download(b.outsider, SHA)).await,
            Err(ServiceError::NotFound)
        ));
        assert!(matches!(
            b.service
                .download(b.download(b.member, "../etc/passwd"))
                .await,
            Err(ServiceError::NotFound)
        ));
    }

    #[tokio::test]
    async fn an_unknown_hash_is_not_found() {
        let mut f = fixture();
        f.has_bundle(false);
        f.storage.expect_download_file().never();
        let b = f.build(generous());
        assert!(matches!(
            b.service.download(b.download(b.member, SHA)).await,
            Err(ServiceError::NotFound)
        ));
    }
}
