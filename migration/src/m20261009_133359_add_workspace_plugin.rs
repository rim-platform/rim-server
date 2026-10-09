use sea_orm_migration::prelude::*;

/// Workspace plugin registries.
///
/// - `plugin_bundle` lists the web plugin bundles uploaded to a workspace, keyed by their
///   SHA-256; the bytes live in file storage under `storage_key`.
/// - `workspace_plugin` holds at most one entry per plugin id and workspace: the publisher's
///   signed statement exactly as signed, the columns copied from it, and the workspace's status
///   and approval. `publisher_key_log` is the publisher's key log, kept only for a publisher not
///   enrolled on this server.
#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &[&str] = &[
    r#"CREATE TABLE plugin_bundle (
        server_id uuid NOT NULL,
        sha256 text NOT NULL,
        size_bytes bigint NOT NULL,
        storage_key text NOT NULL,
        uploaded_by uuid NULL,
        created_at timestamptz NOT NULL DEFAULT now(),
        CONSTRAINT pk_plugin_bundle PRIMARY KEY (server_id, sha256),
        CONSTRAINT chk_plugin_bundle_sha256 CHECK (sha256 ~ '^[0-9a-f]{64}$'),
        CONSTRAINT chk_plugin_bundle_size CHECK (size_bytes BETWEEN 1 AND 8388608),
        CONSTRAINT fk_plugin_bundle_server FOREIGN KEY (server_id)
            REFERENCES server (id) ON DELETE CASCADE ON UPDATE CASCADE,
        CONSTRAINT fk_plugin_bundle_uploaded_by FOREIGN KEY (uploaded_by)
            REFERENCES "user" (id) ON DELETE SET NULL ON UPDATE CASCADE
    )"#,
    "CREATE INDEX idx_plugin_bundle_created_at ON plugin_bundle (created_at)",
    r#"CREATE TABLE workspace_plugin (
        id uuid PRIMARY KEY,
        server_id uuid NOT NULL,
        plugin_id text NOT NULL,
        version text NOT NULL,
        display_name text NOT NULL,
        runtime_kind text NOT NULL,
        status text NOT NULL,
        source_kind text NOT NULL,
        git_url text NULL,
        git_commit text NULL,
        git_tree_sha256 text NULL,
        bundle_url text NULL,
        bundle_sha256 text NULL,
        bundle_size bigint NULL,
        statement_payload text NOT NULL,
        statement_signature text NOT NULL,
        publisher_principal_id text NOT NULL,
        publisher_key_seq integer NOT NULL,
        publisher_key_log jsonb NULL,
        approved_by uuid NULL,
        approved_at timestamptz NOT NULL,
        created_at timestamptz NOT NULL DEFAULT now(),
        updated_at timestamptz NOT NULL DEFAULT now(),
        CONSTRAINT uq_workspace_plugin_server_plugin UNIQUE (server_id, plugin_id),
        CONSTRAINT chk_workspace_plugin_runtime_kind
            CHECK (runtime_kind IN ('desktop_local_process', 'web_sandboxed')),
        CONSTRAINT chk_workspace_plugin_status CHECK (status IN ('recommended', 'required')),
        CONSTRAINT chk_workspace_plugin_source CHECK (
            (source_kind = 'git'
                AND git_url IS NOT NULL AND git_commit IS NOT NULL AND git_tree_sha256 IS NOT NULL
                AND bundle_url IS NULL AND bundle_sha256 IS NULL AND bundle_size IS NULL)
            OR (source_kind = 'bundle'
                AND bundle_url IS NOT NULL AND bundle_sha256 IS NOT NULL AND bundle_size IS NOT NULL
                AND git_url IS NULL AND git_commit IS NULL AND git_tree_sha256 IS NULL)),
        CONSTRAINT chk_workspace_plugin_web_bundle
            CHECK (runtime_kind <> 'web_sandboxed' OR source_kind = 'bundle'),
        CONSTRAINT chk_workspace_plugin_key_seq CHECK (publisher_key_seq >= 0),
        CONSTRAINT fk_workspace_plugin_server FOREIGN KEY (server_id)
            REFERENCES server (id) ON DELETE CASCADE ON UPDATE CASCADE,
        CONSTRAINT fk_workspace_plugin_approved_by FOREIGN KEY (approved_by)
            REFERENCES "user" (id) ON DELETE SET NULL ON UPDATE CASCADE
    )"#,
    "CREATE INDEX idx_workspace_plugin_bundle ON workspace_plugin (server_id, bundle_sha256) \
     WHERE bundle_sha256 IS NOT NULL",
];

/// Nothing references either table. Bundle files stay in storage under `plugin-bundles/`.
const DOWN_SQL: &[&str] = &["DROP TABLE workspace_plugin", "DROP TABLE plugin_bundle"];

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        for sql in UP_SQL {
            conn.execute_unprepared(sql).await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        for sql in DOWN_SQL {
            conn.execute_unprepared(sql).await?;
        }
        Ok(())
    }
}
