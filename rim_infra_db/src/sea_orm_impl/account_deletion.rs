use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rim_core::account_deletion::model::{
    AccountStanding, LeftConversation, OwnedSharedWorkspace, PurgeStep, TOMBSTONE_DISPLAY_NAME,
    avatar_storage_key, tombstone_username_candidates,
};
use rim_core::account_deletion::repository::AccountDeletionRepository;
use rim_core::repository::error::{RepoError, RepoErrorKind, RepoResult};
use rim_core::server::model::ServerId;
use rim_core::user::model::UserId;
use sea_orm::prelude::{DateTimeWithTimeZone, Uuid};
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement, Value};

use crate::DbHandle;
use crate::error::map_db_err;
use crate::sea_orm_impl::server::delete_workspaces;

/// Deletion requests and the account purge over raw SQL. Built on a transaction's handle, the
/// purge steps of one account commit or roll back together.
pub struct SeaOrmAccountDeletionRepository {
    db: DbHandle,
}

impl SeaOrmAccountDeletionRepository {
    pub fn new(db: DbHandle) -> Self {
        Self { db }
    }

    async fn execute(&self, sql: &str, values: Vec<Value>) -> RepoResult<u64> {
        Ok(self
            .db
            .execute_raw(statement(sql, values))
            .await
            .map_err(map_db_err)?
            .rows_affected())
    }

    async fn all<T: FromQueryResult>(&self, sql: &str, values: Vec<Value>) -> RepoResult<Vec<T>> {
        T::find_by_statement(statement(sql, values))
            .all(&self.db)
            .await
            .map_err(map_db_err)
    }

    async fn one<T: FromQueryResult>(
        &self,
        sql: &str,
        values: Vec<Value>,
    ) -> RepoResult<Option<T>> {
        T::find_by_statement(statement(sql, values))
            .one(&self.db)
            .await
            .map_err(map_db_err)
    }

    async fn ids(&self, sql: &str, values: Vec<Value>) -> RepoResult<Vec<Uuid>> {
        Ok(self
            .all::<IdRow>(sql, values)
            .await?
            .into_iter()
            .map(|r| r.id)
            .collect())
    }

    async fn texts(&self, sql: &str, values: Vec<Value>) -> RepoResult<Vec<String>> {
        Ok(self
            .all::<TextRow>(sql, values)
            .await?
            .into_iter()
            .map(|r| r.value)
            .collect())
    }

    async fn count(&self, sql: &str, values: Vec<Value>) -> RepoResult<u64> {
        let n = self.one::<CountRow>(sql, values).await?.map_or(0, |r| r.n);
        Ok(u64::try_from(n).unwrap_or(0))
    }

