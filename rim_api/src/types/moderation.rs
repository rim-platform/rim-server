//! Roles and moderation: bans, timeouts and message reports.

use async_graphql::{ComplexObject, Context, Enum, InputObject, Result, SimpleObject};
use base64::Engine;

use super::{PageInfo, User};
use crate::resolve::ResolveExt;

/// A workspace ban. Active bans block every way of joining the workspace.
#[derive(SimpleObject, Clone, Debug)]
#[graphql(complex)]
pub struct ServerBan {
    pub server_id: async_graphql::ID,
    pub user_id: async_graphql::ID,
    pub reason: Option<String>,
    pub banned_by: Option<async_graphql::ID>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// Null for a ban that lasts until it is lifted.
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[ComplexObject]
impl ServerBan {
    /// The banned user (null if the account no longer exists).
    async fn user(&self, ctx: &Context<'_>) -> Result<Option<User>> {
        ctx.load_user(&self.user_id).await
    }
}

impl From<rim_services::moderation::dto::ServerBanDto> for ServerBan {
    fn from(value: rim_services::moderation::dto::ServerBanDto) -> Self {
        Self {
            server_id: async_graphql::ID(value.server_id.to_string()),
            user_id: async_graphql::ID(value.user_id.to_string()),
            reason: value.reason,
            banned_by: value.banned_by.map(|id| async_graphql::ID(id.to_string())),
            created_at: value.created_at,
            expires_at: value.expires_at,
        }
    }
}

/// A member's timeout: until `until` they can read but not write in the workspace.
#[derive(SimpleObject, Clone, Debug)]
pub struct MemberTimeout {
    pub server_id: async_graphql::ID,
    pub user_id: async_graphql::ID,
    pub until: chrono::DateTime<chrono::Utc>,
    pub reason: Option<String>,
    pub set_by: Option<async_graphql::ID>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl From<rim_services::moderation::dto::MemberTimeoutDto> for MemberTimeout {
    fn from(value: rim_services::moderation::dto::MemberTimeoutDto) -> Self {
        Self {
            server_id: async_graphql::ID(value.server_id.to_string()),
            user_id: async_graphql::ID(value.user_id.to_string()),
            until: value.until,
            reason: value.reason,
            set_by: value.set_by.map(|id| async_graphql::ID(id.to_string())),
            created_at: value.created_at,
        }
    }
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum MessageReportStatus {
    Open,
    Resolved,
    Dismissed,
}

impl From<rim_core::moderation::model::MessageReportStatus> for MessageReportStatus {
    fn from(value: rim_core::moderation::model::MessageReportStatus) -> Self {
        use rim_core::moderation::model::MessageReportStatus as D;
        match value {
            D::Open => Self::Open,
            D::Resolved => Self::Resolved,
            D::Dismissed => Self::Dismissed,
        }
    }
}

impl From<MessageReportStatus> for rim_core::moderation::model::MessageReportStatus {
    fn from(value: MessageReportStatus) -> Self {
        match value {
            MessageReportStatus::Open => Self::Open,
            MessageReportStatus::Resolved => Self::Resolved,
            MessageReportStatus::Dismissed => Self::Dismissed,
        }
    }
}

/// How a moderator closes a report.
#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum ReportOutcome {
    Resolved,
    Dismissed,
}

impl From<ReportOutcome> for rim_services::moderation::command::ReportOutcome {
    fn from(value: ReportOutcome) -> Self {
        match value {
            ReportOutcome::Resolved => Self::Resolved,
            ReportOutcome::Dismissed => Self::Dismissed,
        }
    }
}

/// A member's report of a message. `messageText` and `messageAuthorId` are present only when
/// the viewing moderator can read the conversation (`messageReadable`); otherwise only the ids
/// and the reporter's reason are shown. `messageText` is also null for a deleted message.
///
/// A report of the direct space (DMs and groups) goes to the instance operators, who are not
/// members of its conversations: they see the author, send time and text of the one reported
/// message when it is not encrypted, and the author's principal id (`messageAuthorPrincipalId`)
/// whenever the author is shown.
///
/// A report of an encrypted message is franked: the server verified that its content is what
/// the sender's device sent (`frankingVerifiedAt`). Its author, send time and sender device are
/// shown to every moderator; its content (`frankedContent`) only by `messageReport`.
#[derive(SimpleObject, Clone, Debug)]
pub struct MessageReport {
    pub id: async_graphql::ID,
    pub server_id: async_graphql::ID,
    pub conversation_id: async_graphql::ID,
    pub message_id: async_graphql::ID,
    /// The member who filed the report; null for a report the automatic text art check filed.
    pub reporter_user_id: Option<async_graphql::ID>,
    pub reason: String,
    pub status: MessageReportStatus,
    pub resolved_by: Option<async_graphql::ID>,
    pub resolved_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub message_readable: bool,
    pub message_text: Option<String>,
    pub message_author_id: Option<async_graphql::ID>,
    /// The author's principal id, for `instanceBlockPrincipal`: filled for a report of the
    /// direct space whenever `messageAuthorId` is, null elsewhere and for an author without
    /// one.
    pub message_author_principal_id: Option<String>,
    /// When the reported message was sent; null when its details are not shown.
    pub message_sent_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Set when the server verified a franked report of an encrypted message.
    pub franking_verified_at: Option<chrono::DateTime<chrono::Utc>>,
    /// The client device id that sent the reported encrypted message.
    pub franked_sender_device_id: Option<async_graphql::ID>,
    /// The exact content bytes of a verified report, unpadded base64url. Only `messageReport`
    /// fills it; null in lists and once retention cleared it.
    pub franked_content: Option<String>,
    /// When the verified content is deleted: 90 days after the report was resolved or
    /// dismissed; null while it is open.
    pub franked_content_expires_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Whether the automatic text art check filed the report rather than a member. Listed in
    /// `serverInfo.capabilities` as `message_flag`.
    pub automatic: bool,
}

impl From<rim_services::moderation::dto::MessageReportDto> for MessageReport {
    fn from(value: rim_services::moderation::dto::MessageReportDto) -> Self {
        Self {
            id: async_graphql::ID(value.id.to_string()),
            server_id: async_graphql::ID(value.server_id.to_string()),
            conversation_id: async_graphql::ID(value.conversation_id.to_string()),
            message_id: async_graphql::ID(value.message_id.to_string()),
            reporter_user_id: value
                .reporter_user_id
                .map(|id| async_graphql::ID(id.to_string())),
            automatic: value.reporter_user_id.is_none(),
            reason: value.reason,
            status: value.status.into(),
            resolved_by: value
                .resolved_by
                .map(|id| async_graphql::ID(id.to_string())),
            resolved_at: value.resolved_at,
            created_at: value.created_at,
            message_readable: value.message_readable,
            message_text: value.message_text,
            message_author_id: value
                .message_author_id
                .map(|id| async_graphql::ID(id.to_string())),
            message_author_principal_id: value.message_author_principal_id,
            message_sent_at: value.message_sent_at,
            franking_verified_at: value.franking_verified_at,
            franked_sender_device_id: value
                .franked_sender_device_id
                .map(|id| async_graphql::ID(id.to_string())),
            franked_content: value
                .franked_content
                .map(|c| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(c)),
            franked_content_expires_at: value.franked_content_expires_at,
        }
    }
}

#[derive(InputObject, Clone, Debug)]
pub struct MessageReportCursorInput {
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub report_id: async_graphql::ID,
}

impl TryFrom<MessageReportCursorInput> for rim_services::moderation::dto::ReportPageCursor {
    type Error = async_graphql::Error;

