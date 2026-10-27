//! `DefaultNotificationService` unit tests: the async wrapper around the pure
//! decision (what it loads, stores and emits), settings validation and mark-read semantics.

use std::sync::{Arc, Mutex};

use chrono::{Duration, NaiveTime, Utc};

use rim_core::conversation::model::{
    Conversation, ConversationId, ConversationType, PostingPolicy,
};
use rim_core::conversation::repository::MockConversationRepository;
use rim_core::conversation_member::repository::MockConversationMemberRepository;
use rim_core::message::model::{Message, MessageId};
use rim_core::message::repository::MockMessageRepository;
use rim_core::message_thread::model::MessageThreadId;
use rim_core::notification::model::{
    DigestFrequency, DndPause, DndWindow, NewReactionNotification, NewUserNotification,
    NotificationLevel, NotificationReaction, NotificationReason, StoredReactionNotification,
    UserNotification, UserNotificationId, UserNotificationSettings,
};
use rim_core::notification::repository::MockNotificationRepository;
use rim_core::notification_preference::model::NotificationPreference;
use rim_core::notification_preference::repository::MockNotificationPreferenceRepository;
use rim_core::server::model::ServerId;
use rim_core::server_member::repository::MockServerMemberRepository;
use rim_core::user::model::UserId;
use rim_services::default_workspace::MockDefaultWorkspaceService;
use rim_services::events::{Broadcaster, Observers, WorkspaceEvent};
use rim_services::notification::command::{
    GetNotificationSettingsQuery, MarkNotificationsReadCmd, MarkNotificationsReadTarget,
    NotifyForMessageCmd, NotifyForReactionCmd, SetDndPauseCmd, UpdateNotificationSettingsCmd,
};
use rim_services::notification::service::NotificationService;
use rim_services::{Actor, ServiceError};

use crate::notification::{DefaultNotificationService, NotificationRepos};

#[derive(Default)]
struct RecordingBroadcaster(Mutex<Vec<WorkspaceEvent>>);

impl Broadcaster for RecordingBroadcaster {
    fn broadcast(&self, event: WorkspaceEvent) {
        self.0.lock().expect("lock").push(event);
    }
}

struct Mocks {
    notification_repo: MockNotificationRepository,
    preference_repo: MockNotificationPreferenceRepository,
    server_member_repo: MockServerMemberRepository,
    message_repo: MockMessageRepository,
    /// The server's default workspace; `None` wires no default workspace service.
    default_workspace: Option<ServerId>,
}

impl Mocks {
    fn new() -> Self {
        Self {
            notification_repo: MockNotificationRepository::new(),
            preference_repo: MockNotificationPreferenceRepository::new(),
            server_member_repo: MockServerMemberRepository::new(),
            message_repo: MockMessageRepository::new(),
            default_workspace: None,
        }
    }

    fn default_workspace(mut self, workspace: ServerId) -> Self {
        self.default_workspace = Some(workspace);
        self
    }

    fn member(mut self, is_member: bool) -> Self {
        self.server_member_repo
            .expect_is_member()
            .returning(move |_, _| Ok(is_member));
        self.server_member_repo
            .expect_join_direct_space()
            .returning(|_, _| Ok(false));
        self
    }

    fn build(self) -> (DefaultNotificationService, Arc<RecordingBroadcaster>) {
        let broadcaster = Arc::new(RecordingBroadcaster::default());
        let svc = DefaultNotificationService::new(
            NotificationRepos {
                notification_repo: Arc::new(self.notification_repo),
                notification_preference_repo: Arc::new(self.preference_repo),
                server_member_repo: Arc::new(self.server_member_repo),
                conversation_repo: Arc::new(MockConversationRepository::new()),
                conversation_member_repo: Arc::new(MockConversationMemberRepository::new()),
                message_repo: Arc::new(self.message_repo),
            },
            Observers::new(Arc::clone(&broadcaster) as Arc<dyn Broadcaster>),
        );
        let svc = match self.default_workspace {
            Some(workspace) => {
                let mut default_workspace = MockDefaultWorkspaceService::new();
                default_workspace
                    .expect_current()
                    .returning(move || Box::pin(async move { Ok(Some(workspace)) }));
                svc.with_default_workspace(Arc::new(default_workspace))
            }
            None => svc,
        };
        (svc, broadcaster)
    }
}

