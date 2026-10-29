//! The notification email digest and retention jobs. The composition root
//! (`rim_server/src/app.rs`) drives them on a timer, assuming a single scheduler node; the jobs themselves are plain async methods behind
//! [`NotificationJobs`] so they are testable without a scheduler.
//!
//! Digest selection: a user with digest `hourly`/`daily` in a workspace gets one email when
//! (1) their oldest unread, un-emailed notification is at least one interval old (60 min /
//! 24 h), (2) their previous digest in that workspace, if any, is at least one interval old, and
//! (3) they have had no presence heartbeat in the workspace for [`ACTIVE_WINDOW_MINUTES`]. The
//! database query applies (1) and (2); [`digest_due`] re-checks them and adds (3).
//!
//! Access is checked again when the digest is composed: a recipient who is no longer a member
//! of the workspace gets nothing, and a notification of a conversation they can no longer read
//! is left out. Both kinds of rows are deleted so they stop qualifying.
//!
//! Digests list messages only: reaction notifications never make a user a candidate, are never
//! listed and are never marked emailed.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};

use rim_core::conversation::model::{Conversation, ConversationId, ConversationType};
use rim_core::conversation::repository::ConversationRepository;
use rim_core::conversation_member::repository::ConversationMemberRepository;
use rim_core::message::model::Message;
use rim_core::message::repository::MessageRepository;
use rim_core::notification::model::{DigestCandidate, NotificationReason, UserNotification};
use rim_core::notification::repository::NotificationRepository;
use rim_core::server::model::ServerId;
use rim_core::server_member::model::ServerMemberRole;
use rim_core::server_member::repository::ServerMemberRepository;
use rim_core::user::model::UserId;
use rim_core::user::repository::UserRepository;
use rim_services::ServiceError;
use rim_services::presence::PresenceService;

use crate::email::EmailSender;
use crate::email::templates::{DigestGroup, DigestItem, notification_digest};
use crate::util::ensure_conversation_access_as;

/// A user seen (presence heartbeat or disconnect) this recently gets no digest.
pub const ACTIVE_WINDOW_MINUTES: i64 = 15;
/// Notifications older than this are deleted by [`NotificationJobs::purge_expired`].
pub const RETENTION_DAYS: i64 = 90;
/// Most notifications one digest email lists; the rest go out with the next digest.
pub const MAX_DIGEST_ITEMS: u32 = 50;
/// Characters of message text shown per digest item.
pub const DIGEST_SNIPPET_CHARS: usize = 80;

/// What one digest run did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DigestRunReport {
    /// `(user, workspace)` pairs the query returned.
    pub candidates: usize,
    /// Emails sent (and their notifications marked emailed).
    pub sent: usize,
    /// Candidates skipped because the user was recently active.
    pub skipped_active: usize,
    /// Emails that failed to send; their notifications stay pending for the next run.
    pub failed: usize,
}

/// The periodic notification jobs.
#[async_trait]
pub trait NotificationJobs: Send + Sync {
    /// Send every digest that is due at `now`.
    async fn run_digest(&self, now: DateTime<Utc>) -> anyhow::Result<DigestRunReport>;

    /// Delete notifications older than [`RETENTION_DAYS`]. Returns rows deleted.
    async fn purge_expired(&self, now: DateTime<Utc>) -> anyhow::Result<u64>;
}

