use async_graphql::{ComplexObject, Context, Enum, InputObject, Object, Result, SimpleObject};

use super::{Attachment, ConversationTypeEnum, MessageCursorInput, User};
use crate::resolve::ResolveExt;

#[derive(SimpleObject, Clone, Debug)]
#[graphql(complex)]
pub struct SearchResultMessage {
    pub id: async_graphql::ID,
    pub server_id: async_graphql::ID,
    pub conversation_id: async_graphql::ID,
    pub conversation_title: Option<String>,
    pub conversation_type: ConversationTypeEnum,
    pub thread_id: Option<async_graphql::ID>,
    pub root_message_id: Option<async_graphql::ID>,
    pub author_id: async_graphql::ID,
    pub text: Option<String>,
    pub sent_at: chrono::DateTime<chrono::Utc>,
    /// Relevance score of the match. Informational, or for a sort within the page: results are
    /// paginated by recency (`sentAt`, `id`), not by rank.
    pub rank: f64,
    /// `ts_headline` snippet with `<mark>...</mark>` highlights around matched terms. `None` when
    /// the query had no free-text component (a purely operator-based search, e.g. `from:@alice`).
    pub headline: Option<String>,
}

#[ComplexObject]
impl SearchResultMessage {
    #[graphql(complexity = 5)]
    async fn author(&self, ctx: &Context<'_>) -> Result<Option<User>> {
        ctx.load_user(&self.author_id).await
    }
}

#[derive(Clone, Debug)]
pub struct Message {
    pub id: async_graphql::ID,
    pub server_id: async_graphql::ID,
    pub conversation_id: async_graphql::ID,
    pub thread_id: Option<async_graphql::ID>,
    pub root_thread_id: Option<async_graphql::ID>,
    pub author_id: async_graphql::ID,
    pub text: Option<String>,
    pub has_attachments: bool,
    pub attachments: Vec<Attachment>,
    pub parent_message_id: Option<async_graphql::ID>,
    pub forwarded_from_message_id: Option<async_graphql::ID>,
    pub is_deleted: bool,
    pub sent_at: chrono::DateTime<chrono::Utc>,
    pub encryption: Option<super::MessageEncryption>,
}

impl From<rim_services::message::dto::MessageDto> for Message {
    fn from(value: rim_services::message::dto::MessageDto) -> Self {
        Self {
            id: async_graphql::ID(value.id.to_string()),
            server_id: async_graphql::ID(value.server_id.to_string()),
            conversation_id: async_graphql::ID(value.conversation_id.to_string()),
            thread_id: value.thread_id.map(|id| async_graphql::ID(id.to_string())),
            root_thread_id: value
                .root_thread_id
                .map(|id| async_graphql::ID(id.to_string())),
            author_id: async_graphql::ID(value.author_id.to_string()),
            text: if value.is_deleted { None } else { value.text },
            has_attachments: value.has_attachments,
            // A deleted message is a tombstone: no text and no attachment names either.
            attachments: if value.is_deleted {
                Vec::new()
            } else {
                value.attachments.into_iter().map(Into::into).collect()
            },
            parent_message_id: value
                .parent_message_id
                .map(|id| async_graphql::ID(id.to_string())),
            forwarded_from_message_id: value
                .forwarded_from_message_id
                .map(|id| async_graphql::ID(id.to_string())),
            is_deleted: value.is_deleted,
            sent_at: value.sent_at,
            encryption: value.encryption.map(Into::into),
        }
    }
}

#[Object]
impl Message {
    async fn id(&self) -> &async_graphql::ID {
        &self.id
    }

    async fn server_id(&self) -> &async_graphql::ID {
        &self.server_id
    }

    async fn conversation_id(&self) -> &async_graphql::ID {
        &self.conversation_id
    }

    async fn thread_id(&self) -> &Option<async_graphql::ID> {
        &self.thread_id
    }

    async fn root_thread_id(&self) -> &Option<async_graphql::ID> {
        &self.root_thread_id
    }

    async fn author_id(&self) -> &async_graphql::ID {
        &self.author_id
    }

    async fn text(&self) -> &Option<String> {
        &self.text
    }

    async fn has_attachments(&self) -> bool {
        self.has_attachments
    }

    async fn attachments(&self) -> &Vec<Attachment> {
        &self.attachments
    }

    async fn parent_message_id(&self) -> &Option<async_graphql::ID> {
        &self.parent_message_id
    }

    async fn forwarded_from_message_id(&self) -> &Option<async_graphql::ID> {
        &self.forwarded_from_message_id
    }

