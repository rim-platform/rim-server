use crate::conversation::model::ConversationId;
use crate::message::model::{Message, MessageId, NewMessage};
use crate::message_thread::model::MessageThreadId;
use crate::repository::error::RepoResult;
use crate::repository::pagination::{Page, PageDir, SortOrder};
use crate::user::model::UserId;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MessageCursor {
    pub sent_at: chrono::DateTime<chrono::Utc>,
    pub message_id: MessageId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListMessagesParams {
    pub conversation_id: ConversationId,
    pub thread_id: Option<MessageThreadId>,
    pub limit: u32,
    pub dir: PageDir,
    pub cursor: Option<MessageCursor>,
    pub include_deleted: bool,
    pub order: SortOrder,
}

/// Parameters for [`MessageRepository::search_messages_fts`].
///
/// **Visibility-agnostic by design:** this struct carries the caller-computed accessible
/// conversation set (`conversation_ids`) — the repository does NOT decide, widen, or narrow
/// visibility. The service layer is the sole authority: it MUST derive `conversation_ids` from
/// `ConversationRepository::list_accessible_conversation_ids` (plus the member-access gate for an
/// explicit single-conversation scope) before calling this method. An empty `conversation_ids`
/// yields zero rows — that is the correct "nothing accessible" outcome, never "search everything".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageSearchParams {
    /// The accessible conversation set to search within. Empty => empty result, never "all".
    pub conversation_ids: Vec<ConversationId>,
    /// Free-text portion of the query, passed to `websearch_to_tsquery`. May be empty
    /// (operator-only search matches purely on the structured filters below).
    pub tsquery: String,
    /// `from:` operator, already resolved to a concrete user id by the service layer.
    pub author_id: Option<UserId>,
    /// `has:link` operator.
    pub has_link: bool,
    /// `has:attachment` operator.
    pub has_attachment: bool,
    /// `after:` operator — inclusive lower bound on `sent_at`.
    pub after: Option<DateTime<Utc>>,
    /// `before:` operator — exclusive upper bound on `sent_at`.
    pub before: Option<DateTime<Utc>>,
    /// Page size. The repository fetches up to `limit + 1` rows so the caller can detect whether
    /// a next page exists; it does NOT truncate or compute `Page::next`/`Page::prev` itself.
    pub limit: u32,
    pub cursor: Option<MessageCursor>,
    pub dir: PageDir,
}

/// A single full-text-search hit: the matched message plus its relevance rank and an optional
/// highlighted snippet. Results are always ordered `(sent_at DESC, id DESC)` — see
/// [`MessageRepository::search_messages_fts`] for why relevance is not the sort key.
#[derive(Debug, Clone, PartialEq)]
pub struct MessageSearchHit {
    pub message: Message,
    /// `ts_rank` score against the query. Informational only — never used for ordering or
    /// pagination, since it is neither unique nor stable across concurrent inserts.
    pub rank: f32,
    /// `ts_headline` snippet with `<mark>...</mark>` highlights around matched terms.
    pub headline: Option<String>,
}

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait MessageRepository: Send + Sync {
    async fn get_by_id(&self, id: MessageId) -> RepoResult<Option<Message>>;

    async fn get_by_ids(&self, ids: &[MessageId]) -> RepoResult<HashMap<MessageId, Message>>;

    async fn create_message(&self, new_message: &NewMessage) -> RepoResult<Message>;

    async fn list_messages(
        &self,
        params: ListMessagesParams,
    ) -> RepoResult<Page<Message, MessageCursor>>;

    /// Full-text search within an explicit set of conversation ids (the caller-computed accessible
    /// set — see [`MessageSearchParams`] for the visibility contract). Always excludes deleted
    /// messages. Ordered `(sent_at DESC, id DESC)` for stable cursor pagination; returns up to
    /// `params.limit + 1` rows so the caller can detect a next page — this method does not
    /// truncate or compute pagination cursors itself, that is the service layer's job.
    async fn search_messages_fts(
        &self,
        params: MessageSearchParams,
    ) -> RepoResult<Vec<MessageSearchHit>>;

    async fn update_message_text(
        &self,
        message_id: MessageId,
        new_text: String,
    ) -> RepoResult<Message>;

    async fn set_has_attachments(
        &self,
        message_id: MessageId,
        has_attachments: bool,
    ) -> RepoResult<Message>;

    async fn soft_delete_message(&self, message_id: MessageId) -> RepoResult<()>;
}
