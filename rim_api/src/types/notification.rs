//! Notification rules and the notification inbox.

use async_graphql::{
    ComplexObject, Context, Enum, Error, ErrorExtensions, InputObject, Result, SimpleObject,
};

use super::{Conversation, Message, PageInfo};
use crate::errors::map_service_error;
use crate::ids::parse_uuid_id;
use crate::resolve::ResolveExt;

/// Which messages of a conversation notify the viewer.
#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum NotificationLevel {
    /// Every message.
    All,
    /// Direct messages, mentions (`<@user>`, `@channel`, `@here`), keyword hits and replies in
    /// the viewer's threads.
    Mentions,
    /// Nothing, mentions included.
    None,
}

impl From<rim_core::notification::model::NotificationLevel> for NotificationLevel {
    fn from(value: rim_core::notification::model::NotificationLevel) -> Self {
        use rim_core::notification::model::NotificationLevel as D;
        match value {
            D::All => Self::All,
            D::Mentions => Self::Mentions,
            D::None => Self::None,
        }
    }
}

impl From<NotificationLevel> for rim_core::notification::model::NotificationLevel {
    fn from(value: NotificationLevel) -> Self {
        match value {
            NotificationLevel::All => Self::All,
            NotificationLevel::Mentions => Self::Mentions,
            NotificationLevel::None => Self::None,
        }
    }
}

/// Why the viewer was notified (the strongest reason that applied).
#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum NotificationReason {
    Direct,
    Mention,
    Keyword,
    ThreadReply,
    All,
}

impl From<rim_core::notification::model::NotificationReason> for NotificationReason {
    fn from(value: rim_core::notification::model::NotificationReason) -> Self {
        use rim_core::notification::model::NotificationReason as D;
        match value {
            D::Direct => Self::Direct,
            D::Mention => Self::Mention,
            D::Keyword => Self::Keyword,
            D::ThreadReply => Self::ThreadReply,
            D::All => Self::All,
        }
    }
}

/// How often unread notifications are emailed.
#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum DigestFrequency {
    Off,
    Hourly,
    Daily,
}

impl From<rim_core::notification::model::DigestFrequency> for DigestFrequency {
    fn from(value: rim_core::notification::model::DigestFrequency) -> Self {
        use rim_core::notification::model::DigestFrequency as D;
        match value {
            D::Off => Self::Off,
            D::Hourly => Self::Hourly,
            D::Daily => Self::Daily,
        }
    }
}

impl From<DigestFrequency> for rim_core::notification::model::DigestFrequency {
    fn from(value: DigestFrequency) -> Self {
        match value {
            DigestFrequency::Off => Self::Off,
            DigestFrequency::Hourly => Self::Hourly,
            DigestFrequency::Daily => Self::Daily,
        }
    }
}

/// A daily do-not-disturb window. `start`/`end` are `"HH:MM"` wall-clock times in `timezone`
/// (an IANA name); the window crosses midnight when `end` is before `start`.
#[derive(SimpleObject, Clone, Debug)]
pub struct DndWindow {
    pub start: String,
    pub end: String,
    pub timezone: String,
}

#[derive(InputObject, Clone, Debug)]
pub struct DndWindowInput {
    /// `"HH:MM"` (or `"HH:MM:SS"`).
    pub start: String,
    /// `"HH:MM"` (or `"HH:MM:SS"`); must differ from `start`.
    pub end: String,
    /// IANA timezone name, e.g. `Europe/Prague`.
    pub timezone: String,
}

fn parse_time(field: &str, raw: &str) -> Result<chrono::NaiveTime> {
    chrono::NaiveTime::parse_from_str(raw.trim(), "%H:%M")
        .or_else(|_| chrono::NaiveTime::parse_from_str(raw.trim(), "%H:%M:%S"))
        .map_err(|_| {
            Error::new(format!("invalid {field} {raw:?}, expected HH:MM")).extend_with(|_, e| {
                e.set("code", "VALIDATION");
            })
        })
}

impl TryFrom<DndWindowInput> for rim_core::notification::model::DndWindow {
    type Error = Error;

    fn try_from(value: DndWindowInput) -> Result<Self> {
        Ok(Self {
            start: parse_time("start", &value.start)?,
            end: parse_time("end", &value.end)?,
            timezone: value.timezone,
        })
    }
}

/// The viewer's notification settings in one workspace (defaults until first saved).
#[derive(SimpleObject, Clone, Debug)]
pub struct NotificationSettings {
    pub server_id: async_graphql::ID,
    /// Applies to every conversation without its own level.
    pub default_level: NotificationLevel,
    /// Lower-cased; a whole-word, case-insensitive hit notifies (level `MENTIONS` or `ALL`).
    pub keywords: Vec<String>,
    pub dnd: Option<DndWindow>,
    /// The manual do-not-disturb pause (`setDoNotDisturb`, `notifications.dnd`).
    pub dnd_paused: bool,
    /// When the manual pause ends; null while `dndPaused` is false or the pause has no end.
    pub dnd_paused_until: Option<chrono::DateTime<chrono::Utc>>,
    pub digest: DigestFrequency,
}

