use async_trait::async_trait;
use chrono::{TimeDelta, Utc};
use rim_infra_db::begin_transaction;
use sea_orm::DatabaseConnection;

use rim_services::events::{EventObserver, Observers, WorkspaceEvent};
use rim_services::read_state::dto::{
    MarkAllAsReadCmd, MarkAsReadCmd, MarkAsUnreadCmd, UnreadCountDto, UnreadCountsQuery,
};
use rim_services::read_state::service::ReadStateService;
use rim_services::{ServiceError, ServiceResult};

use crate::ReadStateRepos;
use crate::repos::AppRepos;
use crate::util::{
    ConversationAccessRepos, ensure_conversation_member_access, ensure_server_member,
};

#[derive(Clone)]
pub struct DefaultReadStateService {
    repos: ReadStateRepos,
    observers: Observers,
    /// When set, `mark_as_read` writes the read state and marks the conversation's
    /// notifications read in one transaction. `None` in unit tests.
    db: Option<DatabaseConnection>,
}

impl DefaultReadStateService {
    pub fn new(repos: ReadStateRepos, observers: Observers) -> Self {
        Self {
            repos,
            observers,
            db: None,
        }
    }

    pub fn with_db(mut self, db: DatabaseConnection) -> Self {
        self.db = Some(db);
        self
    }
}

#[async_trait]
impl ReadStateService for DefaultReadStateService {
    async fn mark_as_read(&self, cmd: MarkAsReadCmd) -> ServiceResult<()> {
        // Clamp read_at: never allow a future timestamp.
        let now = Utc::now();
        let read_at = cmd.read_at.min(now);

        ensure_conversation_member_access(
            ConversationAccessRepos::from_arcs(
                &self.repos.server_member_repo,
                &self.repos.conversation_repo,
                &self.repos.conversation_member_repo,
            ),
            cmd.actor.user_id,
            cmd.workspace_id,
            cmd.conversation_id,
        )
        .await?;

        // Upsert with forward-only guard (handled at DB level), and mark the conversation's
        // notifications up to `read_at` read, in one transaction when a database
        // connection is available.
        let scope = match &self.db {
            Some(db) => Some(
                begin_transaction(db)
                    .await
                    .map_err(ServiceError::internal)?,
            ),
            None => None,
        };
        let repos = match &scope {
            Some(scope) => AppRepos::with_handle(&scope.handle()).for_read_state(),
            None => self.repos.clone(),
        };
        repos
            .read_state_repo
            .upsert(cmd.actor.user_id, cmd.conversation_id, read_at)
            .await?;
        repos
            .notification_repo
            .mark_read_for_conversation(cmd.actor.user_id, cmd.conversation_id, read_at, now)
            .await?;
        if let Some(scope) = scope {
            scope.commit().await.map_err(ServiceError::internal)?;
        }

        self.observers.emit(WorkspaceEvent::ReadStateUpdated {
            workspace_id: cmd.workspace_id,
            conversation_id: cmd.conversation_id,
            user_id: cmd.actor.user_id,
            last_read_at: read_at,
        });

        Ok(())
    }

    async fn mark_as_unread(&self, cmd: MarkAsUnreadCmd) -> ServiceResult<()> {
        ensure_conversation_member_access(
            ConversationAccessRepos::from_arcs(
                &self.repos.server_member_repo,
                &self.repos.conversation_repo,
                &self.repos.conversation_member_repo,
            ),
            cmd.actor.user_id,
            cmd.workspace_id,
            cmd.conversation_id,
        )
        .await?;

        let message = self
            .repos
            .message_repo
            .get_by_id(cmd.message_id)
            .await?
            .ok_or(ServiceError::NotFound)?;
        if message.conversation_id != cmd.conversation_id || message.is_deleted {
            return Err(ServiceError::NotFound);
        }
        if message.thread_id.is_some() {
            return Err(ServiceError::Validation(
                "a thread reply cannot be marked unread".to_string(),
            ));
        }

        // Unread means sent after the marker, so the marker goes just before the message.
        let rewound = self
            .repos
            .read_state_repo
            .rewind(
                cmd.actor.user_id,
                cmd.conversation_id,
                message.sent_at - TimeDelta::microseconds(1),
            )
            .await?;
        if rewound {
            self.observers.emit(WorkspaceEvent::ReadMarkerRewound {
                workspace_id: cmd.workspace_id,
                conversation_id: cmd.conversation_id,
                user_id: cmd.actor.user_id,
            });
        }

        Ok(())
    }