fn conversation(server_id: ServerId, kind: ConversationType) -> Conversation {
    Conversation {
        id: ConversationId::new(),
        conversation_type: kind,
        title: Some("general".to_string()),
        topic: None,
        created_at: Utc::now(),
        last_activity_at: Utc::now(),
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

fn message(conversation_id: ConversationId, author: UserId, text: &str) -> Message {
    Message {
        id: MessageId::new(),
        conversation_id,
        thread_id: None,
        parent_message_id: None,
        forwarded_from_message_id: None,
        sent_at: Utc::now(),
        edited_at: None,
        sent_by: author,
        text: Some(text.to_string()),
        is_deleted: false,
        has_attachments: false,
        encryption: None,
    }
}

fn stored(new: &NewUserNotification) -> UserNotification {
    UserNotification {
        id: UserNotificationId::new(),
        user_id: new.user_id,
        server_id: new.server_id,
        conversation_id: new.conversation_id,
        message_id: new.message_id,
        reason: new.reason,
        created_at: Utc::now(),
        read_at: None,
        emailed_at: None,
        reaction: None,
    }
}

/// A valid, almost all-day window in UTC.
fn always_quiet() -> DndWindow {
    DndWindow {
        start: NaiveTime::from_hms_opt(0, 0, 0).expect("time"),
        end: NaiveTime::from_hms_opt(23, 59, 59).expect("time"),
        timezone: "UTC".to_string(),
    }
}

#[tokio::test]
async fn notify_stores_rows_and_emits_one_private_event_per_recipient() {
    let server_id = ServerId::new();
    let conv = conversation(server_id, ConversationType::Channel);
    let alice = UserId::new();
    let bob = UserId::new();
    let carol = UserId::new();
    let dave = UserId::new();
    let msg = message(conv.id, alice, &format!("hey <@{bob}>"));

    let mut mocks = Mocks::new();
    // carol: MENTIONS by default (no mention -> nothing); dave: paused (quiet), level ALL.
    let carol_settings = UserNotificationSettings {
        default_level: NotificationLevel::Mentions,
        ..UserNotificationSettings::defaults(carol, server_id, Utc::now())
    };
    let dave_settings = UserNotificationSettings {
        dnd_pause: DndPause::Indefinite,
        ..UserNotificationSettings::defaults(dave, server_id, Utc::now())
    };
    mocks
        .preference_repo
        .expect_list_for_conversation()
        .returning(|_, _| Box::pin(async { Ok(vec![]) }));
    mocks
        .notification_repo
        .expect_get_settings_for_users()
        .withf(move |sid, users| *sid == server_id && users.len() == 3 && !users.contains(&alice))
        .returning(move |_, _| {
            let s = vec![carol_settings.clone(), dave_settings.clone()];
            Box::pin(async move { Ok(s) })
        });
    mocks
        .notification_repo
        .expect_create_notifications()
        .times(1)
        .returning(|rows, _| {
            let out: Vec<_> = rows.iter().map(stored).collect();
            Box::pin(async move { Ok(out) })
        });
    mocks
        .message_repo
        .expect_list_thread_participant_ids()
        .never();
    let (svc, events) = mocks.build();

    let created = svc
        .notify_for_message(NotifyForMessageCmd {
            server_id,
            conversation: conv.clone(),
            message: msg.clone(),
            member_user_ids: vec![alice, bob, carol, dave],
            encrypted_mentions: None,
        })
        .await
        .expect("notify");
    assert_eq!(created, 2);

    let events = events.0.lock().expect("lock").clone();
    assert_eq!(events.len(), 2);
    let mut seen = vec![];
    for event in events {
        let WorkspaceEvent::NotificationDue {
            workspace_id,
            user_id,
            conversation_id,
            message_id,
            reason,
            quiet,
            member_user_ids,
            ..
        } = event
        else {
            panic!("unexpected event {event:?}");
        };
        assert_eq!(workspace_id, server_id);
        assert_eq!(conversation_id, conv.id);
        assert_eq!(message_id, msg.id);
        assert_eq!(
            member_user_ids,
            vec![user_id],
            "scoped to the recipient only"
        );
        seen.push((user_id, reason, quiet));
    }
    seen.sort_by_key(|(u, _, _)| u.to_string());
    let mut expected = vec![
        (bob, "mention".to_string(), false),
        (dave, "all".to_string(), true),
    ];
    expected.sort_by_key(|(u, _, _)| u.to_string());
    assert_eq!(seen, expected);
}

/// The unique `(user, message)` row means a second call creates (and announces) nothing.
#[tokio::test]
async fn notify_emits_only_for_rows_actually_inserted() {
    let server_id = ServerId::new();
    let conv = conversation(server_id, ConversationType::Direct);
    let alice = UserId::new();
    let bob = UserId::new();
    let msg = message(conv.id, alice, "hi");
    let mut mocks = Mocks::new();
    mocks
        .preference_repo
        .expect_list_for_conversation()
        .returning(|_, _| Box::pin(async { Ok(vec![]) }));
    mocks
        .notification_repo
        .expect_get_settings_for_users()
        .returning(|_, _| Box::pin(async { Ok(vec![]) }));
    mocks
        .notification_repo
        .expect_create_notifications()
        .withf(|rows, _| rows.len() == 1 && rows[0].reason == NotificationReason::Direct)
        .returning(|_, _| Box::pin(async { Ok(vec![]) }));
    let (svc, events) = mocks.build();
    let created = svc
        .notify_for_message(NotifyForMessageCmd {
            server_id,
            conversation: conv,
            message: msg,
            member_user_ids: vec![alice, bob],
            encrypted_mentions: None,
        })
        .await
        .expect("notify");
    assert_eq!(created, 0);
    assert!(events.0.lock().expect("lock").is_empty());
}

#[tokio::test]
async fn notify_without_other_members_touches_nothing() {
    let server_id = ServerId::new();
    let conv = conversation(server_id, ConversationType::Channel);
    let alice = UserId::new();
    let mut mocks = Mocks::new();
    mocks.preference_repo.expect_list_for_conversation().never();
    mocks
        .notification_repo
        .expect_get_settings_for_users()
        .never();
    mocks
        .notification_repo
        .expect_create_notifications()
        .never();
    let (svc, _) = mocks.build();
    let created = svc
        .notify_for_message(NotifyForMessageCmd {
            server_id,
            conversation: conv.clone(),
            message: message(conv.id, alice, "talking to myself"),
            member_user_ids: vec![alice],
            encrypted_mentions: None,
        })
        .await
        .expect("notify");
    assert_eq!(created, 0);
}

/// A thread reply loads the thread's participants: bob (who replied before) is told under
/// `MENTIONS`, carol (who never took part) is not.
#[tokio::test]
async fn thread_reply_notifies_participants() {
    let server_id = ServerId::new();
    let conv = conversation(server_id, ConversationType::Channel);
    let alice = UserId::new();
    let bob = UserId::new();
    let carol = UserId::new();
    let thread_id = MessageThreadId::new();
    let mut msg = message(conv.id, alice, "reply");
    msg.thread_id = Some(thread_id);

    let mut mocks = Mocks::new();
    mocks
        .message_repo
        .expect_list_thread_participant_ids()
        .withf(move |t| *t == thread_id)
        .times(1)
        .returning(move |_| Box::pin(async move { Ok(vec![alice, bob]) }));
    let prefs = vec![
        NotificationPreference {
            user_id: bob,
            conversation_id: conv.id,
            muted_until: None,
            level: Some(NotificationLevel::Mentions),
        },
        NotificationPreference {
            user_id: carol,
            conversation_id: conv.id,
            muted_until: None,
            level: Some(NotificationLevel::Mentions),
        },
    ];
    mocks
        .preference_repo
        .expect_list_for_conversation()
        .returning(move |_, _| {
            let p = prefs.clone();
            Box::pin(async move { Ok(p) })
        });
    mocks
        .notification_repo
        .expect_get_settings_for_users()
        .returning(|_, _| Box::pin(async { Ok(vec![]) }));
    mocks
        .notification_repo
        .expect_create_notifications()
        .withf(move |rows, _| {
            rows.len() == 1
                && rows[0].user_id == bob
                && rows[0].reason == NotificationReason::ThreadReply
        })
        .times(1)
        .returning(|rows, _| {
            let out: Vec<_> = rows.iter().map(stored).collect();
            Box::pin(async move { Ok(out) })
        });
    let (svc, _) = mocks.build();
    let created = svc
        .notify_for_message(NotifyForMessageCmd {
            server_id,
            conversation: conv,
            message: msg,
            member_user_ids: vec![alice, bob, carol],
            encrypted_mentions: None,
        })
        .await
        .expect("notify");
    assert_eq!(created, 1);
}

fn update_cmd(
    server_id: ServerId,
    keywords: Vec<String>,
    dnd: Option<DndWindow>,
) -> UpdateNotificationSettingsCmd {
    UpdateNotificationSettingsCmd {
        actor: Actor::new(UserId::new()),
        server_id,
        default_level: NotificationLevel::Mentions,
        keywords,
        dnd,
        digest: DigestFrequency::Daily,
        reactions: None,
    }
}

#[tokio::test]
async fn update_settings_rejects_bad_input_before_writing() {
    let server_id = ServerId::new();
    let bad_window = DndWindow {
        timezone: "Nowhere/Land".to_string(),
        ..always_quiet()
    };
    let cases = vec![
        update_cmd(server_id, vec!["x".to_string()], None),
        update_cmd(server_id, (0..21).map(|i| format!("kw{i}")).collect(), None),
        update_cmd(server_id, vec![], Some(bad_window)),
    ];
    for cmd in cases {
        let mut mocks = Mocks::new().member(true);
        mocks.notification_repo.expect_get_settings().never();
        mocks.notification_repo.expect_upsert_settings().never();
        let (svc, _) = mocks.build();
        let err = svc.update_settings(cmd).await.unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)), "{err:?}");
    }
}

