use chrono::{DateTime, Utc};

use rim_core::conversation::model::{ConversationId, PostingPolicy};
use rim_core::message::model::MessageId;
use rim_core::moderation::model::{MessageReportId, MessageReportStatus};
use rim_core::repository::pagination::PageDir;
use rim_core::server::model::ServerId;
use rim_core::server_member::model::ServerMemberRole;
use rim_core::user::model::UserId;

use crate::Actor;
use crate::moderation::dto::ReportPageCursor;

/// Owner: any role except `Owner`, never on themselves. Moderator: `Member` <-> `Guest` only.
#[derive(Debug, Clone)]
pub struct SetMemberRoleCmd {
    pub actor: Actor,
    pub server_id: ServerId,
    pub user_id: UserId,
    pub role: ServerMemberRole,
}

/// Owner/Moderator. Cannot target an Owner; a Moderator cannot target a Moderator.
#[derive(Debug, Clone)]
pub struct KickMemberCmd {
    pub actor: Actor,
    pub server_id: ServerId,
    pub user_id: UserId,
    pub reason: Option<String>,
}

/// The caller leaves the workspace. Anyone but its Owner, outside the direct space and the
/// server's default workspace.
#[derive(Debug, Clone, Copy)]
pub struct LeaveWorkspaceCmd {
    pub actor: Actor,
    pub server_id: ServerId,
}

/// Same targeting rules as [`KickMemberCmd`]. Removes the member (if present) and records the
/// ban; `expires_at = None` bans until unbanned.
#[derive(Debug, Clone)]
pub struct BanMemberCmd {
    pub actor: Actor,
    pub server_id: ServerId,
    pub user_id: UserId,
    pub reason: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct UnbanMemberCmd {
    pub actor: Actor,
    pub server_id: ServerId,
    pub user_id: UserId,
}

#[derive(Debug, Clone)]
pub struct ListBansQuery {
    pub actor: Actor,
    pub server_id: ServerId,
}

/// Same targeting rules as [`KickMemberCmd`]; `until` must be in the future and at most
/// [`crate::moderation::MAX_TIMEOUT_DAYS`] days ahead.
#[derive(Debug, Clone)]
pub struct TimeoutMemberCmd {
    pub actor: Actor,
    pub server_id: ServerId,
    pub user_id: UserId,
    pub until: DateTime<Utc>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ClearTimeoutCmd {
    pub actor: Actor,
    pub server_id: ServerId,
    pub user_id: UserId,
}

/// Owner/Moderator; channels only; `seconds` in `0..=MAX_SLOW_MODE_SECONDS` (`0` turns it off).
#[derive(Debug, Clone)]
pub struct SetSlowModeCmd {
    pub actor: Actor,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,
    pub seconds: i32,
}

/// Owner/Moderator; channels only. Sets who may post in the channel and, under
/// `PostingPolicy::Moderators`, whether everyone else may still react and reply in threads.
#[derive(Debug, Clone)]
pub struct SetChannelPostingCmd {
    pub actor: Actor,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,
    pub policy: PostingPolicy,
    pub allows_threads: bool,
}

/// Any member who can read the message, once per message.
#[derive(Debug, Clone)]
pub struct ReportMessageCmd {
    pub actor: Actor,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,
    pub message_id: MessageId,
    pub reason: String,
}

/// Owner/Moderator; an instance operator for the direct space.
#[derive(Debug, Clone)]
pub struct ListReportsQuery {
    pub actor: Actor,
    pub server_id: ServerId,
    /// `None` lists every status.
    pub status: Option<MessageReportStatus>,
    pub limit: u32,
    pub dir: PageDir,
    pub cursor: Option<ReportPageCursor>,
}

/// Owner/Moderator, or an instance operator for the direct space; the report must belong to
/// `server_id`. Reveals a verified report's content and records that the moderator viewed it.
#[derive(Debug, Clone)]
pub struct GetReportQuery {
    pub actor: Actor,
    pub server_id: ServerId,
    pub report_id: MessageReportId,
}

/// Owner/Moderator of the report's workspace, or an instance operator for a report of the
/// direct space; the blob must be an attachment of the reported message or be listed in the
/// report's verified content.
#[derive(Debug, Clone)]
pub struct AuthorizeReportAttachmentQuery {
    pub actor_user_id: UserId,
    pub report_id: MessageReportId,
    /// SHA-256 of the encrypted blob.
    pub blob_sha256: Vec<u8>,
}

/// How a moderator closes a report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportOutcome {
    Resolved,
    Dismissed,
}

impl From<ReportOutcome> for MessageReportStatus {
    fn from(value: ReportOutcome) -> Self {
        match value {
            ReportOutcome::Resolved => MessageReportStatus::Resolved,
            ReportOutcome::Dismissed => MessageReportStatus::Dismissed,
        }
    }
}

/// Owner/Moderator; the conversation must be in `server_id` and the message in the
/// conversation.
#[derive(Debug, Clone)]
pub struct ClearMessageFlagCmd {
    pub actor: Actor,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,
    pub message_id: MessageId,
}

/// Counts `user_id`'s messages in the workspace's channels sent at or after `since` (every
/// message when `None`). Same targeting rules as `KickMemberCmd`; the target need not be a
/// member.
#[derive(Debug, Clone)]
pub struct MemberMessageCountsQuery {
    pub actor: Actor,
    pub server_id: ServerId,
    pub user_id: UserId,
    pub since: Option<DateTime<Utc>>,
}

/// Deletes `user_id`'s messages in the workspace's channels sent at or after `since` (every
/// message when `None`); private channels the actor is not in only with
/// `include_unjoined_private`. Same targeting rules as `KickMemberCmd`; the target need not be a
/// member.
#[derive(Debug, Clone)]
pub struct DeleteMemberMessagesCmd {
    pub actor: Actor,
    pub server_id: ServerId,
    pub user_id: UserId,
    pub since: Option<DateTime<Utc>>,
    pub include_unjoined_private: bool,
}

/// Owner/Moderator, or an instance operator for the direct space; the report must belong to
/// `server_id` and still be open.
#[derive(Debug, Clone)]
pub struct ResolveReportCmd {
    pub actor: Actor,
    pub server_id: ServerId,
    pub report_id: MessageReportId,
    pub outcome: ReportOutcome,
}