    /// Run each `(name, sql)` with `values` and count the rows each touched.
    async fn run_all(
        &self,
        statements: &[(&'static str, &str)],
        values: &[Value],
    ) -> RepoResult<Vec<(&'static str, u64)>> {
        let mut counts = Vec::with_capacity(statements.len());
        for (name, sql) in statements {
            counts.push((*name, self.execute(sql, values.to_vec()).await?));
        }
        Ok(counts)
    }
}

fn statement(sql: &str, values: Vec<Value>) -> Statement {
    Statement::from_sql_and_values(DbBackend::Postgres, sql, values)
}

fn uuid(id: UserId) -> Value {
    Uuid::from(id).into()
}

fn utc(at: DateTime<Utc>) -> Value {
    DateTimeWithTimeZone::from(at).into()
}

#[derive(FromQueryResult)]
struct IdRow {
    id: Uuid,
}

#[derive(FromQueryResult)]
struct TextRow {
    value: String,
}

#[derive(FromQueryResult)]
struct CountRow {
    n: i64,
}

#[derive(FromQueryResult)]
struct StandingRow {
    id: Uuid,
    username: String,
    kind: String,
    deletion_scheduled_for: Option<DateTimeWithTimeZone>,
    deleted_at: Option<DateTimeWithTimeZone>,
}

#[derive(FromQueryResult)]
struct WorkspaceRow {
    id: Uuid,
    name: String,
}

#[derive(FromQueryResult)]
struct LeftConversationRow {
    server_id: Uuid,
    conversation_id: Uuid,
    members: Vec<Uuid>,
}

#[derive(FromQueryResult)]
struct AvatarRow {
    avatar_url: Option<String>,
}

/// The workspaces (not the direct space) where the account is the only member.
const SOLE_MEMBER_WORKSPACES: &str = "SELECT s.id FROM server s \
     JOIN server_member m ON m.server_id = s.id AND m.user_id = $1 \
     WHERE s.kind = 'workspace' \
       AND NOT EXISTS (SELECT 1 FROM server_member o \
                       WHERE o.server_id = s.id AND o.user_id <> $1)";

/// The encrypted conversations the account is a member of, with their other active members.
const LEFT_ENCRYPTED_CONVERSATIONS: &str = "SELECT c.server_id, c.id AS conversation_id, \
         COALESCE(array_agg(o.user_id) FILTER (WHERE o.user_id IS NOT NULL), '{}') AS members \
     FROM conversation_member cm \
     JOIN conversation c ON c.id = cm.conversation_id \
     LEFT JOIN conversation_member o ON o.conversation_id = c.id AND o.user_id <> $1 \
         AND o.pending_since IS NULL \
     WHERE cm.user_id = $1 AND c.encrypted_at IS NOT NULL \
     GROUP BY c.server_id, c.id \
     ORDER BY c.id";

/// The account's plaintext messages that stay, as deleted, because a message that is kept
/// replies to them or sits in the thread they started; and, in turn, what those reply to or
/// start. Everything else the account sent in plaintext is hard-deleted.
const KEPT_PARENTS: &str = "WITH RECURSIVE doomed AS MATERIALIZED ( \
         SELECT id FROM message WHERE sent_by = $1 AND NOT encrypted \
     ), kept(id) AS ( \
         SELECT d.id FROM doomed d \
         WHERE EXISTS (SELECT 1 FROM message x \
                       WHERE x.parent_message_id = d.id \
                         AND (x.sent_by <> $1 OR x.encrypted)) \
            OR EXISTS (SELECT 1 FROM message_thread t JOIN message x ON x.thread_id = t.id \
                       WHERE t.first_message_id = d.id \
                         AND (x.sent_by <> $1 OR x.encrypted)) \
         UNION \
         SELECT d.id FROM kept k \
         JOIN message m ON m.id = k.id \
         JOIN doomed d ON d.id = m.parent_message_id \
             OR d.id = (SELECT t.first_message_id FROM message_thread t WHERE t.id = m.thread_id) \
     ) SELECT id FROM kept";

#[async_trait]
impl AccountDeletionRepository for SeaOrmAccountDeletionRepository {
    async fn standing(&self, user_id: UserId) -> RepoResult<Option<AccountStanding>> {
        Ok(self
            .one::<StandingRow>(
                r#"SELECT id, username, kind, deletion_scheduled_for, deleted_at
                   FROM "user" WHERE id = $1"#,
                vec![uuid(user_id)],
            )
            .await?
            .map(|row| AccountStanding {
                user_id: row.id.into(),
                username: row.username,
                is_remote: row.kind == "remote",
                deletion_scheduled_for: row.deletion_scheduled_for.map(Into::into),
                deleted_at: row.deleted_at.map(Into::into),
            }))
    }

    async fn owned_shared_workspaces(
        &self,
        user_id: UserId,
    ) -> RepoResult<Vec<OwnedSharedWorkspace>> {
        Ok(self
            .all::<WorkspaceRow>(
                "SELECT s.id, s.name FROM server s \
                 JOIN server_member me ON me.server_id = s.id AND me.user_id = $1 \
                      AND me.member_role = 'owner' \
                 WHERE s.kind = 'workspace' \
                   AND EXISTS (SELECT 1 FROM server_member o \
                               WHERE o.server_id = s.id AND o.user_id <> $1) \
                   AND NOT EXISTS (SELECT 1 FROM server_member o \
                                   WHERE o.server_id = s.id AND o.user_id <> $1 \
                                     AND o.member_role = 'owner') \
                 ORDER BY s.name, s.id",
                vec![uuid(user_id)],
            )
            .await?
            .into_iter()
            .map(|row| OwnedSharedWorkspace {
                id: row.id.into(),
                name: row.name,
            })
            .collect())
    }

    async fn record_request(
        &self,
        user_id: UserId,
        requested_at: DateTime<Utc>,
        scheduled_for: DateTime<Utc>,
    ) -> RepoResult<bool> {
        Ok(self
            .execute(
                r#"UPDATE "user" SET deletion_requested_at = $2, deletion_scheduled_for = $3,
                       updated_at = $2
                   WHERE id = $1 AND deleted_at IS NULL AND deletion_scheduled_for IS NULL"#,
                vec![uuid(user_id), utc(requested_at), utc(scheduled_for)],
            )
            .await?
            == 1)
    }

    async fn clear_request(&self, user_id: UserId) -> RepoResult<bool> {
        Ok(self
            .execute(
                r#"UPDATE "user" SET deletion_requested_at = NULL, deletion_scheduled_for = NULL,
                       updated_at = now()
                   WHERE id = $1 AND deleted_at IS NULL AND deletion_scheduled_for IS NOT NULL"#,
                vec![uuid(user_id)],
            )
            .await?
            == 1)
    }

    async fn due(&self, now: DateTime<Utc>, limit: u64) -> RepoResult<Vec<UserId>> {
        Ok(self
            .ids(
                r#"SELECT id FROM "user"
                   WHERE deletion_scheduled_for <= $1 AND deleted_at IS NULL
                   ORDER BY deletion_scheduled_for, id LIMIT $2"#,
                vec![utc(now), i64::try_from(limit).unwrap_or(i64::MAX).into()],
            )
            .await?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    async fn claim_due(&self, user_id: UserId, now: DateTime<Utc>) -> RepoResult<bool> {
        Ok(!self
            .ids(
                r#"SELECT id FROM "user"
                   WHERE id = $1 AND deletion_scheduled_for <= $2 AND deleted_at IS NULL
                   FOR UPDATE"#,
                vec![uuid(user_id), utc(now)],
            )
            .await?
            .is_empty())
    }

    async fn purge_workspaces(&self, user_id: UserId, now: DateTime<Utc>) -> RepoResult<PurgeStep> {
        let mut step = PurgeStep::default();

        let sole: Vec<ServerId> = self
            .ids(SOLE_MEMBER_WORKSPACES, vec![uuid(user_id)])
            .await?
            .into_iter()
            .map(Into::into)
            .collect();
        let deleted = delete_workspaces(&self.db, &sole).await?;
        step.storage_keys = deleted.storage_keys;
        step.counts.push(("workspaces_deleted", deleted.deleted));

        step.left_conversations = self
            .all::<LeftConversationRow>(LEFT_ENCRYPTED_CONVERSATIONS, vec![uuid(user_id)])
            .await?
            .into_iter()
            .map(|row| LeftConversation {
                workspace_id: row.server_id.into(),
                conversation_id: row.conversation_id.into(),
                member_user_ids: row.members.into_iter().map(Into::into).collect(),
            })
            .collect();
        step.left_workspaces = self
            .ids(
                "SELECT server_id AS id FROM server_member WHERE user_id = $1 ORDER BY server_id",
                vec![uuid(user_id)],
            )
            .await?
            .into_iter()
            .map(Into::into)
            .collect();

        let id = [uuid(user_id)];
        step.counts.extend(
            self.run_all(
                &[
                    (
                        "conversation_memberships",
                        "DELETE FROM conversation_member WHERE user_id = $1",
                    ),
                    (
                        "workspace_memberships",
                        "DELETE FROM server_member WHERE user_id = $1",
                    ),
                    ("bans", "DELETE FROM server_ban WHERE user_id = $1"),
                    (
                        "timeouts",
                        "DELETE FROM server_member_timeout WHERE user_id = $1",
                    ),
                    (
                        "delegations",
                        "DELETE FROM host_delegation WHERE user_id = $1",
                    ),
                ],
                &id,
            )
            .await?,
        );
        let id_and_now = [uuid(user_id), utc(now)];
        step.counts.extend(
            self.run_all(
                &[
                    (
                        "invites_revoked",
                        "UPDATE invite SET revoked_at = $2 \
                         WHERE created_by_user_id = $1 AND revoked_at IS NULL",
                    ),
                    (
                        "instance_invites_revoked",
                        "UPDATE instance_invite SET revoked_at = $2 \
                         WHERE created_by_user_id = $1 AND revoked_at IS NULL",
                    ),
                ],
                &id_and_now,
            )
            .await?,
        );
        Ok(step)
    }

    async fn purge_messages(&self, user_id: UserId) -> RepoResult<PurgeStep> {
        let mut step = PurgeStep::default();
        let kept = self.ids(KEPT_PARENTS, vec![uuid(user_id)]).await?;

        step.storage_keys = self
            .texts(
                "SELECT a.storage_url AS value FROM message_attachment a \
                 JOIN message m ON m.id = a.message_id \
                 WHERE m.sent_by = $1 AND NOT m.encrypted \
                   AND a.storage_url NOT LIKE 'pending://%'",
                vec![uuid(user_id)],
            )
            .await?;
        step.counts.push((
            "attachments",
            self.execute(
                "DELETE FROM message_attachment a USING message m \
                 WHERE m.id = a.message_id AND m.sent_by = $1 AND NOT m.encrypted",
                vec![uuid(user_id)],
            )
            .await?,
        ));

        // What hangs off a kept message goes; a deleted one takes it along by cascade.
        let kept_value: Value = kept.clone().into();
        for sql in [
            "DELETE FROM message_reaction WHERE message_id = ANY($1)",
            "DELETE FROM message_pin WHERE message_id = ANY($1)",
            "DELETE FROM message_mention WHERE message_id = ANY($1)",
            "DELETE FROM link_preview WHERE message_id = ANY($1)",
            "DELETE FROM user_notification WHERE message_id = ANY($1)",
        ] {
            self.execute(sql, vec![kept_value.clone()]).await?;
        }
        step.counts.push((
            "messages_kept_as_deleted",
            self.execute(
                "UPDATE message SET is_deleted = true, text = NULL, has_attachments = false \
                 WHERE id = ANY($1)",
                vec![kept_value.clone()],
            )
            .await?,
        ));
        step.counts.push((
            "threads",
            self.execute(
                "DELETE FROM message_thread t USING message m \
                 WHERE t.first_message_id = m.id AND m.sent_by = $1 AND NOT m.encrypted \
                   AND m.id <> ALL($2)",
                vec![uuid(user_id), kept_value.clone()],
            )
            .await?,
        ));
        step.counts.push((
            "messages_deleted",
            self.execute(
                "DELETE FROM message WHERE sent_by = $1 AND NOT encrypted AND id <> ALL($2)",
                vec![uuid(user_id), kept_value],
            )
            .await?,
        ));
        step.counts.push((
            "encrypted_messages_kept",
            self.count(
                "SELECT count(*) AS n FROM message WHERE sent_by = $1 AND encrypted",
                vec![uuid(user_id)],
            )
            .await?,
        ));
        step.counts.extend(
            self.run_all(
                &[
                    (
                        "reactions",
                        "DELETE FROM message_reaction WHERE user_id = $1",
                    ),
                    (
                        "mentions",
                        "DELETE FROM message_mention WHERE mentioned_user_id = $1",
                    ),
                    (
                        "read_states",
                        "DELETE FROM conversation_read_state WHERE user_id = $1",
                    ),
                ],
                &[uuid(user_id)],
            )
            .await?,
        );
        Ok(step)
    }

    async fn purge_credentials(
        &self,
        user_id: UserId,
        now: DateTime<Utc>,
    ) -> RepoResult<PurgeStep> {
        let id = [uuid(user_id)];
        let mut counts = self
            .run_all(
                &[
                    ("sessions", "DELETE FROM session WHERE user_id = $1"),
                    (
                        "password_reset_tokens",
                        "DELETE FROM password_reset_token WHERE user_id = $1",
                    ),
                    ("device_links", "DELETE FROM device_link WHERE user_id = $1"),
                    (
                        "key_packages",
                        "DELETE FROM mls_key_package WHERE user_id = $1",
                    ),
                    (
                        "welcomes",
                        "DELETE FROM mls_welcome_recipient WHERE user_id = $1",
                    ),
                    (
                        "readd_requests",
                        "DELETE FROM mls_readd_request WHERE device_key_id IN \
                         (SELECT id FROM device_key WHERE user_id = $1)",
                    ),
                    (
                        "readd_nonces",
                        "DELETE FROM mls_readd_nonce WHERE device_key_id IN \
                         (SELECT id FROM device_key WHERE user_id = $1)",
                    ),
                    (
                        "key_log_entries",
                        "DELETE FROM key_log_entry WHERE user_id = $1",
                    ),
                    (
                        "profile_cards",
                        "DELETE FROM profile_card WHERE user_id = $1",
                    ),
                ],
                &id,
            )
            .await?;
        counts.push((
            "devices_revoked",
            self.execute(
                "UPDATE device_key SET revoked_at = $2 WHERE user_id = $1 AND revoked_at IS NULL",
                vec![uuid(user_id), utc(now)],
            )
            .await?,
        ));
        Ok(PurgeStep::counted(counts))
    }

    async fn purge_personal_data(&self, user_id: UserId) -> RepoResult<PurgeStep> {
        let id = [uuid(user_id)];
        let mut step = PurgeStep::counted(
            self.run_all(
                &[
                    (
                        "notification_settings",
                        "DELETE FROM user_notification_settings WHERE user_id = $1",
                    ),
                    (
                        "notification_preferences",
                        "DELETE FROM notification_preference WHERE user_id = $1",
                    ),
                    (
                        "notifications",
                        "DELETE FROM user_notification WHERE user_id = $1",
                    ),
                    (
                        "keybinds",
                        "DELETE FROM keybind WHERE scope = 'user' AND subject_id = $1",
                    ),
                    (
                        "macros",
                        "DELETE FROM macro m WHERE m.user_id = $1 \
                         AND NOT EXISTS (SELECT 1 FROM pipeline p \
                                         WHERE p.server_id = m.server_id \
                                           AND strpos(p.actions_data::text, \
                                                      'macro.' || m.id::text) > 0)",
                    ),
                    (
                        "commands",
                        "DELETE FROM user_command WHERE owner_user_id = $1",
                    ),
                    (
                        "channel_lenses",
                        "DELETE FROM channel_lens WHERE created_by = $1",
                    ),
                    (
                        "workspace_state",
                        "DELETE FROM workspace_state WHERE user_id = $1 OR created_by = $1",
                    ),
                ],
                &id,
            )
            .await?,
        );
        step.counts.push((
            "macros_kept",
            self.count(
                "SELECT count(*) AS n FROM macro WHERE user_id = $1",
                id.to_vec(),
            )
            .await?,
        ));
        step.counts.push((
            "pipelines_kept",
            self.count(
                "SELECT count(*) AS n FROM pipeline WHERE created_by = $1",
                id.to_vec(),
            )
            .await?,
        ));
        step.storage_keys = self
            .texts(
                "DELETE FROM custom_emoji WHERE uploaded_by = $1 RETURNING image_url AS value",
                id.to_vec(),
            )
            .await?;
        step.counts.push((
            "custom_emoji",
            u64::try_from(step.storage_keys.len()).unwrap_or(u64::MAX),
        ));
        Ok(step)
    }

    async fn purge_relay_state(&self, user_id: UserId) -> RepoResult<PurgeStep> {
        Ok(PurgeStep::counted(
            self.run_all(
                &[
                    ("relay_items", "DELETE FROM relay_item WHERE user_id = $1"),
                    (
                        "dm_requests",
                        "DELETE FROM relay_dm_request WHERE user_id = $1",
                    ),
                    ("contacts", "DELETE FROM relay_contact WHERE user_id = $1"),
                    ("blocks", "DELETE FROM relay_block WHERE user_id = $1"),
                    (
                        "subscriptions",
                        "DELETE FROM relay_subscription WHERE user_id = $1",
                    ),
                ],
                &[uuid(user_id)],
            )
            .await?,
        ))
    }

    async fn tombstone(&self, user_id: UserId, now: DateTime<Utc>) -> RepoResult<PurgeStep> {
        let avatar = self
            .one::<AvatarRow>(
                r#"SELECT avatar_url FROM "user" WHERE id = $1"#,
                vec![uuid(user_id)],
            )
            .await?
            .ok_or_else(|| RepoError::new(RepoErrorKind::NotFound))?
            .avatar_url;
        let username = self
            .texts(
                r#"SELECT t.c AS value FROM unnest($2::text[]) WITH ORDINALITY AS t(c, n)
                   WHERE NOT EXISTS (SELECT 1 FROM "user" u WHERE lower(u.username) = t.c AND u.id <> $1)
                   ORDER BY t.n LIMIT 1"#,
                vec![uuid(user_id), tombstone_username_candidates(user_id).into()],
            )
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| {
                RepoError::with_message(
                    RepoErrorKind::Conflict,
                    "every tombstone username is taken",
                )
            })?;
        let updated = self
            .execute(
                r#"UPDATE "user" SET deleted_at = $2, updated_at = $2, username = $3,
                       display_name = $4, email = NULL, password_hash = NULL, avatar_url = NULL,
                       status_text = NULL, bio = NULL, timezone = NULL, last_login_at = NULL,
                       terms_accepted_at = NULL, terms_accepted_url = NULL,
                       identity_key = NULL, home_relay = NULL
                   WHERE id = $1"#,
                vec![
                    uuid(user_id),
                    utc(now),
                    username.into(),
                    TOMBSTONE_DISPLAY_NAME.into(),
                ],
            )
            .await?;
        if updated != 1 {
            return Err(RepoError::new(RepoErrorKind::NotFound));
        }
        Ok(PurgeStep {
            counts: vec![("tombstone", 1)],
            storage_keys: avatar
                .as_deref()
                .and_then(avatar_storage_key)
                .map(str::to_string)
                .into_iter()
                .collect(),
            ..PurgeStep::default()
        })
    }
}