/// Saving settings normalizes keywords and keeps the manual pause, which only `set_dnd_pause`
/// changes.
#[tokio::test]
async fn update_settings_normalizes_and_keeps_the_pause() {
    let server_id = ServerId::new();
    let mut mocks = Mocks::new().member(true);
    mocks
        .notification_repo
        .expect_get_settings()
        .returning(move |user, server| {
            let s = UserNotificationSettings {
                dnd_pause: DndPause::Indefinite,
                ..UserNotificationSettings::defaults(user, server, Utc::now())
            };
            Box::pin(async move { Ok(Some(s)) })
        });
    mocks
        .notification_repo
        .expect_upsert_settings()
        .withf(|s| {
            s.keywords == vec!["deploy".to_string()]
                && s.dnd_pause == DndPause::Indefinite
                && s.default_level == NotificationLevel::Mentions
                && s.digest == DigestFrequency::Daily
                && s.dnd
                    .as_ref()
                    .is_some_and(|w| w.timezone == "Europe/Prague")
        })
        .times(1)
        .returning(|s| Box::pin(async move { Ok(s) }));
    let (svc, _) = mocks.build();
    let window = DndWindow {
        timezone: " Europe/Prague ".to_string(),
        ..always_quiet()
    };
    let dto = svc
        .update_settings(update_cmd(
            server_id,
            vec!["Deploy".to_string(), "deploy ".to_string()],
            Some(window),
        ))
        .await
        .expect("update");
    assert_eq!(dto.keywords, vec!["deploy".to_string()]);
}

