//! Server-side notification rules: levels, per-workspace settings and the
//! notification inbox rows.

use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Duration, NaiveTime, Utc};
use chrono_tz::Tz;

use crate::conversation::model::ConversationId;
use crate::message::model::MessageId;
use crate::model_id;
use crate::server::model::ServerId;
use crate::user::model::UserId;

model_id!(UserNotificationId);

/// Error for the `FromStr` impls of the notification enums.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseNotificationEnumError {
    kind: &'static str,
    input: String,
}

impl fmt::Display for ParseNotificationEnumError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown {}: {}", self.kind, self.input)
    }
}

impl std::error::Error for ParseNotificationEnumError {}

/// How much a user wants to hear from a conversation. Persisted as the snake_case string of
/// [`Self::as_str`] in the Postgres enum `notification_level`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NotificationLevel {
    /// Every message.
    All,
    /// Direct messages, mentions, keyword hits, replies in the user's threads and reactions to
    /// the user's messages.
    Mentions,
    /// Nothing at all, mentions included.
    None,
}

impl NotificationLevel {
    pub fn as_str(&self) -> &'static str {
        match self {
            NotificationLevel::All => "all",
            NotificationLevel::Mentions => "mentions",
            NotificationLevel::None => "none",
        }
    }

    /// Whether a notification for `reason` passes this level (rule 5 of the decision engine).
    pub fn admits(&self, reason: NotificationReason) -> bool {
        match self {
            NotificationLevel::All => true,
            NotificationLevel::Mentions => reason != NotificationReason::All,
            NotificationLevel::None => false,
        }
    }
}

impl fmt::Display for NotificationLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for NotificationLevel {
    type Err = ParseNotificationEnumError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "all" => Ok(NotificationLevel::All),
            "mentions" => Ok(NotificationLevel::Mentions),
            "none" => Ok(NotificationLevel::None),
            other => Err(ParseNotificationEnumError {
                kind: "notification level",
                input: other.to_string(),
            }),
        }
    }
}

/// Why a user was notified. Persisted in the Postgres enum `notification_reason`. When several
/// apply, the highest [`Self::priority`] wins (one row per user and message); a reaction never
/// replaces a row that exists for another reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NotificationReason {
    /// A message in a direct conversation.
    Direct,
    /// `<@user>` naming the user, or `@channel`/`@here`.
    Mention,
    /// One of the user's keywords, as a whole word, case-insensitive.
    Keyword,
    /// A reply in a thread the user started or replied in.
    ThreadReply,
    /// Any other message (level `ALL` only).
    All,
    /// Someone reacted to the user's message.
    Reaction,
}

impl NotificationReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            NotificationReason::Direct => "direct",
            NotificationReason::Mention => "mention",
            NotificationReason::Keyword => "keyword",
            NotificationReason::ThreadReply => "thread_reply",
            NotificationReason::All => "all",
            NotificationReason::Reaction => "reaction",
        }
    }

    /// Lower is stronger: direct > mention > keyword > thread_reply > all > reaction.
    pub fn priority(&self) -> u8 {
        match self {
            NotificationReason::Direct => 0,
            NotificationReason::Mention => 1,
            NotificationReason::Keyword => 2,
            NotificationReason::ThreadReply => 3,
            NotificationReason::All => 4,
            NotificationReason::Reaction => 5,
        }
    }
}

impl fmt::Display for NotificationReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for NotificationReason {
    type Err = ParseNotificationEnumError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "direct" => Ok(NotificationReason::Direct),
            "mention" => Ok(NotificationReason::Mention),
            "keyword" => Ok(NotificationReason::Keyword),
            "thread_reply" => Ok(NotificationReason::ThreadReply),
            "all" => Ok(NotificationReason::All),
            "reaction" => Ok(NotificationReason::Reaction),
            other => Err(ParseNotificationEnumError {
                kind: "notification reason",
                input: other.to_string(),
            }),
        }
    }
}

/// How often unread notifications are emailed. Persisted in the Postgres enum
/// `digest_frequency`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DigestFrequency {
    Off,
    Hourly,
    Daily,
}

impl DigestFrequency {
    pub fn as_str(&self) -> &'static str {
        match self {
            DigestFrequency::Off => "off",
            DigestFrequency::Hourly => "hourly",
            DigestFrequency::Daily => "daily",
        }
    }

    /// The digest interval: how old the oldest pending notification, and the previous digest,
    /// must be before the next digest goes out. `None` for `Off`.
    pub fn interval(&self) -> Option<Duration> {
        match self {
            DigestFrequency::Off => None,
            DigestFrequency::Hourly => Some(Duration::minutes(60)),
            DigestFrequency::Daily => Some(Duration::hours(24)),
        }
    }
}

impl fmt::Display for DigestFrequency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for DigestFrequency {
    type Err = ParseNotificationEnumError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "off" => Ok(DigestFrequency::Off),
            "hourly" => Ok(DigestFrequency::Hourly),
            "daily" => Ok(DigestFrequency::Daily),
            other => Err(ParseNotificationEnumError {
                kind: "digest frequency",
                input: other.to_string(),
            }),
        }
    }
}

