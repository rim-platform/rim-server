//! Web plugin bundles a workspace keeps for its registry, so the web app can fetch them from a
//! host that sends CORS headers. The server checks a bundle's structure, never its code: the
//! hash in the publisher's signed statement decides whether the bytes are right.

use std::sync::Arc;

use async_trait::async_trait;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use chrono::{Duration, Utc};
use serde::Deserialize;
use tracing::{info, instrument, warn};

use rim_core::server::model::ServerId;
use rim_core::server_member::repository::ServerMemberRepository;
use rim_core::user::model::UserId;
use rim_core::workspace_plugin::model::NewPluginBundle;
use rim_core::workspace_plugin::repository::WorkspacePluginRepository;
use rim_services::command::rate_limit::RateLimitDecision;
use rim_services::storage::service::StorageService;
use rim_services::workspace_plugin::command::{DownloadPluginBundleQuery, UploadPluginBundleCmd};
use rim_services::workspace_plugin::dto::UploadedPluginBundleDto;
use rim_services::workspace_plugin::service::PluginBundleService;
use rim_services::workspace_plugin::{
    MAX_PLUGIN_BUNDLE_BYTES, PLUGIN_BUNDLE_MEDIA_TYPE, plugin_bundle_path,
};
use rim_services::{ServiceError, ServiceResult};

use crate::registration_rate_limiter::RegistrationRateLimiter;

/// Most files a bundle holds.
const MAX_BUNDLE_FILES: usize = 256;

/// Longest path in a bundle, in bytes.
const MAX_BUNDLE_PATH_LEN: usize = 255;

/// How long a bundle no entry names is kept.
const UNNAMED_BUNDLE_AGE: Duration = Duration::hours(24);

/// Limiter key for uploads.
const UPLOAD_KEY: &str = "plugin_bundle_upload";

#[derive(Clone)]
pub struct PluginBundleRepos {
    pub server_member_repo: Arc<dyn ServerMemberRepository>,
    pub workspace_plugin_repo: Arc<dyn WorkspacePluginRepository>,
}