    async fn mark_all_as_read(&self, cmd: MarkAllAsReadCmd) -> ServiceResult<()> {
        let now = Utc::now();

        // Verify server membership.
        ensure_server_member(
            self.repos.server_member_repo.as_ref(),
            cmd.workspace_id,
            cmd.actor.user_id,
        )
        .await?;

        // Conversations the user is a member of — the same set the unread counts cover. Public
        // channels they can read but never joined have no unread state to clear.
        let conversation_ids = self
            .repos
            .conversation_repo
            .list_joined_conversation_ids(cmd.workspace_id, cmd.actor.user_id)
            .await?;

        if conversation_ids.is_empty() {
            return Ok(());
        }

        // Bulk upsert with forward-only guard.
        self.repos
            .read_state_repo
            .mark_all_read(cmd.actor.user_id, &conversation_ids, now)
            .await?;
        // Everything is read now, so are the workspace's notifications.
        self.repos
            .notification_repo
            .mark_read_before(cmd.actor.user_id, cmd.workspace_id, now, now)
            .await?;

        for conv_id in &conversation_ids {
            self.observers.emit(WorkspaceEvent::ReadStateUpdated {
                workspace_id: cmd.workspace_id,
                conversation_id: *conv_id,
                user_id: cmd.actor.user_id,
                last_read_at: now,
            });
        }

        Ok(())
    }

