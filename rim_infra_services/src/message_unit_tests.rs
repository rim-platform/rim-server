#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use rim_core::conversation::model::{Conversation, ConversationId};
    use rim_core::message::model::{Message, MessageId, NewMessage};
    use rim_core::message_thread::model::{MessageThread, MessageThreadId};
    use rim_core::repository::pagination::{Page, PageDir, SortOrder};
    use rim_core::server::model::ServerId;
    use rim_core::user::model::UserId;
    use rim_services::message::command::{
        DeleteMessageCmd, EditMessageCmd, GetMessageQuery, ListMessagesQuery, SendMessageCmd,
    };
    use rim_services::message::service::MessageService;
    use rim_services::{Actor, ServiceError};

    use crate::message::DefaultMessageService;
    use crate::test_support::message::{
        MessageReposPorts, MockConversationMemberRepo, MockConversationRepo, MockMessageRepo,
        MockServerMemberRepo, MockThreadRepo,
    };

    fn conversation(server_id: ServerId, main_thread_id: MessageThreadId) -> Conversation {
        Conversation {
            id: ConversationId::new(),
            server_id,
            conversation_type: rim_core::conversation::model::ConversationType::Channel,
            main_thread_id,
            created_at: chrono::Utc::now(),
            last_activity_at: chrono::Utc::now(),
        }
    }

    fn message(author: UserId, thread_id: MessageThreadId) -> Message {
        Message {
            id: MessageId::new(),
            message_thread_id: thread_id,
            sent_by: author,
            text: Some("t".to_string()),
            has_attachments: false,
            parent_message_id: None,
            is_deleted: false,
            sent_at: chrono::Utc::now(),
        }
    }

    #[tokio::test]
    async fn send_message_validates_non_empty_text() {
        let actor_id = UserId::new();
        let server_id = ServerId::new();
        let conv_id = ConversationId::new();
        let main_thread_id = MessageThreadId::new();
        let conv = conversation(server_id, main_thread_id);

        let mut server_member_repo = MockServerMemberRepo::new();
        server_member_repo
            .expect_is_member()
            .returning(|_sid, _uid| Ok(true));

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
        let thread_repo = MockThreadRepo::new();

        let svc = DefaultMessageService::new_for_tests(MessageReposPorts {
            conversation_repo: Arc::new(conversation_repo),
            conversation_member_repo: Arc::new(conversation_member_repo),
            message_repo: Arc::new(message_repo),
            message_thread_repo: Arc::new(thread_repo),
            server_member_repo: Arc::new(server_member_repo),
        });

        let err = svc
            .send_message(SendMessageCmd {
                actor: Actor::new(actor_id),
                server_id,
                conversation_id: conv_id,
                text: Some("   ".to_string()),
                parent_message_id: None,
                attachments: vec![],
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[tokio::test]
    async fn send_message_creates_message_and_updates_activity() {
        let actor_id = UserId::new();
        let server_id = ServerId::new();
        let conv_id = ConversationId::new();
        let main_thread_id = MessageThreadId::new();
        let conv = conversation(server_id, main_thread_id);
        let msg = message(actor_id, main_thread_id);
        let msg_id = msg.id;
        let sent_at = msg.sent_at;

        let mut server_member_repo = MockServerMemberRepo::new();
        server_member_repo
            .expect_is_member()
            .returning(|_sid, _uid| Ok(true));

        let mut conversation_repo = MockConversationRepo::new();
        let conv_for_repo = conv.clone();
        conversation_repo
            .expect_get_by_id()
            .returning(move |_id| Ok(Some(conv_for_repo.clone())));
        conversation_repo
            .expect_touch_last_activity_at()
            .returning(move |_cid, _at| Ok(()));

        let mut conversation_member_repo = MockConversationMemberRepo::new();
        conversation_member_repo
            .expect_is_member()
            .returning(|_cid, _uid| Ok(true));

        let mut message_repo = MockMessageRepo::new();
        let msg_for_repo = msg.clone();
        message_repo
            .expect_create_message()
            .returning(move |new: &NewMessage| {
                assert!(new.text.as_ref().unwrap().contains('h'));
                Ok(msg_for_repo.clone())
            });

        let now = chrono::Utc::now();
        let mut thread_repo = MockThreadRepo::new();
        thread_repo.expect_get_by_id().returning(move |_tid| {
            Ok(Some(MessageThread {
                id: main_thread_id,
                conversation_id: conv_id,
                first_message_id: Some(MessageId::new()),
                created_at: now,
                last_message_at: now,
            }))
        });
        thread_repo.expect_set_first_message().never();
        thread_repo
            .expect_touch_last_message_at()
            .returning(|_tid, _at| Ok(()));

        let svc = DefaultMessageService::new_for_tests(MessageReposPorts {
            conversation_repo: Arc::new(conversation_repo),
            conversation_member_repo: Arc::new(conversation_member_repo),
            message_repo: Arc::new(message_repo),
            message_thread_repo: Arc::new(thread_repo),
            server_member_repo: Arc::new(server_member_repo),
        });

        let dto = svc
            .send_message(SendMessageCmd {
                actor: Actor::new(actor_id),
                server_id,
                conversation_id: conv_id,
                text: Some("hi".to_string()),
                parent_message_id: None,
                attachments: vec![],
            })
            .await
            .unwrap();
        assert_eq!(dto.id, msg_id);
        assert_eq!(dto.sent_at, sent_at);
    }

    #[tokio::test]
    async fn edit_message_denies_non_author() {
        let actor_id = UserId::new();
        let other_id = UserId::new();
        let server_id = ServerId::new();
        let conv_id = ConversationId::new();
        let main_thread_id = MessageThreadId::new();
        let conv = conversation(server_id, main_thread_id);

        let mut server_member_repo = MockServerMemberRepo::new();
        server_member_repo
            .expect_is_member()
            .returning(|_sid, _uid| Ok(true));

        let mut conversation_repo = MockConversationRepo::new();
        let conv_for_repo = conv.clone();
        conversation_repo
            .expect_get_by_id()
            .returning(move |_id| Ok(Some(conv_for_repo.clone())));

        let mut conversation_member_repo = MockConversationMemberRepo::new();
        conversation_member_repo
            .expect_is_member()
            .returning(|_cid, _uid| Ok(true));

        let msg = message(other_id, main_thread_id);
        let msg_id = msg.id;
        let mut message_repo = MockMessageRepo::new();
        let msg_for_repo = msg.clone();
        message_repo
            .expect_get_by_id()
            .returning(move |_id| Ok(Some(msg_for_repo.clone())));
        message_repo.expect_update_message_text().never();

        let now = chrono::Utc::now();
        let mut thread_repo = MockThreadRepo::new();
        thread_repo.expect_get_by_id().returning(move |_tid| {
            Ok(Some(MessageThread {
                id: main_thread_id,
                conversation_id: conv_id,
                first_message_id: Some(MessageId::new()),
                created_at: now,
                last_message_at: now,
            }))
        });

        let svc = DefaultMessageService::new_for_tests(MessageReposPorts {
            conversation_repo: Arc::new(conversation_repo),
            conversation_member_repo: Arc::new(conversation_member_repo),
            message_repo: Arc::new(message_repo),
            message_thread_repo: Arc::new(thread_repo),
            server_member_repo: Arc::new(server_member_repo),
        });

        let err = svc
            .edit_message(EditMessageCmd {
                actor: Actor::new(actor_id),
                server_id,
                conversation_id: conv_id,
                message_id: msg_id,
                text: "x".to_string(),
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::PermissionDenied));
    }

    #[tokio::test]
    async fn delete_message_denies_hard_delete() {
        let server_id = ServerId::new();
        let conv_id = ConversationId::new();

        let mut server_member_repo = MockServerMemberRepo::new();
        server_member_repo
            .expect_is_member()
            .returning(|_sid, _uid| Ok(true));

        let mut conversation_repo = MockConversationRepo::new();
        conversation_repo
            .expect_get_by_id()
            .returning(move |_id| {
                Ok(Some(conversation(server_id, MessageThreadId::new())))
            });

        let mut conversation_member_repo = MockConversationMemberRepo::new();
        conversation_member_repo
            .expect_is_member()
            .returning(|_cid, _uid| Ok(true));

        let svc = DefaultMessageService::new_for_tests(MessageReposPorts {
            conversation_repo: Arc::new(conversation_repo),
            conversation_member_repo: Arc::new(conversation_member_repo),
            message_repo: Arc::new(MockMessageRepo::new()),
            message_thread_repo: Arc::new(MockThreadRepo::new()),
            server_member_repo: Arc::new(server_member_repo),
        });

        let err = svc
            .delete_message(DeleteMessageCmd {
                actor: Actor::new(UserId::new()),
                server_id,
                conversation_id: conv_id,
                message_id: MessageId::new(),
                hard: true,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[tokio::test]
    async fn list_messages_calls_repo() {
        let actor_id = UserId::new();
        let server_id = ServerId::new();
        let conv_id = ConversationId::new();
        let main_thread_id = MessageThreadId::new();
        let conv = conversation(server_id, main_thread_id);

        let mut server_member_repo = MockServerMemberRepo::new();
        server_member_repo
            .expect_is_member()
            .returning(|_sid, _uid| Ok(true));

        let mut conversation_repo = MockConversationRepo::new();
        let conv_for_repo = conv.clone();
        conversation_repo
            .expect_get_by_id()
            .returning(move |_id| Ok(Some(conv_for_repo.clone())));

        let mut conversation_member_repo = MockConversationMemberRepo::new();
        conversation_member_repo
            .expect_is_member()
            .returning(|_cid, _uid| Ok(true));

        let now = chrono::Utc::now();
        let mut thread_repo = MockThreadRepo::new();
        thread_repo.expect_get_by_id().returning(move |_tid| {
            Ok(Some(MessageThread {
                id: main_thread_id,
                conversation_id: conv_id,
                first_message_id: None,
                created_at: now,
                last_message_at: now,
            }))
        });

        let msg = message(actor_id, main_thread_id);
        let mut message_repo = MockMessageRepo::new();
        let msg_for_repo = msg.clone();
        message_repo
            .expect_list_messages()
            .returning(move |_tid, _limit, _dir, _cursor, _inc_del, order| {
                assert_eq!(order, SortOrder::Desc);
                Ok(Page::new(vec![msg_for_repo.clone()], None, None))
            });

        let svc = DefaultMessageService::new_for_tests(MessageReposPorts {
            conversation_repo: Arc::new(conversation_repo),
            conversation_member_repo: Arc::new(conversation_member_repo),
            message_repo: Arc::new(message_repo),
            message_thread_repo: Arc::new(thread_repo),
            server_member_repo: Arc::new(server_member_repo),
        });

        let page = svc
            .list_messages(ListMessagesQuery {
                actor: Actor::new(actor_id),
                server_id,
                conversation_id: conv_id,
                limit: 10,
                dir: PageDir::After,
                cursor: None,
                include_deleted: false,
            })
            .await
            .unwrap();
        assert_eq!(page.items.len(), 1);
    }

    #[tokio::test]
    async fn get_message_not_found_when_thread_not_in_conversation() {
        let actor_id = UserId::new();
        let server_id = ServerId::new();
        let conv_id = ConversationId::new();
        let main_thread_id = MessageThreadId::new();
        let conv = conversation(server_id, main_thread_id);
        let msg = message(actor_id, MessageThreadId::new());

        let mut server_member_repo = MockServerMemberRepo::new();
        server_member_repo
            .expect_is_member()
            .returning(|_sid, _uid| Ok(true));

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
        let msg_for_repo = msg.clone();
        message_repo
            .expect_get_by_id()
            .returning(move |_id| Ok(Some(msg_for_repo.clone())));

        let mut thread_repo = MockThreadRepo::new();
        thread_repo
            .expect_get_by_id()
            .returning(|_tid| Ok(None));

        let svc = DefaultMessageService::new_for_tests(MessageReposPorts {
            conversation_repo: Arc::new(conversation_repo),
            conversation_member_repo: Arc::new(conversation_member_repo),
            message_repo: Arc::new(message_repo),
            message_thread_repo: Arc::new(thread_repo),
            server_member_repo: Arc::new(server_member_repo),
        });

        let err = svc
            .get_message(GetMessageQuery {
                actor: Actor::new(actor_id),
                server_id,
                conversation_id: conv_id,
                message_id: msg.id,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::NotFound));
    }
}
