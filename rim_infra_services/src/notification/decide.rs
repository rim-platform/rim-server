//! The per-message notification decision: a pure function, no I/O. The async
//! wrapper in `service.rs` loads its inputs and stores what it returns.
//!
//! Rules, per member except the author:
//! 1. Only conversation members are candidates (the caller passes the member list; banned,
//!    removed and non-members are not in it). A timeout does not matter for receiving.
//! 2. A timed mute in effect (`muted_until > now`) yields nothing.
//! 3. Effective level = the conversation override, else the workspace default. `NONE` yields
//!    nothing, mentions and direct messages included.
//! 4. Reason = the strongest that applies: direct conversation, then a mention (`<@user>`,
//!    `@channel`, `@here`), then a keyword hit, then a reply in a thread the user started or
//!    replied in, else `all`.
//! 5. `MENTIONS` admits every reason but `all`; `ALL` admits everything.
//! 6. Inside the user's do-not-disturb window or pause the notification is still recorded, but
//!    marked `quiet`.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};

use rim_core::conversation::model::{Conversation, ConversationType};
use rim_core::message::model::Message;
use rim_core::notification::model::{NotificationReason, UserNotificationSettings};
use rim_core::notification_preference::model::NotificationPreference;
use rim_core::user::model::UserId;

use rim_services::notification::command::EncryptedMentions;

use crate::mention::parse_mention_ids;

/// Tokens that mention everyone in the conversation (whole word, case-insensitive).
const CHANNEL_MENTION_TOKENS: [&str; 2] = ["@channel", "@here"];

/// Everything [`decide`] needs, loaded by the caller.
pub(crate) struct DecisionInput<'a> {
    pub message: &'a Message,
    pub conversation: &'a Conversation,
    /// Root author and repliers of the thread `message` replies in; empty for a timeline
    /// message.
    pub thread_participants: &'a HashSet<UserId>,
    /// The conversation's members (the candidates).
    pub members: &'a [UserId],
    pub prefs_by_user: &'a HashMap<UserId, NotificationPreference>,
    /// Users without an entry have the default settings.
    pub settings_by_user: &'a HashMap<UserId, UserNotificationSettings>,
    pub now: DateTime<Utc>,
    /// The declared mentions of an encrypted message, which has no text to read them from;
    /// keywords never match it.
    pub encrypted_mentions: Option<&'a EncryptedMentions>,
}

/// One recipient of a notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Decision {
    pub user_id: UserId,
    pub reason: NotificationReason,
    /// Inside the user's do-not-disturb window or pause: recorded, but no toast or sound.
    pub quiet: bool,
}

/// Who is notified about `input.message`, and why. One entry per recipient, in member order.
pub(crate) fn decide(input: &DecisionInput<'_>) -> Vec<Decision> {
    let message = input.message;
    if message.is_deleted {
        return vec![];
    }
    let text = message.text.as_deref().unwrap_or("");
    let (mentioned, plain_lower, channel_mention) = match input.encrypted_mentions {
        Some(mentions) => (
            mentions.user_ids.iter().copied().collect::<HashSet<_>>(),
            String::new(),
            mentions.all,
        ),
        None => {
            // Keywords and channel mentions are matched outside `<@uuid>` tokens, so a keyword
            // never hits inside a mentioned user's id.
            let plain_lower = strip_mention_tokens(text).to_lowercase();
            let channel_mention = CHANNEL_MENTION_TOKENS
                .iter()
                .any(|token| contains_whole_word(&plain_lower, token));
            (
                parse_mention_ids(text).into_iter().collect(),
                plain_lower,
                channel_mention,
            )
        }
    };
    let is_direct = input.conversation.conversation_type == ConversationType::Direct;

    let mut seen = HashSet::new();
    let mut decisions = Vec::new();
    for &user_id in input.members {
        if user_id == message.sent_by || !seen.insert(user_id) {
            continue;
        }
        let pref = input.prefs_by_user.get(&user_id);
        if pref.is_some_and(|p| p.is_timed_muted_at(input.now)) {
            continue;
        }
        let settings = input.settings_by_user.get(&user_id);
        let level = pref
            .and_then(|p| p.level)
            .or(settings.map(|s| s.default_level))
            .unwrap_or(rim_core::notification::model::NotificationLevel::All);

        let reason = if is_direct {
            NotificationReason::Direct
        } else if channel_mention || mentioned.contains(&user_id) {
            NotificationReason::Mention
        } else if input.encrypted_mentions.is_none()
            && settings.is_some_and(|s| {
                s.keywords
                    .iter()
                    .any(|k| contains_whole_word(&plain_lower, &k.to_lowercase()))
            })
        {
            NotificationReason::Keyword
        } else if message.thread_id.is_some() && input.thread_participants.contains(&user_id) {
            NotificationReason::ThreadReply
        } else {
            NotificationReason::All
        };

        if !level.admits(reason) {
            continue;
        }
        decisions.push(Decision {
            user_id,
            reason,
            quiet: settings.is_some_and(|s| s.is_quiet_at(input.now)),
        });
    }
    decisions
}

