use chrono::{DateTime, Utc};
use rim_core::server::model::ServerId;
use rim_core::user::model::UserId;

#[derive(Debug, Clone)]
pub struct AuthSessionDto {
    pub user_id: UserId,
    pub session_id: rim_core::auth::model::SessionId,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct InviteDto {
    pub id: rim_core::auth::model::InviteId,
    pub server_id: ServerId,
    pub created_by_user_id: UserId,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub max_uses: i32,
    pub use_count: i32,
    pub revoked_at: Option<DateTime<Utc>>,
    pub role: rim_core::server_member::model::ServerMemberRole,
}

impl InviteDto {
    /// Whether the invite still admits anyone at `now`, and if not, why: a revocation wins
    /// over an expiry, and an expiry over the uses running out.
    pub fn state_at(&self, now: DateTime<Utc>) -> InviteState {
        if self.revoked_at.is_some() {
            InviteState::Revoked
        } else if self.expires_at.is_some_and(|at| at <= now) {
            InviteState::Expired
        } else if self.use_count >= self.max_uses {
            InviteState::UsedUp
        } else {
            InviteState::Active
        }
    }
}

/// Whether a workspace invite still admits anyone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InviteState {
    /// Unrevoked, unexpired and with uses left.
    Active,
    /// Past its expiry.
    Expired,
    /// Every use taken.
    UsedUp,
    /// Revoked by an owner, or by the instance when its creator was blocked or deleted.
    Revoked,
}

/// `(created_at, id)` cursor of the workspace invite list (newest first).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvitePageCursor {
    pub created_at: DateTime<Utc>,
    pub invite_id: rim_core::auth::model::InviteId,
}

impl From<rim_core::auth::repository::InviteCursor> for InvitePageCursor {
    fn from(c: rim_core::auth::repository::InviteCursor) -> Self {
        Self {
            created_at: c.created_at,
            invite_id: c.invite_id,
        }
    }
}

impl From<InvitePageCursor> for rim_core::auth::repository::InviteCursor {
    fn from(c: InvitePageCursor) -> Self {
        Self {
            created_at: c.created_at,
            invite_id: c.invite_id,
        }
    }
}

#[derive(Debug, Clone)]
pub struct LoginResultDto {
    pub user_id: UserId,
    pub session: AuthSessionDto,
    pub session_token: String,
    pub csrf_token: String,
    /// The session may only set a password: the local account has none.
    pub must_set_password: bool,
}

#[derive(Debug, Clone)]
pub struct CreateInviteResultDto {
    pub invite: InviteDto,
    pub invite_token: String,
}

#[derive(Debug, Clone)]
pub struct AcceptInviteResultDto {
    pub user_id: UserId,
    pub server_id: ServerId,
    pub session: AuthSessionDto,
    pub session_token: String,
    pub csrf_token: String,
}

#[derive(Debug, Clone)]
pub struct JoinServerByInviteResultDto {
    pub server_id: ServerId,
}

#[derive(Debug, Clone, Default)]
pub struct RequestAuthResultDto {
    pub user_id: Option<UserId>,
    pub session_id: Option<rim_core::auth::model::SessionId>,
    /// The device the session is bound to.
    pub device_key_id: Option<rim_core::key_identity::model::DeviceKeyId>,
    /// When the session expires.
    pub expires_at: Option<DateTime<Utc>>,
    /// The session may only set a password: its local account has none.
    pub must_set_password: bool,
}

/// Result of any signup path (instance invite, workspace invite, open).
///
/// `server_id` is `Some` only when a workspace invite was used. The server's default workspace,
/// which every signup joins when one is set, is not reported here.
#[derive(Debug, Clone)]
pub struct SignupResultDto {
    pub user_id: UserId,
    pub server_id: Option<ServerId>,
    pub session: AuthSessionDto,
    pub session_token: String,
    pub csrf_token: String,
    /// After a key signup: the new account's server share, in the clear.
    pub server_share: Option<Vec<u8>>,
}

/// Token type discriminant for the preview query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InviteTokenType {
    /// Instance invite: "create your RIM account" flow via `instanceInviteToken` input.
    Instance,
    /// Workspace invite: "join workspace X" copy.
    Workspace,
}

/// Minimal workspace info included in a workspace-invite preview.
#[derive(Debug, Clone)]
pub struct WorkspaceInvitePreviewDto {
    pub server_id: ServerId,
    pub server_name: String,
    pub invited_by_display_name: String,
    /// The owner's display name, else username; `None` when the workspace has no owner.
    pub owner_display_name: Option<String>,
}

/// Read-only preview of a registration token.
///
/// Does NOT consume the token.
#[derive(Debug, Clone)]
pub struct InviteTokenPreviewDto {
    pub token_type: InviteTokenType,
    pub expires_at: Option<DateTime<Utc>>,
    /// Populated only when `token_type == Workspace`.
    pub workspace: Option<WorkspaceInvitePreviewDto>,
}

/// The registration mode in effect for this deployment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistrationMode {
    /// Default: signup requires a valid token (instance or workspace invite).
    InviteOnly,
    /// `RIM_OPEN_REGISTRATION=true`: signup accepts no token; rate-limited.
    Open,
    /// `RIM_REGISTRATION_POW_BITS`: a key-only signup takes a registration token or a
    /// proof-of-work of `bits` leading zero bits (rate-limited as open signups); a password
    /// signup takes a token, as in invite-only mode.
    Pow { bits: u32 },
}

#[cfg(test)]
mod tests {
    use chrono::Duration;

    use super::*;

    fn invite(
        expires_in: Option<Duration>,
        use_count: i32,
        revoked: bool,
        now: DateTime<Utc>,
    ) -> InviteDto {
        InviteDto {
            id: rim_core::auth::model::InviteId::new(),
            server_id: ServerId::new(),
            created_by_user_id: UserId::new(),
            created_at: now - Duration::days(1),
            expires_at: expires_in.map(|d| now + d),
            max_uses: 2,
            use_count,
            revoked_at: revoked.then_some(now),
            role: rim_core::server_member::model::ServerMemberRole::Member,
        }
    }

    /// Revoked wins over expired, expired over used up; an invite without an expiry never
    /// expires.
    #[test]
    fn invite_state_derivation() {
        let now = Utc::now();
        let day = Some(Duration::days(1));
        let past = Some(Duration::seconds(-1));
        for (expires_in, use_count, revoked, expected) in [
            (day, 0, false, InviteState::Active),
            (None, 1, false, InviteState::Active),
            (past, 0, false, InviteState::Expired),
            (Some(Duration::zero()), 0, false, InviteState::Expired),
            (day, 2, false, InviteState::UsedUp),
            (None, 2, false, InviteState::UsedUp),
            (past, 2, false, InviteState::Expired),
            (day, 0, true, InviteState::Revoked),
            (past, 2, true, InviteState::Revoked),
        ] {
            assert_eq!(
                invite(expires_in, use_count, revoked, now).state_at(now),
                expected,
                "{expires_in:?} {use_count} {revoked}"
            );
        }
    }
}