/// A daily do-not-disturb window `[start, end)` in wall-clock time of `timezone` (an IANA name).
/// The window crosses midnight when `end < start` (e.g. 22:00 to 07:00).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DndWindow {
    pub start: NaiveTime,
    pub end: NaiveTime,
    pub timezone: String,
}

impl DndWindow {
    /// Whether `now` falls inside the window. An unparseable timezone (never stored: the service
    /// validates it) or an empty window (`start == end`) is never inside.
    pub fn contains(&self, now: DateTime<Utc>) -> bool {
        let Ok(tz) = Tz::from_str(&self.timezone) else {
            return false;
        };
        let local = now.with_timezone(&tz).time();
        if self.start < self.end {
            self.start <= local && local < self.end
        } else if self.start > self.end {
            local >= self.start || local < self.end
        } else {
            false
        }
    }
}

/// A manual do-not-disturb pause, independent of the daily window (the `notifications.dnd`
/// command). Persisted as `dnd_paused` + `dnd_paused_until`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DndPause {
    Off,
    /// Until turned off.
    Indefinite,
    /// Until the given instant; ignored afterwards.
    Until(DateTime<Utc>),
}

impl DndPause {
    pub fn is_active_at(&self, now: DateTime<Utc>) -> bool {
        match self {
            DndPause::Off => false,
            DndPause::Indefinite => true,
            DndPause::Until(until) => now < *until,
        }
    }
}

/// A user's notification settings in one workspace. A user without a stored row has
/// [`Self::defaults`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserNotificationSettings {
    pub user_id: UserId,
    pub server_id: ServerId,
    pub default_level: NotificationLevel,
    /// Lower-cased, deduplicated; at most [`MAX_KEYWORDS`], each [`KEYWORD_LEN`] characters.
    pub keywords: Vec<String>,
    pub dnd: Option<DndWindow>,
    pub dnd_pause: DndPause,
    pub digest: DigestFrequency,
    pub updated_at: DateTime<Utc>,
    /// Whether reactions to the user's messages notify them.
    pub reactions: bool,
}

/// Most keywords a user may set per workspace.
pub const MAX_KEYWORDS: usize = 20;
/// Allowed keyword length in characters (after trimming).
pub const KEYWORD_LEN: std::ops::RangeInclusive<usize> = 2..=40;

impl UserNotificationSettings {
    /// The settings of a user who never saved any: level `ALL`, no keywords, no do-not-disturb,
    /// no digest, reactions on.
    pub fn defaults(user_id: UserId, server_id: ServerId, now: DateTime<Utc>) -> Self {
        Self {
            user_id,
            server_id,
            default_level: NotificationLevel::All,
            keywords: vec![],
            dnd: None,
            dnd_pause: DndPause::Off,
            digest: DigestFrequency::Off,
            updated_at: now,
            reactions: true,
        }
    }

    /// Whether notifications at `now` are quiet: inside the daily window or during a pause.
    pub fn is_quiet_at(&self, now: DateTime<Utc>) -> bool {
        self.dnd_pause.is_active_at(now) || self.dnd.as_ref().is_some_and(|w| w.contains(now))
    }
}

/// One notification in a user's inbox: at most one per `(user_id, message_id)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserNotification {
    pub id: UserNotificationId,
    pub user_id: UserId,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,
    pub message_id: MessageId,
    pub reason: NotificationReason,
    pub created_at: DateTime<Utc>,
    pub read_at: Option<DateTime<Utc>>,
    pub emailed_at: Option<DateTime<Utc>>,
    /// The latest reaction, set exactly when `reason` is [`NotificationReason::Reaction`].
    pub reaction: Option<NotificationReaction>,
}

/// The reaction a [`NotificationReason::Reaction`] notification is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationReaction {
    pub emoji: String,
    /// The reactor.
    pub by: UserId,
}

/// A new notification about a message. Reaction notifications are stored with
/// [`NewReactionNotification`] instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewUserNotification {
    pub user_id: UserId,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,
    pub message_id: MessageId,
    pub reason: NotificationReason,
}

/// A reaction notification for the author of `message_id`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewReactionNotification {
    pub user_id: UserId,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,
    pub message_id: MessageId,
    pub reaction: NotificationReaction,
}

