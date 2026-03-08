#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use rim_core::conversation::model::{Conversation, ConversationId};
    use rim_core::message::model::{Message, MessageId};
    use rim_core::message_thread::model::{MessageThread, MessageThreadId};
    use rim_core::repository::pagination::{Page, PageDir};
    use rim_core::server::model::ServerId;
    use rim_core::user::model::UserId;
    use rim_services::message_thread::command::{
        CreateThreadCmd, GetThreadByRootMessageQuery, GetThreadQuery, ListThreadsQuery,
    };
    use rim_services::message_thread::service::ThreadService;
    use rim_services::{Actor, ServiceError};

    use crate::test_support::thread::{
        MockConversationMemberRepo, MockConversationRepo, MockMessageRepo, MockServerMemberRepo,
        MockThreadRepo, ThreadReposPorts,
    };
    use crate::thread::DefaultThreadService;

    fn conversation(server_id: ServerId) -> Conversation {
        Conversation {
            id: ConversationId::new(),
            server_id,
            conversation_type: rim_core::conversation::model::ConversationType::Channel,
            created_at: chrono::Utc::now(),
            last_activity_at: chrono::Utc::now(),
        }
    }

    fn thread(conv_id: ConversationId) -> MessageThread {
        let now = chrono::Utc::now();
        MessageThread {
            id: MessageThreadId::new(),
            conversation_id: conv_id,
            first_message_id: None,
            created_at: now,
            last_message_at: now,
        }
    }

    #[tokio::test]
    async fn get_thread_denies_non_member() {
        let actor_id = UserId::new();
        let server_id = ServerId::new();

        let conversation_repo = MockConversationRepo::new();
        let conversation_member_repo = MockConversationMemberRepo::new();
        let message_repo = MockMessageRepo::new();
        let message_thread_repo = MockThreadRepo::new();
        let mut server_member_repo = MockServerMemberRepo::new();
        server_member_repo
            .expect_is_member()
            .returning(|_sid, _uid| Ok(false));

        let svc = DefaultThreadService::new_for_tests(ThreadReposPorts {
            conversation_repo: Arc::new(conversation_repo),
            conversation_member_repo: Arc::new(conversation_member_repo),
            message_repo: Arc::new(message_repo),
            message_thread_repo: Arc::new(message_thread_repo),
            server_member_repo: Arc::new(server_member_repo),
        });

        let err = svc
            .get_thread(GetThreadQuery {
                actor: Actor::new(actor_id),
                server_id,
                conversation_id: ConversationId::new(),
                thread_id: MessageThreadId::new(),
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::PermissionDenied));
    }

    #[tokio::test]
    async fn create_thread_creates_or_returns_existing_thread_for_timeline_message() {
        let actor_id = UserId::new();
        let server_id = ServerId::new();
        let conv = conversation(server_id);
        let conv_id = conv.id;
        let root_message_id = MessageId::new();
        let root_message = Message {
            id: root_message_id,
            conversation_id: conv_id,
            thread_id: None,
            sent_by: actor_id,
            text: Some("root".to_string()),
            has_attachments: false,
            parent_message_id: None,
            is_deleted: false,
            sent_at: chrono::Utc::now(),
        };
        let created_thread = thread(conv_id);

        let mut conversation_repo = MockConversationRepo::new();
        let conv_for_repo = conv.clone();
        conversation_repo
            .expect_get_by_id()
            .returning(move |_id| Ok(Some(conv_for_repo.clone())));

        let mut conversation_member_repo = MockConversationMemberRepo::new();
        conversation_member_repo
            .expect_is_member()
            .returning(|_cid, _uid| Ok(true));

        let mut message_repo = MockMessageRepo::new();
        let root_for_repo = root_message.clone();
        message_repo
            .expect_get_by_id()
            .returning(move |_id| Ok(Some(root_for_repo.clone())));

        let mut message_thread_repo = MockThreadRepo::new();
        message_thread_repo
            .expect_get_by_root_message()
            .return_once(move |_mid| Ok(None));
        let thread_for_create = created_thread.clone();
        message_thread_repo
            .expect_create_thread_for_root()
            .return_once(move |_cid, _mid| Ok(thread_for_create.clone()));

        let mut server_member_repo = MockServerMemberRepo::new();
        server_member_repo
            .expect_is_member()
            .returning(|_sid, _uid| Ok(true));

        let svc = DefaultThreadService::new_for_tests(ThreadReposPorts {
            conversation_repo: Arc::new(conversation_repo),
            conversation_member_repo: Arc::new(conversation_member_repo),
            message_repo: Arc::new(message_repo),
            message_thread_repo: Arc::new(message_thread_repo),
            server_member_repo: Arc::new(server_member_repo),
        });

        let dto = svc
            .create_thread(CreateThreadCmd {
                actor: Actor::new(actor_id),
                server_id,
                conversation_id: conv_id,
                root_message_id,
            })
            .await
            .unwrap();

        assert_eq!(dto.id, created_thread.id);
        assert_eq!(dto.root_message_id, created_thread.first_message_id);
    }

    #[tokio::test]
    async fn get_thread_by_root_message_returns_existing_thread() {
        let actor_id = UserId::new();
        let server_id = ServerId::new();
        let conv = conversation(server_id);
        let conv_id = conv.id;
        let root_message_id = MessageId::new();
        let root_message = Message {
            id: root_message_id,
            conversation_id: conv_id,
            thread_id: None,
            sent_by: actor_id,
            text: Some("root".to_string()),
            has_attachments: false,
            parent_message_id: None,
            is_deleted: false,
            sent_at: chrono::Utc::now(),
        };
        let existing_thread = thread(conv_id);

        let mut conversation_repo = MockConversationRepo::new();
        let conv_for_repo = conv.clone();
        conversation_repo
            .expect_get_by_id()
            .returning(move |_| Ok(Some(conv_for_repo.clone())));

        let mut conversation_member_repo = MockConversationMemberRepo::new();
        conversation_member_repo
            .expect_is_member()
            .returning(|_, _| Ok(true));

        let mut message_repo = MockMessageRepo::new();
        let root_for_repo = root_message.clone();
        message_repo
            .expect_get_by_id()
            .return_once(move |_| Ok(Some(root_for_repo.clone())));

        let mut message_thread_repo = MockThreadRepo::new();
        let existing_for_repo = existing_thread.clone();
        message_thread_repo
            .expect_get_by_root_message()
            .return_once(move |_| Ok(Some(existing_for_repo.clone())));

        let mut server_member_repo = MockServerMemberRepo::new();
        server_member_repo
            .expect_is_member()
            .returning(|_, _| Ok(true));

        let svc = DefaultThreadService::new_for_tests(ThreadReposPorts {
            conversation_repo: Arc::new(conversation_repo),
            conversation_member_repo: Arc::new(conversation_member_repo),
            message_repo: Arc::new(message_repo),
            message_thread_repo: Arc::new(message_thread_repo),
            server_member_repo: Arc::new(server_member_repo),
        });

        let dto = svc
            .get_thread_by_root_message(GetThreadByRootMessageQuery {
                actor: Actor::new(actor_id),
                server_id,
                conversation_id: conv_id,
                root_message_id,
            })
            .await
            .unwrap()
            .unwrap();

        assert_eq!(dto.id, existing_thread.id);
    }

    #[tokio::test]
    async fn list_threads_maps_page() {
        let actor_id = UserId::new();
        let server_id = ServerId::new();
        let conv = conversation(server_id);
        let conv_id = conv.id;
        let root_id = MessageId::new();
        let mut t = thread(conv_id);
        t.first_message_id = Some(root_id);
        let t_id = t.id;

        let mut conversation_repo = MockConversationRepo::new();
        let conv_for_repo = conv.clone();
        conversation_repo
            .expect_get_by_id()
            .returning(move |_id| Ok(Some(conv_for_repo.clone())));

        let mut conversation_member_repo = MockConversationMemberRepo::new();
        conversation_member_repo
            .expect_is_member()
            .returning(|_cid, _uid| Ok(true));

        let mut message_repo = MockMessageRepo::new();
        message_repo.expect_get_by_ids().return_once(move |ids| {
            let mut out = std::collections::HashMap::new();
            if ids.contains(&root_id) {
                out.insert(
                    root_id,
                    Message {
                        id: root_id,
                        conversation_id: conv_id,
                        thread_id: None,
                        sent_by: actor_id,
                        text: Some("root".to_string()),
                        has_attachments: false,
                        parent_message_id: None,
                        is_deleted: false,
                        sent_at: chrono::Utc::now(),
                    },
                );
            }
            Ok(out)
        });

        let mut message_thread_repo = MockThreadRepo::new();
        let t_for_repo = t.clone();
        message_thread_repo
            .expect_list_threads_for_conversation()
            .returning(move |_cid, _limit, _dir, _cursor| {
                Ok(Page::new(vec![t_for_repo.clone()], None, None))
            });

        let mut server_member_repo = MockServerMemberRepo::new();
        server_member_repo
            .expect_is_member()
            .returning(|_sid, _uid| Ok(true));

        let svc = DefaultThreadService::new_for_tests(ThreadReposPorts {
            conversation_repo: Arc::new(conversation_repo),
            conversation_member_repo: Arc::new(conversation_member_repo),
            message_repo: Arc::new(message_repo),
            message_thread_repo: Arc::new(message_thread_repo),
            server_member_repo: Arc::new(server_member_repo),
        });

        let page = svc
            .list_threads(ListThreadsQuery {
                actor: Actor::new(actor_id),
                server_id,
                conversation_id: conv_id,
                limit: 10,
                dir: PageDir::After,
                cursor: None,
            })
            .await
            .unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].id, t_id);
    }

    #[tokio::test]
    async fn get_thread_not_found_when_wrong_conversation() {
        let actor_id = UserId::new();
        let server_id = ServerId::new();
        let conv = conversation(server_id);

        let mut conversation_repo = MockConversationRepo::new();
        let conv_for_repo = conv.clone();
        conversation_repo
            .expect_get_by_id()
            .returning(move |_id| Ok(Some(conv_for_repo.clone())));

        let mut conversation_member_repo = MockConversationMemberRepo::new();
        conversation_member_repo
            .expect_is_member()
            .returning(|_cid, _uid| Ok(true));

        let message_repo = MockMessageRepo::new();

        let other_conv_id = ConversationId::new();
        let now = chrono::Utc::now();
        let mut message_thread_repo = MockThreadRepo::new();
        message_thread_repo
            .expect_get_by_id()
            .returning(move |_id| {
                Ok(Some(MessageThread {
                    id: MessageThreadId::new(),
                    conversation_id: other_conv_id,
                    first_message_id: None,
                    created_at: now,
                    last_message_at: now,
                }))
            });

        let mut server_member_repo = MockServerMemberRepo::new();
        server_member_repo
            .expect_is_member()
            .returning(|_sid, _uid| Ok(true));

        let svc = DefaultThreadService::new_for_tests(ThreadReposPorts {
            conversation_repo: Arc::new(conversation_repo),
            conversation_member_repo: Arc::new(conversation_member_repo),
            message_repo: Arc::new(message_repo),
            message_thread_repo: Arc::new(message_thread_repo),
            server_member_repo: Arc::new(server_member_repo),
        });

        let err = svc
            .get_thread(GetThreadQuery {
                actor: Actor::new(actor_id),
                server_id,
                conversation_id: conv.id,
                thread_id: MessageThreadId::new(),
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::NotFound));

        let _ = Message {
            // keep Message type referenced so imports stay stable under coverage
            id: MessageId::new(),
            conversation_id: conv.id,
            thread_id: Some(MessageThreadId::new()),
            sent_by: UserId::new(),
            text: None,
            has_attachments: false,
            parent_message_id: None,
            is_deleted: false,
            sent_at: now,
        };
    }
}
