use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Utc};

use crate::conversation::model::ConversationId;
use crate::message::model::MessageId;
use crate::model_id;
use crate::server::model::ServerId;
use crate::user::model::UserId;

model_id!(IncidentId);
model_id!(IncidentPhaseId);

// ── IncidentStatus ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum IncidentStatus {
    Active,
    Mitigated,
    Resolved,
}

impl IncidentStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Mitigated => "mitigated",
            Self::Resolved => "resolved",
        }
    }
}

impl fmt::Display for IncidentStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseIncidentStatusError {
    input: String,
}

impl fmt::Display for ParseIncidentStatusError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown incident status: {}", self.input)
    }
}

impl std::error::Error for ParseIncidentStatusError {}

impl FromStr for IncidentStatus {
    type Err = ParseIncidentStatusError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "active" => Ok(Self::Active),
            "mitigated" => Ok(Self::Mitigated),
            "resolved" => Ok(Self::Resolved),
            other => Err(ParseIncidentStatusError {
                input: other.to_string(),
            }),
        }
    }
}

// ── IncidentSeverity ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum IncidentSeverity {
    Sev1,
    Sev2,
    Sev3,
    Sev4,
}

impl IncidentSeverity {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Sev1 => "sev1",
            Self::Sev2 => "sev2",
            Self::Sev3 => "sev3",
            Self::Sev4 => "sev4",
        }
    }
}

impl fmt::Display for IncidentSeverity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseIncidentSeverityError {
    input: String,
}

impl fmt::Display for ParseIncidentSeverityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown incident severity: {}", self.input)
    }
}

impl std::error::Error for ParseIncidentSeverityError {}

impl FromStr for IncidentSeverity {
    type Err = ParseIncidentSeverityError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "sev1" => Ok(Self::Sev1),
            "sev2" => Ok(Self::Sev2),
            "sev3" => Ok(Self::Sev3),
            "sev4" => Ok(Self::Sev4),
            other => Err(ParseIncidentSeverityError {
                input: other.to_string(),
            }),
        }
    }
}

// ── IncidentPhaseName ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum IncidentPhaseName {
    Detection,
    Triage,
    Mitigation,
    Resolution,
    Postmortem,
}

impl IncidentPhaseName {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Detection => "detection",
            Self::Triage => "triage",
            Self::Mitigation => "mitigation",
            Self::Resolution => "resolution",
            Self::Postmortem => "postmortem",
        }
    }
}

impl fmt::Display for IncidentPhaseName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseIncidentPhaseNameError {
    input: String,
}

impl fmt::Display for ParseIncidentPhaseNameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown incident phase: {}", self.input)
    }
}

impl std::error::Error for ParseIncidentPhaseNameError {}

impl FromStr for IncidentPhaseName {
    type Err = ParseIncidentPhaseNameError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "detection" => Ok(Self::Detection),
            "triage" => Ok(Self::Triage),
            "mitigation" => Ok(Self::Mitigation),
            "resolution" => Ok(Self::Resolution),
            "postmortem" => Ok(Self::Postmortem),
            other => Err(ParseIncidentPhaseNameError {
                input: other.to_string(),
            }),
        }
    }
}

// ── MessageTagType ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum MessageTagType {
    Decision,
    ActionItem,
    RootCause,
    TimelineEvent,
}

impl MessageTagType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Decision => "decision",
            Self::ActionItem => "action_item",
            Self::RootCause => "root_cause",
            Self::TimelineEvent => "timeline_event",
        }
    }
}

impl fmt::Display for MessageTagType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseMessageTagTypeError {
    input: String,
}

impl fmt::Display for ParseMessageTagTypeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown message tag type: {}", self.input)
    }
}

impl std::error::Error for ParseMessageTagTypeError {}

impl FromStr for MessageTagType {
    type Err = ParseMessageTagTypeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "decision" => Ok(Self::Decision),
            "action_item" => Ok(Self::ActionItem),
            "root_cause" => Ok(Self::RootCause),
            "timeline_event" => Ok(Self::TimelineEvent),
            other => Err(ParseMessageTagTypeError {
                input: other.to_string(),
            }),
        }
    }
}

// ── Domain models ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Incident {
    pub id: IncidentId,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,
    pub started_by: UserId,
    pub status: IncidentStatus,
    pub severity: Option<IncidentSeverity>,
    pub title: String,
    pub original_topic: Option<String>,
    pub started_at: DateTime<Utc>,
    pub mitigated_at: Option<DateTime<Utc>>,
    pub resolved_at: Option<DateTime<Utc>>,
    pub postmortem_draft: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Dedup key for alert-ingested incidents (Track B2). `None` for human-declared incidents.
    /// At most one non-resolved incident per `(server_id, alert_fingerprint)` — enforced by
    /// the partial unique index `idx_incident_active_fingerprint`.
    pub alert_fingerprint: Option<String>,
    /// `"manual" | "generic" | "alertmanager" | "grafana"`. Defaults to `"manual"` for
    /// human-declared incidents (see `m20260708_233516_add_incident_alert_fields`).
    pub source: String,
    /// The escalation policy armed for this incident's [`crate::escalation::model::EscalationRun`],
    /// if any. `None` when no policy applied (no channel default and no override at declare
    /// time), and unchanged thereafter — the *run itself* (not this field) tracks whether the
    /// chain is still active/cancelled/completed. See `m20260711_..._add_incident_escalation_and_ack_fields`.
    pub escalation_policy_id: Option<crate::escalation::model::EscalationPolicyId>,
    /// Set by `acknowledge_incident`. Orthogonal to `status` (§4 of the spec): acking means "a
    /// human has this, stop paging" and can coexist with `Active`. `None` until acked.
    pub acknowledged_at: Option<DateTime<Utc>>,
    pub acknowledged_by: Option<UserId>,
}

#[derive(Debug, Clone)]
pub struct NewIncident {
    pub server_id: ServerId,
    pub conversation_id: ConversationId,
    pub started_by: UserId,
    pub severity: Option<IncidentSeverity>,
    pub title: String,
    pub original_topic: Option<String>,
    /// `None` + `"manual"` for a human-declared incident (`start_incident`). Alert ingestion
    /// (`ingest_alert`) sets both.
    pub alert_fingerprint: Option<String>,
    pub source: String,
    /// The escalation policy to arm at declare time (an explicit `StartIncidentCmd` override, or
    /// the conversation's default), resolved by the service before insert. `None` if neither
    /// applies — the incident is declared without an escalation chain.
    pub escalation_policy_id: Option<crate::escalation::model::EscalationPolicyId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncidentPhaseRecord {
    pub id: IncidentPhaseId,
    pub incident_id: IncidentId,
    pub phase: IncidentPhaseName,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncidentMessageTag {
    pub message_id: MessageId,
    pub incident_id: IncidentId,
    pub phase: IncidentPhaseName,
    pub tag_type: Option<MessageTagType>,
}

#[derive(Debug, Clone)]
pub struct NewIncidentMessageTag {
    pub message_id: MessageId,
    pub incident_id: IncidentId,
    pub phase: IncidentPhaseName,
    pub tag_type: Option<MessageTagType>,
}