#[tokio::test]
async fn settings_require_workspace_membership() {
    let mut mocks = Mocks::new().member(false);
    mocks.notification_repo.expect_upsert_settings().never();
    let (svc, _) = mocks.build();
    let err = svc
        .update_settings(update_cmd(ServerId::new(), vec![], None))
        .await
        .unwrap_err();
    assert!(matches!(err, ServiceError::PermissionDenied));
}

#[tokio::test]
async fn dnd_pause_until_must_be_in_the_future() {
    let mut mocks = Mocks::new().member(true);
    mocks.notification_repo.expect_upsert_settings().never();
    let (svc, _) = mocks.build();
    let err = svc
        .set_dnd_pause(SetDndPauseCmd {
            actor: Actor::new(UserId::new()),
            server_id: ServerId::new(),
            pause: DndPause::Until(Utc::now() - Duration::minutes(1)),
        })
        .await
        .unwrap_err();
    assert!(matches!(err, ServiceError::Validation(_)));
}

#[tokio::test]
async fn mark_read_by_ids_is_scoped_to_the_actor_and_bounded() {
    let actor = UserId::new();
    let server_id = ServerId::new();
    let ids = vec![UserNotificationId::new(), UserNotificationId::new()];
    let expected = ids.clone();
    let mut mocks = Mocks::new().member(true);
    mocks
        .notification_repo
        .expect_mark_read_by_ids()
        .withf(move |user, server, got, _| {
            *user == actor && *server == server_id && got == expected.as_slice()
        })
        .times(1)
        .returning(|_, _, _, _| Box::pin(async { Ok(1) }));
    let (svc, _) = mocks.build();
    let changed = svc
        .mark_read(MarkNotificationsReadCmd {
            actor: Actor::new(actor),
            server_id,
            target: MarkNotificationsReadTarget::Ids(ids),
        })
        .await
        .expect("mark read");
    assert_eq!(changed, 1);

    let mut mocks = Mocks::new().member(true);
    mocks.notification_repo.expect_mark_read_by_ids().never();
    let (svc, _) = mocks.build();
    let err = svc
        .mark_read(MarkNotificationsReadCmd {
            actor: Actor::new(actor),
            server_id,
            target: MarkNotificationsReadTarget::Ids(
                (0..501).map(|_| UserNotificationId::new()).collect(),
            ),
        })
        .await
        .unwrap_err();
    assert!(matches!(err, ServiceError::Validation(_)));
}

