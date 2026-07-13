use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Utc};

use crate::incident::model::IncidentId;
use crate::model_id;
use crate::oncall::model::OnCallScheduleId;
use crate::server::model::ServerId;
use crate::user::model::UserId;

model_id!(EscalationPolicyId);
model_id!(EscalationStepId);
model_id!(EscalationRunId);

// ── EscalationTarget ────────────────────────────────────────────────────────

/// Who an [`EscalationStep`] pages when it fires.
///
/// Stored across two nullable columns (`target_user_id`/`target_schedule_id`) with a DB
/// CHECK enforcing exactly one is set per `target_kind` (see
/// `migration::m20260710_..._create_escalation_policies`). A `Webhook` variant is reserved
/// for a future track — do not add handling for it without a schema change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum EscalationTarget {
    /// Page a specific user directly.
    User(UserId),
    /// Page whoever [`crate::oncall::resolve::resolve_on_call`] resolves for this schedule at
    /// fire time.
    OnCall(OnCallScheduleId),
}

impl EscalationTarget {
    /// The DB discriminator string stored in `escalation_step.target_kind`.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::User(_) => "user",
            Self::OnCall(_) => "on_call",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseEscalationTargetKindError {
    input: String,
}

impl fmt::Display for ParseEscalationTargetKindError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown escalation target kind: {}", self.input)
    }
}

impl std::error::Error for ParseEscalationTargetKindError {}

/// A bare discriminator, used when parsing the DB `target_kind` column before the caller has
/// decoded `target_user_id`/`target_schedule_id` — mirrors
/// `crate::oncall::model::RotationKindTag`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EscalationTargetKindTag {
    User,
    OnCall,
}

impl FromStr for EscalationTargetKindTag {
    type Err = ParseEscalationTargetKindError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "user" => Ok(Self::User),
            "on_call" => Ok(Self::OnCall),
            other => Err(ParseEscalationTargetKindError {
                input: other.to_string(),
            }),
        }
    }
}

// ── EscalationAction ────────────────────────────────────────────────────────

/// An action performed when an [`EscalationStep`] fires. Steps AND-combine every action in
/// their `actions` list (§3 of the spec) — at least one is required.
///
/// Deliberately an open set: `Push` is reserved for Track D4 and intentionally has no handling
/// in C1 — do not implement it prematurely, but keep the enum slot so a future action doesn't
/// need a migration (see `docs/oncall.md`).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum EscalationAction {
    MentionInChannel,
    Email,
}

impl EscalationAction {
    /// The DB discriminator string stored (as one comma-separated token) in
    /// `escalation_step.actions` — see [`encode_actions`]/[`decode_actions`].
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::MentionInChannel => "mention_in_channel",
            Self::Email => "email",
        }
    }
}

impl fmt::Display for EscalationAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseEscalationActionError {
    input: String,
}

impl fmt::Display for ParseEscalationActionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown escalation action: {}", self.input)
    }
}

impl std::error::Error for ParseEscalationActionError {}

impl FromStr for EscalationAction {
    type Err = ParseEscalationActionError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "mention_in_channel" => Ok(Self::MentionInChannel),
            "email" => Ok(Self::Email),
            other => Err(ParseEscalationActionError {
                input: other.to_string(),
            }),
        }
    }
}

/// Encodes a step's action set as the sorted, deduplicated, comma-separated string stored in
/// `escalation_step.actions` (§8: "a sorted CSV/bitmask of action kinds"). Sorting makes the
/// stored value stable regardless of input order, so two calls with the same logical set
/// produce byte-identical rows.
///
/// # Examples
/// ```
/// use rim_core::escalation::model::{EscalationAction, encode_actions};
/// assert_eq!(
///     encode_actions(&[EscalationAction::Email, EscalationAction::MentionInChannel]),
///     "mention_in_channel,email"
/// );
/// ```
pub fn encode_actions(actions: &[EscalationAction]) -> String {
    let mut sorted: Vec<EscalationAction> = actions.to_vec();
    sorted.sort();
    sorted.dedup();
    sorted
        .iter()
        .map(EscalationAction::as_str)
        .collect::<Vec<_>>()
        .join(",")
}

/// Inverse of [`encode_actions`]. Returns an error naming the first unrecognized token.
///
/// # Examples
/// ```
/// use rim_core::escalation::model::{EscalationAction, decode_actions};
/// let actions = decode_actions("mention_in_channel,email").unwrap();
/// assert_eq!(actions, vec![EscalationAction::MentionInChannel, EscalationAction::Email]);
/// ```
pub fn decode_actions(raw: &str) -> Result<Vec<EscalationAction>, ParseEscalationActionError> {
    raw.split(',')
        .filter(|s| !s.is_empty())
        .map(EscalationAction::from_str)
        .collect()
}

