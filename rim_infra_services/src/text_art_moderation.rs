//! The automatic text art check of workspace messages: a readable message that looks like text
//! art is scored after it is stored, and a high score flags it and reports it to the
//! workspace's Owner and Moderators and to the instance's operators. Sending never waits for
//! the check, and an unavailable service leaves the message unflagged.

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use tokio::sync::Semaphore;

use rim_core::conversation::model::ConversationId;
use rim_core::message::repository::MessageRepository;
use rim_core::moderation::model::NewMessageReport;
use rim_core::moderation::repository::ModerationRepository;
use rim_core::repository::error::RepoErrorKind;
use rim_core::server::model::ServerId;
use rim_core::server_feature::model::ServerFeature;
use rim_core::user::model::UserId;
use rim_core::user::repository::UserRepository;
use rim_services::events::{EventObserver, MessageEventData, Observers, WorkspaceEvent};
use rim_services::feature::service::FeatureGate;
use rim_services::operator::OperatorCaller;
use rim_services::relay::dto::FileAbuseReport;
use rim_services::relay::service::RelayAbuseService;

use crate::text_art_check::{TextScorer, art_block};

/// How many messages are scored at once.
const CONCURRENCY: usize = 2;
/// How long a message waits for a free scoring slot before it is left unchecked.
const SLOT_WAIT: Duration = Duration::from_secs(30);

/// What the check needs: the scorer, its threshold and where its results go.
pub struct TextArtModerationParts {
    pub scorer: Arc<dyn TextScorer>,
    /// A sexual score at or above this flags the message.
    pub flag_threshold: f64,
    /// The relay's direct space, whose messages are never checked.
    pub direct_space_id: Option<ServerId>,
    pub feature_gate: Arc<dyn FeatureGate>,
    pub message_repo: Arc<dyn MessageRepository>,
    pub moderation_repo: Arc<dyn ModerationRepository>,
    pub relay_abuse: Arc<dyn RelayAbuseService>,
    pub user_repo: Arc<dyn UserRepository>,
    pub observers: Arc<Observers>,
}

/// The text art check, run on every new or edited message event.
pub struct TextArtModeration {
    parts: TextArtModerationParts,
    slots: Arc<Semaphore>,
}

/// The message event a check runs for, with the visibility fields its flag events copy.
struct Subject<'a> {
    workspace_id: ServerId,
    conversation_id: ConversationId,
    conversation_type: &'a str,
    is_private: bool,
    member_user_ids: &'a [UserId],
    message: &'a MessageEventData,
    edited: bool,
}

impl TextArtModeration {
    /// The check over `parts`, scoring at most two messages at once.
    pub fn new(parts: TextArtModerationParts) -> Self {
        Self {
            parts,
            slots: Arc::new(Semaphore::new(CONCURRENCY)),
        }
    }

    /// Check the message of a `NewMessage` or `MessageUpdated` event; any other event is
    /// ignored. A message is skipped while the feature is off, when it is encrypted, deleted,
    /// in the direct space or without text. Text art scoring at or above the threshold is
    /// flagged and reported once; an edit that is no longer text art, or scores below the
    /// threshold, clears the flag. Every failure is logged and leaves the message as it is.
    pub async fn handle(&self, event: &WorkspaceEvent) {
        let subject = match event {
            WorkspaceEvent::NewMessage {
                workspace_id,
                conversation_id,
                conversation_type,
                member_user_ids,
                is_private,
                message,
            }
            | WorkspaceEvent::MessageUpdated {
                workspace_id,
                conversation_id,
                conversation_type,
                member_user_ids,
                is_private,
                message,
            } => Subject {
                workspace_id: *workspace_id,
                conversation_id: *conversation_id,
                conversation_type,
                is_private: *is_private,
                member_user_ids,
                message,
                edited: matches!(event, WorkspaceEvent::MessageUpdated { .. }),
            },
            _ => return,
        };
        if !self
            .parts
            .feature_gate
            .state(ServerFeature::TextArtCheck)
            .is_enabled()
            || subject.message.encryption.is_some()
            || subject.message.is_deleted
            || Some(subject.workspace_id) == self.parts.direct_space_id
        {
            return;
        }
        let Some(text) = subject.message.text.as_deref() else {
            return;
        };
        let Some(block) = art_block(text) else {
            if subject.edited {
                self.settle(&subject, None).await;
            }
            return;
        };
        let message_id = subject.message.id;
        let Ok(Ok(slot)) =
            tokio::time::timeout(SLOT_WAIT, Arc::clone(&self.slots).acquire_owned()).await
        else {
            tracing::warn!(
                message_id = %message_id,
                "text art check skipped: too many checks waiting"
            );
            return;
        };
        let score = self.parts.scorer.score(&block).await;
        drop(slot);
        match score {
            Ok(score) => self.settle(&subject, Some(score)).await,
            Err(e) => {
                tracing::warn!(error = %e, message_id = %message_id, "text art check unavailable");
            }
        }
    }

