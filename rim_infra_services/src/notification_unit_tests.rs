//! `DefaultNotificationService` unit tests (Track 1.3): the async wrapper around the pure
//! decision (what it loads, stores and emits), settings validation and mark-read semantics.

use std::sync::{Arc, Mutex};

use chrono::{Duration, NaiveTime, Utc};

use rim_core::conversation::model::{Conversation, ConversationId, ConversationType};
use rim_core::conversation::repository::MockConversationRepository;
use rim_core::conversation_member::repository::MockConversationMemberRepository;
use rim_core::message::model::{Message, MessageId};
use rim_core::message::repository::MockMessageRepository;
use rim_core::message_thread::model::MessageThreadId;
use rim_core::notification::model::{
    DigestFrequency, DndPause, DndWindow, NewUserNotification, NotificationLevel,
    NotificationReason, UserNotification, UserNotificationId, UserNotificationSettings,
};
use rim_core::notification::repository::MockNotificationRepository;
use rim_core::notification_preference::model::NotificationPreference;
use rim_core::notification_preference::repository::MockNotificationPreferenceRepository;
use rim_core::server::model::ServerId;
use rim_core::server_member::repository::MockServerMemberRepository;
use rim_core::user::model::UserId;
use rim_services::events::{Broadcaster, Observers, WorkspaceEvent};
use rim_services::notification::command::{
    MarkNotificationsReadCmd, MarkNotificationsReadTarget, NotifyForMessageCmd, SetDndPauseCmd,
    UpdateNotificationSettingsCmd,
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
}

impl Mocks {
    fn new() -> Self {
        Self {
            notification_repo: MockNotificationRepository::new(),
            preference_repo: MockNotificationPreferenceRepository::new(),
            server_member_repo: MockServerMemberRepository::new(),
            message_repo: MockMessageRepository::new(),
        }
    }

    fn member(mut self, is_member: bool) -> Self {
        self.server_member_repo
            .expect_is_member()
            .returning(move |_, _| Ok(is_member));
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
        sent_by: author,
        text: Some(text.to_string()),
        is_deleted: false,
        has_attachments: false,
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
