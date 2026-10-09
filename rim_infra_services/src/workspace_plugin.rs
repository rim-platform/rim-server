//! Workspace plugin registries. Every statement is verified against its publisher's key log
//! before it is stored: this server's copy when the publisher is enrolled here, else the log the
//! caller supplies, which the entry then keeps. Whether a stored statement still stands is
//! judged again each time the entry is read.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use sea_orm::DatabaseConnection;
use tracing::{info, instrument};

use rim_core::key_identity::repository::KeyIdentityRepository;
use rim_core::server::model::ServerId;
use rim_core::server_member::repository::ServerMemberRepository;
use rim_core::user::model::UserId;
use rim_core::workspace_plugin::model::{
    MAX_WORKSPACE_PLUGINS, PluginRuntimeKind, PluginSource, StatementText, WorkspacePlugin,
    WorkspacePluginWrite,
};
use rim_core::workspace_plugin::repository::WorkspacePluginRepository;
use rim_identity::{
    IdentityError, KeyHistory, PluginManifestStatement, RotationSigner, SignedStatement, Validity,
    entry_signer, read_plugin_manifest, verify_plugin_manifest,
};
use rim_infra_db::{TransactionScope, begin_transaction};
use rim_services::command::rate_limit::RateLimitDecision;
use rim_services::events::{EventObserver, Observers, WorkspaceEvent, WorkspacePluginChange};
use rim_services::key_auth::command::SignedPayload;
use rim_services::workspace_plugin::command::{
    AddWorkspacePluginCmd, ListWorkspacePluginsQuery, RemoveWorkspacePluginCmd,
    UpdateWorkspacePluginCmd,
};
use rim_services::workspace_plugin::dto::{PluginSignatureStatus, WorkspacePluginDto};
use rim_services::workspace_plugin::plugin_bundle_path;
use rim_services::workspace_plugin::service::WorkspacePluginService;
use rim_services::{ServiceError, ServiceResult};

use crate::registration_rate_limiter::RegistrationRateLimiter;
use crate::repos::AppRepos;

/// Most entries a supplied publisher key log may have.
const MAX_PUBLISHER_KEY_LOG_LEN: usize = 256;

/// Longest `bundleUrl`, in bytes.
const MAX_BUNDLE_URL_LEN: usize = 2048;

/// Longest manifest `display_name`, in characters.
const MAX_DISPLAY_NAME_CHARS: usize = 80;

/// Plugin id prefixes kept for RIM's own plugins.
const RESERVED_PLUGIN_ID_PREFIXES: &[&str] = &["dev.rim.", "com.rim."];

/// Limiter key for adds, updates and removals.
const CHANGE_KEY: &str = "workspace_plugin_change";

#[derive(Clone)]
pub struct WorkspacePluginRepos {
    pub server_member_repo: Arc<dyn ServerMemberRepository>,
    pub key_identity_repo: Arc<dyn KeyIdentityRepository>,
    pub workspace_plugin_repo: Arc<dyn WorkspacePluginRepository>,
}

pub struct DefaultWorkspacePluginService {
    repos: WorkspacePluginRepos,
    /// When set, adds and updates run in one transaction with the server row locked.
    db: Option<DatabaseConnection>,
    observers: Observers,
    /// Adds, updates and removals per user.
    change_rate_limiter: RegistrationRateLimiter,
}

/// A statement that verified, and the publisher key log the entry keeps with it.
struct Verified {
    statement: PluginManifestStatement,
    /// `Some` only for a publisher not enrolled on this server.
    key_log_snapshot: Option<Vec<StatementText>>,
}

fn to_statement(p: &SignedPayload) -> SignedStatement {
    SignedStatement {
        payload: p.payload.clone(),
        signature: p.signature.clone(),
    }
}

fn to_text(p: &SignedPayload) -> StatementText {
    StatementText {
        payload: p.payload.clone(),
        signature: p.signature.clone(),
    }
}

fn text_statement(t: &StatementText) -> SignedStatement {
    SignedStatement {
        payload: t.payload.clone(),
        signature: t.signature.clone(),
    }
}

fn to_payload(t: StatementText) -> SignedPayload {
    SignedPayload {
        payload: t.payload,
        signature: t.signature,
    }
}

fn invalid(err: IdentityError) -> ServiceError {
    crate::util::statement_refused(err)
}

