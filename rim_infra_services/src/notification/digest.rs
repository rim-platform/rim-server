//! The notification email digest and retention jobs. The composition root
//! (`rim_server/src/app.rs`) drives them on a timer, assuming a single scheduler node; the jobs themselves are plain async methods behind
//! [`NotificationJobs`] so they are testable without a scheduler.
//!
//! Digest selection: a user with digest `hourly`/`daily` in a workspace gets one email when
//! (1) their oldest unread, un-emailed notification is at least one interval old (60 min /
//! 24 h), (2) their previous digest in that workspace, if any, is at least one interval old, and
//! (3) they have had no presence heartbeat in the workspace for [`ACTIVE_WINDOW_MINUTES`]. The
//! database query applies (1) and (2); [`digest_due`] re-checks them and adds (3).

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};

use rim_core::conversation::model::{Conversation, ConversationId, ConversationType};
use rim_core::conversation::repository::ConversationRepository;
use rim_core::message::model::Message;
use rim_core::message::repository::MessageRepository;
use rim_core::notification::model::{DigestCandidate, UserNotification};
use rim_core::notification::repository::NotificationRepository;
use rim_core::server::model::ServerId;
use rim_core::user::model::UserId;
use rim_core::user::repository::UserRepository;
use rim_services::presence::PresenceService;

