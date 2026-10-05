use async_graphql::{Enum, InputObject, SimpleObject};

// ── Audit log types (Phase 5 Task 1) ────────────────────────────────────────

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum TypedVerbGql {
    Read,
    Write,
    Delete,
    Send,
    Open,
    Close,
    Toggle,
    Navigate,
    Execute,
    Configure,
    Other,
}

impl From<rim_services::command::verb::TypedVerb> for TypedVerbGql {
    fn from(v: rim_services::command::verb::TypedVerb) -> Self {
        use rim_services::command::verb::TypedVerb;
        match v {
            TypedVerb::Read => Self::Read,
            TypedVerb::Write => Self::Write,
            TypedVerb::Delete => Self::Delete,
            TypedVerb::Send => Self::Send,
            TypedVerb::Open => Self::Open,
            TypedVerb::Close => Self::Close,
            TypedVerb::Toggle => Self::Toggle,
            TypedVerb::Navigate => Self::Navigate,
            TypedVerb::Execute => Self::Execute,
            TypedVerb::Configure => Self::Configure,
            TypedVerb::Other => Self::Other,
        }
    }
}

impl From<TypedVerbGql> for rim_services::command::verb::TypedVerb {
    fn from(v: TypedVerbGql) -> Self {
        match v {
            TypedVerbGql::Read => Self::Read,
            TypedVerbGql::Write => Self::Write,
            TypedVerbGql::Delete => Self::Delete,
            TypedVerbGql::Send => Self::Send,
            TypedVerbGql::Open => Self::Open,
            TypedVerbGql::Close => Self::Close,
            TypedVerbGql::Toggle => Self::Toggle,
            TypedVerbGql::Navigate => Self::Navigate,
            TypedVerbGql::Execute => Self::Execute,
            TypedVerbGql::Configure => Self::Configure,
            TypedVerbGql::Other => Self::Other,
        }
    }
}

/// GraphQL enum for audit log outcome — mirrors [`rim_services::audit::dto::AuditOutcome`].
/// Rust variants → SCREAMING_SNAKE in GraphQL (e.g. `MacroPartialFailure` → `MACRO_PARTIAL_FAILURE`).
#[derive(async_graphql::Enum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuditOutcomeGql {
    Success,
    Error,
    ConfirmationRequired,
    PermissionDenied,
    RateLimited,
    StepError,
    MacroPartialFailure,
}

impl From<rim_services::audit::dto::AuditOutcome> for AuditOutcomeGql {
    fn from(o: rim_services::audit::dto::AuditOutcome) -> Self {
        use rim_services::audit::dto::AuditOutcome;
        match o {
            AuditOutcome::Success => Self::Success,
            AuditOutcome::Error => Self::Error,
            AuditOutcome::ConfirmationRequired => Self::ConfirmationRequired,
            AuditOutcome::PermissionDenied => Self::PermissionDenied,
            AuditOutcome::RateLimited => Self::RateLimited,
            AuditOutcome::StepError => Self::StepError,
            AuditOutcome::MacroPartialFailure => Self::MacroPartialFailure,
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct AuditLogEntry {
    pub id: async_graphql::ID,
    pub actor_user_id: async_graphql::ID,
    pub server_id: async_graphql::ID,
    pub command_id: String,
    pub initiator: String,
    pub args_redacted: async_graphql::Json<serde_json::Value>,
    pub sensitivity: String,
    pub dangerous: bool,
    pub confirmation_kind: String,
    pub typed_verb: Option<TypedVerbGql>,
    pub outcome: AuditOutcomeGql,
    pub error_kind: Option<String>,
    pub capability_check_outcome: Option<String>,
    pub confirmation_outcome: Option<String>,
    pub created_at: String,
}

impl From<rim_services::audit::dto::AuditLogEntryDto> for AuditLogEntry {
    fn from(dto: rim_services::audit::dto::AuditLogEntryDto) -> Self {
        Self {
            id: dto.id.to_string().into(),
            actor_user_id: dto.actor_user_id.to_string().into(),
            server_id: dto.server_id.to_string().into(),
            command_id: dto.command_id,
            initiator: dto.initiator,
            args_redacted: async_graphql::Json(dto.args_redacted),
            sensitivity: dto.sensitivity,
            dangerous: dto.dangerous,
            confirmation_kind: dto.confirmation_kind,
            typed_verb: dto.typed_verb.map(TypedVerbGql::from),
            outcome: AuditOutcomeGql::from(dto.outcome),
            error_kind: dto.error_kind,
            capability_check_outcome: dto.capability_check_outcome,
            confirmation_outcome: dto.confirmation_outcome,
            created_at: dto.created_at.to_rfc3339(),
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct AuditLogPageInfo {
    pub has_next_page: bool,
    pub has_prev_page: bool,
}

#[derive(SimpleObject, Clone, Debug)]
pub struct AuditLogPage {
    pub items: Vec<AuditLogEntry>,
    /// Opaque cursor pointing after the last item; pass to `after` on next query.
    pub next_cursor: Option<String>,
    pub page_info: AuditLogPageInfo,
}

#[derive(InputObject, Clone, Debug, Default)]
pub struct AuditLogFilterInput {
    pub actor_user_id: Option<async_graphql::ID>,
    pub server_id: Option<async_graphql::ID>,
    pub command_id: Option<String>,
    pub typed_verb: Option<TypedVerbGql>,
    pub since: Option<String>,
    pub until: Option<String>,
}

#[derive(InputObject, Clone, Debug, Default)]
pub struct AuditLogPaginationInput {
    /// Opaque cursor from the previous page's `nextCursor`.
    pub after: Option<String>,
    pub limit: Option<i32>,
}
