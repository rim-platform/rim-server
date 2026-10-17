use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::command::verb::TypedVerb;

/// Typed enumeration of all audit-log outcome values.
///
/// **Canonical string representation** is provided by [`AuditOutcome::as_str`].
/// All writers (command.rs, macro_def.rs) MUST use the typed variant — never
/// string literals — so the single definition here is the source of truth.
///
/// The DB column is `text` with a CHECK constraint (migration
/// `m20260427_…_audit_outcome_widen_check`). The `AuditLogEntryDto.outcome`
/// field is this typed enum. Conversion to/from `String` happens only
/// at the repo boundary via `as_str()` / `TryFrom<&str>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AuditOutcome {
    Success,
    Error,
    ConfirmationRequired,
    PermissionDenied,
    RateLimited,
    /// A single step inside a macro returned an error.
    StepError,
    /// A macro completed partial steps before one failed.
    MacroPartialFailure,
}

impl AuditOutcome {
    /// Lowercase snake-case string stored in the DB `outcome` column.
    /// Matches the CHECK constraint in `m20260427_…_audit_outcome_widen_check`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Error => "error",
            Self::ConfirmationRequired => "confirmation_required",
            Self::PermissionDenied => "permission_denied",
            Self::RateLimited => "rate_limited",
            Self::StepError => "step_error",
            Self::MacroPartialFailure => "macro_partial_failure",
        }
    }

    /// All variants in definition order. Used by the integration test that
    /// writes one row per variant and asserts all 7 succeed.
    pub fn all() -> &'static [Self] {
        &[
            Self::Success,
            Self::Error,
            Self::ConfirmationRequired,
            Self::PermissionDenied,
            Self::RateLimited,
            Self::StepError,
            Self::MacroPartialFailure,
        ]
    }
}

impl TryFrom<&str> for AuditOutcome {
    type Error = String;

    fn try_from(s: &str) -> Result<Self, <AuditOutcome as TryFrom<&str>>::Error> {
        match s {
            "success" => Ok(Self::Success),
            "error" => Ok(Self::Error),
            "confirmation_required" => Ok(Self::ConfirmationRequired),
            "permission_denied" => Ok(Self::PermissionDenied),
            "rate_limited" => Ok(Self::RateLimited),
            "step_error" => Ok(Self::StepError),
            "macro_partial_failure" => Ok(Self::MacroPartialFailure),
            other => Err(format!("unknown audit outcome: {other}")),
        }
    }
}

#[derive(Debug, Clone)]
pub struct AuditLogEntryDto {
    pub id: Uuid,
    pub actor_user_id: Uuid,
    pub server_id: Uuid,
    pub command_id: String,
    pub initiator: String,
    pub args_redacted: serde_json::Value,
    pub sensitivity: String,
    pub dangerous: bool,
    pub confirmation_kind: String,
    pub typed_verb: Option<TypedVerb>,
    pub outcome: AuditOutcome,
    pub error_kind: Option<String>,
    pub capability_check_outcome: Option<String>,
    pub confirmation_outcome: Option<String>,
    /// Set on child rows emitted for macro steps. `None` for top-level entries.
    ///
    /// By design, child rows are written **before** the parent row: macro
    /// step audit rows are inserted inside the step loop, and the parent
    /// macro audit row is written only after the loop completes (see
    /// `macro_def.rs`). These are separate inserts, not one transaction, and
    /// `parent_audit_id` has no foreign key (see the migration that adds
    /// this column for the rationale). If a macro crashes or the process
    /// dies mid-execution, child rows can be left permanently referencing a
    /// `parent_audit_id` whose parent row was never written. Forensic
    /// tooling and any query that joins on `parent_audit_id` must tolerate
    /// dangling references — treat a missing parent as "macro execution did
    /// not complete," not as a data-integrity error.
    pub parent_audit_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default)]
pub struct ListAuditLogQuery {
    pub actor_user_id: Option<Uuid>,
    pub server_id: Option<Uuid>,
    pub command_id: Option<String>,
    pub typed_verb: Option<String>,
    pub since: Option<DateTime<Utc>>,
    pub until: Option<DateTime<Utc>>,
    /// Cursor: `(created_at, id)` — last item from previous page.
    pub cursor: Option<(DateTime<Utc>, Uuid)>,
    pub limit: u32,
}

#[derive(Debug, Clone)]
pub struct AuditLogPageInfo {
    pub has_next_page: bool,
    pub has_prev_page: bool,
}

#[derive(Debug, Clone)]
pub struct AuditLogPage {
    pub items: Vec<AuditLogEntryDto>,
    /// Cursor pointing after the last item in this page.
    pub next_cursor: Option<(DateTime<Utc>, Uuid)>,
    pub page_info: AuditLogPageInfo,
}