    #[graphql(complexity = 5)]
    async fn forwarded_from(&self, ctx: &Context<'_>) -> Result<Option<Message>> {
        let Some(ref fwd_id) = self.forwarded_from_message_id else {
            return Ok(None);
        };
        let msg_svc =
            ctx.data::<std::sync::Arc<dyn rim_services::message::service::MessageService>>()?;
        let message_id = crate::ids::parse_uuid_id::<rim_core::message::model::MessageId>(fwd_id)?;
        let messages = msg_svc
            .get_messages_by_ids(&[message_id])
            .await
            .map_err(crate::errors::map_service_error)?;
        Ok(messages.into_iter().next().map(Into::into))
    }

    async fn is_deleted(&self) -> bool {
        self.is_deleted
    }

    async fn sent_at(&self) -> chrono::DateTime<chrono::Utc> {
        self.sent_at
    }

    #[graphql(complexity = 5)]
    async fn author(&self, ctx: &Context<'_>) -> Result<Option<User>> {
        ctx.load_user(&self.author_id).await
    }

    #[graphql(complexity = "child_complexity * 10")]
    async fn reactions(&self, ctx: &Context<'_>) -> Result<Vec<ReactionGroup>> {
        ctx.load_reactions(&self.id).await
    }

    #[graphql(complexity = 5)]
    async fn is_pinned(&self, ctx: &Context<'_>) -> Result<bool> {
        ctx.load_is_pinned(&self.id).await
    }

    #[graphql(complexity = 5)]
    async fn mentioned_user_ids(&self, ctx: &Context<'_>) -> Result<Vec<async_graphql::ID>> {
        let user_ids = ctx.load_mentioned_user_ids(&self.id).await?;
        Ok(user_ids
            .into_iter()
            .map(|id| async_graphql::ID(id.to_string()))
            .collect())
    }

    #[graphql(complexity = 5)]
    async fn link_previews(&self, ctx: &Context<'_>) -> Result<Vec<LinkPreview>> {
        ctx.load_link_previews(&self.id).await
    }

    #[graphql(complexity = "child_complexity * 10")]
    async fn mentions(&self, ctx: &Context<'_>) -> Result<Vec<User>> {
        ctx.load_mentions(&self.id).await
    }

    /// The MLS envelope of an encrypted message, whose `text` is always null; null for
    /// plaintext messages.
    async fn encryption(&self) -> &Option<super::MessageEncryption> {
        &self.encryption
    }
}

/// Input for the `searchMessages` full-text-search query. `query` accepts free text plus the
/// `from:`, `in:`, `has:`, `before:` and `after:` operators.
#[derive(InputObject, Clone, Debug)]
pub struct SearchMessagesInput {
    pub server_id: async_graphql::ID,
    pub query: String,
    /// Optional explicit single-conversation scope, in addition to the `in:` search operator.
    pub conversation_id: Option<async_graphql::ID>,
    /// Clamped server-side; defaults to 25 if omitted.
    pub limit: Option<i32>,
    pub cursor: Option<MessageCursorInput>,
    /// Defaults to `AFTER` (most-recent-first) if omitted.
    pub dir: Option<crate::pagination::PageDir>,
}

// ── Message change feed ───────────────────────────────────────────────────
//
// Clients cache changes and store cursors by these types, so their shape and semantics stay
// fixed.

/// One conversation to read in `messageChanges`, with the cursor the client stored for it.
#[derive(InputObject, Clone, Debug)]
pub struct MessageChangeCursorInput {
    pub conversation_id: async_graphql::ID,
    /// The `cursor` a previous `messageChanges` call returned for this conversation. Opaque.
    /// Omit (or pass null) to read the conversation's whole history from the beginning.
    pub after: Option<String>,
}

/// Input for `messageChanges`.
#[derive(InputObject, Clone, Debug)]
pub struct MessageChangesInput {
    pub server_id: async_graphql::ID,
    /// At most 100 conversations. One the caller cannot read comes back with status `NO_ACCESS`.
    pub conversations: Vec<MessageChangeCursorInput>,
    /// Maximum changes per conversation. Clamped to 1..=200; defaults to 100.
    pub limit: Option<i32>,
}

/// What happened to a message, judged from its state at its newest change. `DELETED` wins over
/// `EDITED`; a message whose text never changed is `CREATED`.
#[derive(Enum, Copy, Clone, Debug, PartialEq, Eq)]
pub enum MessageChangeKind {
    Created,
    Edited,
    Deleted,
}