/// `allBefore` in the future is clamped to now, so notifications created later stay unread.
#[tokio::test]
async fn mark_read_all_before_is_clamped_to_now() {
    let before_call = Utc::now();
    let mut mocks = Mocks::new().member(true);
    mocks
        .notification_repo
        .expect_mark_read_before()
        .withf(move |_, _, before, read_at| {
            *before >= before_call && *before <= Utc::now() && before == read_at
        })
        .times(1)
        .returning(|_, _, _, _| Box::pin(async { Ok(5) }));
    let (svc, _) = mocks.build();
    let changed = svc
        .mark_read(MarkNotificationsReadCmd {
            actor: Actor::new(UserId::new()),
            server_id: ServerId::new(),
            target: MarkNotificationsReadTarget::AllBefore(Utc::now() + Duration::days(1)),
        })
        .await
        .expect("mark read");
    assert_eq!(changed, 5);
}

#[tokio::test]
async fn forget_message_deletes_unread_rows() {
    let message_id = MessageId::new();
    let mut mocks = Mocks::new();
    mocks
        .notification_repo
        .expect_delete_unread_for_message()
        .withf(move |id| *id == message_id)
        .times(1)
        .returning(|_| Box::pin(async { Ok(2) }));
    let (svc, _) = mocks.build();
    assert_eq!(svc.forget_message(message_id).await.expect("forget"), 2);
}

