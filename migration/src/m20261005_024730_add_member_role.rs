use sea_orm_migration::prelude::*;

/// A real `member` role and guest invites (Track 1.2, `docs/specs/track-1-2-roles-and-moderation.md`).
///
/// - `server_member_role` gains `member` (between `moderator` and `guest`), and
///   `server_member.member_role` now defaults to `member`.
/// - `invite.role` (`server_member_role`, default `member`): the role an invite grants. Only
///   `member` and `guest` are ever written (enforced in the service layer).
///
/// The enum is rebuilt rather than extended with `ALTER TYPE ... ADD VALUE`: on Postgres a value
/// added that way cannot be used in the same transaction, and SeaORM runs each Postgres migration
/// inside one. Rebuilding (create the new type, convert the column, drop the old type, rename)
/// keeps the whole migration atomic on every supported Postgres version.
///
/// **Data step:** every existing `guest` row becomes `member`. Until now every new member was
/// created as `guest`, so today's guests are ordinary members; `guest` now means the restricted
/// role (read and write only where explicitly added). See [`GUEST_TO_MEMBER_SQL`].
#[derive(DeriveMigrationName)]
pub struct Migration;

/// The data step: every `guest` becomes `member`. Public so the `rim_infra_db` db-tests can
/// reference it.
pub const GUEST_TO_MEMBER_SQL: &str =
    "UPDATE server_member SET member_role = 'member' WHERE member_role = 'guest'";

const UP_SQL: &[&str] = &[
    "CREATE TYPE server_member_role_v2 AS ENUM ('owner', 'moderator', 'member', 'guest')",
    "ALTER TABLE server_member ALTER COLUMN member_role DROP DEFAULT",
    "ALTER TABLE server_member ALTER COLUMN member_role TYPE server_member_role_v2 \
     USING member_role::text::server_member_role_v2",
    "DROP TYPE server_member_role",
    "ALTER TYPE server_member_role_v2 RENAME TO server_member_role",
    "ALTER TABLE server_member ALTER COLUMN member_role SET DEFAULT 'member'",
    GUEST_TO_MEMBER_SQL,
    "ALTER TABLE invite ADD COLUMN role server_member_role NOT NULL DEFAULT 'member'",
];

/// Reverts to the three-value enum. `member` and `guest` both become `guest` (the pre-Track-1.2
/// meaning of `guest`: an ordinary member), so the role distinction is lost on the way down.
const DOWN_SQL: &[&str] = &[
    "ALTER TABLE invite DROP COLUMN role",
    "CREATE TYPE server_member_role_v1 AS ENUM ('owner', 'moderator', 'guest')",
    "ALTER TABLE server_member ALTER COLUMN member_role DROP DEFAULT",
    "ALTER TABLE server_member ALTER COLUMN member_role TYPE server_member_role_v1 \
     USING (CASE WHEN member_role::text = 'member' THEN 'guest' ELSE member_role::text END)\
     ::server_member_role_v1",
    "DROP TYPE server_member_role",
    "ALTER TYPE server_member_role_v1 RENAME TO server_member_role",
    "ALTER TABLE server_member ALTER COLUMN member_role SET DEFAULT 'guest'",
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
