use chrono::{DateTime, Utc};

use rim_core::conversation::model::ConversationId;
use rim_core::message::model::MessageId;
use rim_core::moderation::model::{
    MemberTimeout, MessageReport, MessageReportId, MessageReportStatus, ServerBan,
};
use rim_core::moderation::repository::MessageReportCursor;
use rim_core::server::model::ServerId;
use rim_core::user::model::UserId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerBanDto {
    pub server_id: ServerId,
    pub user_id: UserId,
    pub reason: Option<String>,
    pub banned_by: Option<UserId>,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
}

impl From<ServerBan> for ServerBanDto {
    fn from(b: ServerBan) -> Self {
        Self {
            server_id: b.server_id,
            user_id: b.user_id,
            reason: b.reason,
            banned_by: b.banned_by,
            created_at: b.created_at,
            expires_at: b.expires_at,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberTimeoutDto {
    pub server_id: ServerId,
    pub user_id: UserId,
    pub until: DateTime<Utc>,
    pub reason: Option<String>,
    pub set_by: Option<UserId>,
    pub created_at: DateTime<Utc>,
}

impl From<MemberTimeout> for MemberTimeoutDto {
    fn from(t: MemberTimeout) -> Self {
        Self {
            server_id: t.server_id,
            user_id: t.user_id,
            until: t.until,
            reason: t.reason,
            set_by: t.set_by,
            created_at: t.created_at,
        }
    }
}

/// A report as moderators see it. The reported message's current text and author are included
/// only when the viewing moderator can read the conversation (`message_readable`); otherwise
/// they are `None` and only the ids and the reporter's reason are shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageReportDto {
    pub id: MessageReportId,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,
    pub message_id: MessageId,
    pub reporter_user_id: UserId,
    pub reason: String,
    pub status: MessageReportStatus,
    pub resolved_by: Option<UserId>,
    pub resolved_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub message_readable: bool,
    /// `None` when unreadable or when the message was deleted.
    pub message_text: Option<String>,
    pub message_author_id: Option<UserId>,
}

impl MessageReportDto {
    /// The report alone, without message details.
    pub fn without_message(r: MessageReport) -> Self {
        Self {
            id: r.id,
            server_id: r.server_id,
            conversation_id: r.conversation_id,
            message_id: r.message_id,
            reporter_user_id: r.reporter_user_id,
            reason: r.reason,
            status: r.status,
            resolved_by: r.resolved_by,
            resolved_at: r.resolved_at,
            created_at: r.created_at,
            message_readable: false,
            message_text: None,
            message_author_id: None,
        }
    }
}

/// `(created_at, id)` cursor for [`crate::moderation::service::ModerationService::list_reports`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReportPageCursor {
    pub created_at: DateTime<Utc>,
    pub report_id: MessageReportId,
}

impl From<MessageReportCursor> for ReportPageCursor {
    fn from(c: MessageReportCursor) -> Self {
        Self {
            created_at: c.created_at,
            report_id: c.report_id,
        }
    }
}

impl From<ReportPageCursor> for MessageReportCursor {
    fn from(c: ReportPageCursor) -> Self {
        Self {
            created_at: c.created_at,
            report_id: c.report_id,
        }
    }
}