pub struct DefaultPluginBundleService {
    repos: PluginBundleRepos,
    storage: Arc<dyn StorageService>,
    /// Uploads per user.
    upload_rate_limiter: RegistrationRateLimiter,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBundle {
    format: String,
    v: u32,
    files: Vec<RawBundleFile>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBundleFile {
    path: String,
    text: Option<String>,
    base64: Option<String>,
}

/// Where a workspace's bundle lives in file storage.
fn storage_key(server_id: ServerId, sha256: &str) -> String {
    format!("plugin-bundles/{server_id}/{sha256}")
}

fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn check_path(path: &str) -> Result<(), String> {
    let well_formed = (1..=MAX_BUNDLE_PATH_LEN).contains(&path.len())
        && path.split('/').all(|segment| {
            !segment.is_empty()
                && !segment.starts_with('.')
                && segment
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
        });
    if well_formed {
        Ok(())
    } else {
        Err(format!(
            "{path:?} is not a bundle path: 1 to {MAX_BUNDLE_PATH_LEN} bytes of \
             '/'-separated segments of A-Z a-z 0-9 . _ -, none empty or starting with '.'"
        ))
    }
}

/// Check that `bytes` are a plugin bundle: the format and version, 1 to 256 files sorted by
/// path with each path once, the path rules, one of `text` or `base64` per file, and a
/// `rim-plugin.json`. Returns why not.
pub(crate) fn check_bundle(bytes: &[u8]) -> Result<(), String> {
    // A struct also deserializes from a JSON array, so the object is checked by its first byte.
    if bytes.first() != Some(&b'{') {
        return Err("a bundle is a JSON object".to_string());
    }
    let bundle: RawBundle = serde_json::from_slice(bytes).map_err(|e| {
        tracing::info!(detail = %e, "refused a malformed plugin bundle");
        "not a bundle: the JSON does not have the bundle's fields and types".to_string()
    })?;
    if bundle.format != "rim-plugin-bundle" {
        return Err("format must be rim-plugin-bundle".to_string());
    }
    if bundle.v != 1 {
        return Err(format!("unsupported bundle version {}", bundle.v));
    }
    if bundle.files.is_empty() || bundle.files.len() > MAX_BUNDLE_FILES {
        return Err(format!("a bundle has 1 to {MAX_BUNDLE_FILES} files"));
    }
    for file in &bundle.files {
        check_path(&file.path)?;
        match (&file.text, &file.base64) {
            (Some(_), None) => {}
            (None, Some(encoded)) => {
                STANDARD
                    .decode(encoded)
                    .map_err(|_| format!("{:?} is not valid base64", file.path))?;
            }
            _ => {
                return Err(format!(
                    "{:?} must have exactly one of text and base64",
                    file.path
                ));
            }
        }
    }
    if !bundle
        .files
        .windows(2)
        .all(|pair| pair[0].path.as_bytes() < pair[1].path.as_bytes())
    {
        return Err("files must be sorted by path, each path once".to_string());
    }
    if !bundle.files.iter().any(|f| f.path == "rim-plugin.json") {
        return Err("rim-plugin.json is missing".to_string());
    }
    Ok(())
}

impl DefaultPluginBundleService {
    pub fn new(repos: PluginBundleRepos, storage: Arc<dyn StorageService>) -> Self {
        Self {
            repos,
            storage,
            upload_rate_limiter: RegistrationRateLimiter::new(3600, 20),
        }
    }

    /// Replace the limiter of uploads per user.
    #[cfg(test)]
    pub(crate) fn with_rate_limiter(mut self, upload: RegistrationRateLimiter) -> Self {
        self.upload_rate_limiter = upload;
        self
    }

    fn limit(&self, user_id: UserId) -> ServiceResult<()> {
        match self.upload_rate_limiter.check_and_record(
            &user_id.to_string(),
            UPLOAD_KEY,
            Utc::now(),
        ) {
            RateLimitDecision::Allowed => Ok(()),
            RateLimitDecision::Limited {
                retry_after_seconds,
            } => {
                crate::metrics::rate_limited(UPLOAD_KEY);
                Err(ServiceError::RateLimited {
                    retry_after_seconds,
                })
            }
        }
    }

    /// Delete the workspace's bundles no entry names that are over a day old. Best effort: a
    /// file left behind costs only space, and the next upload tries again.
    async fn clean_up(&self, server_id: ServerId) {
        let keys = match self
            .repos
            .workspace_plugin_repo
            .delete_unnamed_bundles(server_id, UNNAMED_BUNDLE_AGE)
            .await
        {
            Ok(keys) => keys,
            Err(e) => {
                warn!(server_id = %server_id, error = %e, "could not clean up plugin bundles");
                return;
            }
        };
        for key in keys {
            if let Err(e) = self.storage.delete_file(&key).await {
                warn!(key = %key, error = %e, "could not delete a plugin bundle file");
            }
        }
    }

    /// Store a new bundle within the workspace's quota. Returns `false` when a concurrent
    /// upload recorded the same hash first.
    async fn store(
        &self,
        server_id: ServerId,
        actor_id: UserId,
        sha256: &str,
        bytes: &[u8],
    ) -> ServiceResult<bool> {
        let size = bytes.len() as u64;
        let usage = self.storage.get_quota_usage(server_id).await?;
        if let Some(max) = usage.max_bytes
            && usage.used_bytes.saturating_add(size) > max
        {
            return Err(ServiceError::Validation(format!(
                "storage quota exceeded: {} of {max} bytes used",
                usage.used_bytes
            )));
        }
        let key = storage_key(server_id, sha256);
        self.storage
            .upload_file(&key, bytes, PLUGIN_BUNDLE_MEDIA_TYPE)
            .await?;
        let inserted = self
            .repos
            .workspace_plugin_repo
            .insert_bundle(NewPluginBundle {
                server_id,
                sha256: sha256.to_string(),
                size_bytes: size,
                storage_key: key,
                uploaded_by: actor_id,
            })
            .await?;
        Ok(inserted.is_some())
    }
}

#[async_trait]
impl PluginBundleService for DefaultPluginBundleService {
    #[instrument(name = "plugin_bundle.upload", skip_all, fields(actor_id = %cmd.actor.user_id))]
    async fn upload(&self, cmd: UploadPluginBundleCmd) -> ServiceResult<UploadedPluginBundleDto> {
        let actor_id = cmd.actor.user_id;
        let server_id = cmd.server_id;
        self.limit(actor_id)?;
        let member = self
            .repos
            .server_member_repo
            .get_member(server_id, actor_id)
            .await?
            .ok_or(ServiceError::PermissionDenied)?;
        if !member.member_role.is_moderator() {
            return Err(ServiceError::PermissionDenied);
        }
        if cmd.bytes.len() > MAX_PLUGIN_BUNDLE_BYTES {
            return Err(ServiceError::Validation(format!(
                "a bundle is at most {MAX_PLUGIN_BUNDLE_BYTES} bytes"
            )));
        }
        if !is_sha256_hex(&cmd.sha256) {
            return Err(ServiceError::internal(anyhow::anyhow!(
                "the bundle hash is not 64 lowercase hex characters"
            )));
        }
        check_bundle(&cmd.bytes).map_err(ServiceError::Validation)?;

        // First, so a bundle re-uploaded after it aged out is stored again, not deleted after.
        self.clean_up(server_id).await;

        let size = cmd.bytes.len() as u64;
        let url = plugin_bundle_path(server_id, &cmd.sha256);
        let existing = self
            .repos
            .workspace_plugin_repo
            .get_bundle(server_id, &cmd.sha256)
            .await?;
        let created = match existing {
            Some(_) => false,
            None => {
                self.store(server_id, actor_id, &cmd.sha256, &cmd.bytes)
                    .await?
            }
        };
        info!(
            target = "audit",
            event = "plugin_bundle_uploaded",
            server_id = %server_id,
            actor_id = %actor_id,
            bundle_sha256 = %cmd.sha256,
            size,
            created,
            "plugin bundle uploaded"
        );
        Ok(UploadedPluginBundleDto {
            sha256: cmd.sha256,
            size,
            url,
            created,
        })
    }

    #[instrument(name = "plugin_bundle.download", skip_all, fields(actor_id = %query.actor.user_id))]
    async fn download(&self, query: DownloadPluginBundleQuery) -> ServiceResult<Vec<u8>> {
        self.repos
            .server_member_repo
            .get_member(query.server_id, query.actor.user_id)
            .await?
            .ok_or(ServiceError::NotFound)?;
        if !is_sha256_hex(&query.sha256) {
            return Err(ServiceError::NotFound);
        }
        let bundle = self
            .repos
            .workspace_plugin_repo
            .get_bundle(query.server_id, &query.sha256)
            .await?
            .ok_or(ServiceError::NotFound)?;
        self.storage.download_file(&bundle.storage_key).await
    }
}