/// The row `upsert_reaction_notification` stores for `new`, inserted or replacing a read one.
fn stored_reaction(new: &NewReactionNotification) -> StoredReactionNotification {
    StoredReactionNotification {
        notification: UserNotification {
            id: UserNotificationId::new(),
            user_id: new.user_id,
            server_id: new.server_id,
            conversation_id: new.conversation_id,
            message_id: new.message_id,
            reason: NotificationReason::Reaction,
            created_at: Utc::now(),
            read_at: None,
            emailed_at: None,
            reaction: Some(new.reaction.clone()),
        },
        replaced_unread_at: None,
    }
}

/// Bob reacts to alice's message in a channel both are members of.
struct ReactionSetup {
    server_id: ServerId,
    alice: UserId,
    bob: UserId,
    cmd: NotifyForReactionCmd,
}

fn reaction_setup() -> ReactionSetup {
    let server_id = ServerId::new();
    let conv = conversation(server_id, ConversationType::Channel);
    let alice = UserId::new();
    let bob = UserId::new();
    let msg = message(conv.id, alice, "ship it");
    ReactionSetup {
        server_id,
        alice,
        bob,
        cmd: NotifyForReactionCmd {
            server_id,
            conversation: conv,
            message: msg,
            member_user_ids: vec![alice, bob],
            emoji: "\u{1f44d}".to_string(),
            by: bob,
        },
    }
}

/// Mocks for the author's preference (none) and settings.
fn reaction_mocks(settings: Option<UserNotificationSettings>) -> Mocks {
    let mut mocks = Mocks::new();
    mocks
        .preference_repo
        .expect_get_preference()
        .returning(|_, _| Box::pin(async { Ok(None) }));
    mocks
        .notification_repo
        .expect_get_settings()
        .returning(move |_, _| {
            let s = settings.clone();
            Box::pin(async move { Ok(s) })
        });
    mocks
}

/// The stored (inserted or re-surfaced) row is announced to the author only, with the emoji
/// and the reactor; inside do-not-disturb the event is quiet.
#[tokio::test]
async fn reaction_notifies_the_author_with_emoji_and_reactor() {
    for paused in [false, true] {
        let s = reaction_setup();
        let settings = paused.then(|| UserNotificationSettings {
            dnd_pause: DndPause::Indefinite,
            ..UserNotificationSettings::defaults(s.alice, s.server_id, Utc::now())
        });
        let mut mocks = reaction_mocks(settings);
        let (alice, bob, message_id) = (s.alice, s.bob, s.cmd.message.id);
        mocks
            .notification_repo
            .expect_upsert_reaction_notification()
            .withf(move |n, _| {
                n.user_id == alice
                    && n.message_id == message_id
                    && n.reaction
                        == NotificationReaction {
                            emoji: "\u{1f44d}".to_string(),
                            by: bob,
                        }
            })
            .times(1)
            .returning(|n, _| {
                let row = stored_reaction(&n);
                Box::pin(async move { Ok(Some(row)) })
            });
        let (svc, events) = mocks.build();
        assert!(svc.notify_for_reaction(s.cmd).await.expect("notify"));

        let events = events.0.lock().expect("lock").clone();
        let [
            WorkspaceEvent::NotificationDue {
                user_id,
                message_id: event_message,
                reason,
                quiet,
                reaction_emoji,
                reaction_by,
                member_user_ids,
                ..
            },
        ] = events.as_slice()
        else {
            panic!("expected one NotificationDue, got {events:?}");
        };
        assert_eq!(*user_id, alice);
        assert_eq!(*event_message, message_id);
        assert_eq!(reason, "reaction");
        assert_eq!(*quiet, paused);
        assert_eq!(reaction_emoji.as_deref(), Some("\u{1f44d}"));
        assert_eq!(*reaction_by, Some(bob));
        assert_eq!(member_user_ids, &vec![alice]);
    }
}

/// A row the message already has for another reason is left alone: nothing is announced.
#[tokio::test]
async fn reaction_on_a_message_with_another_notification_announces_nothing() {
    let s = reaction_setup();
    let mut mocks = reaction_mocks(None);
    mocks
        .notification_repo
        .expect_upsert_reaction_notification()
        .times(1)
        .returning(|_, _| Box::pin(async { Ok(None) }));
    let (svc, events) = mocks.build();
    assert!(!svc.notify_for_reaction(s.cmd).await.expect("notify"));
    assert!(events.0.lock().expect("lock").is_empty());
}

