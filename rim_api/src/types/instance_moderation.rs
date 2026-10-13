//! GraphQL types of the instance moderation queue: abuse reports, principal blocks and IP
//! blocks as instance operators read and change them.

use async_graphql::{ID, SimpleObject};
use chrono::{DateTime, Utc};
use rim_services::ip_block::dto::{IpBlockDto, IpBlockOutcome};
use rim_services::relay::dto::{
    AbuseReportDto, AccountSummaryDto, PrincipalBlockDto, PrincipalBlockOutcome,
    PrincipalUnblockOutcome,
};

/// An abuse report filed with this server, by a local user or from the abuse address.
#[derive(SimpleObject, Clone, Debug)]
pub struct InstanceAbuseReport {
    /// The report's id.
    pub id: ID,
    /// The local user who filed the report; null for one the operator filed from the abuse
    /// address.
    pub reporter_user_id: Option<ID>,
    /// The reporter's username; null with `reporterUserId`.
    pub reporter_username: Option<String>,
    /// The principal reported (`rimp1_…`); null for a report naming only a server.
    pub subject_principal_id: Option<String>,
    /// The username of the account holding the reported principal here; null when no local
    /// account holds it.
    pub subject_username: Option<String>,
    /// The server reported (`rims1_…`); null for a report naming only a principal.
    pub subject_server_id: Option<String>,
    /// The DM request the report was filed on; null for one filed otherwise.
    pub request_id: Option<ID>,
    /// Why the report was filed.
    pub reason: String,
    /// When the report was filed.
    pub created_at: DateTime<Utc>,
    /// When the report was resolved; null while it is open.
    pub resolved_at: Option<DateTime<Utc>>,
    /// Whether the report is resolved.
    pub resolved: bool,
}

impl From<AbuseReportDto> for InstanceAbuseReport {
    fn from(dto: AbuseReportDto) -> Self {
        Self {
            id: ID(dto.id.to_string()),
            reporter_user_id: dto.reporter_user_id.map(|id| ID(id.to_string())),
            reporter_username: dto.reporter_username,
            subject_principal_id: dto.subject_principal_id,
            subject_username: dto.subject_username,
            subject_server_id: dto.subject_server_id,
            request_id: dto.request_id.map(|id| ID(id.to_string())),
            reason: dto.reason,
            created_at: dto.created_at,
            resolved: dto.resolved_at.is_some(),
            resolved_at: dto.resolved_at,
        }
    }
}

/// A principal this server refuses: its sessions were ended, and its signups, logins and
/// deliveries are refused.
#[derive(SimpleObject, Clone, Debug)]
pub struct InstancePrincipalBlock {
    /// The principal blocked (`rimp1_…`).
    pub principal_id: String,
    /// The username of the account holding the principal here; null when no local account
    /// holds it.
    pub username: Option<String>,
    /// Why the principal was blocked.
    pub reason: String,
    /// When the principal was blocked.
    pub created_at: DateTime<Utc>,
}

impl From<PrincipalBlockDto> for InstancePrincipalBlock {
    fn from(dto: PrincipalBlockDto) -> Self {
        Self {
            principal_id: dto.principal_id,
            username: dto.username,
            reason: dto.reason,
            created_at: dto.created_at,
        }
    }
}

/// An address or range on this server's IP deny list, refused on every route but the liveness
/// probe.
#[derive(SimpleObject, Clone, Debug)]
pub struct InstanceIpBlock {
    /// The address or range in CIDR form, IPv4 or IPv6.
    pub cidr: String,
    /// Why the address or range was blocked.
    pub reason: String,
    /// When the block lifts; null for a block that lasts until it is removed.
    pub expires_at: Option<DateTime<Utc>>,
    /// When the block was made, or its reason and expiry last replaced.
    pub created_at: DateTime<Utc>,
    /// Whether the block has expired and is no longer in force.
    pub expired: bool,
}