impl From<rim_services::notification::dto::NotificationSettingsDto> for NotificationSettings {
    fn from(value: rim_services::notification::dto::NotificationSettingsDto) -> Self {
        use rim_core::notification::model::DndPause;
        let (dnd_paused, dnd_paused_until) = match value.dnd_pause {
            DndPause::Off => (false, None),
            DndPause::Indefinite => (true, None),
            DndPause::Until(until) => (true, Some(until)),
        };
        Self {
            server_id: async_graphql::ID(value.server_id.to_string()),
            default_level: value.default_level.into(),
            keywords: value.keywords,
            dnd: value.dnd.map(|w| DndWindow {
                start: w.start.format("%H:%M").to_string(),
                end: w.end.format("%H:%M").to_string(),
                timezone: w.timezone,
            }),
            dnd_paused,
            dnd_paused_until,
            digest: value.digest.into(),
        }
    }
}

/// Replaces the editable settings (the manual pause is separate: `setDoNotDisturb`).
#[derive(InputObject, Clone, Debug)]
pub struct UpdateNotificationSettingsInput {
    pub default_level: NotificationLevel,
    /// At most 20, each 2 to 40 characters; stored lower-cased and deduplicated.
    pub keywords: Vec<String>,
    /// Null turns the daily window off.
    pub dnd: Option<DndWindowInput>,
    pub digest: DigestFrequency,
}

/// The viewer's level for one conversation, as returned by `setConversationNotificationLevel`.
#[derive(SimpleObject, Clone, Debug)]
pub struct ConversationNotificationLevel {
    pub conversation_id: async_graphql::ID,
    /// The conversation's own level; null inherits the workspace default.
    pub level: Option<NotificationLevel>,
    /// What applies now: `NONE` while muted, else `level`, else the workspace default.
    pub effective_level: NotificationLevel,
    pub muted_until: Option<chrono::DateTime<chrono::Utc>>,
}

impl From<rim_services::notification::dto::ConversationNotificationLevelDto>
    for ConversationNotificationLevel
{
    fn from(value: rim_services::notification::dto::ConversationNotificationLevelDto) -> Self {
        Self {
            conversation_id: async_graphql::ID(value.conversation_id.to_string()),
            level: value.level.map(Into::into),
            effective_level: value.effective_level.into(),
            muted_until: value.muted_until,
        }
    }
}

/// One entry of the viewer's notification inbox.
#[derive(SimpleObject, Clone, Debug)]
#[graphql(complex)]
pub struct Notification {
    pub id: async_graphql::ID,
    pub server_id: async_graphql::ID,
    pub conversation_id: async_graphql::ID,
    pub message_id: async_graphql::ID,
    pub reason: NotificationReason,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub read_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[ComplexObject]
impl Notification {
    /// Null when the viewer can no longer read the conversation.
    async fn conversation(&self, ctx: &Context<'_>) -> Result<Option<Conversation>> {
        let conversation = ctx
            .load_readable_conversation(&self.server_id, &self.conversation_id)
            .await?;
        Ok(conversation.map(Into::into))
    }

    /// The message (timeline or thread reply); null when the viewer can no longer read the
    /// conversation or the message is gone.
    async fn message(&self, ctx: &Context<'_>) -> Result<Option<Message>> {
        if ctx
            .load_readable_conversation(&self.server_id, &self.conversation_id)
            .await?
            .is_none()
        {
            return Ok(None);
        }
        let svc =
            ctx.data::<std::sync::Arc<dyn rim_services::message::service::MessageService>>()?;
        let message_id = parse_uuid_id::<rim_core::message::model::MessageId>(&self.message_id)?;
        let messages = svc
            .get_messages_by_ids(&[message_id])
            .await
            .map_err(map_service_error)?;
        Ok(messages.into_iter().next().map(Into::into))
    }
}

impl From<rim_services::notification::dto::NotificationDto> for Notification {
    fn from(value: rim_services::notification::dto::NotificationDto) -> Self {
        Self {
            id: async_graphql::ID(value.id.to_string()),
            server_id: async_graphql::ID(value.server_id.to_string()),
            conversation_id: async_graphql::ID(value.conversation_id.to_string()),
            message_id: async_graphql::ID(value.message_id.to_string()),
            reason: value.reason.into(),
            created_at: value.created_at,
            read_at: value.read_at,
        }
    }
}

#[derive(InputObject, Clone, Debug)]
pub struct NotificationCursorInput {
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub notification_id: async_graphql::ID,
}

impl TryFrom<NotificationCursorInput> for rim_services::notification::dto::NotificationPageCursor {
    type Error = Error;

    fn try_from(value: NotificationCursorInput) -> Result<Self> {
        Ok(Self {
            created_at: value.created_at,
            notification_id: parse_uuid_id::<rim_core::notification::model::UserNotificationId>(
                &value.notification_id,
            )?,
        })
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct NotificationCursorOutput {
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub notification_id: async_graphql::ID,
}

impl From<rim_services::notification::dto::NotificationPageCursor> for NotificationCursorOutput {
    fn from(value: rim_services::notification::dto::NotificationPageCursor) -> Self {
        Self {
            created_at: value.created_at,
            notification_id: async_graphql::ID(value.notification_id.to_string()),
        }
    }
}

/// Notifications ordered `createdAt DESC, id DESC` (newest first), with the standard cursor
/// semantics: `AFTER` pages go to older notifications.
#[derive(SimpleObject, Clone, Debug)]
pub struct NotificationPage {
    pub items: Vec<Notification>,
    pub next: Option<NotificationCursorOutput>,
    pub prev: Option<NotificationCursorOutput>,
    pub page_info: PageInfo,
}

impl NotificationPage {
    pub fn from_page(
        page: rim_core::repository::pagination::Page<
            rim_services::notification::dto::NotificationDto,
            rim_services::notification::dto::NotificationPageCursor,
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