/// `text` with every `<@...>` mention token removed (replaced by a space).
fn strip_mention_tokens(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("<@") {
        out.push_str(&rest[..start]);
        match rest[start..].find('>') {
            Some(end) => {
                out.push(' ');
                rest = &rest[start + end + 1..];
            }
            None => {
                out.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Whether `needle` occurs in `haystack` with no word character directly before or after it.
/// Both are expected lower-cased already; an empty needle never matches.
pub(crate) fn contains_whole_word(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return false;
    }
    let mut from = 0;
    while let Some(pos) = haystack[from..].find(needle) {
        let start = from + pos;
        let end = start + needle.len();
        let before_ok = haystack[..start]
            .chars()
            .next_back()
            .is_none_or(|c| !is_word_char(c));
        let after_ok = haystack[end..]
            .chars()
            .next()
            .is_none_or(|c| !is_word_char(c));
        if before_ok && after_ok {
            return true;
        }
        // Step past the first character of this occurrence (char-boundary safe).
        from = start
            + haystack[start..]
                .chars()
                .next()
                .map(char::len_utf8)
                .unwrap_or(1);
        if from >= haystack.len() {
            break;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    use chrono::{Duration, NaiveTime};
    use rim_core::conversation::model::ConversationId;
    use rim_core::message::model::MessageId;
    use rim_core::message_thread::model::MessageThreadId;
    use rim_core::notification::model::{DndPause, DndWindow, NotificationLevel};
    use rim_core::server::model::ServerId;

    use NotificationLevel as L;
    use NotificationReason as R;

    fn now() -> DateTime<Utc> {
        chrono::NaiveDate::from_ymd_opt(2026, 3, 10)
            .and_then(|d| d.and_hms_opt(12, 0, 0))
            .map(|dt| dt.and_utc())
            .expect("valid time")
    }

    fn conversation(kind: ConversationType) -> Conversation {
        Conversation {
            id: ConversationId::new(),
            conversation_type: kind,
            title: Some("general".to_string()),
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

    fn message(author: UserId, text: &str, thread: bool) -> Message {
        Message {
            id: MessageId::new(),
            conversation_id: ConversationId::new(),
            thread_id: thread.then(MessageThreadId::new),
            parent_message_id: None,
            forwarded_from_message_id: None,
            sent_at: now(),
            sent_by: author,
            text: Some(text.to_string()),
            is_deleted: false,
            has_attachments: false,
            encryption: None,
        }
    }

    fn settings(
        user: UserId,
        level: NotificationLevel,
        keywords: &[String],
    ) -> UserNotificationSettings {
        UserNotificationSettings {
            default_level: level,
            keywords: keywords.to_vec(),
            ..UserNotificationSettings::defaults(user, ServerId::new(), now())
        }
    }

    fn pref(user: UserId, level: Option<NotificationLevel>) -> NotificationPreference {
        NotificationPreference {
            user_id: user,
            conversation_id: ConversationId::new(),
            muted_until: None,
            level,
        }
    }

    /// The decision for one recipient `bob` of a message by `alice`, or `None`.
    struct Case {
        kind: ConversationType,
        text: String,
        thread: bool,
        bob_in_thread: bool,
        default_level: Option<NotificationLevel>,
        override_level: Option<NotificationLevel>,
        keywords: Vec<String>,
        muted_until: Option<DateTime<Utc>>,
        dnd: Option<DndWindow>,
        pause: DndPause,
    }

    impl Default for Case {
        fn default() -> Self {
            Self {
                kind: ConversationType::Channel,
                text: "hello there".to_string(),
                thread: false,
                bob_in_thread: false,
                default_level: None,
                override_level: None,
                keywords: vec![],
                muted_until: None,
                dnd: None,
                pause: DndPause::Off,
            }
        }
    }

    fn run(case: Case, bob: UserId) -> Option<Decision> {
        let alice = UserId::new();
        let conv = conversation(case.kind);
        let msg = message(alice, &case.text, case.thread);
        let thread_participants: HashSet<UserId> = if case.bob_in_thread {
            [alice, bob].into_iter().collect()
        } else {
            [alice].into_iter().collect()
        };
        let mut prefs = HashMap::new();
        if case.override_level.is_some() || case.muted_until.is_some() {
            let mut p = pref(bob, case.override_level);
            p.muted_until = case.muted_until;
            prefs.insert(bob, p);
        }
        let mut settings_by_user = HashMap::new();
        if case.default_level.is_some()
            || !case.keywords.is_empty()
            || case.dnd.is_some()
            || case.pause != DndPause::Off
        {
            let mut s = settings(bob, case.default_level.unwrap_or(L::All), &case.keywords);
            s.dnd = case.dnd;
            s.dnd_pause = case.pause;
            settings_by_user.insert(bob, s);
        }
        let members = [alice, bob];
        let decisions = decide(&DecisionInput {
            message: &msg,
            conversation: &conv,
            thread_participants: &thread_participants,
            members: &members,
            prefs_by_user: &prefs,
            settings_by_user: &settings_by_user,
            now: now(),
            encrypted_mentions: None,
        });
        assert!(
            decisions.iter().all(|d| d.user_id != alice),
            "author notified"
        );
        decisions.into_iter().find(|d| d.user_id == bob)
    }

    /// The reason each message shape resolves to, for one recipient.
    fn shape(reason: NotificationReason, bob: UserId) -> Case {
        match reason {
            R::Direct => Case {
                kind: ConversationType::Direct,
                ..Case::default()
            },
            R::Mention => Case {
                text: format!("ping <@{bob}> please"),
                ..Case::default()
            },
            R::Keyword => Case {
                text: "the Deploy is broken".to_string(),
                keywords: vec!["deploy".to_string()],
                ..Case::default()
            },
            R::ThreadReply => Case {
                thread: true,
                bob_in_thread: true,
                ..Case::default()
            },
            R::All => Case::default(),
        }
    }

    const REASONS: [NotificationReason; 5] =
        [R::Direct, R::Mention, R::Keyword, R::ThreadReply, R::All];

    /// Levels x reasons, through the workspace default and through the conversation override.
    #[test]
    fn level_reason_matrix() {
        for level in [L::All, L::Mentions, L::None] {
            for reason in REASONS {
                let expected = match level {
                    L::All => true,
                    L::Mentions => reason != R::All,
                    L::None => false,
                };
                let bob = UserId::new();
                let via_default = run(
                    Case {
                        default_level: Some(level),
                        ..shape(reason, bob)
                    },
                    bob,
                );
                assert_eq!(
                    via_default.map(|d| d.reason),
                    expected.then_some(reason),
                    "default {level:?} / {reason:?}"
                );
                let bob = UserId::new();
                let via_override = run(
                    Case {
                        default_level: Some(if level == L::All { L::None } else { L::All }),
                        override_level: Some(level),
                        ..shape(reason, bob)
                    },
                    bob,
                );
                assert_eq!(
                    via_override.map(|d| d.reason),
                    expected.then_some(reason),
                    "override {level:?} / {reason:?}"
                );
            }
        }
    }

    #[test]
    fn no_settings_means_level_all() {
        let bob = UserId::new();
        assert_eq!(run(Case::default(), bob).map(|d| d.reason), Some(R::All));
    }

    #[test]
    fn strongest_reason_wins() {
        let bob = UserId::new();
        // A DM that also mentions bob and hits a keyword is `direct`.
        let d = run(
            Case {
                kind: ConversationType::Direct,
                text: format!("<@{bob}> deploy"),
                keywords: vec!["deploy".to_string()],
                ..Case::default()
            },
            bob,
        );
        assert_eq!(d.map(|d| d.reason), Some(R::Direct));
        // Mention beats keyword beats thread reply.
        let bob = UserId::new();
        let d = run(
            Case {
                text: format!("<@{bob}> deploy"),
                keywords: vec!["deploy".to_string()],
                thread: true,
                bob_in_thread: true,
                ..Case::default()
            },
            bob,
        );
        assert_eq!(d.map(|d| d.reason), Some(R::Mention));
        let bob = UserId::new();
        let d = run(
            Case {
                text: "deploy now".to_string(),
                keywords: vec!["deploy".to_string()],
                thread: true,
                bob_in_thread: true,
                ..Case::default()
            },
            bob,
        );
        assert_eq!(d.map(|d| d.reason), Some(R::Keyword));
    }

    #[test]
    fn channel_mentions_reach_everyone() {
        for text in ["heads up @channel", "@HERE: standup", "(@channel)"] {
            let bob = UserId::new();
            let d = run(
                Case {
                    text: text.to_string(),
                    default_level: Some(L::Mentions),
                    ..Case::default()
                },
                bob,
            );
            assert_eq!(d.map(|d| d.reason), Some(R::Mention), "{text}");
        }
        // Not a whole word: an email address or a longer word.
        for text in ["mail me@channel.example", "@channels are fun"] {
            let bob = UserId::new();
            let d = run(
                Case {
                    text: text.to_string(),
                    default_level: Some(L::Mentions),
                    ..Case::default()
                },
                bob,
            );
            assert_eq!(d, None, "{text}");
        }
    }

    #[test]
    fn mention_of_someone_else_is_not_a_mention() {
        let bob = UserId::new();
        let carol = UserId::new();
        let d = run(
            Case {
                text: format!("<@{carol}> look"),
                default_level: Some(L::Mentions),
                ..Case::default()
            },
            bob,
        );
        assert_eq!(d, None);
    }

    #[test]
    fn thread_reply_needs_participation() {
        let bob = UserId::new();
        let d = run(
            Case {
                thread: true,
                bob_in_thread: false,
                default_level: Some(L::Mentions),
                ..Case::default()
            },
            bob,
        );
        assert_eq!(d, None);
        // Participation in a thread is irrelevant for a timeline message.
        let bob = UserId::new();
        let d = run(
            Case {
                thread: false,
                bob_in_thread: true,
                default_level: Some(L::Mentions),
                ..Case::default()
            },
            bob,
        );
        assert_eq!(d, None);
    }

    #[test]
    fn timed_mute_silences_everything_until_it_expires() {
        for reason in REASONS {
            let bob = UserId::new();
            let d = run(
                Case {
                    muted_until: Some(now() + Duration::minutes(10)),
                    ..shape(reason, bob)
                },
                bob,
            );
            assert_eq!(d, None, "{reason:?}");
            let bob = UserId::new();
            let d = run(
                Case {
                    muted_until: Some(now() - Duration::minutes(1)),
                    ..shape(reason, bob)
                },
                bob,
            );
            assert_eq!(d.map(|d| d.reason), Some(reason), "expired mute {reason:?}");
        }
    }

    #[test]
    fn keywords_are_case_insensitive_whole_words() {
        let hit = |text: &str, keyword: &str| {
            let bob = UserId::new();
            run(
                Case {
                    text: text.to_string(),
                    keywords: vec![keyword.to_string()],
                    default_level: Some(L::Mentions),
                    ..Case::default()
                },
                bob,
            )
            .map(|d| d.reason)
        };
        assert_eq!(hit("Outage in EU", "outage"), Some(R::Keyword));
        assert_eq!(hit("outage!", "outage"), Some(R::Keyword));
        assert_eq!(hit("the on-call rota", "on-call"), Some(R::Keyword));
        assert_eq!(hit("prod db down", "db down"), Some(R::Keyword));
        assert_eq!(hit("Ärger im Büro", "ärger"), Some(R::Keyword));
        assert_eq!(hit("outages everywhere", "outage"), None);
        assert_eq!(hit("blackout", "outage"), None);
        assert_eq!(hit("snake_outage", "outage"), None);
        // A keyword never matches inside a mention token.
        let id = UserId::new();
        assert_eq!(hit(&format!("hi <@{id}>"), &id.to_string()[..8]), None);
    }

    #[test]
    fn dnd_window_and_pause_mark_quiet_but_still_record() {
        let window = |start: u32, end: u32| DndWindow {
            start: NaiveTime::from_hms_opt(start, 0, 0).expect("time"),
            end: NaiveTime::from_hms_opt(end, 0, 0).expect("time"),
            timezone: "UTC".to_string(),
        };
        for reason in REASONS {
            let bob = UserId::new();
            let inside = run(
                Case {
                    dnd: Some(window(11, 13)),
                    ..shape(reason, bob)
                },
                bob,
            )
            .expect("recorded inside the window");
            assert!(inside.quiet, "{reason:?}");
            assert_eq!(inside.reason, reason);

            let bob = UserId::new();
            let outside = run(
                Case {
                    dnd: Some(window(22, 7)),
                    ..shape(reason, bob)
                },
                bob,
            )
            .expect("recorded outside the window");
            assert!(!outside.quiet, "{reason:?}");

            let bob = UserId::new();
            let paused = run(
                Case {
                    pause: DndPause::Indefinite,
                    ..shape(reason, bob)
                },
                bob,
            )
            .expect("recorded while paused");
            assert!(paused.quiet, "{reason:?}");
        }
        // DND does not override the level: NONE is still nothing.
        let bob = UserId::new();
        assert_eq!(
            run(
                Case {
                    dnd: Some(window(11, 13)),
                    default_level: Some(L::None),
                    ..Case::default()
                },
                bob
            ),
            None
        );
    }

    #[test]
    fn author_and_non_members_are_excluded_and_members_deduplicated() {
        let alice = UserId::new();
        let bob = UserId::new();
        let outsider = UserId::new();
        let conv = conversation(ConversationType::Channel);
        let msg = message(alice, &format!("<@{outsider}> <@{alice}> @channel"), false);
        let members = [alice, bob, bob];
        let decisions = decide(&DecisionInput {
            message: &msg,
            conversation: &conv,
            thread_participants: &HashSet::new(),
            members: &members,
            prefs_by_user: &HashMap::new(),
            settings_by_user: &HashMap::new(),
            now: now(),
            encrypted_mentions: None,
        });
        assert_eq!(
            decisions,
            vec![Decision {
                user_id: bob,
                reason: R::Mention,
                quiet: false
            }]
        );
    }

    #[test]
    fn deleted_message_notifies_nobody() {
        let alice = UserId::new();
        let bob = UserId::new();
        let conv = conversation(ConversationType::Direct);
        let mut msg = message(alice, "hi", false);
        msg.is_deleted = true;
        let decisions = decide(&DecisionInput {
            message: &msg,
            conversation: &conv,
            thread_participants: &HashSet::new(),
            members: &[alice, bob],
            prefs_by_user: &HashMap::new(),
            settings_by_user: &HashMap::new(),
            now: now(),
            encrypted_mentions: None,
        });
        assert!(decisions.is_empty());
    }

    #[test]
    fn whole_word_matcher_edges() {
        assert!(contains_whole_word("deploy", "deploy"));
        assert!(contains_whole_word("re deploy", "deploy"));
        assert!(!contains_whole_word("redeploy deployment", "deploy"));
        assert!(contains_whole_word("redeploy deploy", "deploy"));
        assert!(!contains_whole_word("anything", ""));
        assert!(contains_whole_word("über alles", "über"));
        assert!(!contains_whole_word("überall", "über"));
    }

    #[test]
    fn strip_mention_tokens_handles_unterminated_tokens() {
        assert_eq!(strip_mention_tokens("a <@x> b"), "a   b");
        assert_eq!(strip_mention_tokens("a <@x b"), "a <@x b");
        assert_eq!(strip_mention_tokens("plain"), "plain");
    }
}