// ── EscalationRunStatus ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum EscalationRunStatus {
    Active,
    Cancelled,
    Completed,
}

impl EscalationRunStatus {
    /// The DB discriminator string stored in `escalation_run.status`.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Cancelled => "cancelled",
            Self::Completed => "completed",
        }
    }
}

impl fmt::Display for EscalationRunStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseEscalationRunStatusError {
    input: String,
}

impl fmt::Display for ParseEscalationRunStatusError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown escalation run status: {}", self.input)
    }
}

impl std::error::Error for ParseEscalationRunStatusError {}

impl FromStr for EscalationRunStatus {
    type Err = ParseEscalationRunStatusError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "active" => Ok(Self::Active),
            "cancelled" => Ok(Self::Cancelled),
            "completed" => Ok(Self::Completed),
            other => Err(ParseEscalationRunStatusError {
                input: other.to_string(),
            }),
        }
    }
}

// ── Domain models ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EscalationPolicy {
    pub id: EscalationPolicyId,
    pub server_id: ServerId,
    pub name: String,
    pub created_by: UserId,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct NewEscalationPolicy {
    pub server_id: ServerId,
    pub name: String,
    pub created_by: UserId,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EscalationStep {
    pub id: EscalationStepId,
    pub policy_id: EscalationPolicyId,
    /// 0-based order within the policy. Unique per `(policy_id, position)`.
    pub position: i32,
    /// Wait before this step fires, measured from the previous step firing (or from incident
    /// declare for position 0). `0` fires immediately at declare time.
    pub delay_seconds: i64,
    pub target: EscalationTarget,
    /// AND-combined actions performed when this step fires. Never empty — validated at the
    /// service boundary.
    pub actions: Vec<EscalationAction>,
}

#[derive(Debug, Clone)]
pub struct NewEscalationStep {
    pub position: i32,
    pub delay_seconds: i64,
    pub target: EscalationTarget,
    pub actions: Vec<EscalationAction>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EscalationRun {
    pub id: EscalationRunId,
    pub incident_id: IncidentId,
    pub policy_id: EscalationPolicyId,
    pub current_step_index: i32,
    /// When the current step is due to fire. `None` once the run is `Cancelled`/`Completed`.
    pub next_fire_at: Option<DateTime<Utc>>,
    pub status: EscalationRunStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct NewEscalationRun {
    pub incident_id: IncidentId,
    pub policy_id: EscalationPolicyId,
    pub next_fire_at: Option<DateTime<Utc>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_actions_sorts_and_dedups() {
        assert_eq!(
            encode_actions(&[
                EscalationAction::Email,
                EscalationAction::MentionInChannel,
                EscalationAction::Email,
            ]),
            "mention_in_channel,email"
        );
    }

    #[test]
    fn encode_single_action() {
        assert_eq!(
            encode_actions(&[EscalationAction::Email]),
            "email".to_string()
        );
    }

    #[test]
    fn decode_actions_round_trips_encode() {
        let actions = vec![EscalationAction::MentionInChannel, EscalationAction::Email];
        let encoded = encode_actions(&actions);
        let decoded = decode_actions(&encoded).unwrap();
        assert_eq!(
            decoded,
            vec![EscalationAction::MentionInChannel, EscalationAction::Email]
        );
    }

    #[test]
    fn decode_actions_rejects_unknown_token() {
        let err = decode_actions("email,bogus").unwrap_err();
        assert_eq!(err.to_string(), "unknown escalation action: bogus");
    }

    #[test]
    fn decode_actions_empty_string_yields_empty_vec() {
        assert_eq!(decode_actions("").unwrap(), Vec::<EscalationAction>::new());
    }

    #[test]
    fn escalation_target_as_str() {
        assert_eq!(EscalationTarget::User(UserId::new()).as_str(), "user");
        assert_eq!(
            EscalationTarget::OnCall(OnCallScheduleId::new()).as_str(),
            "on_call"
        );
    }

    #[test]
    fn escalation_target_kind_tag_round_trip() {
        assert_eq!(
            "user".parse::<EscalationTargetKindTag>().unwrap(),
            EscalationTargetKindTag::User
        );
        assert_eq!(
            "on_call".parse::<EscalationTargetKindTag>().unwrap(),
            EscalationTargetKindTag::OnCall
        );
        assert!("bogus".parse::<EscalationTargetKindTag>().is_err());
    }

    #[test]
    fn escalation_run_status_round_trip() {
        for status in [
            EscalationRunStatus::Active,
            EscalationRunStatus::Cancelled,
            EscalationRunStatus::Completed,
        ] {
            let s = status.as_str();
            let parsed: EscalationRunStatus = s.parse().unwrap();
            assert_eq!(parsed, status);
        }
        assert!("bogus".parse::<EscalationRunStatus>().is_err());
    }
}