    async fn get_unread_counts(
        &self,
        query: UnreadCountsQuery,
    ) -> ServiceResult<Vec<UnreadCountDto>> {
        // Verify server membership.
        ensure_server_member(
            self.repos.server_member_repo.as_ref(),
            query.workspace_id,
            query.actor.user_id,
        )
        .await?;

        // Only conversations the user is a member of (joined public channels, private
        // channels, DMs, groups): unjoined public channels are readable but not followed, so
        // they carry no unread badge.
        let conversation_ids = self
            .repos
            .conversation_repo
            .list_joined_conversation_ids(query.workspace_id, query.actor.user_id)
            .await?;

        if conversation_ids.is_empty() {
            return Ok(vec![]);
        }

        // Batch query unread counts.
        let counts = self
            .repos
            .read_state_repo
            .get_unread_counts(query.actor.user_id, &conversation_ids)
            .await?;

        Ok(counts
            .into_iter()
            .map(|unread| UnreadCountDto {
                conversation_id: unread.conversation_id,
                count: unread.count,
                unread_after: unread.unread_after,
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use chrono::Utc;
    use rim_core::conversation::model::{
        Conversation, ConversationId, ConversationType, PostingPolicy,
    };
    use rim_core::conversation::repository::MockConversationRepository;
    use rim_core::conversation_member::repository::MockConversationMemberRepository;
    use rim_core::conversation_read_state::repository::{
        ConversationUnread, MockConversationReadStateRepository,
    };
    use rim_core::message::model::{Message, MessageId};
    use rim_core::message::repository::MockMessageRepository;
    use rim_core::message_thread::model::MessageThreadId;
    use rim_core::notification::repository::MockNotificationRepository;
    use rim_core::server::model::ServerId;
    use rim_core::server_member::model::{MemberStanding, ServerMemberRole};
    use rim_core::server_member::repository::MockServerMemberRepository;
    use rim_core::user::model::UserId;
    use rim_services::events::{Broadcaster, NoopBroadcaster, WorkspaceEvent};
    use rim_services::read_state::dto::{MarkAllAsReadCmd, MarkAsUnreadCmd, UnreadCountsQuery};
    use rim_services::read_state::service::ReadStateService;
    use rim_services::{Actor, ServiceError, ServiceResult};

    use super::DefaultReadStateService;
    use crate::ReadStateRepos;

    /// Unread counts and mark-all-read cover only the conversations the user is a
    /// member of — never every public channel they could read.
    fn service(
        joined: Vec<ConversationId>,
        read_state_repo: MockConversationReadStateRepository,
    ) -> DefaultReadStateService {
        service_with(joined, read_state_repo, MockNotificationRepository::new())
    }

    fn service_with(
        joined: Vec<ConversationId>,
        read_state_repo: MockConversationReadStateRepository,
        notification_repo: MockNotificationRepository,
    ) -> DefaultReadStateService {
        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_is_member()
            .returning(|_, _| Ok(true));
        let mut conversation_repo = MockConversationRepository::new();
        conversation_repo
            .expect_list_joined_conversation_ids()
            .returning(move |_, _| {
                let joined = joined.clone();
                Box::pin(async move { Ok(joined) })
            });
        conversation_repo
            .expect_list_accessible_conversation_ids()
            .never();
        DefaultReadStateService::new(
            ReadStateRepos {
                read_state_repo: Arc::new(read_state_repo),
                conversation_repo: Arc::new(conversation_repo),
                server_member_repo: Arc::new(server_member_repo),
                conversation_member_repo: Arc::new(MockConversationMemberRepository::new()),
                notification_repo: Arc::new(notification_repo),
                message_repo: Arc::new(MockMessageRepository::new()),
            },
            rim_services::events::Observers::new(Arc::new(NoopBroadcaster)),
        )
    }

    /// Records every event a service emits.
    #[derive(Default)]
    struct RecordingBroadcaster(Mutex<Vec<WorkspaceEvent>>);

    impl Broadcaster for RecordingBroadcaster {
        fn broadcast(&self, event: WorkspaceEvent) {
            self.0.lock().expect("events").push(event);
        }
    }

    /// What `mark_as_unread` finds: who is calling, the conversation and the message.
    struct UnreadCase {
        workspace: ServerId,
        actor: UserId,
        conversation: Conversation,
        message: Option<Message>,
        standing: Option<MemberStanding>,
        conversation_member: bool,
        read_state_repo: MockConversationReadStateRepository,
    }

    impl UnreadCase {
        /// A member of `workspace` marking a public channel's root message unread.
        fn new() -> Self {
            let workspace = ServerId::new();
            let conversation = Conversation {
                id: ConversationId::new(),
                server_id: workspace,
                conversation_type: ConversationType::Channel,
                title: None,
                topic: None,
                created_at: Utc::now(),
                last_activity_at: Utc::now(),
                is_private: false,
                archived_at: None,
                created_by: None,
                slow_mode_seconds: 0,
                posting_policy: PostingPolicy::Everyone,
                posting_allows_threads: true,
                encrypted_at: None,
            };
            let message = Message {
                id: MessageId::new(),
                conversation_id: conversation.id,
                thread_id: None,
                parent_message_id: None,
                forwarded_from_message_id: None,
                sent_at: Utc::now(),
                edited_at: None,
                sent_by: UserId::new(),
                text: Some("hello".to_string()),
                is_deleted: false,
                has_attachments: false,
                encryption: None,
                flagged_at: None,
            };
            Self {
                workspace,
                actor: UserId::new(),
                conversation,
                message: Some(message),
                standing: Some(MemberStanding {
                    role: ServerMemberRole::Member,
                    timed_out_until: None,
                }),
                conversation_member: true,
                read_state_repo: MockConversationReadStateRepository::new(),
            }
        }

        /// The message the call names: the seeded one, or a new id when there is none.
        fn message_id(&self) -> MessageId {
            self.message.as_ref().map_or_else(MessageId::new, |m| m.id)
        }

        /// Run `mark_as_unread`, returning its result and the events it emitted.
        async fn run(self) -> (ServiceResult<()>, Vec<WorkspaceEvent>) {
            let cmd = MarkAsUnreadCmd {
                actor: Actor::new(self.actor),
                workspace_id: self.workspace,
                conversation_id: self.conversation.id,
                message_id: self.message_id(),
            };
            let mut server_member_repo = MockServerMemberRepository::new();
            let standing = self.standing;
            server_member_repo
                .expect_get_member_standing()
                .returning(move |_, _| Ok(standing));
            server_member_repo
                .expect_join_direct_space()
                .returning(|_, _| Ok(false));
            let mut conversation_repo = MockConversationRepository::new();
            let conversation = self.conversation;
            conversation_repo.expect_get_by_id().returning(move |_| {
                let conversation = conversation.clone();
                Box::pin(async move { Ok(Some(conversation)) })
            });
            let mut conversation_member_repo = MockConversationMemberRepository::new();
            let is_member = self.conversation_member;
            conversation_member_repo
                .expect_is_member()
                .returning(move |_, _| Box::pin(async move { Ok(is_member) }));
            let mut message_repo = MockMessageRepository::new();
            let message = self.message;
            message_repo.expect_get_by_id().returning(move |_| {
                let message = message.clone();
                Box::pin(async move { Ok(message) })
            });
            let events = Arc::new(RecordingBroadcaster::default());
            let service = DefaultReadStateService::new(
                ReadStateRepos {
                    read_state_repo: Arc::new(self.read_state_repo),
                    conversation_repo: Arc::new(conversation_repo),
                    server_member_repo: Arc::new(server_member_repo),
                    conversation_member_repo: Arc::new(conversation_member_repo),
                    notification_repo: Arc::new(MockNotificationRepository::new()),
                    message_repo: Arc::new(message_repo),
                },
                rim_services::events::Observers::new(Arc::clone(&events) as Arc<dyn Broadcaster>),
            );
            let result = service.mark_as_unread(cmd).await;
            let emitted = events.0.lock().expect("events").clone();
            (result, emitted)
        }

        /// Run a call that must be refused, returning the error. Nothing is written or emitted.
        async fn refused(mut self) -> ServiceError {
            self.read_state_repo.expect_rewind().never();
            let (result, events) = self.run().await;
            assert!(events.is_empty(), "{events:?}");
            result.expect_err("refused")
        }
    }

    #[tokio::test]
    async fn mark_as_unread_rewinds_to_just_before_the_message_and_tells_the_owner() {
        let mut case = UnreadCase::new();
        let (actor, workspace, conversation) = (case.actor, case.workspace, case.conversation.id);
        let sent_at = case.message.as_ref().expect("message").sent_at;
        case.read_state_repo
            .expect_rewind()
            .withf(move |user, id, to| {
                *user == actor
                    && *id == conversation
                    && *to == sent_at - chrono::TimeDelta::microseconds(1)
            })
            .times(1)
            .returning(|_, _, _| Box::pin(async { Ok(true) }));
        let (result, events) = case.run().await;
        result.expect("marked unread");
        let [
            WorkspaceEvent::ReadMarkerRewound {
                workspace_id,
                conversation_id,
                user_id,
            },
        ] = events.as_slice()
        else {
            panic!("expected one ReadMarkerRewound, got {events:?}");
        };
        assert_eq!(
            (*workspace_id, *conversation_id, *user_id),
            (workspace, conversation, actor)
        );
    }

    #[tokio::test]
    async fn mark_as_unread_emits_nothing_when_the_marker_stays() {
        let mut case = UnreadCase::new();
        case.read_state_repo
            .expect_rewind()
            .times(1)
            .returning(|_, _, _| Box::pin(async { Ok(false) }));
        let (result, events) = case.run().await;
        result.expect("nothing to do is not an error");
        assert!(events.is_empty(), "{events:?}");
    }

    #[tokio::test]
    async fn mark_as_unread_refuses_someone_outside_the_workspace() {
        let mut case = UnreadCase::new();
        case.standing = None;
        assert!(matches!(
            case.refused().await,
            ServiceError::PermissionDenied
        ));
    }

    #[tokio::test]
    async fn mark_as_unread_answers_not_found_for_a_conversation_of_another_workspace() {
        let mut case = UnreadCase::new();
        case.conversation.server_id = ServerId::new();
        assert!(matches!(case.refused().await, ServiceError::NotFound));
    }

    #[tokio::test]
    async fn mark_as_unread_answers_not_found_for_a_private_channel_of_others() {
        let mut case = UnreadCase::new();
        case.conversation.is_private = true;
        case.conversation_member = false;
        assert!(matches!(case.refused().await, ServiceError::NotFound));
    }

    #[tokio::test]
    async fn mark_as_unread_refuses_a_dm_of_others() {
        let mut case = UnreadCase::new();
        case.conversation.conversation_type = ConversationType::Direct;
        case.conversation_member = false;
        assert!(matches!(
            case.refused().await,
            ServiceError::PermissionDenied
        ));
    }

    #[tokio::test]
    async fn mark_as_unread_answers_not_found_for_a_message_that_does_not_exist() {
        let mut case = UnreadCase::new();
        case.message = None;
        assert!(matches!(case.refused().await, ServiceError::NotFound));
    }

    #[tokio::test]
    async fn mark_as_unread_answers_not_found_for_a_message_of_another_conversation() {
        let mut case = UnreadCase::new();
        case.message.as_mut().expect("message").conversation_id = ConversationId::new();
        assert!(matches!(case.refused().await, ServiceError::NotFound));
    }

    #[tokio::test]
    async fn mark_as_unread_answers_not_found_for_a_deleted_message() {
        let mut case = UnreadCase::new();
        case.message.as_mut().expect("message").is_deleted = true;
        assert!(matches!(case.refused().await, ServiceError::NotFound));
    }

    #[tokio::test]
    async fn mark_as_unread_refuses_a_thread_reply_as_validation() {
        let mut case = UnreadCase::new();
        case.message.as_mut().expect("message").thread_id = Some(MessageThreadId::new());
        let err = case.refused().await;
        let ServiceError::Validation(message) = err else {
            panic!("expected Validation, got {err:?}");
        };
        assert_eq!(message, "a thread reply cannot be marked unread");
    }

    #[tokio::test]
    async fn unread_counts_cover_joined_conversations_only() {
        let joined = vec![ConversationId::new(), ConversationId::new()];
        let expected = joined.clone();
        let mut read_state_repo = MockConversationReadStateRepository::new();
        read_state_repo
            .expect_get_unread_counts()
            .withf(move |_, ids| ids == expected.as_slice())
            .returning(|_, ids| {
                let counts = ids
                    .iter()
                    .map(|id| ConversationUnread {
                        conversation_id: *id,
                        count: 1,
                        unread_after: Utc::now(),
                    })
                    .collect();
                Box::pin(async move { Ok(counts) })
            });

        let counts = service(joined.clone(), read_state_repo)
            .get_unread_counts(UnreadCountsQuery {
                actor: Actor::new(UserId::new()),
                workspace_id: ServerId::new(),
            })
            .await
            .expect("unread counts");
        let ids: Vec<_> = counts.iter().map(|c| c.conversation_id).collect();
        assert_eq!(ids, joined);
    }

    #[tokio::test]
    async fn unread_counts_empty_without_memberships() {
        let mut read_state_repo = MockConversationReadStateRepository::new();
        read_state_repo.expect_get_unread_counts().never();
        let counts = service(vec![], read_state_repo)
            .get_unread_counts(UnreadCountsQuery {
                actor: Actor::new(UserId::new()),
                workspace_id: ServerId::new(),
            })
            .await
            .expect("unread counts");
        assert!(counts.is_empty());
    }

    #[tokio::test]
    async fn mark_all_as_read_covers_joined_conversations_only() {
        let joined = vec![ConversationId::new()];
        let expected = joined.clone();
        let mut read_state_repo = MockConversationReadStateRepository::new();
        read_state_repo
            .expect_mark_all_read()
            .withf(move |_, ids, _| ids == expected.as_slice())
            .returning(|_, _, _| Box::pin(async { Ok(()) }));
        // Marking everything read also marks the workspace's notifications read.
        let workspace = ServerId::new();
        let actor = UserId::new();
        let mut notification_repo = MockNotificationRepository::new();
        notification_repo
            .expect_mark_read_before()
            .withf(move |user, server, before, read_at| {
                *user == actor && *server == workspace && before == read_at
            })
            .times(1)
            .returning(|_, _, _, _| Box::pin(async { Ok(3) }));
        service_with(joined, read_state_repo, notification_repo)
            .mark_all_as_read(MarkAllAsReadCmd {
                actor: Actor::new(actor),
                workspace_id: workspace,
            })
            .await
            .expect("mark all as read");
    }
}