use crate::email::EmailSender;
use crate::email::templates::{DigestGroup, DigestItem, notification_digest};

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
/// that still exists and is not deleted.
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

    /// Send one candidate's digest. `Ok(true)` when sent, `Ok(false)` when nothing was left to
    /// list (rows are still marked emailed so they stop qualifying).
    async fn send_digest(
        &self,
        candidate: &DigestCandidate,
        now: DateTime<Utc>,
    ) -> anyhow::Result<bool> {
        let pending = self
            .repos
            .notification_repo
            .list_pending_digest(candidate.user_id, candidate.server_id, MAX_DIGEST_ITEMS)
            .await?;
        if pending.is_empty() {
            return Ok(false);
        }
        let message_ids: Vec<_> = pending.iter().map(|n| n.message_id).collect();
        let messages = self.repos.message_repo.get_by_ids(&message_ids).await?;
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

    use rim_core::conversation::repository::MockConversationRepository;
    use rim_core::message::model::MessageId;
    use rim_core::message::repository::MockMessageRepository;
    use rim_core::message_thread::model::MessageThreadId;
    use rim_core::notification::model::{DigestFrequency, NotificationReason, UserNotificationId};
    use rim_core::notification::repository::MockNotificationRepository;
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
            sent_by: UserId::new(),
            text: Some(text.to_string()),
            is_deleted: deleted,
            has_attachments: false,
            encryption: None,
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
        }
    }

    fn channel(id: ConversationId, title: &str) -> Conversation {
        Conversation {
            id,
            conversation_type: ConversationType::Channel,
            title: Some(title.to_string()),
            topic: None,
            created_at: now(),
            last_activity_at: now(),
            server_id: ServerId::new(),
            is_private: false,
            archived_at: None,
            created_by: None,
            slow_mode_seconds: 0,
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
        let conversations: HashMap<_, _> = [channel(general, "general")]
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

    fn jobs(
        notification_repo: MockNotificationRepository,
        message_repo: MockMessageRepository,
        conversation_repo: MockConversationRepository,
        email: MockEmailSender,
        presence: Vec<UserPresence>,
    ) -> DefaultNotificationJobs {
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
                notification_repo: Arc::new(notification_repo),
                message_repo: Arc::new(message_repo),
                conversation_repo: Arc::new(conversation_repo),
                user_repo: Arc::new(rim_core::user::repository::MockUserRepository::new()),
            },
            Arc::new(email),
            Arc::new(FixedPresence(presence)),
            Some("https://rim.example.com".to_string()),
        )
    }

    #[tokio::test]
    async fn run_digest_sends_and_marks_emailed() {
        let c = candidate(DigestFrequency::Hourly, 120);
        let general = ConversationId::new();
        let m = message(general, "deploy is <b>broken</b>", false);
        let pending = vec![notification(c.user_id, c.server_id, &m)];
        let pending_ids: Vec<_> = pending.iter().map(|n| n.id).collect();

        let mut repo = MockNotificationRepository::new();
        let cand = c.clone();
        repo.expect_list_digest_candidates()
            .withf(|hourly, daily| {
                *hourly == now() - Duration::minutes(60) && *daily == now() - Duration::hours(24)
            })
            .returning(move |_, _| {
                let cand = cand.clone();
                Box::pin(async move { Ok(vec![cand]) })
            });
        let pending_clone = pending.clone();
        repo.expect_list_pending_digest()
            .withf(|_, _, limit| *limit == MAX_DIGEST_ITEMS)
            .returning(move |_, _, _| {
                let p = pending_clone.clone();
                Box::pin(async move { Ok(p) })
            });
        repo.expect_mark_emailed()
            .withf(move |ids, at| ids == pending_ids.as_slice() && *at == now())
            .times(1)
            .returning(|ids, _| {
                let n = ids.len() as u64;
                Box::pin(async move { Ok(n) })
            });
        let mut messages = MockMessageRepository::new();
        let m_clone = m.clone();
        messages.expect_get_by_ids().returning(move |_| {
            let m = m_clone.clone();
            Box::pin(async move { Ok([(m.id, m)].into_iter().collect()) })
        });
        let mut conversations = MockConversationRepository::new();
        conversations.expect_get_by_ids().returning(move |_| {
            let ch = channel(general, "ops");
            Box::pin(async move { Ok(vec![ch]) })
        });
        let mut email = MockEmailSender::new();
        email
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

        let report = jobs(repo, messages, conversations, email, vec![])
            .run_digest(now())
            .await
            .expect("run");
        assert_eq!(
            report,
            DigestRunReport {
                candidates: 1,
                sent: 1,
                skipped_active: 0,
                failed: 0
            }
        );
    }

    #[tokio::test]
    async fn run_digest_skips_active_users_and_keeps_failures_pending() {
        let active = candidate(DigestFrequency::Hourly, 120);
        let failing = candidate(DigestFrequency::Hourly, 120);
        let general = ConversationId::new();
        let m = message(general, "hello", false);
        let pending = vec![notification(failing.user_id, failing.server_id, &m)];

        let mut repo = MockNotificationRepository::new();
        let both = vec![active.clone(), failing.clone()];
        repo.expect_list_digest_candidates().returning(move |_, _| {
            let both = both.clone();
            Box::pin(async move { Ok(both) })
        });
        let failing_user = failing.user_id;
        repo.expect_list_pending_digest()
            .withf(move |user, _, _| *user == failing_user)
            .times(1)
            .returning(move |_, _, _| {
                let p = pending.clone();
                Box::pin(async move { Ok(p) })
            });
        repo.expect_mark_emailed().never();
        let mut messages = MockMessageRepository::new();
        messages.expect_get_by_ids().returning(move |_| {
            let m = m.clone();
            Box::pin(async move { Ok([(m.id, m)].into_iter().collect()) })
        });
        let mut conversations = MockConversationRepository::new();
        conversations
            .expect_get_by_ids()
            .returning(|_| Box::pin(async { Ok(vec![]) }));
        let mut email = MockEmailSender::new();
        email
            .expect_send()
            .times(1)
            .returning(|_| Err(anyhow::anyhow!("smtp down")));

        let presence = vec![UserPresence {
            user_id: active.user_id,
            status: PresenceStatus::Online,
            last_seen_at: now() - Duration::minutes(2),
        }];
        let report = jobs(repo, messages, conversations, email, presence)
            .run_digest(now())
            .await
            .expect("run");
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
    async fn purge_deletes_rows_older_than_ninety_days() {
        let mut repo = MockNotificationRepository::new();
        repo.expect_delete_created_before()
            .withf(|cutoff| *cutoff == now() - Duration::days(90))
            .times(1)
            .returning(|_| Box::pin(async { Ok(7) }));
        let deleted = jobs(
            repo,
            MockMessageRepository::new(),
            MockConversationRepository::new(),
            MockEmailSender::new(),
            vec![],
        )
        .purge_expired(now())
        .await
        .expect("purge");
        assert_eq!(deleted, 7);
    }
}
