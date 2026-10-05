//! Roles and moderation (Track 1.2): bans, timeouts and message reports.

use async_graphql::{ComplexObject, Context, Enum, InputObject, Result, SimpleObject};

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
#[derive(SimpleObject, Clone, Debug)]
pub struct MessageReport {
    pub id: async_graphql::ID,
    pub server_id: async_graphql::ID,
    pub conversation_id: async_graphql::ID,
    pub message_id: async_graphql::ID,
    pub reporter_user_id: async_graphql::ID,
    pub reason: String,
    pub status: MessageReportStatus,
    pub resolved_by: Option<async_graphql::ID>,
    pub resolved_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub message_readable: bool,
    pub message_text: Option<String>,
    pub message_author_id: Option<async_graphql::ID>,
}

impl From<rim_services::moderation::dto::MessageReportDto> for MessageReport {
    fn from(value: rim_services::moderation::dto::MessageReportDto) -> Self {
        Self {
            id: async_graphql::ID(value.id.to_string()),
            server_id: async_graphql::ID(value.server_id.to_string()),
            conversation_id: async_graphql::ID(value.conversation_id.to_string()),
            message_id: async_graphql::ID(value.message_id.to_string()),
            reporter_user_id: async_graphql::ID(value.reporter_user_id.to_string()),
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

/// Reports ordered `createdAt ASC, id ASC`, standard cursor semantics (`docs/pagination.md`).
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