    fn try_from(value: MessageReportCursorInput) -> Result<Self, Self::Error> {
        Ok(Self {
            created_at: value.created_at,
            report_id: crate::ids::parse_uuid_id::<rim_core::moderation::model::MessageReportId>(
                &value.report_id,
            )?,
        })
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct MessageReportCursorOutput {
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub report_id: async_graphql::ID,
}

impl From<rim_services::moderation::dto::ReportPageCursor> for MessageReportCursorOutput {
    fn from(value: rim_services::moderation::dto::ReportPageCursor) -> Self {
        Self {
            created_at: value.created_at,
            report_id: async_graphql::ID(value.report_id.to_string()),
        }
    }
}

/// Reports ordered `createdAt ASC, id ASC`, with the standard cursor semantics.
#[derive(SimpleObject, Clone, Debug)]
pub struct MessageReportPage {
    pub items: Vec<MessageReport>,
    pub next: Option<MessageReportCursorOutput>,
    pub prev: Option<MessageReportCursorOutput>,
    pub page_info: PageInfo,
}

impl MessageReportPage {
    pub fn from_page(
        page: rim_core::repository::pagination::Page<
            rim_services::moderation::dto::MessageReportDto,
            rim_services::moderation::dto::ReportPageCursor,
        >,
    ) -> Self {
        let has_next = page.next.is_some();
        let has_prev = page.prev.is_some();
        Self {
            items: page.items.into_iter().map(Into::into).collect(),
            next: page.next.map(Into::into),
            prev: page.prev.map(Into::into),
            page_info: PageInfo { has_next, has_prev },
        }
    }
}