impl From<rim_services::message::dto::MessageChangeKind> for MessageChangeKind {
    fn from(value: rim_services::message::dto::MessageChangeKind) -> Self {
        use rim_services::message::dto::MessageChangeKind as Kind;
        match value {
            Kind::Created => Self::Created,
            Kind::Edited => Self::Edited,
            Kind::Deleted => Self::Deleted,
        }
    }
}

/// A message as it is now, at its newest change. A message appears at most once per page.
#[derive(SimpleObject, Clone, Debug)]
pub struct MessageChange {
    pub kind: MessageChangeKind,
    /// The message's position in its conversation's change sequence, as a decimal string.
    /// Increases with every change; compare it to a cached copy to keep the newer one.
    pub version: String,
    /// When the text last changed; null for a never-edited message.
    pub edited_at: Option<chrono::DateTime<chrono::Utc>>,
    /// The message. Deleted messages come back with `isDeleted: true` and null `text`.
    pub message: Message,
}

/// How one conversation of a `messageChanges` request was answered.
#[derive(Enum, Copy, Clone, Debug, PartialEq, Eq)]
pub enum ConversationChangesStatus {
    /// The changes after the requested cursor.
    Ok,
    /// The caller cannot read this conversation (removed from it, not a member of a private one,
    /// or it does not exist). No changes; the cursor is echoed. Stop syncing it.
    NoAccess,
    /// The stored cursor is ahead of the server (its counters were reset). The changes start
    /// from the beginning: drop the cached copy of this conversation, then apply them.
    Reset,
}

impl From<rim_services::message::dto::ConversationChangesStatus> for ConversationChangesStatus {
    fn from(value: rim_services::message::dto::ConversationChangesStatus) -> Self {
        use rim_services::message::dto::ConversationChangesStatus as Status;
        match value {
            Status::Ok => Self::Ok,
            Status::NoAccess => Self::NoAccess,
            Status::Reset => Self::Reset,
        }
    }
}

/// The changes of one conversation after the requested cursor, oldest first.
#[derive(SimpleObject, Clone, Debug)]
pub struct ConversationMessageChanges {
    pub conversation_id: async_graphql::ID,
    pub status: ConversationChangesStatus,
    pub changes: Vec<MessageChange>,
    /// Store this and pass it as `after` next time. Opaque.
    pub cursor: String,
    /// More changes follow `cursor`; ask again with it right away.
    pub has_more: bool,
}

impl From<rim_services::message::dto::ConversationMessageChangesDto>
    for ConversationMessageChanges
{
    fn from(value: rim_services::message::dto::ConversationMessageChangesDto) -> Self {
        Self {
            conversation_id: async_graphql::ID(value.conversation_id.to_string()),
            status: value.status.into(),
            changes: value
                .changes
                .into_iter()
                .map(|change| MessageChange {
                    kind: change.kind.into(),
                    version: change.version.to_string(),
                    edited_at: change.edited_at,
                    message: change.message.into(),
                })
                .collect(),
            cursor: value.cursor.to_string(),
            has_more: value.has_more,
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct ReactionGroup {
    pub emoji: String,
    pub count: u32,
    pub reacted_by_me: bool,
    pub user_ids: Vec<async_graphql::ID>,
}

impl From<rim_services::message_reaction::dto::ReactionGroupDto> for ReactionGroup {
    fn from(value: rim_services::message_reaction::dto::ReactionGroupDto) -> Self {
        Self {
            emoji: value.emoji,
            count: value.count,
            reacted_by_me: value.reacted_by_me,
            user_ids: value
                .user_ids
                .into_iter()
                .map(|id| async_graphql::ID(id.to_string()))
                .collect(),
        }
    }
}

// ── Link Preview ──────────────────────────────────────────────────────────

#[derive(SimpleObject, Clone, Debug)]
pub struct LinkPreview {
    pub id: async_graphql::ID,
    pub message_id: async_graphql::ID,
    pub url: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub image_url: Option<String>,
    pub site_name: Option<String>,
    pub fetched_at: chrono::DateTime<chrono::Utc>,
}

impl From<rim_services::link_preview::dto::LinkPreviewDto> for LinkPreview {
    fn from(value: rim_services::link_preview::dto::LinkPreviewDto) -> Self {
        Self {
            id: async_graphql::ID(value.id.to_string()),
            message_id: async_graphql::ID(value.message_id.to_string()),
            url: value.url,
            title: value.title,
            description: value.description,
            image_url: value.image_url,
            site_name: value.site_name,
            fetched_at: value.fetched_at,
        }
    }
}