fn validation(message: impl Into<String>) -> ServiceError {
    ServiceError::Validation(message.into())
}

fn runtime_kind(kind: rim_identity::PluginRuntimeKind) -> PluginRuntimeKind {
    match kind {
        rim_identity::PluginRuntimeKind::DesktopLocalProcess => {
            PluginRuntimeKind::DesktopLocalProcess
        }
        rim_identity::PluginRuntimeKind::WebSandboxed => PluginRuntimeKind::WebSandboxed,
    }
}

/// The manifest fields a registry entry needs; the host checks the rest before it runs one.
fn check_manifest(statement: &PluginManifestStatement) -> ServiceResult<()> {
    if statement.manifest_version != 1 {
        return Err(validation("the manifest's manifest_version must be 1"));
    }
    let id = &statement.plugin_id;
    let segments: Vec<&str> = id.split('.').collect();
    let well_formed = segments.len() >= 3
        && segments
            .iter()
            .all(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'));
    if !well_formed {
        return Err(validation(
            "plugin_id must be a reverse-domain name of at least three segments of ASCII \
             letters, digits and hyphens",
        ));
    }
    let lowercase = id.to_ascii_lowercase();
    if RESERVED_PLUGIN_ID_PREFIXES
        .iter()
        .any(|prefix| lowercase.starts_with(prefix))
    {
        return Err(validation(
            "plugin_id must not be under dev.rim. or com.rim.",
        ));
    }
    let name = &statement.display_name;
    if name.trim().is_empty() || name.chars().count() > MAX_DISPLAY_NAME_CHARS {
        return Err(validation(format!(
            "the manifest's display_name must be 1 to {MAX_DISPLAY_NAME_CHARS} characters"
        )));
    }
    Ok(())
}

fn check_bundle_url(url: &str) -> ServiceResult<()> {
    let ok = url.strip_prefix("https://").is_some_and(|r| !r.is_empty())
        && url.len() <= MAX_BUNDLE_URL_LEN
        && !url.chars().any(|c| c.is_whitespace() || c.is_control());
    if !ok {
        return Err(validation(format!(
            "bundleUrl must be an https:// URL of at most {MAX_BUNDLE_URL_LEN} bytes"
        )));
    }
    Ok(())
}

/// Whether a log extends `stored`: the same entries first, then possibly more.
fn extends(log: &[StatementText], stored: &[StatementText]) -> bool {
    log.len() >= stored.len() && log[..stored.len()] == *stored
}

/// The signature status of a statement at `key_seq`, from the signers of the later entries of
/// a log whose signatures were verified when it was stored.
fn signature_status(log: &[StatementText], key_seq: u32) -> ServiceResult<PluginSignatureStatus> {
    for entry in log.iter().skip(key_seq as usize + 1) {
        let signer = entry_signer(&text_statement(entry)).map_err(|e| {
            ServiceError::internal(anyhow::anyhow!("stored key log entry is unreadable: {e}"))
        })?;
        if signer == Some(RotationSigner::Recovery) {
            return Ok(PluginSignatureStatus::PublisherRecovered);
        }
    }
    Ok(PluginSignatureStatus::Valid)
}

/// The audit fields of an entry: never the statement text.
fn audit(event: &str, actor_id: UserId, entry: &WorkspacePlugin, message: &str) {
    info!(
        target = "audit",
        event,
        server_id = %entry.server_id,
        actor_id = %actor_id,
        plugin_id = %entry.plugin_id,
        version = %entry.version,
        status = entry.status.as_str(),
        publisher_principal_id = %entry.publisher_principal_id,
        bundle_sha256 = entry.source.bundle_sha256().unwrap_or(""),
        "{message}"
    );
}

impl DefaultWorkspacePluginService {
    /// The service with the default limit; `db` makes adds and updates transactional.
    pub fn new(
        repos: WorkspacePluginRepos,
        db: Option<DatabaseConnection>,
        observers: Observers,
    ) -> Self {
        Self {
            repos,
            db,
            observers,
            change_rate_limiter: RegistrationRateLimiter::new(3600, 60),
        }
    }

    /// Replace the limiter of adds, updates and removals per user.
    #[cfg(test)]
    pub(crate) fn with_rate_limiter(mut self, change: RegistrationRateLimiter) -> Self {
        self.change_rate_limiter = change;
        self
    }

    /// Open a transaction when a database is configured, and the repos that run inside it.
    async fn begin(&self) -> ServiceResult<(Option<TransactionScope>, WorkspacePluginRepos)> {
        let scope = match &self.db {
            Some(db) => Some(
                begin_transaction(db)
                    .await
                    .map_err(ServiceError::internal)?,
            ),
            None => None,
        };
        let repos = match &scope {
            Some(s) => AppRepos::with_handle(&s.handle()).for_workspace_plugin(),
            None => self.repos.clone(),
        };
        Ok((scope, repos))
    }

    async fn commit(scope: Option<TransactionScope>) -> ServiceResult<()> {
        if let Some(scope) = scope {
            scope.commit().await.map_err(ServiceError::internal)?;
        }
        Ok(())
    }

    fn limit(&self, user_id: UserId) -> ServiceResult<()> {
        match self.change_rate_limiter.check_and_record(
            &user_id.to_string(),
            CHANGE_KEY,
            Utc::now(),
        ) {
            RateLimitDecision::Allowed => Ok(()),
            RateLimitDecision::Limited {
                retry_after_seconds,
            } => Err(ServiceError::RateLimited {
                retry_after_seconds,
            }),
        }
    }

    /// `PermissionDenied` unless the user is a member of the workspace, of any role.
    async fn require_member(&self, server_id: ServerId, user_id: UserId) -> ServiceResult<()> {
        self.repos
            .server_member_repo
            .get_member(server_id, user_id)
            .await?
            .map(|_| ())
            .ok_or(ServiceError::PermissionDenied)
    }

    /// `PermissionDenied` unless the user is an Owner or Moderator of the workspace.
    async fn require_manager(&self, server_id: ServerId, user_id: UserId) -> ServiceResult<()> {
        let member = self
            .repos
            .server_member_repo
            .get_member(server_id, user_id)
            .await?
            .ok_or(ServiceError::PermissionDenied)?;
        if !member.member_role.is_moderator() {
            return Err(ServiceError::PermissionDenied);
        }
        Ok(())
    }

    /// This server's key log of `principal_id`, or `None` when no account here enrolled it.
    async fn local_key_log(
        repos: &WorkspacePluginRepos,
        principal_id: &str,
    ) -> ServiceResult<Option<Vec<StatementText>>> {
        let Some(user_id) = repos
            .key_identity_repo
            .find_user_by_principal_id(principal_id)
            .await?
        else {
            return Ok(None);
        };
        let entries = repos.key_identity_repo.list_key_log(user_id).await?;
        Ok(Some(
            entries
                .into_iter()
                .map(|e| StatementText {
                    payload: e.payload,
                    signature: e.signature,
                })
                .collect(),
        ))
    }

    /// Verify a statement against its publisher's key log and the registry's own rules.
    ///
    /// The local log wins over a supplied one. Without one, the supplied log is used, or the
    /// log `existing` keeps for the same publisher; a supplied log must extend that one.
    async fn verify(
        repos: &WorkspacePluginRepos,
        statement: &SignedStatement,
        supplied_log: Option<&[SignedPayload]>,
        existing: Option<&WorkspacePlugin>,
    ) -> ServiceResult<Verified> {
        let read = read_plugin_manifest(statement).map_err(invalid)?;
        check_manifest(&read)?;

        let (log, key_log_snapshot) = match Self::local_key_log(repos, &read.principal_id).await? {
            Some(local) => (local, None),
            None => {
                let stored = existing
                    .filter(|e| e.publisher_principal_id == read.principal_id)
                    .and_then(|e| e.publisher_key_log.clone());
                let log = match (supplied_log, stored) {
                    (Some(supplied), stored) => {
                        if supplied.len() > MAX_PUBLISHER_KEY_LOG_LEN {
                            return Err(validation(format!(
                                "publisherKeyLog may have at most \
                                     {MAX_PUBLISHER_KEY_LOG_LEN} entries"
                            )));
                        }
                        let supplied: Vec<StatementText> = supplied.iter().map(to_text).collect();
                        if let Some(stored) = stored
                            && !extends(&supplied, &stored)
                        {
                            return Err(validation(
                                "publisherKeyLog must extend the log this entry keeps",
                            ));
                        }
                        supplied
                    }
                    (None, Some(stored)) => stored,
                    (None, None) => {
                        return Err(validation(
                            "the publisher is not enrolled on this server: \
                                 publisherKeyLog is required",
                        ));
                    }
                };
                (log.clone(), Some(log))
            }
        };

        let statements: Vec<SignedStatement> = log.iter().map(text_statement).collect();
        let history = KeyHistory::replay(&statements).map_err(|e| {
            if key_log_snapshot.is_some() {
                validation(format!("publisherKeyLog does not replay: {e}"))
            } else {
                ServiceError::internal(anyhow::anyhow!("stored key log does not verify: {e}"))
            }
        })?;
        let (verified, validity) = verify_plugin_manifest(&history, statement).map_err(invalid)?;
        if let Validity::PublisherRecovered { .. } = validity {
            return Err(validation(format!(
                "the publisher recovered their key after keySeq {}",
                verified.key_seq
            )));
        }
        Ok(Verified {
            statement: verified,
            key_log_snapshot,
        })
    }

    /// The entry's source: a given `bundleUrl`, or this server's copy of an uploaded bundle.
    async fn source(
        repos: &WorkspacePluginRepos,
        server_id: ServerId,
        statement: &PluginManifestStatement,
        bundle_url: Option<String>,
    ) -> ServiceResult<PluginSource> {
        match (&statement.source, bundle_url) {
            (rim_identity::PluginSource::Git { .. }, Some(_)) => {
                Err(validation("bundleUrl is only for a bundle source"))
            }
            (
                rim_identity::PluginSource::Git {
                    url,
                    commit,
                    tree_sha256,
                },
                None,
            ) => Ok(PluginSource::Git {
                url: url.clone(),
                commit: commit.clone(),
                tree_sha256: tree_sha256.clone(),
            }),
            (rim_identity::PluginSource::Bundle { sha256, size }, Some(url)) => {
                check_bundle_url(&url)?;
                Ok(PluginSource::Bundle {
                    url,
                    sha256: sha256.clone(),
                    size: *size,
                })
            }
            (rim_identity::PluginSource::Bundle { sha256, size }, None) => {
                let short = &sha256[..8];
                let bundle = repos
                    .workspace_plugin_repo
                    .get_bundle(server_id, sha256)
                    .await?
                    .ok_or_else(|| {
                        validation(format!("bundle {short}… is not uploaded to this workspace"))
                    })?;
                if bundle.size_bytes != *size {
                    return Err(validation(format!(
                        "bundle {short}… is {} bytes, the statement says {size}",
                        bundle.size_bytes
                    )));
                }
                Ok(PluginSource::Bundle {
                    url: plugin_bundle_path(server_id, sha256),
                    sha256: sha256.clone(),
                    size: *size,
                })
            }
        }
    }

    fn write(
        server_id: ServerId,
        actor_id: UserId,
        verified: Verified,
        source: PluginSource,
        status: rim_core::workspace_plugin::model::WorkspacePluginStatus,
        statement: StatementText,
    ) -> WorkspacePluginWrite {
        let s = verified.statement;
        WorkspacePluginWrite {
            server_id,
            plugin_id: s.plugin_id,
            version: s.version,
            display_name: s.display_name,
            runtime_kind: runtime_kind(s.runtime_kind),
            status,
            source,
            statement,
            publisher_principal_id: s.principal_id,
            publisher_key_seq: s.key_seq,
            publisher_key_log: verified.key_log_snapshot,
            approved_by: actor_id,
        }
    }

    /// An entry as clients see it. `local_logs` caches this server's key logs by principal id
    /// for the length of one call.
    async fn dto(
        &self,
        entry: WorkspacePlugin,
        local_logs: &mut HashMap<String, Option<Vec<StatementText>>>,
    ) -> ServiceResult<WorkspacePluginDto> {
        let statement = read_plugin_manifest(&text_statement(&entry.statement)).map_err(|e| {
            ServiceError::internal(anyhow::anyhow!(
                "stored plugin statement is unreadable: {e}"
            ))
        })?;
        let principal = &entry.publisher_principal_id;
        let local = match local_logs.get(principal) {
            Some(log) => log.clone(),
            None => {
                let log = Self::local_key_log(&self.repos, principal).await?;
                local_logs.insert(principal.clone(), log.clone());
                log
            }
        };
        let log = local
            .or_else(|| entry.publisher_key_log.clone())
            .unwrap_or_default();
        let signature_status = signature_status(&log, entry.publisher_key_seq)?;
        Ok(WorkspacePluginDto {
            id: entry.id,
            server_id: entry.server_id,
            plugin_id: entry.plugin_id,
            version: entry.version,
            display_name: entry.display_name,
            runtime_kind: entry.runtime_kind,
            status: entry.status,
            source: entry.source,
            manifest: statement.manifest,
            statement: to_payload(entry.statement),
            publisher_principal_id: entry.publisher_principal_id,
            publisher_name: statement.publisher_name,
            publisher_key_seq: entry.publisher_key_seq,
            publisher_key_log: log.into_iter().map(to_payload).collect(),
            signature_status,
            approved_by: entry.approved_by,
            approved_at: entry.approved_at,
            updated_at: entry.updated_at,
        })
    }

    fn emit(&self, server_id: ServerId, plugin_id: &str, change: WorkspacePluginChange) {
        self.observers.emit(WorkspaceEvent::WorkspacePluginChanged {
            workspace_id: server_id,
            plugin_id: plugin_id.to_string(),
            change,
        });
    }
}

#[async_trait]
impl WorkspacePluginService for DefaultWorkspacePluginService {
    #[instrument(name = "workspace_plugin.list", skip_all, fields(actor_id = %query.actor.user_id))]
    async fn list(
        &self,
        query: ListWorkspacePluginsQuery,
    ) -> ServiceResult<Vec<WorkspacePluginDto>> {
        self.require_member(query.server_id, query.actor.user_id)
            .await?;
        let entries = self
            .repos
            .workspace_plugin_repo
            .list(query.server_id)
            .await?;
        let mut local_logs = HashMap::new();
        let mut dtos = Vec::with_capacity(entries.len());
        for entry in entries {
            dtos.push(self.dto(entry, &mut local_logs).await?);
        }
        Ok(dtos)
    }

    #[instrument(name = "workspace_plugin.add", skip_all, fields(actor_id = %cmd.actor.user_id))]
    async fn add(&self, cmd: AddWorkspacePluginCmd) -> ServiceResult<WorkspacePluginDto> {
        let actor_id = cmd.actor.user_id;
        self.limit(actor_id)?;
        self.require_manager(cmd.server_id, actor_id).await?;

        let statement = to_statement(&cmd.statement);
        let (scope, repos) = self.begin().await?;
        // The lock serializes adds to one registry, so the cap and the duplicate check hold.
        if !repos
            .workspace_plugin_repo
            .lock_server(cmd.server_id)
            .await?
        {
            return Err(ServiceError::NotFound);
        }
        let verified =
            Self::verify(&repos, &statement, cmd.publisher_key_log.as_deref(), None).await?;
        let plugin_id = verified.statement.plugin_id.clone();
        if repos
            .workspace_plugin_repo
            .get(cmd.server_id, &plugin_id)
            .await?
            .is_some()
        {
            return Err(ServiceError::Conflict(format!(
                "{plugin_id} is already in this workspace's plugins"
            )));
        }
        let source =
            Self::source(&repos, cmd.server_id, &verified.statement, cmd.bundle_url).await?;
        let write = Self::write(
            cmd.server_id,
            actor_id,
            verified,
            source,
            cmd.status,
            to_text(&cmd.statement),
        );
        let entry = repos
            .workspace_plugin_repo
            .insert(write, MAX_WORKSPACE_PLUGINS)
            .await?
            .ok_or_else(|| {
                ServiceError::Conflict(format!(
                    "this workspace already has {MAX_WORKSPACE_PLUGINS} plugins"
                ))
            })?;
        Self::commit(scope).await?;

        audit(
            "workspace_plugin_added",
            actor_id,
            &entry,
            "workspace plugin added",
        );
        self.emit(
            entry.server_id,
            &entry.plugin_id,
            WorkspacePluginChange::Added,
        );
        self.dto(entry, &mut HashMap::new()).await
    }

    #[instrument(name = "workspace_plugin.update", skip_all, fields(actor_id = %cmd.actor.user_id))]
    async fn update(&self, cmd: UpdateWorkspacePluginCmd) -> ServiceResult<WorkspacePluginDto> {
        let actor_id = cmd.actor.user_id;
        self.limit(actor_id)?;
        self.require_manager(cmd.server_id, actor_id).await?;
        if cmd.statement.is_none() && cmd.status.is_none() && cmd.bundle_url.is_none() {
            return Err(validation(
                "nothing to update: give a statement, a status or a bundleUrl",
            ));
        }

        let (scope, repos) = self.begin().await?;
        if !repos
            .workspace_plugin_repo
            .lock_server(cmd.server_id)
            .await?
        {
            return Err(ServiceError::NotFound);
        }
        let existing = repos
            .workspace_plugin_repo
            .get(cmd.server_id, &cmd.plugin_id)
            .await?
            .ok_or(ServiceError::NotFound)?;
        let status = cmd.status.unwrap_or(existing.status);

        let write = match (&cmd.statement, &cmd.publisher_key_log) {
            // A new statement, or the stored one again with a new publisher key log: verify it
            // and take every column from it.
            (Some(_), _) | (None, Some(_)) => {
                let text = cmd
                    .statement
                    .as_ref()
                    .map(to_text)
                    .unwrap_or_else(|| existing.statement.clone());
                let statement = text_statement(&text);
                let verified = Self::verify(
                    &repos,
                    &statement,
                    cmd.publisher_key_log.as_deref(),
                    Some(&existing),
                )
                .await?;
                if verified.statement.plugin_id != existing.plugin_id {
                    return Err(validation(format!(
                        "the statement is for {}, not {}",
                        verified.statement.plugin_id, existing.plugin_id
                    )));
                }
                let source = match (&cmd.bundle_url, cmd.statement.is_some()) {
                    // The same statement keeps its source, and with it where its bundle is.
                    (None, false) => existing.source.clone(),
                    (url, _) => {
                        Self::source(&repos, cmd.server_id, &verified.statement, url.clone())
                            .await?
                    }
                };
                Self::write(cmd.server_id, actor_id, verified, source, status, text)
            }
            (None, None) => {
                let source = match (existing.source.clone(), cmd.bundle_url) {
                    (source, None) => source,
                    (PluginSource::Bundle { sha256, size, .. }, Some(url)) => {
                        check_bundle_url(&url)?;
                        PluginSource::Bundle { url, sha256, size }
                    }
                    (PluginSource::Git { .. }, Some(_)) => {
                        return Err(validation("bundleUrl is only for a bundle source"));
                    }
                };
                WorkspacePluginWrite {
                    server_id: existing.server_id,
                    plugin_id: existing.plugin_id.clone(),
                    version: existing.version.clone(),
                    display_name: existing.display_name.clone(),
                    runtime_kind: existing.runtime_kind,
                    status,
                    source,
                    statement: existing.statement.clone(),
                    publisher_principal_id: existing.publisher_principal_id.clone(),
                    publisher_key_seq: existing.publisher_key_seq,
                    publisher_key_log: existing.publisher_key_log.clone(),
                    approved_by: actor_id,
                }
            }
        };
        let entry = repos
            .workspace_plugin_repo
            .update(existing.id, write)
            .await?
            .ok_or(ServiceError::NotFound)?;
        Self::commit(scope).await?;

        audit(
            "workspace_plugin_updated",
            actor_id,
            &entry,
            "workspace plugin updated",
        );
        self.emit(
            entry.server_id,
            &entry.plugin_id,
            WorkspacePluginChange::Updated,
        );
        self.dto(entry, &mut HashMap::new()).await
    }

    #[instrument(name = "workspace_plugin.remove", skip_all, fields(actor_id = %cmd.actor.user_id))]
    async fn remove(&self, cmd: RemoveWorkspacePluginCmd) -> ServiceResult<()> {
        let actor_id = cmd.actor.user_id;
        self.limit(actor_id)?;
        self.require_manager(cmd.server_id, actor_id).await?;
        let existing = self
            .repos
            .workspace_plugin_repo
            .get(cmd.server_id, &cmd.plugin_id)
            .await?
            .ok_or(ServiceError::NotFound)?;
        if !self
            .repos
            .workspace_plugin_repo
            .delete(cmd.server_id, &cmd.plugin_id)
            .await?
        {
            return Err(ServiceError::NotFound);
        }
        audit(
            "workspace_plugin_removed",
            actor_id,
            &existing,
            "workspace plugin removed",
        );
        self.emit(
            existing.server_id,
            &existing.plugin_id,
            WorkspacePluginChange::Removed,
        );
        Ok(())
    }
}