    /// Flag the subject's message for a `score` at or above the threshold, or clear its flag
    /// after an edit that scored below it or is not text art (`None`). Nothing happens when
    /// the stored message is gone, deleted or no longer has the event's text.
    async fn settle(&self, subject: &Subject<'_>, score: Option<f64>) {
        let message_id = subject.message.id;
        let stored = match self.parts.message_repo.get_by_id(message_id).await {
            Ok(stored) => stored,
            Err(e) => {
                tracing::warn!(error = %e, message_id = %message_id, "text art check: message lookup failed");
                return;
            }
        };
        let Some(stored) = stored else {
            return;
        };
        if stored.is_deleted || stored.text != subject.message.text {
            return;
        }
        match score {
            Some(score) if score >= self.parts.flag_threshold => self.flag(subject, score).await,
            _ if subject.edited => self.clear(subject).await,
            _ => {}
        }
    }

    /// Flag the message, then report it to the moderators and the operators, when it was not
    /// flagged yet.
    async fn flag(&self, subject: &Subject<'_>, score: f64) {
        let message_id = subject.message.id;
        if !self.set_flag(subject, true).await {
            return;
        }
        let author_id = subject.message.author_id;
        tracing::info!(
            target: "audit",
            event = "message.flagged",
            message_id = %message_id,
            conversation_id = %subject.conversation_id,
            workspace_id = %subject.workspace_id,
            author_id = %author_id,
            score,
            "the automatic text art check flagged a message"
        );
        let report = self
            .parts
            .moderation_repo
            .create_report(NewMessageReport {
                server_id: subject.workspace_id,
                conversation_id: subject.conversation_id,
                message_id,
                reporter_user_id: None,
                reason: format!("automatic text art check: sexual {score:.2}"),
                franked: None,
            })
            .await;
        match report {
            Ok(_) => {}
            Err(e) if matches!(e.kind(), RepoErrorKind::Conflict) => {}
            Err(e) => {
                tracing::error!(error = %e, message_id = %message_id, "could not report a flagged message to the moderators");
            }
        }
        let principal_id = match self.parts.user_repo.get_by_id(author_id).await {
            Ok(author) => author.and_then(|author| author.principal_id),
            Err(e) => {
                tracing::error!(error = %e, author_id = %author_id, "could not look up the author of a flagged message");
                return;
            }
        };
        let Some(principal_id) = principal_id else {
            tracing::warn!(
                message_id = %message_id,
                author_id = %author_id,
                "a message was flagged, but its author has no principal id to report"
            );
            return;
        };
        let reason = format!(
            "automatic text art check: sexual {score:.2} — message {message_id} in conversation \
             {} of workspace {}",
            subject.conversation_id, subject.workspace_id
        );
        if let Err(e) = self
            .parts
            .relay_abuse
            .file_report(
                OperatorCaller::Server,
                FileAbuseReport {
                    subject_principal_id: Some(principal_id),
                    subject_server_id: None,
                    reason,
                },
            )
            .await
        {
            tracing::error!(error = %e, message_id = %message_id, "could not report a flagged message to the operators");
        }
    }