/// The `reactions` setting off stores nothing.
#[tokio::test]
async fn reactions_setting_off_stores_nothing() {
    let s = reaction_setup();
    let mut mocks = reaction_mocks(Some(UserNotificationSettings {
        reactions: false,
        ..UserNotificationSettings::defaults(s.alice, s.server_id, Utc::now())
    }));
    mocks
        .notification_repo
        .expect_upsert_reaction_notification()
        .never();
    let (svc, events) = mocks.build();
    assert!(!svc.notify_for_reaction(s.cmd).await.expect("notify"));
    assert!(events.0.lock().expect("lock").is_empty());
}

/// An own reaction, or one on the message of an author who left the conversation, loads and
/// stores nothing.
#[tokio::test]
async fn own_reaction_or_departed_author_touches_nothing() {
    let own = {
        let mut s = reaction_setup();
        s.cmd.by = s.alice;
        s.cmd
    };
    let departed = {
        let mut s = reaction_setup();
        s.cmd.member_user_ids = vec![s.bob];
        s.cmd
    };
    for cmd in [own, departed] {
        let mut mocks = Mocks::new();
        mocks.preference_repo.expect_get_preference().never();
        mocks.notification_repo.expect_get_settings().never();
        mocks
            .notification_repo
            .expect_upsert_reaction_notification()
            .never();
        let (svc, _) = mocks.build();
        assert!(!svc.notify_for_reaction(cmd).await.expect("notify"));
    }
}

/// `reactions: None` keeps the stored value; `Some` replaces it.
#[tokio::test]
async fn update_settings_keeps_reactions_unless_given() {
    for (given, expected) in [(None, false), (Some(true), true)] {
        let server_id = ServerId::new();
        let mut mocks = Mocks::new().member(true);
        mocks
            .notification_repo
            .expect_get_settings()
            .returning(move |user, server| {
                let s = UserNotificationSettings {
                    reactions: false,
                    ..UserNotificationSettings::defaults(user, server, Utc::now())
                };
                Box::pin(async move { Ok(Some(s)) })
            });
        mocks
            .notification_repo
            .expect_upsert_settings()
            .withf(move |s| s.reactions == expected)
            .times(1)
            .returning(|s| Box::pin(async move { Ok(s) }));
        let (svc, _) = mocks.build();
        let dto = svc
            .update_settings(UpdateNotificationSettingsCmd {
                reactions: given,
                ..update_cmd(server_id, vec![], None)
            })
            .await
            .expect("update");
        assert_eq!(dto.reactions, expected);
    }
}

/// A reaction replacing a notification still unread from the last ten minutes updates the row
/// without announcing it; one replacing an older unread row, a read row or none is announced.
#[tokio::test]
async fn reaction_replacing_a_recent_unread_notification_is_not_announced_again() {
    for (replaced_minutes_ago, announced) in [(Some(2), false), (Some(11), true), (None, true)] {
        let s = reaction_setup();
        let mut mocks = reaction_mocks(None);
        mocks
            .notification_repo
            .expect_upsert_reaction_notification()
            .times(1)
            .returning(move |n, now| {
                let row = StoredReactionNotification {
                    replaced_unread_at: replaced_minutes_ago.map(|m| now - Duration::minutes(m)),
                    ..stored_reaction(&n)
                };
                Box::pin(async move { Ok(Some(row)) })
            });
        let (svc, events) = mocks.build();
        assert!(svc.notify_for_reaction(s.cmd).await.expect("notify"));
        assert_eq!(
            events.0.lock().expect("lock").len(),
            usize::from(announced),
            "replaced {replaced_minutes_ago:?} minutes ago"
        );
    }
}