impl InstanceIpBlock {
    /// The block as it stands at `now`.
    pub(crate) fn at(dto: IpBlockDto, now: DateTime<Utc>) -> Self {
        Self {
            cidr: dto.range.to_string(),
            reason: dto.reason,
            expired: dto.expires_at.is_some_and(|at| at <= now),
            expires_at: dto.expires_at,
            created_at: dto.created_at,
        }
    }
}

/// A local account, as the moderation queue names it.
#[derive(SimpleObject, Clone, Debug)]
pub struct InstanceAccount {
    /// The account's user id.
    pub user_id: ID,
    /// The account's username.
    pub username: String,
    /// The account's principal (`rimp1_…`); null for an account that never enrolled a key.
    pub principal_id: Option<String>,
    /// When the account was created.
    pub created_at: DateTime<Utc>,
}

impl From<AccountSummaryDto> for InstanceAccount {
    fn from(dto: AccountSummaryDto) -> Self {
        Self {
            user_id: ID(dto.user_id.to_string()),
            username: dto.username,
            principal_id: dto.principal_id,
            created_at: dto.created_at,
        }
    }
}

/// What blocking one principal or account did.
#[derive(SimpleObject, Clone, Debug)]
pub struct InstancePrincipalBlockOutcome {
    /// The principal blocked; null for an account of a cascade that has no key identity.
    pub principal_id: Option<String>,
    /// False when it was blocked already; its reason was replaced.
    pub newly_blocked: bool,
    /// The local account blocked; null when no local account holds the principal.
    pub account: Option<InstanceAccount>,
    /// The reason recorded; a cascade's accounts carry an "(invited by …)" suffix.
    pub reason: String,
    /// How many of the account's sessions were ended.
    pub ended_sessions: u64,
    /// How many pending DM requests from the principal were ignored.
    pub ignored_requests: u64,
    /// How many of the account's invites with uses left were revoked.
    pub revoked_invites: u64,
    /// Whether the account's email is now refused at signup and password login.
    pub email_blocked: bool,
    /// The accounts the account's invites created, oldest first.
    pub invitees: Vec<InstanceAccount>,
}

impl From<PrincipalBlockOutcome> for InstancePrincipalBlockOutcome {
    fn from(outcome: PrincipalBlockOutcome) -> Self {
        Self {
            principal_id: outcome.principal_id,
            newly_blocked: outcome.newly_blocked,
            account: outcome.account.map(Into::into),
            reason: outcome.reason,
            ended_sessions: outcome.ended_sessions,
            ignored_requests: outcome.ignored_requests,
            revoked_invites: outcome.revoked_invites,
            email_blocked: outcome.blocked_email.is_some(),
            invitees: outcome.invitees.into_iter().map(Into::into).collect(),
        }
    }
}

/// What lifting a principal's block did.
#[derive(SimpleObject, Clone, Debug)]
pub struct InstancePrincipalUnblockOutcome {
    /// Whether the principal was blocked.
    pub lifted: bool,
    /// Whether the account's email was blocked and no longer is.
    pub email_unblocked: bool,
}

impl From<PrincipalUnblockOutcome> for InstancePrincipalUnblockOutcome {
    fn from(outcome: PrincipalUnblockOutcome) -> Self {
        Self {
            lifted: outcome.lifted,
            email_unblocked: outcome.email_unblocked,
        }
    }
}

/// What blocking an address or range did.
#[derive(SimpleObject, Clone, Debug)]
pub struct InstanceIpBlockOutcome {
    /// The block now in force.
    pub block: InstanceIpBlock,
    /// False when the range was blocked already; its reason and expiry were replaced.
    pub newly_blocked: bool,
}

impl InstanceIpBlockOutcome {
    /// The outcome with its block as it stands at `now`.
    pub(crate) fn at(outcome: IpBlockOutcome, now: DateTime<Utc>) -> Self {
        Self {
            block: InstanceIpBlock::at(outcome.block, now),
            newly_blocked: outcome.newly_blocked,
        }
    }
}