    /// Clear the message's flag after an edit; its reports stay as they are.
    async fn clear(&self, subject: &Subject<'_>) {
        if !self.set_flag(subject, false).await {
            return;
        }
        tracing::info!(
            target: "audit",
            event = "message.flag_cleared",
            message_id = %subject.message.id,
            conversation_id = %subject.conversation_id,
            workspace_id = %subject.workspace_id,
            by = "check",
            "the automatic text art check cleared a message's flag"
        );
    }

    /// Set or clear the stored flag and announce the change; `false` when nothing changed or
    /// the write failed.
    async fn set_flag(&self, subject: &Subject<'_>, flagged: bool) -> bool {
        let message_id = subject.message.id;
        let flagged_at = flagged.then(Utc::now);
        match self
            .parts
            .message_repo
            .set_flagged_at(message_id, flagged_at)
            .await
        {
            Ok(true) => {}
            Ok(false) => return false,
            Err(e) => {
                tracing::warn!(error = %e, message_id = %message_id, "text art check: could not store the flag");
                return false;
            }
        }
        self.parts
            .observers
            .emit(WorkspaceEvent::MessageFlagChanged {
                workspace_id: subject.workspace_id,
                conversation_id: subject.conversation_id,
                message_id,
                flagged,
                conversation_type: subject.conversation_type.to_owned(),
                is_private: subject.is_private,
                member_user_ids: subject.member_user_ids.to_vec(),
            });
        true
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;

    use rim_core::message::model::{Message, MessageId};
    use rim_core::message::repository::MockMessageRepository;
    use rim_core::moderation::model::{MessageReport, MessageReportId, MessageReportStatus};
    use rim_core::moderation::repository::MockModerationRepository;
    use rim_core::repository::error::RepoError;
    use rim_core::user::model::User;
    use rim_core::user::repository::MockUserRepository;
    use rim_services::ServiceError;
    use rim_services::events::{Broadcaster, MessageEncryptionData};
    use rim_services::relay::service::MockRelayAbuseService;

    use super::*;
    use crate::feature_gate::InstanceFeatureGate;
    use crate::text_art_check::TextArtCheckError;

    const PRINCIPAL: &str = "rimp1_author";

    #[derive(Default)]
    struct Recording(Mutex<Vec<WorkspaceEvent>>);

    impl Broadcaster for Recording {
        fn broadcast(&self, event: WorkspaceEvent) {
            self.0.lock().expect("events").push(event);
        }
    }

    /// A scorer that answers `score`, counts its calls and records the texts it was given.
    struct FakeScorer {
        score: Result<f64, TextArtCheckError>,
        calls: AtomicUsize,
        seen: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl TextScorer for FakeScorer {
        async fn score(&self, text: &str) -> Result<f64, TextArtCheckError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.seen.lock().expect("seen").push(text.to_string());
            self.score.clone()
        }
    }

    fn art() -> String {
        ["⣿⣿⣿⡿⠿⠿⢿⣿⣿⣿⣿⣿⣿⡿⠿⠿⢿⣿⣿⣿⣿⣿"; 8].join("\n")
    }

    fn event_data(text: &str) -> MessageEventData {
        MessageEventData {
            id: MessageId::new(),
            server_id: ServerId::new(),
            conversation_id: ConversationId::new(),
            thread_id: None,
            root_thread_id: None,
            author_id: UserId::new(),
            text: Some(text.to_string()),
            has_attachments: false,
            parent_message_id: None,
            forwarded_from_message_id: None,
            is_deleted: false,
            sent_at: Utc::now(),
            edited_at: None,
            reactions: vec![],
            encryption: None,
        }
    }

    fn new_message(workspace_id: ServerId, message: MessageEventData) -> WorkspaceEvent {
        WorkspaceEvent::NewMessage {
            workspace_id,
            conversation_id: message.conversation_id,
            conversation_type: "Channel".to_string(),
            member_user_ids: vec![message.author_id],
            is_private: true,
            message,
        }
    }

    fn updated(workspace_id: ServerId, message: MessageEventData) -> WorkspaceEvent {
        WorkspaceEvent::MessageUpdated {
            workspace_id,
            conversation_id: message.conversation_id,
            conversation_type: "Channel".to_string(),
            member_user_ids: vec![message.author_id],
            is_private: true,
            message,
        }
    }

    fn stored(data: &MessageEventData) -> Message {
        Message {
            id: data.id,
            conversation_id: data.conversation_id,
            thread_id: None,
            parent_message_id: None,
            forwarded_from_message_id: None,
            sent_at: data.sent_at,
            edited_at: None,
            sent_by: data.author_id,
            text: data.text.clone(),
            is_deleted: false,
            has_attachments: false,
            encryption: None,
            flagged_at: None,
        }
    }

    fn author(id: UserId, principal_id: Option<&str>) -> User {
        User {
            id,
            username: "author".to_string(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            last_login_at: None,
            display_name: None,
            avatar_url: None,
            status_text: None,
            bio: None,
            email: None,
            timezone: None,
            principal_id: principal_id.map(str::to_string),
            deletion_scheduled_for: None,
            profile_locked_until: None,
            handle: rim_core::user::model::UserHandle::Local,
        }
    }

    /// The check's collaborators; every repository call fails the test unless expected.
    struct Fx {
        workspace_id: ServerId,
        direct_space_id: ServerId,
        scorer: Arc<FakeScorer>,
        disabled: Vec<ServerFeature>,
        message_repo: MockMessageRepository,
        moderation_repo: MockModerationRepository,
        relay_abuse: MockRelayAbuseService,
        user_repo: MockUserRepository,
        events: Arc<Recording>,
    }

    impl Fx {
        fn new(score: Result<f64, TextArtCheckError>) -> Self {
            Self {
                workspace_id: ServerId::new(),
                direct_space_id: ServerId::new(),
                scorer: Arc::new(FakeScorer {
                    score,
                    calls: AtomicUsize::new(0),
                    seen: Mutex::new(Vec::new()),
                }),
                disabled: Vec::new(),
                message_repo: MockMessageRepository::new(),
                moderation_repo: MockModerationRepository::new(),
                relay_abuse: MockRelayAbuseService::new(),
                user_repo: MockUserRepository::new(),
                events: Arc::new(Recording::default()),
            }
        }

        /// The stored message reads back as `message`.
        fn storing(&mut self, message: Option<Message>) {
            self.message_repo.expect_get_by_id().returning(move |_| {
                let m = message.clone();
                Box::pin(async move { Ok(m) })
            });
        }

        /// `set_flagged_at` is called once with a time when `flag`, else with `None`, and
        /// answers `changed`.
        fn setting_flag(&mut self, flag: bool, changed: bool) {
            self.message_repo
                .expect_set_flagged_at()
                .withf(move |_, at| at.is_some() == flag)
                .times(1)
                .returning(move |_, _| Box::pin(async move { Ok(changed) }));
        }

        fn run(self) -> (Arc<FakeScorer>, Arc<Recording>, TextArtModeration) {
            let check = TextArtModeration::new(TextArtModerationParts {
                scorer: Arc::clone(&self.scorer) as Arc<dyn TextScorer>,
                flag_threshold: 0.6,
                direct_space_id: Some(self.direct_space_id),
                feature_gate: Arc::new(InstanceFeatureGate::new(&self.disabled, &[])),
                message_repo: Arc::new(self.message_repo),
                moderation_repo: Arc::new(self.moderation_repo),
                relay_abuse: Arc::new(self.relay_abuse),
                user_repo: Arc::new(self.user_repo),
                observers: Arc::new(Observers::new(
                    Arc::clone(&self.events) as Arc<dyn Broadcaster>
                )),
            });
            (self.scorer, self.events, check)
        }
    }

    fn flag_events(events: &Recording) -> Vec<(MessageId, bool)> {
        events
            .0
            .lock()
            .expect("events")
            .iter()
            .filter_map(|e| match e {
                WorkspaceEvent::MessageFlagChanged {
                    message_id,
                    flagged,
                    ..
                } => Some((*message_id, *flagged)),
                _ => None,
            })
            .collect()
    }

    #[tokio::test]
    async fn a_hit_flags_the_message_and_reports_it_to_moderators_and_operators() {
        let mut fx = Fx::new(Ok(0.8));
        let data = event_data(&art());
        let (message_id, author_id, workspace_id) = (data.id, data.author_id, fx.workspace_id);
        let conversation_id = data.conversation_id;
        fx.storing(Some(stored(&data)));
        fx.setting_flag(true, true);
        fx.moderation_repo
            .expect_create_report()
            .withf(move |r| {
                r.reporter_user_id.is_none()
                    && r.message_id == message_id
                    && r.server_id == workspace_id
                    && r.conversation_id == conversation_id
                    && r.reason == "automatic text art check: sexual 0.80"
                    && r.franked.is_none()
            })
            .times(1)
            .returning(|r| {
                Box::pin(async move {
                    Ok(MessageReport {
                        id: MessageReportId::new(),
                        server_id: r.server_id,
                        conversation_id: r.conversation_id,
                        message_id: r.message_id,
                        reporter_user_id: None,
                        reason: r.reason,
                        status: MessageReportStatus::Open,
                        resolved_by: None,
                        resolved_at: None,
                        created_at: Utc::now(),
                        franked: None,
                    })
                })
            });
        fx.user_repo
            .expect_get_by_id()
            .withf(move |id| *id == author_id)
            .returning(|id| Ok(Some(author(id, Some(PRINCIPAL)))));
        let reason = format!(
            "automatic text art check: sexual 0.80 — message {message_id} in conversation \
             {conversation_id} of workspace {workspace_id}"
        );
        fx.relay_abuse
            .expect_file_report()
            .withf(move |by, report| {
                *by == OperatorCaller::Server
                    && report.subject_principal_id.as_deref() == Some(PRINCIPAL)
                    && report.subject_server_id.is_none()
                    && report.reason == reason
            })
            .times(1)
            .returning(|_, _| Box::pin(async { Err(ServiceError::NotFound) }));
        let (scorer, events, check) = fx.run();

        check.handle(&new_message(workspace_id, data)).await;

        assert_eq!(scorer.calls.load(Ordering::SeqCst), 1);
        assert_eq!(flag_events(&events), [(message_id, true)]);
        let events = events.0.lock().expect("events");
        assert!(matches!(
            events.as_slice(),
            [WorkspaceEvent::MessageFlagChanged {
                workspace_id: w,
                conversation_id: c,
                conversation_type,
                is_private: true,
                member_user_ids,
                ..
            }] if *w == workspace_id && *c == conversation_id && conversation_type == "Channel"
                && member_user_ids.as_slice() == [author_id]
        ));
    }

    #[tokio::test]
    async fn a_second_hit_on_a_flagged_message_files_nothing() {
        let mut fx = Fx::new(Ok(0.9));
        let data = event_data(&art());
        fx.storing(Some(stored(&data)));
        fx.setting_flag(true, false);
        fx.moderation_repo.expect_create_report().never();
        fx.relay_abuse.expect_file_report().never();
        let workspace_id = fx.workspace_id;
        let (_, events, check) = fx.run();

        check.handle(&updated(workspace_id, data)).await;

        assert!(flag_events(&events).is_empty());
    }

    #[tokio::test]
    async fn an_automatic_report_that_exists_already_is_ignored() {
        let mut fx = Fx::new(Ok(0.9));
        let data = event_data(&art());
        fx.storing(Some(stored(&data)));
        fx.setting_flag(true, true);
        fx.moderation_repo
            .expect_create_report()
            .times(1)
            .returning(|_| Box::pin(async { Err(RepoError::new(RepoErrorKind::Conflict)) }));
        fx.user_repo
            .expect_get_by_id()
            .returning(|id| Ok(Some(author(id, None))));
        fx.relay_abuse.expect_file_report().never();
        let workspace_id = fx.workspace_id;
        let (_, events, check) = fx.run();

        check.handle(&new_message(workspace_id, data.clone())).await;

        assert_eq!(flag_events(&events), [(data.id, true)]);
    }

    #[tokio::test]
    async fn a_miss_on_a_new_message_does_nothing() {
        let mut fx = Fx::new(Ok(0.59));
        let data = event_data(&art());
        fx.storing(Some(stored(&data)));
        fx.message_repo.expect_set_flagged_at().never();
        let workspace_id = fx.workspace_id;
        let (scorer, events, check) = fx.run();

        check.handle(&new_message(workspace_id, data)).await;

        assert_eq!(scorer.calls.load(Ordering::SeqCst), 1);
        assert!(flag_events(&events).is_empty());
    }

    #[tokio::test]
    async fn only_the_art_block_is_scored() {
        let mut fx = Fx::new(Ok(0.59));
        let data = event_data(&format!("hey here is my new picture:\n{}", art()));
        fx.storing(Some(stored(&data)));
        let workspace_id = fx.workspace_id;
        let (scorer, _, check) = fx.run();

        check.handle(&new_message(workspace_id, data)).await;

        assert_eq!(*scorer.seen.lock().expect("seen"), [art()]);
    }

    #[tokio::test]
    async fn an_edit_below_the_threshold_clears_the_flag() {
        let mut fx = Fx::new(Ok(0.2));
        let data = event_data(&art());
        fx.storing(Some(stored(&data)));
        fx.setting_flag(false, true);
        fx.moderation_repo.expect_create_report().never();
        let workspace_id = fx.workspace_id;
        let (_, events, check) = fx.run();

        check.handle(&updated(workspace_id, data.clone())).await;

        assert_eq!(flag_events(&events), [(data.id, false)]);
    }

    #[tokio::test]
    async fn an_edit_that_is_no_longer_art_clears_the_flag_unscored() {
        let mut fx = Fx::new(Ok(0.9));
        let data = event_data("just words now");
        fx.storing(Some(stored(&data)));
        fx.setting_flag(false, true);
        let workspace_id = fx.workspace_id;
        let (scorer, events, check) = fx.run();

        check.handle(&updated(workspace_id, data.clone())).await;

        assert_eq!(scorer.calls.load(Ordering::SeqCst), 0);
        assert_eq!(flag_events(&events), [(data.id, false)]);
    }

    #[tokio::test]
    async fn clearing_an_unflagged_message_announces_nothing() {
        let mut fx = Fx::new(Ok(0.2));
        let data = event_data(&art());
        fx.storing(Some(stored(&data)));
        fx.setting_flag(false, false);
        let workspace_id = fx.workspace_id;
        let (_, events, check) = fx.run();

        check.handle(&updated(workspace_id, data)).await;

        assert!(flag_events(&events).is_empty());
    }

    #[tokio::test]
    async fn skipped_messages_are_never_scored() {
        let encrypted = {
            let mut data = event_data(&art());
            data.text = None;
            data.encryption = Some(MessageEncryptionData {
                incarnation: 1,
                epoch: 1,
                sender_device_id: uuid::Uuid::now_v7(),
                ciphertext: Some("AAAA".to_string()),
                franking_receipt: "AAAA".to_string(),
                franking_key_id: 1,
                sent_at_us: "0".to_string(),
            });
            data
        };
        let deleted = {
            let mut data = event_data(&art());
            data.is_deleted = true;
            data
        };
        let textless = {
            let mut data = event_data(&art());
            data.text = None;
            data
        };
        type Case = (
            &'static str,
            Box<dyn Fn(&Fx) -> WorkspaceEvent>,
            Vec<ServerFeature>,
        );
        let cases: Vec<Case> = vec![
            (
                "encrypted",
                Box::new(move |fx| new_message(fx.workspace_id, encrypted.clone())),
                vec![],
            ),
            (
                "deleted",
                Box::new(move |fx| updated(fx.workspace_id, deleted.clone())),
                vec![],
            ),
            (
                "without text",
                Box::new(move |fx| new_message(fx.workspace_id, textless.clone())),
                vec![],
            ),
            (
                "direct space",
                Box::new(|fx| new_message(fx.direct_space_id, event_data(&art()))),
                vec![],
            ),
            (
                "not art",
                Box::new(|fx| new_message(fx.workspace_id, event_data("hello there"))),
                vec![],
            ),
            (
                "feature off",
                Box::new(|fx| new_message(fx.workspace_id, event_data(&art()))),
                vec![ServerFeature::TextArtCheck],
            ),
        ];
        for (name, event, disabled) in cases {
            let mut fx = Fx::new(Ok(1.0));
            fx.disabled = disabled;
            fx.message_repo.expect_get_by_id().never();
            fx.message_repo.expect_set_flagged_at().never();
            let event = event(&fx);
            let (scorer, events, check) = fx.run();

            check.handle(&event).await;

            assert_eq!(scorer.calls.load(Ordering::SeqCst), 0, "{name}");
            assert!(flag_events(&events).is_empty(), "{name}");
        }
    }

    #[tokio::test]
    async fn an_unavailable_scorer_leaves_the_message_unflagged() {
        let mut fx = Fx::new(Err(TextArtCheckError::Unavailable("503".to_string())));
        fx.message_repo.expect_get_by_id().never();
        fx.message_repo.expect_set_flagged_at().never();
        let workspace_id = fx.workspace_id;
        let (scorer, events, check) = fx.run();

        check
            .handle(&updated(workspace_id, event_data(&art())))
            .await;

        assert_eq!(scorer.calls.load(Ordering::SeqCst), 1);
        assert!(flag_events(&events).is_empty());
    }

    #[tokio::test]
    async fn text_changed_during_the_check_does_nothing() {
        type Reread = fn(&MessageEventData) -> Option<Message>;
        let cases: [(&str, Reread); 3] = [
            ("edited", |data| {
                let mut m = stored(data);
                m.text = Some(format!("{}\n⣿⣿", art()));
                Some(m)
            }),
            ("deleted", |data| {
                let mut m = stored(data);
                m.is_deleted = true;
                Some(m)
            }),
            ("gone", |_| None),
        ];
        for (name, message) in cases {
            let mut fx = Fx::new(Ok(0.95));
            let data = event_data(&art());
            fx.storing(message(&data));
            fx.message_repo.expect_set_flagged_at().never();
            let workspace_id = fx.workspace_id;
            let (_, events, check) = fx.run();

            check.handle(&updated(workspace_id, data)).await;

            assert!(flag_events(&events).is_empty(), "{name}");
        }
    }

    #[tokio::test]
    async fn other_events_are_ignored() {
        let mut fx = Fx::new(Ok(1.0));
        fx.message_repo.expect_get_by_id().never();
        let workspace_id = fx.workspace_id;
        let (scorer, _, check) = fx.run();

        check
            .handle(&WorkspaceEvent::MessageDeleted {
                workspace_id,
                conversation_id: ConversationId::new(),
                conversation_type: "Channel".to_string(),
                member_user_ids: vec![],
                is_private: false,
                message_id: MessageId::new(),
                thread_id: None,
            })
            .await;

        assert_eq!(scorer.calls.load(Ordering::SeqCst), 0);
    }
}
