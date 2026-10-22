use chrono::{DateTime, Utc};
use uuid::Uuid;

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
    /// `None` when unreadable or when the message was deleted. An instance operator reads the
    /// text of a reported unencrypted message of the direct space.
    pub message_text: Option<String>,
    /// Shown when the conversation is readable, for a verified franked report, and to an
    /// instance operator for a report of an unencrypted message of the direct space.
    pub message_author_id: Option<UserId>,
    /// The author's principal id, for a report of the direct space whose author is shown;
    /// `None` elsewhere and for an author without one.
    pub message_author_principal_id: Option<String>,
    pub message_sent_at: Option<DateTime<Utc>>,
    /// Set for a verified franked report of an encrypted message.
    pub franking_verified_at: Option<DateTime<Utc>>,
    /// The sender's client device id the receipt covered.
    pub franked_sender_device_id: Option<Uuid>,
    /// The verified content bytes. Filled only when one moderator opens one report
    /// ([`crate::moderation::service::ModerationService::get_report`]); `None` in lists and
    /// once retention cleared it.
    pub franked_content: Option<Vec<u8>>,
    /// When retention clears the content: 90 days after the report was resolved or dismissed.
    pub franked_content_expires_at: Option<DateTime<Utc>>,
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
            message_author_principal_id: None,
            message_sent_at: None,
            franking_verified_at: None,
            franked_sender_device_id: None,
            franked_content: None,
            franked_content_expires_at: None,
        }
    }
}

/// Where a reported encrypted attachment's blob is stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportAttachmentDto {
    pub storage_key: String,
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