/// In the default workspace a member who never saved settings is at `MENTIONS`: a plain
/// message reaches only members who chose `ALL`, in their settings or for the conversation.
/// In any other workspace the same member is at `ALL`.
#[tokio::test]
async fn the_default_workspace_defaults_members_without_settings_to_mentions() {
    for in_default_workspace in [true, false] {
        let server_id = ServerId::new();
        let conv = conversation(server_id, ConversationType::Channel);
        let (alice, bob, carol, dave, erin) = (
            UserId::new(),
            UserId::new(),
            UserId::new(),
            UserId::new(),
            UserId::new(),
        );
        let msg = message(conv.id, alice, &format!("hey <@{bob}>"));
        let mut mocks = Mocks::new().default_workspace(if in_default_workspace {
            server_id
        } else {
            ServerId::new()
        });
        let conv_id = conv.id;
        mocks
            .preference_repo
            .expect_list_for_conversation()
            .returning(move |_, _| {
                let prefs = vec![NotificationPreference {
                    user_id: erin,
                    conversation_id: conv_id,
                    muted_until: None,
                    level: Some(NotificationLevel::All),
                }];
                Box::pin(async move { Ok(prefs) })
            });
        mocks
            .notification_repo
            .expect_get_settings_for_users()
            .returning(move |server, _| {
                let saved = vec![UserNotificationSettings {
                    default_level: NotificationLevel::All,
                    ..UserNotificationSettings::defaults(dave, server, Utc::now())
                }];
                Box::pin(async move { Ok(saved) })
            });
        mocks
            .notification_repo
            .expect_create_notifications()
            .times(1)
            .returning(|rows, _| {
                let out: Vec<_> = rows.iter().map(stored).collect();
                Box::pin(async move { Ok(out) })
            });
        let (svc, events) = mocks.build();
        svc.notify_for_message(NotifyForMessageCmd {
            server_id,
            conversation: conv,
            message: msg,
            member_user_ids: vec![alice, bob, carol, dave, erin],
            encrypted_mentions: None,
        })
        .await
        .expect("notify");

        let mut told: Vec<(UserId, String)> = events
            .0
            .lock()
            .expect("lock")
            .iter()
            .map(|event| match event {
                WorkspaceEvent::NotificationDue {
                    user_id, reason, ..
                } => (*user_id, reason.clone()),
                other => panic!("unexpected event {other:?}"),
            })
            .collect();
        told.sort_by_key(|(u, _)| u.to_string());
        let mut expected = vec![
            (bob, "mention".to_string()),
            (dave, "all".to_string()),
            (erin, "all".to_string()),
        ];
        if !in_default_workspace {
            expected.push((carol, "all".to_string()));
        }
        expected.sort_by_key(|(u, _)| u.to_string());
        assert_eq!(
            told, expected,
            "in the default workspace: {in_default_workspace}"
        );
    }
}

/// The settings of a member who never saved any read `MENTIONS` in the default workspace and
/// `ALL` elsewhere; saved settings are returned as saved.
#[tokio::test]
async fn unsaved_settings_follow_the_default_workspace() {
    let default_workspace = ServerId::new();
    for (server_id, saved, expected) in [
        (default_workspace, None, NotificationLevel::Mentions),
        (ServerId::new(), None, NotificationLevel::All),
        (
            default_workspace,
            Some(NotificationLevel::All),
            NotificationLevel::All,
        ),
    ] {
        let mut mocks = Mocks::new()
            .member(true)
            .default_workspace(default_workspace);
        mocks
            .notification_repo
            .expect_get_settings()
            .returning(move |user, server| {
                let s = saved.map(|level| UserNotificationSettings {
                    default_level: level,
                    ..UserNotificationSettings::defaults(user, server, Utc::now())
                });
                Box::pin(async move { Ok(s) })
            });
        let (svc, _) = mocks.build();
        let dto = svc
            .get_settings(GetNotificationSettingsQuery {
                actor: Actor::new(UserId::new()),
                server_id,
            })
            .await
            .expect("settings");
        assert_eq!(dto.default_level, expected);
    }
}