/// The pure digest predicate: whether `candidate` gets a digest at `now`, given when the user
/// was last seen in the workspace (`None`: no presence entry).
pub fn digest_due(
    candidate: &DigestCandidate,
    last_seen_at: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> bool {
    let Some(interval) = candidate.frequency.interval() else {
        return false;
    };
    let cutoff = now - interval;
    candidate.pending_count > 0
        && candidate.oldest_pending_at <= cutoff
        && candidate.last_digest_at.is_none_or(|last| last <= cutoff)
        && last_seen_at.is_none_or(|seen| seen <= now - Duration::minutes(ACTIVE_WINDOW_MINUTES))
}

/// First [`DIGEST_SNIPPET_CHARS`] characters of `text`, with an ellipsis when cut.
pub(crate) fn snippet(text: &str) -> String {
    let flat: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = flat.chars();
    let head: String = chars.by_ref().take(DIGEST_SNIPPET_CHARS).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

/// How a conversation is named in the digest.
fn conversation_label(conversation: Option<&Conversation>) -> String {
    match conversation {
        Some(c) => match (c.conversation_type, c.title.as_deref()) {
            (ConversationType::Channel, Some(title)) => format!("#{title}"),
            (_, Some(title)) if !title.trim().is_empty() => title.to_string(),
            (ConversationType::Direct, _) => "Direct message".to_string(),
            (ConversationType::Group, _) => "Group conversation".to_string(),
            (ConversationType::Channel, None) => "Channel".to_string(),
        },
        None => "Conversation".to_string(),
    }
}

/// The message permalink: the thread path for a reply, the timeline path otherwise.
fn message_link(base_url: Option<&str>, server_id: ServerId, message: &Message) -> Option<String> {
    let base = base_url?.trim_end_matches('/');
    Some(match message.thread_id {
        Some(thread_id) => format!(
            "{base}/workspaces/{server_id}/conversations/{}/threads/{thread_id}/messages/{}",
            message.conversation_id, message.id
        ),
        None => format!(
            "{base}/workspaces/{server_id}/conversations/{}/messages/{}",
            message.conversation_id, message.id
        ),
    })
}

/// Group pending notifications by conversation (first-seen order) with one item per message
/// that still exists and is not deleted. Reaction notifications are skipped: the queries
/// already leave them out, and a digest lists messages only.
pub(crate) fn build_digest_groups(
    server_id: ServerId,
    pending: &[UserNotification],
    messages: &HashMap<rim_core::message::model::MessageId, Message>,
    conversations: &HashMap<ConversationId, Conversation>,
    authors: &HashMap<UserId, String>,
    base_url: Option<&str>,
) -> Vec<DigestGroup> {
    let mut groups: Vec<DigestGroup> = Vec::new();
    let mut index: HashMap<ConversationId, usize> = HashMap::new();
    for notification in pending {
        if notification.reason == NotificationReason::Reaction {
            continue;
        }
        let Some(message) = messages
            .get(&notification.message_id)
            .filter(|m| !m.is_deleted)
        else {
            continue;
        };
        let text = match message.encryption {
            // The server cannot read it; the email names only who sent it.
            Some(_) => format!(
                "Encrypted message from {}",
                authors
                    .get(&message.sent_by)
                    .map_or("a member", String::as_str)
            ),
            None => message
                .text
                .clone()
                .unwrap_or_else(|| "(attachment)".to_string()),
        };
        let item = DigestItem {
            snippet: snippet(&text),
            link: message_link(base_url, server_id, message),
        };
        let slot = *index
            .entry(notification.conversation_id)
            .or_insert_with(|| {
                groups.push(DigestGroup {
                    conversation_label: conversation_label(
                        conversations.get(&notification.conversation_id),
                    ),
                    items: vec![],
                });
                groups.len() - 1
            });
        groups[slot].items.push(item);
    }
    groups
}

#[derive(Clone)]
pub struct NotificationJobRepos {
    pub notification_repo: Arc<dyn NotificationRepository>,
    pub message_repo: Arc<dyn MessageRepository>,
    pub conversation_repo: Arc<dyn ConversationRepository>,
    /// Names the senders of encrypted messages.
    pub user_repo: Arc<dyn UserRepository>,
    /// Whether the recipient is still a member of the workspace, and in which role.
    pub server_member_repo: Arc<dyn ServerMemberRepository>,
    /// Whether the recipient can still read each conversation.
    pub conversation_member_repo: Arc<dyn ConversationMemberRepository>,
}

pub struct DefaultNotificationJobs {
    repos: NotificationJobRepos,
    email_sender: Arc<dyn EmailSender>,
    presence: Arc<dyn PresenceService>,
    public_base_url: Option<String>,
}

impl DefaultNotificationJobs {
    pub fn new(
        repos: NotificationJobRepos,
        email_sender: Arc<dyn EmailSender>,
        presence: Arc<dyn PresenceService>,
        public_base_url: Option<String>,
    ) -> Self {
        Self {
            repos,
            email_sender,
            presence,
            public_base_url,
        }
    }

    /// Conversations among `conversation_ids` the candidate can no longer read under the
    /// conversation access rule as `role`: gone, outside the workspace, or closed to them.
    async fn unreadable_conversations(
        &self,
        candidate: &DigestCandidate,
        role: ServerMemberRole,
        conversation_ids: &[ConversationId],
        conversations: &HashMap<ConversationId, Conversation>,
    ) -> anyhow::Result<HashSet<ConversationId>> {
        let mut unreadable = HashSet::new();
        for id in conversation_ids {
            let readable = match conversations.get(id) {
                Some(c) if c.server_id == candidate.server_id => {
                    match ensure_conversation_access_as(
                        self.repos.conversation_member_repo.as_ref(),
                        c,
                        candidate.user_id,
                        role,
                    )
                    .await
                    {
                        Ok(()) => true,
                        Err(ServiceError::NotFound | ServiceError::PermissionDenied) => false,
                        Err(e) => return Err(e.into()),
                    }
                }
                _ => false,
            };
            if !readable {
                unreadable.insert(*id);
            }
        }
        Ok(unreadable)
    }

    /// Send one candidate's digest. `Ok(true)` when sent, `Ok(false)` when nothing was left to
    /// list (rows of deleted messages are still marked emailed so they stop qualifying; rows the
    /// candidate may no longer read are deleted).
    async fn send_digest(
        &self,
        candidate: &DigestCandidate,
        now: DateTime<Utc>,
    ) -> anyhow::Result<bool> {
        let Some(standing) = self
            .repos
            .server_member_repo
            .get_member_standing(candidate.server_id, candidate.user_id)
            .await?
        else {
            let dropped = self
                .repos
                .notification_repo
                .delete_for_user_in_server(candidate.user_id, candidate.server_id)
                .await?;
            tracing::info!(
                user_id = %candidate.user_id,
                server_id = %candidate.server_id,
                dropped,
                "digest recipient is no longer a workspace member; dropped their notifications"
            );
            return Ok(false);
        };
        let mut pending = self
            .repos
            .notification_repo
            .list_pending_digest(candidate.user_id, candidate.server_id, MAX_DIGEST_ITEMS)
            .await?;
        if pending.is_empty() {
            return Ok(false);
        }
        let mut conversation_ids: Vec<ConversationId> =
            pending.iter().map(|n| n.conversation_id).collect();
        conversation_ids.sort_by_key(|id| uuid::Uuid::from(*id));
        conversation_ids.dedup();
        let conversations: HashMap<ConversationId, Conversation> = self
            .repos
            .conversation_repo
            .get_by_ids(&conversation_ids)
            .await?
            .into_iter()
            .map(|c| (c.id, c))
            .collect();
        let unreadable = self
            .unreadable_conversations(candidate, standing.role, &conversation_ids, &conversations)
            .await?;
        for conversation_id in &unreadable {
            let dropped = self
                .repos
                .notification_repo
                .delete_for_user_in_conversation(candidate.user_id, *conversation_id)
                .await?;
            tracing::info!(
                user_id = %candidate.user_id,
                conversation_id = %conversation_id,
                dropped,
                "digest recipient can no longer read the conversation; dropped their notifications"
            );
        }
        pending.retain(|n| !unreadable.contains(&n.conversation_id));
        if pending.is_empty() {
            return Ok(false);
        }
        let message_ids: Vec<_> = pending.iter().map(|n| n.message_id).collect();
        let messages = self.repos.message_repo.get_by_ids(&message_ids).await?;
        let mut encrypted_authors: Vec<UserId> = messages
            .values()
            .filter(|m| m.encryption.is_some())
            .map(|m| m.sent_by)
            .collect();
        encrypted_authors.sort_by_key(|id| uuid::Uuid::from(*id));
        encrypted_authors.dedup();
        let authors: HashMap<UserId, String> = if encrypted_authors.is_empty() {
            HashMap::new()
        } else {
            self.repos
                .user_repo
                .get_by_ids(&encrypted_authors)
                .await?
                .into_iter()
                .map(|u| (u.id, u.display_name.unwrap_or(u.username)))
                .collect()
        };

        let groups = build_digest_groups(
            candidate.server_id,
            &pending,
            &messages,
            &conversations,
            &authors,
            self.public_base_url.as_deref(),
        );
        let ids: Vec<_> = pending.iter().map(|n| n.id).collect();
        if groups.is_empty() {
            self.repos.notification_repo.mark_emailed(&ids, now).await?;
            return Ok(false);
        }
        let workspace_link = self.public_base_url.as_deref().map(|base| {
            format!(
                "{}/workspaces/{}",
                base.trim_end_matches('/'),
                candidate.server_id
            )
        });
        let email = notification_digest(
            &candidate.email,
            &candidate.server_name,
            &groups,
            workspace_link.as_deref(),
        );
        self.email_sender.send(email).await?;
        self.repos.notification_repo.mark_emailed(&ids, now).await?;
        Ok(true)
    }
}

#[async_trait]
impl NotificationJobs for DefaultNotificationJobs {
    async fn run_digest(&self, now: DateTime<Utc>) -> anyhow::Result<DigestRunReport> {
        let candidates = self
            .repos
            .notification_repo
            .list_digest_candidates(now - Duration::minutes(60), now - Duration::hours(24))
            .await?;
        let mut report = DigestRunReport {
            candidates: candidates.len(),
            ..DigestRunReport::default()
        };
        let mut presence_by_server: HashMap<ServerId, HashMap<UserId, DateTime<Utc>>> =
            HashMap::new();
        for candidate in &candidates {
            let seen = presence_by_server
                .entry(candidate.server_id)
                .or_insert_with(|| {
                    self.presence
                        .get_workspace_presence(candidate.server_id)
                        .into_iter()
                        .map(|p| (p.user_id, p.last_seen_at))
                        .collect()
                })
                .get(&candidate.user_id)
                .copied();
            if !digest_due(candidate, seen, now) {
                report.skipped_active += 1;
                continue;
            }
            match self.send_digest(candidate, now).await {
                Ok(true) => report.sent += 1,
                Ok(false) => {}
                Err(e) => {
                    report.failed += 1;
                    tracing::warn!(
                        error = %e,
                        user_id = %candidate.user_id,
                        server_id = %candidate.server_id,
                        "notification digest failed; will retry next run"
                    );
                }
            }
        }
        Ok(report)
    }

    async fn purge_expired(&self, now: DateTime<Utc>) -> anyhow::Result<u64> {
        Ok(self
            .repos
            .notification_repo
            .delete_created_before(now - Duration::days(RETENTION_DAYS))
            .await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use rim_core::conversation::model::PostingPolicy;
    use rim_core::conversation::repository::MockConversationRepository;
    use rim_core::conversation_member::repository::MockConversationMemberRepository;
    use rim_core::message::model::MessageId;
    use rim_core::message::repository::MockMessageRepository;
    use rim_core::message_thread::model::MessageThreadId;
    use rim_core::notification::model::{DigestFrequency, NotificationReason, UserNotificationId};
    use rim_core::notification::repository::MockNotificationRepository;
    use rim_core::server_member::model::MemberStanding;
    use rim_core::server_member::repository::MockServerMemberRepository;
    use rim_services::presence::{PresenceStatus, UserPresence};

    use crate::email::MockEmailSender;

    fn now() -> DateTime<Utc> {
        chrono::NaiveDate::from_ymd_opt(2026, 5, 1)
            .and_then(|d| d.and_hms_opt(12, 0, 0))
            .map(|dt| dt.and_utc())
            .expect("valid time")
    }

    fn candidate(frequency: DigestFrequency, oldest_minutes_ago: i64) -> DigestCandidate {
        DigestCandidate {
            user_id: UserId::new(),
            server_id: ServerId::new(),
            email: "bob@example.com".to_string(),
            server_name: "Acme".to_string(),
            frequency,
            oldest_pending_at: now() - Duration::minutes(oldest_minutes_ago),
            pending_count: 2,
            last_digest_at: None,
        }
    }

    #[test]
    fn hourly_needs_an_hour_old_notification() {
        assert!(!digest_due(
            &candidate(DigestFrequency::Hourly, 59),
            None,
            now()
        ));
        assert!(digest_due(
            &candidate(DigestFrequency::Hourly, 60),
            None,
            now()
        ));
        assert!(digest_due(
            &candidate(DigestFrequency::Hourly, 600),
            None,
            now()
        ));
    }

    #[test]
    fn daily_needs_a_day() {
        assert!(!digest_due(
            &candidate(DigestFrequency::Daily, 23 * 60),
            None,
            now()
        ));
        assert!(digest_due(
            &candidate(DigestFrequency::Daily, 24 * 60),
            None,
            now()
        ));
    }

    #[test]
    fn off_never_sends() {
        assert!(!digest_due(
            &candidate(DigestFrequency::Off, 10_000),
            None,
            now()
        ));
    }

    #[test]
    fn previous_digest_must_be_an_interval_old() {
        let mut c = candidate(DigestFrequency::Hourly, 120);
        c.last_digest_at = Some(now() - Duration::minutes(30));
        assert!(!digest_due(&c, None, now()));
        c.last_digest_at = Some(now() - Duration::minutes(60));
        assert!(digest_due(&c, None, now()));
        let mut d = candidate(DigestFrequency::Daily, 48 * 60);
        d.last_digest_at = Some(now() - Duration::hours(23));
        assert!(!digest_due(&d, None, now()));
    }

    #[test]
    fn recent_presence_skips_the_digest() {
        let c = candidate(DigestFrequency::Hourly, 120);
        assert!(!digest_due(&c, Some(now() - Duration::minutes(5)), now()));
        assert!(!digest_due(&c, Some(now() - Duration::minutes(14)), now()));
        assert!(digest_due(&c, Some(now() - Duration::minutes(15)), now()));
        assert!(digest_due(&c, Some(now() - Duration::hours(3)), now()));
    }

    #[test]
    fn nothing_pending_never_sends() {
        let mut c = candidate(DigestFrequency::Hourly, 120);
        c.pending_count = 0;
        assert!(!digest_due(&c, None, now()));
    }

    #[test]
    fn snippet_cuts_at_eighty_characters_and_flattens_whitespace() {
        assert_eq!(snippet("short\n\ntext"), "short text");
        let long = "é".repeat(100);
        let s = snippet(&long);
        assert_eq!(s.chars().count(), 81);
        assert!(s.ends_with('…'));
        assert_eq!(snippet(&"a".repeat(80)), "a".repeat(80));
    }

    fn message(conversation_id: ConversationId, text: &str, deleted: bool) -> Message {
        Message {
            id: MessageId::new(),
            conversation_id,
            thread_id: None,
            parent_message_id: None,
            forwarded_from_message_id: None,
            sent_at: now(),
            edited_at: None,
            sent_by: UserId::new(),
            text: Some(text.to_string()),
            is_deleted: deleted,
            has_attachments: false,
            encryption: None,
            flagged_at: None,
        }
    }

    fn notification(user_id: UserId, server_id: ServerId, m: &Message) -> UserNotification {
        UserNotification {
            id: UserNotificationId::new(),
            user_id,
            server_id,
            conversation_id: m.conversation_id,
            message_id: m.id,
            reason: NotificationReason::Mention,
            created_at: now() - Duration::hours(2),
            read_at: None,
            emailed_at: None,
            reaction: None,
        }
    }

    fn channel(server_id: ServerId, id: ConversationId, title: &str) -> Conversation {
        Conversation {
            id,
            conversation_type: ConversationType::Channel,
            title: Some(title.to_string()),
            topic: None,
            created_at: now(),
            last_activity_at: now(),
            server_id,
            is_private: false,
            archived_at: None,
            created_by: None,
            slow_mode_seconds: 0,
            posting_policy: PostingPolicy::Everyone,
            posting_allows_threads: true,
            encrypted_at: None,
        }
    }

    #[test]
    fn groups_by_conversation_and_skips_deleted_messages() {
        let server = ServerId::new();
        let user = UserId::new();
        let general = ConversationId::new();
        let random = ConversationId::new();
        let m1 = message(general, "first", false);
        let m2 = message(random, "second", false);
        let m3 = message(general, "third", false);
        let gone = message(general, "deleted", true);
        let mut thread_msg = message(random, "in thread", false);
        thread_msg.thread_id = Some(MessageThreadId::new());
        let pending: Vec<_> = [&m1, &m2, &m3, &gone, &thread_msg]
            .iter()
            .map(|m| notification(user, server, m))
            .collect();
        let messages: HashMap<_, _> = [&m1, &m2, &m3, &gone, &thread_msg]
            .into_iter()
            .map(|m| (m.id, m.clone()))
            .collect();
        let conversations: HashMap<_, _> = [channel(server, general, "general")]
            .into_iter()
            .map(|c| (c.id, c))
            .collect();
        let groups = build_digest_groups(
            server,
            &pending,
            &messages,
            &conversations,
            &HashMap::new(),
            Some("https://rim.example.com/"),
        );
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].conversation_label, "#general");
        assert_eq!(groups[0].items.len(), 2);
        assert_eq!(groups[0].items[0].snippet, "first");
        assert_eq!(
            groups[0].items[0].link.as_deref(),
            Some(
                format!(
                    "https://rim.example.com/workspaces/{server}/conversations/{general}/messages/{}",
                    m1.id
                )
                .as_str()
            )
        );
        assert_eq!(groups[1].conversation_label, "Conversation");
        assert!(
            groups[1].items[1]
                .link
                .as_deref()
                .is_some_and(|l| l.contains("/threads/"))
        );
        let without_base = build_digest_groups(
            server,
            &pending,
            &messages,
            &conversations,
            &HashMap::new(),
            None,
        );
        assert!(without_base[0].items[0].link.is_none());
    }

    #[test]
    fn reaction_notifications_are_left_out_of_the_digest() {
        let server = ServerId::new();
        let user = UserId::new();
        let general = ConversationId::new();
        let reacted = message(general, "reacted to", false);
        let mentioned = message(general, "mentioned", false);
        let reaction = UserNotification {
            reason: NotificationReason::Reaction,
            reaction: Some(rim_core::notification::model::NotificationReaction {
                emoji: "\u{1f44d}".to_string(),
                by: UserId::new(),
            }),
            ..notification(user, server, &reacted)
        };
        let messages: HashMap<_, _> = [&reacted, &mentioned]
            .into_iter()
            .map(|m| (m.id, m.clone()))
            .collect();
        let groups = build_digest_groups(
            server,
            &[reaction.clone(), notification(user, server, &mentioned)],
            &messages,
            &HashMap::new(),
            &HashMap::new(),
            None,
        );
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].items.len(), 1);
        assert_eq!(groups[0].items[0].snippet, "mentioned");
        let only_reactions = build_digest_groups(
            server,
            &[reaction],
            &messages,
            &HashMap::new(),
            &HashMap::new(),
            None,
        );
        assert!(only_reactions.is_empty());
    }

    /// The jobs' collaborators; each test sets the expectations it relies on.
    #[derive(Default)]
    struct Mocks {
        notification: MockNotificationRepository,
        message: MockMessageRepository,
        conversation: MockConversationRepository,
        server_member: MockServerMemberRepository,
        conversation_member: MockConversationMemberRepository,
        email: MockEmailSender,
    }

    impl Mocks {
        /// Every candidate is a workspace member with `role`.
        fn member_as(mut self, role: ServerMemberRole) -> Self {
            self.server_member
                .expect_get_member_standing()
                .returning(move |_, _| Ok(Some(MemberStanding::new(role))));
            self
        }

        /// The candidate belongs to exactly the conversations in `joined`.
        fn joined(mut self, joined: Vec<ConversationId>) -> Self {
            self.conversation_member
                .expect_is_member()
                .returning(move |conversation_id, _| {
                    let is = joined.contains(&conversation_id);
                    Box::pin(async move { Ok(is) })
                });
            self
        }

        /// `get_by_ids` answers with `conversations` (missing ids are absent).
        fn conversations(mut self, conversations: Vec<Conversation>) -> Self {
            self.conversation.expect_get_by_ids().returning(move |ids| {
                let found: Vec<_> = conversations
                    .iter()
                    .filter(|c| ids.contains(&c.id))
                    .cloned()
                    .collect();
                Box::pin(async move { Ok(found) })
            });
            self
        }

        /// `list_pending_digest` answers with `pending`.
        fn pending(mut self, pending: Vec<UserNotification>) -> Self {
            self.notification
                .expect_list_pending_digest()
                .withf(|_, _, limit| *limit == MAX_DIGEST_ITEMS)
                .returning(move |_, _, _| {
                    let p = pending.clone();
                    Box::pin(async move { Ok(p) })
                });
            self
        }

        /// `list_digest_candidates` answers with `candidates`.
        fn candidates(mut self, candidates: Vec<DigestCandidate>) -> Self {
            self.notification
                .expect_list_digest_candidates()
                .returning(move |_, _| {
                    let c = candidates.clone();
                    Box::pin(async move { Ok(c) })
                });
            self
        }

        /// `get_by_ids` answers with whichever of `messages` were asked for.
        fn messages(mut self, messages: Vec<Message>) -> Self {
            self.message.expect_get_by_ids().returning(move |ids| {
                let found: HashMap<_, _> = messages
                    .iter()
                    .filter(|m| ids.contains(&m.id))
                    .map(|m| (m.id, m.clone()))
                    .collect();
                Box::pin(async move { Ok(found) })
            });
            self
        }

        fn jobs(self, presence: Vec<UserPresence>) -> DefaultNotificationJobs {
            struct FixedPresence(Vec<UserPresence>);
            impl PresenceService for FixedPresence {
                fn heartbeat(&self, _: UserId, _: ServerId, _: PresenceStatus) {}
                fn get_workspace_presence(&self, _: ServerId) -> Vec<UserPresence> {
                    self.0.clone()
                }
                fn mark_offline(&self, _: UserId) {}
            }
            DefaultNotificationJobs::new(
                NotificationJobRepos {
                    notification_repo: Arc::new(self.notification),
                    message_repo: Arc::new(self.message),
                    conversation_repo: Arc::new(self.conversation),
                    user_repo: Arc::new(rim_core::user::repository::MockUserRepository::new()),
                    server_member_repo: Arc::new(self.server_member),
                    conversation_member_repo: Arc::new(self.conversation_member),
                },
                Arc::new(self.email),
                Arc::new(FixedPresence(presence)),
                Some("https://rim.example.com".to_string()),
            )
        }
    }

    fn private_channel(server_id: ServerId, title: &str) -> Conversation {
        Conversation {
            is_private: true,
            ..channel(server_id, ConversationId::new(), title)
        }
    }

    fn direct(server_id: ServerId) -> Conversation {
        Conversation {
            conversation_type: ConversationType::Direct,
            title: None,
            ..channel(server_id, ConversationId::new(), "")
        }
    }

    fn report(candidates: usize, sent: usize) -> DigestRunReport {
        DigestRunReport {
            candidates,
            sent,
            ..DigestRunReport::default()
        }
    }

    #[tokio::test]
    async fn run_digest_sends_and_marks_emailed() {
        let c = candidate(DigestFrequency::Hourly, 120);
        let ops = channel(c.server_id, ConversationId::new(), "ops");
        let m = message(ops.id, "deploy is <b>broken</b>", false);
        let pending = vec![notification(c.user_id, c.server_id, &m)];
        let pending_ids: Vec<_> = pending.iter().map(|n| n.id).collect();

        let mut mocks = Mocks::default()
            .member_as(ServerMemberRole::Member)
            .conversations(vec![ops])
            .messages(vec![m])
            .pending(pending);
        let cand = c.clone();
        mocks
            .notification
            .expect_list_digest_candidates()
            .withf(|hourly, daily| {
                *hourly == now() - Duration::minutes(60) && *daily == now() - Duration::hours(24)
            })
            .returning(move |_, _| {
                let cand = cand.clone();
                Box::pin(async move { Ok(vec![cand]) })
            });
        mocks
            .notification
            .expect_mark_emailed()
            .withf(move |ids, at| ids == pending_ids.as_slice() && *at == now())
            .times(1)
            .returning(|ids, _| {
                let n = ids.len() as u64;
                Box::pin(async move { Ok(n) })
            });
        mocks
            .email
            .expect_send()
            .withf(|msg| {
                msg.to == "bob@example.com"
                    && msg.subject.contains("Acme")
                    && !msg.subject.contains("deploy")
                    && msg.body_text.contains("#ops")
                    && msg
                        .body_html
                        .as_deref()
                        .is_some_and(|h| h.contains("&lt;b&gt;broken&lt;/b&gt;"))
            })
            .times(1)
            .returning(|_| Ok(()));

        let report = mocks.jobs(vec![]).run_digest(now()).await.expect("run");
        assert_eq!(report, self::report(1, 1));
    }

    #[tokio::test]
    async fn run_digest_skips_active_users_and_keeps_failures_pending() {
        let active = candidate(DigestFrequency::Hourly, 120);
        let failing = candidate(DigestFrequency::Hourly, 120);
        let general = channel(failing.server_id, ConversationId::new(), "general");
        let m = message(general.id, "hello", false);
        let pending = vec![notification(failing.user_id, failing.server_id, &m)];

        let mut mocks = Mocks::default()
            .member_as(ServerMemberRole::Member)
            .candidates(vec![active.clone(), failing.clone()])
            .conversations(vec![general])
            .messages(vec![m]);
        let failing_user = failing.user_id;
        mocks
            .notification
            .expect_list_pending_digest()
            .withf(move |user, _, _| *user == failing_user)
            .times(1)
            .returning(move |_, _, _| {
                let p = pending.clone();
                Box::pin(async move { Ok(p) })
            });
        mocks.notification.expect_mark_emailed().never();
        mocks
            .email
            .expect_send()
            .times(1)
            .returning(|_| Err(anyhow::anyhow!("smtp down")));

        let presence = vec![UserPresence {
            user_id: active.user_id,
            status: PresenceStatus::Online,
            last_seen_at: now() - Duration::minutes(2),
        }];
        let report = mocks.jobs(presence).run_digest(now()).await.expect("run");
        assert_eq!(
            report,
            DigestRunReport {
                candidates: 2,
                sent: 0,
                skipped_active: 1,
                failed: 1
            }
        );
    }

    #[tokio::test]
    async fn kicked_member_gets_no_digest_and_their_rows_stop_qualifying() {
        let c = candidate(DigestFrequency::Hourly, 120);
        let mut mocks = Mocks::default().candidates(vec![c.clone()]);
        mocks
            .server_member
            .expect_get_member_standing()
            .returning(|_, _| Ok(None));
        let (user, server) = (c.user_id, c.server_id);
        mocks
            .notification
            .expect_delete_for_user_in_server()
            .withf(move |u, s| *u == user && *s == server)
            .times(1)
            .returning(|_, _| Box::pin(async { Ok(2) }));
        mocks.notification.expect_list_pending_digest().never();
        mocks.notification.expect_mark_emailed().never();
        mocks.message.expect_get_by_ids().never();
        mocks.email.expect_send().never();

        let report = mocks.jobs(vec![]).run_digest(now()).await.expect("run");
        assert_eq!(report, self::report(1, 0));
    }

    #[tokio::test]
    async fn removal_from_a_private_conversation_drops_only_its_items() {
        let c = candidate(DigestFrequency::Hourly, 120);
        let general = channel(c.server_id, ConversationId::new(), "general");
        let secret = private_channel(c.server_id, "secret");
        let dm = direct(c.server_id);
        let in_general = message(general.id, "standup moved", false);
        let in_secret = message(secret.id, "salary bands", false);
        let in_dm = message(dm.id, "lunch?", false);
        let pending: Vec<_> = [&in_general, &in_secret, &in_dm]
            .into_iter()
            .map(|m| notification(c.user_id, c.server_id, m))
            .collect();
        let kept: Vec<_> = [&pending[0], &pending[2]].map(|n| n.id).to_vec();
        let (secret_id, readable_ids) = (secret.id, [in_general.id, in_dm.id]);

        let mut mocks = Mocks::default()
            .member_as(ServerMemberRole::Member)
            .joined(vec![dm.id])
            .candidates(vec![c.clone()])
            .conversations(vec![general, secret, dm])
            .pending(pending);
        let user = c.user_id;
        mocks
            .notification
            .expect_delete_for_user_in_conversation()
            .withf(move |u, conversation| *u == user && *conversation == secret_id)
            .times(1)
            .returning(|_, _| Box::pin(async { Ok(1) }));
        mocks
            .notification
            .expect_mark_emailed()
            .withf(move |ids, _| ids == kept.as_slice())
            .times(1)
            .returning(|ids, _| {
                let n = ids.len() as u64;
                Box::pin(async move { Ok(n) })
            });
        let messages = [in_general, in_secret, in_dm];
        mocks
            .message
            .expect_get_by_ids()
            .withf(move |ids| ids == readable_ids.as_slice())
            .times(1)
            .returning(move |ids| {
                let found: HashMap<_, _> = messages
                    .iter()
                    .filter(|m| ids.contains(&m.id))
                    .map(|m| (m.id, m.clone()))
                    .collect();
                Box::pin(async move { Ok(found) })
            });
        mocks
            .email
            .expect_send()
            .withf(|msg| {
                msg.body_text.contains("standup moved")
                    && msg.body_text.contains("lunch?")
                    && !msg.body_text.contains("salary")
                    && !msg.body_text.contains("#secret")
            })
            .times(1)
            .returning(|_| Ok(()));

        let report = mocks.jobs(vec![]).run_digest(now()).await.expect("run");
        assert_eq!(report, self::report(1, 1));
    }

    #[tokio::test]
    async fn edit_after_losing_access_is_never_loaded_or_emailed() {
        let c = candidate(DigestFrequency::Hourly, 120);
        // A guest reads a public channel only once added to it, like a private one.
        let lobby = channel(c.server_id, ConversationId::new(), "lobby");
        let secret = private_channel(c.server_id, "secret");
        let elsewhere = channel(ServerId::new(), ConversationId::new(), "elsewhere");
        let gone = ConversationId::new();
        let edited = [
            message(lobby.id, "edited after removal", false),
            message(secret.id, "edited after removal", false),
            message(elsewhere.id, "edited after removal", false),
            message(gone, "edited after removal", false),
        ];
        let pending: Vec<_> = edited
            .iter()
            .map(|m| notification(c.user_id, c.server_id, m))
            .collect();
        let mut expected_dropped = vec![lobby.id, secret.id, elsewhere.id, gone];
        expected_dropped.sort_by_key(|id| uuid::Uuid::from(*id));

        let mut mocks = Mocks::default()
            .member_as(ServerMemberRole::Guest)
            .joined(vec![])
            .candidates(vec![c.clone()])
            .conversations(vec![lobby, secret, elsewhere])
            .pending(pending);
        let dropped = Arc::new(std::sync::Mutex::new(Vec::new()));
        let record = Arc::clone(&dropped);
        let user = c.user_id;
        mocks
            .notification
            .expect_delete_for_user_in_conversation()
            .withf(move |u, _| *u == user)
            .times(4)
            .returning(move |_, conversation| {
                record.lock().expect("lock").push(conversation);
                Box::pin(async { Ok(1) })
            });
        mocks.notification.expect_mark_emailed().never();
        mocks.message.expect_get_by_ids().never();
        mocks.email.expect_send().never();

        let report = mocks.jobs(vec![]).run_digest(now()).await.expect("run");
        assert_eq!(report, self::report(1, 0));
        let mut dropped = dropped.lock().expect("lock").clone();
        dropped.sort_by_key(|id| uuid::Uuid::from(*id));
        assert_eq!(dropped, expected_dropped);
    }

    #[tokio::test]
    async fn purge_deletes_rows_older_than_ninety_days() {
        let mut mocks = Mocks::default();
        mocks
            .notification
            .expect_delete_created_before()
            .withf(|cutoff| *cutoff == now() - Duration::days(90))
            .times(1)
            .returning(|_| Box::pin(async { Ok(7) }));
        let deleted = mocks
            .jobs(vec![])
            .purge_expired(now())
            .await
            .expect("purge");
        assert_eq!(deleted, 7);
    }
}