/// A `(user, workspace)` that has unread, un-emailed notifications and a digest frequency other
/// than `off`: what the digest job's due predicate decides on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DigestCandidate {
    pub user_id: UserId,
    pub server_id: ServerId,
    pub email: String,
    pub server_name: String,
    pub frequency: DigestFrequency,
    /// `created_at` of the oldest unread, un-emailed notification.
    pub oldest_pending_at: DateTime<Utc>,
    pub pending_count: u64,
    /// The latest `emailed_at` of the user's notifications in the workspace (the last digest).
    pub last_digest_at: Option<DateTime<Utc>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(h: u32, m: u32) -> DateTime<Utc> {
        chrono::NaiveDate::from_ymd_opt(2026, 1, 15)
            .and_then(|d| d.and_hms_opt(h, m, 0))
            .map(|dt| dt.and_utc())
            .expect("valid time")
    }

    fn window(start: (u32, u32), end: (u32, u32), tz: &str) -> DndWindow {
        DndWindow {
            start: NaiveTime::from_hms_opt(start.0, start.1, 0).expect("time"),
            end: NaiveTime::from_hms_opt(end.0, end.1, 0).expect("time"),
            timezone: tz.to_string(),
        }
    }

    #[test]
    fn enums_round_trip() {
        for level in [
            NotificationLevel::All,
            NotificationLevel::Mentions,
            NotificationLevel::None,
        ] {
            assert_eq!(level.as_str().parse::<NotificationLevel>(), Ok(level));
        }
        for reason in [
            NotificationReason::Direct,
            NotificationReason::Mention,
            NotificationReason::Keyword,
            NotificationReason::ThreadReply,
            NotificationReason::All,
            NotificationReason::Reaction,
        ] {
            assert_eq!(reason.as_str().parse::<NotificationReason>(), Ok(reason));
        }
        for digest in [
            DigestFrequency::Off,
            DigestFrequency::Hourly,
            DigestFrequency::Daily,
        ] {
            assert_eq!(digest.as_str().parse::<DigestFrequency>(), Ok(digest));
        }
        assert!("loud".parse::<NotificationLevel>().is_err());
        assert!("other".parse::<NotificationReason>().is_err());
        assert!("weekly".parse::<DigestFrequency>().is_err());
    }

    #[test]
    fn reason_priority_is_direct_mention_keyword_thread_all_reaction() {
        let ordered = [
            NotificationReason::Direct,
            NotificationReason::Mention,
            NotificationReason::Keyword,
            NotificationReason::ThreadReply,
            NotificationReason::All,
            NotificationReason::Reaction,
        ];
        for pair in ordered.windows(2) {
            assert!(pair[0].priority() < pair[1].priority(), "{pair:?}");
        }
    }

    #[test]
    fn levels_admit_reasons() {
        use NotificationReason as R;
        for reason in [
            R::Direct,
            R::Mention,
            R::Keyword,
            R::ThreadReply,
            R::All,
            R::Reaction,
        ] {
            assert!(NotificationLevel::All.admits(reason));
            assert!(!NotificationLevel::None.admits(reason));
            assert_eq!(NotificationLevel::Mentions.admits(reason), reason != R::All);
        }
    }

    #[test]
    fn dnd_window_same_day() {
        let w = window((9, 0), (17, 0), "UTC");
        assert!(!w.contains(at(8, 59)));
        assert!(w.contains(at(9, 0)));
        assert!(w.contains(at(16, 59)));
        assert!(!w.contains(at(17, 0)));
    }

    #[test]
    fn dnd_window_crosses_midnight() {
        let w = window((22, 0), (7, 0), "UTC");
        assert!(w.contains(at(23, 30)));
        assert!(w.contains(at(0, 0)));
        assert!(w.contains(at(6, 59)));
        assert!(!w.contains(at(7, 0)));
        assert!(!w.contains(at(12, 0)));
        assert!(w.contains(at(22, 0)));
        assert!(!w.contains(at(21, 59)));
    }

    #[test]
    fn dnd_window_uses_the_users_timezone() {
        // 22:00-07:00 in Prague (UTC+1 in January): 21:30 UTC is 22:30 local.
        let w = window((22, 0), (7, 0), "Europe/Prague");
        assert!(w.contains(at(21, 30)));
        assert!(!w.contains(at(20, 30)));
        assert!(w.contains(at(5, 59)));
        assert!(!w.contains(at(6, 0)));
    }

    #[test]
    fn empty_window_or_bad_timezone_is_never_inside() {
        assert!(!window((9, 0), (9, 0), "UTC").contains(at(9, 0)));
        assert!(!window((0, 0), (23, 59), "Not/AZone").contains(at(12, 0)));
    }

    #[test]
    fn pause_and_window_make_quiet() {
        let now = at(12, 0);
        let mut settings = UserNotificationSettings::defaults(UserId::new(), ServerId::new(), now);
        assert!(!settings.is_quiet_at(now));
        settings.dnd_pause = DndPause::Indefinite;
        assert!(settings.is_quiet_at(now));
        settings.dnd_pause = DndPause::Until(now + Duration::minutes(5));
        assert!(settings.is_quiet_at(now));
        settings.dnd_pause = DndPause::Until(now);
        assert!(!settings.is_quiet_at(now));
        settings.dnd = Some(window((11, 0), (13, 0), "UTC"));
        assert!(settings.is_quiet_at(now));
    }

    #[test]
    fn defaults_notify_about_reactions() {
        let settings = UserNotificationSettings::defaults(UserId::new(), ServerId::new(), at(9, 0));
        assert!(settings.reactions);
    }

    #[test]
    fn digest_intervals() {
        assert_eq!(DigestFrequency::Off.interval(), None);
        assert_eq!(
            DigestFrequency::Hourly.interval(),
            Some(Duration::minutes(60))
        );
        assert_eq!(DigestFrequency::Daily.interval(), Some(Duration::hours(24)));
    }
}
