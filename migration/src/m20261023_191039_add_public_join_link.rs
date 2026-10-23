use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::ConnectionTrait;

/// Adds a workspace's public join link, the joins made through it, and join requests.
///
/// - `workspace_public_link`: one row per workspace that ever had a link. `token_hash` (hex
///   SHA-256 of the token) is what lookups use; `sealed_token` is the token sealed under the
///   server's link key, so the Owner can read the link again. `enabled`, `mode` (`direct` or
///   `requests`), `role` (`member` or `guest`) and `allow_remote` are the Owner's settings;
///   `enabled_at`, `issued_at` and `issued_by_user_id` say when it was turned on, when its token
///   was issued and by whom.
/// - `workspace_public_link_join`: one row per member who joined through the link, for the
///   link's join counts; rows older than seven days are deleted as new ones are written.
/// - `workspace_join_request`: a person asking to join through a link in `requests` mode.
///   `status` is `open` until it is accepted, declined, withdrawn, cancelled or expired; an open
///   request whose `expires_at` has passed counts as expired. At most one open request per
///   person and workspace (`uq_workspace_join_request_open`).
///
/// New tables: the previous release's server never reads them.
#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &[&str] = &[
    r#"CREATE TABLE workspace_public_link (
    server_id uuid PRIMARY KEY REFERENCES server (id) ON DELETE CASCADE,
    token_hash text NOT NULL UNIQUE,
    sealed_token bytea NOT NULL,
    enabled boolean NOT NULL DEFAULT false,
    mode text NOT NULL DEFAULT 'direct' CHECK (mode IN ('direct', 'requests')),
    role text NOT NULL DEFAULT 'member' CHECK (role IN ('member', 'guest')),
    allow_remote boolean NOT NULL DEFAULT true,
    enabled_at timestamptz NULL,
    issued_at timestamptz NOT NULL,
    issued_by_user_id uuid NULL REFERENCES "user" (id) ON DELETE SET NULL,
    updated_at timestamptz NOT NULL DEFAULT now()
)"#,
    r#"CREATE TABLE workspace_public_link_join (
    id bigserial PRIMARY KEY,
    server_id uuid NOT NULL REFERENCES server (id) ON DELETE CASCADE,
    user_id uuid NOT NULL REFERENCES "user" (id) ON DELETE CASCADE,
    joined_at timestamptz NOT NULL DEFAULT now()
)"#,
    "CREATE INDEX idx_workspace_public_link_join_server \
     ON workspace_public_link_join (server_id, joined_at)",
    r#"CREATE TABLE workspace_join_request (
    id uuid PRIMARY KEY,
    server_id uuid NOT NULL REFERENCES server (id) ON DELETE CASCADE,
    user_id uuid NOT NULL REFERENCES "user" (id) ON DELETE CASCADE,
    message text NULL CHECK (message IS NULL OR char_length(message) <= 500),
    status text NOT NULL DEFAULT 'open'
        CHECK (status IN ('open', 'accepted', 'declined', 'withdrawn', 'cancelled', 'expired')),
    created_at timestamptz NOT NULL DEFAULT now(),
    expires_at timestamptz NOT NULL,
    decided_at timestamptz NULL,
    decided_by_user_id uuid NULL REFERENCES "user" (id) ON DELETE SET NULL
)"#,
    "CREATE UNIQUE INDEX uq_workspace_join_request_open \
     ON workspace_join_request (server_id, user_id) WHERE status = 'open'",
    "CREATE INDEX idx_workspace_join_request_server_open \
     ON workspace_join_request (server_id, created_at, id) WHERE status = 'open'",
    "CREATE INDEX idx_workspace_join_request_user \
     ON workspace_join_request (user_id, status)",
];

const DOWN_SQL: &[&str] = &[
    "DROP TABLE workspace_join_request",
    "DROP TABLE workspace_public_link_join",
    "DROP TABLE workspace_public_link",
];

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
