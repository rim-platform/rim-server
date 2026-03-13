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
        DeleteMessageCmd, EditMessageCmd, GetMessageQuery, GetMessageReferenceQuery,
        ListMessagesQuery, ListThreadMessagesQuery, SearchMessagesQuery, SendMessageCmd,
        SendThreadMessageCmd,
    };
    use rim_services::message::dto::MessageReferenceDto;
    use rim_services::message::service::MessageService;
    use rim_services::{Actor, ServiceError};

    use crate::message::DefaultMessageService;
    use crate::test_support::message::{
        MessageReposPorts, MockConversationMemberRepo, MockConversationRepo,
        MockMessageAttachmentRepo, MockMessageRepo, MockServerMemberRepo, MockThreadRepo,
    };

    fn conversation(server_id: ServerId) -> Conversation {
        Conversation {
            id: ConversationId::new(),
            server_id,
            conversation_type: rim_core::conversation::model::ConversationType::Channel,
            title: None,
            created_at: chrono::Utc::now(),
            last_activity_at: chrono::Utc::now(),
        }
    }

    fn message(author: UserId, conversation_id: ConversationId) -> Message {
        Message {
            id: MessageId::new(),
            conversation_id,
            thread_id: None,
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
        let conv = conversation(server_id);

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
        let message_attachment_repo = MockMessageAttachmentRepo::new();
        let thread_repo = MockThreadRepo::new();

        let svc = DefaultMessageService::new_for_tests(MessageReposPorts {
            conversation_repo: Arc::new(conversation_repo),
            conversation_member_repo: Arc::new(conversation_member_repo),
            message_repo: Arc::new(message_repo),
            message_attachment_repo: Arc::new(message_attachment_repo),
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
        let conv = conversation(server_id);
        let msg = message(actor_id, conv_id);
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
        let mut message_attachment_repo = MockMessageAttachmentRepo::new();
        message_attachment_repo
            .expect_get_attachments_for_message()
            .return_once(move |_| Ok(vec![]));

        let mut thread_repo = MockThreadRepo::new();
        thread_repo
            .expect_get_by_root_message()
            .return_once(move |_| Ok(None));

        let svc = DefaultMessageService::new_for_tests(MessageReposPorts {
            conversation_repo: Arc::new(conversation_repo),
            conversation_member_repo: Arc::new(conversation_member_repo),
            message_repo: Arc::new(message_repo),
            message_attachment_repo: Arc::new(message_attachment_repo),
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
        let conv = conversation(server_id);

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

        let msg = message(other_id, conv_id);
        let msg_id = msg.id;
        let mut message_repo = MockMessageRepo::new();
        let msg_for_repo = msg.clone();
        message_repo
            .expect_get_by_id()
            .returning(move |_id| Ok(Some(msg_for_repo.clone())));
        message_repo.expect_update_message_text().never();

        let svc = DefaultMessageService::new_for_tests(MessageReposPorts {
            conversation_repo: Arc::new(conversation_repo),
            conversation_member_repo: Arc::new(conversation_member_repo),
            message_repo: Arc::new(message_repo),
            message_attachment_repo: Arc::new(MockMessageAttachmentRepo::new()),
            message_thread_repo: Arc::new(MockThreadRepo::new()),
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
            .returning(move |_id| Ok(Some(conversation(server_id))));

        let mut conversation_member_repo = MockConversationMemberRepo::new();
        conversation_member_repo
            .expect_is_member()
            .returning(|_cid, _uid| Ok(true));

        let svc = DefaultMessageService::new_for_tests(MessageReposPorts {
            conversation_repo: Arc::new(conversation_repo),
            conversation_member_repo: Arc::new(conversation_member_repo),
            message_repo: Arc::new(MockMessageRepo::new()),
            message_attachment_repo: Arc::new(MockMessageAttachmentRepo::new()),
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
        let conv = conversation(server_id);

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

        let msg = message(actor_id, conv_id);
        let mut message_repo = MockMessageRepo::new();
        let msg_for_repo = msg.clone();
        message_repo.expect_list_messages().returning(
            move |cid, thread_id, _limit, _dir, _cursor, _inc_del, order| {
                assert_eq!(cid, conv_id);
                assert_eq!(thread_id, None);
                assert_eq!(order, SortOrder::Desc);
                Ok(Page::new(vec![msg_for_repo.clone()], None, None))
            },
        );
        let mut message_attachment_repo = MockMessageAttachmentRepo::new();
        message_attachment_repo
            .expect_get_attachments_for_messages()
            .return_once(move |_ids| Ok(std::collections::HashMap::new()));

        let mut thread_repo = MockThreadRepo::new();
        thread_repo
            .expect_get_thread_ids_by_root_messages()
            .return_once(move |message_ids| {
                let mut ids = std::collections::HashMap::new();
                ids.insert(message_ids[0], MessageThreadId::new());
                Ok(ids)
            });

        let svc = DefaultMessageService::new_for_tests(MessageReposPorts {
            conversation_repo: Arc::new(conversation_repo),
            conversation_member_repo: Arc::new(conversation_member_repo),
            message_repo: Arc::new(message_repo),
            message_attachment_repo: Arc::new(message_attachment_repo),
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
        assert!(page.items[0].root_thread_id.is_some());
    }

    #[tokio::test]
    async fn send_thread_message_creates_thread_reply() {
        let actor_id = UserId::new();
        let server_id = ServerId::new();
        let conv_id = ConversationId::new();
        let thread_id = MessageThreadId::new();
        let root_id = MessageId::new();
        let conv = conversation(server_id);
        let created = Message {
            id: MessageId::new(),
            conversation_id: conv_id,
            thread_id: Some(thread_id),
            sent_by: actor_id,
            text: Some("reply".to_string()),
            has_attachments: false,
            parent_message_id: Some(root_id),
            is_deleted: false,
            sent_at: chrono::Utc::now(),
        };

        let mut server_member_repo = MockServerMemberRepo::new();
        server_member_repo
            .expect_is_member()
            .returning(|_, _| Ok(true));

        let mut conversation_repo = MockConversationRepo::new();
        let conv_for_repo = conv.clone();
        conversation_repo
            .expect_get_by_id()
            .returning(move |_| Ok(Some(conv_for_repo.clone())));
        conversation_repo
            .expect_touch_last_activity_at()
            .returning(|_, _| Ok(()));

        let mut conversation_member_repo = MockConversationMemberRepo::new();
        conversation_member_repo
            .expect_is_member()
            .returning(|_, _| Ok(true));

        let mut message_repo = MockMessageRepo::new();
        message_repo.expect_get_by_id().return_once(move |_| {
            Ok(Some(Message {
                id: root_id,
                conversation_id: conv_id,
                thread_id: None,
                sent_by: actor_id,
                text: Some("root".to_string()),
                has_attachments: false,
                parent_message_id: None,
                is_deleted: false,
                sent_at: chrono::Utc::now(),
            }))
        });
        let created_for_repo = created.clone();
        message_repo
            .expect_create_message()
            .return_once(move |new| {
                assert_eq!(new.conversation_id, conv_id);
                assert_eq!(new.thread_id, Some(thread_id));
                Ok(created_for_repo.clone())
            });
        let mut message_attachment_repo = MockMessageAttachmentRepo::new();
        message_attachment_repo
            .expect_get_attachments_for_message()
            .return_once(move |_| Ok(vec![]));

        let mut thread_repo = MockThreadRepo::new();
        thread_repo.expect_get_by_id().return_once(move |_| {
            Ok(Some(MessageThread {
                id: thread_id,
                conversation_id: conv_id,
                first_message_id: Some(root_id),
                created_at: chrono::Utc::now(),
                last_message_at: chrono::Utc::now(),
            }))
        });
        thread_repo
            .expect_touch_last_message_at()
            .returning(|_, _| Ok(()));
        thread_repo.expect_get_by_root_message().never();

        let svc = DefaultMessageService::new_for_tests(MessageReposPorts {
            conversation_repo: Arc::new(conversation_repo),
            conversation_member_repo: Arc::new(conversation_member_repo),
            message_repo: Arc::new(message_repo),
            message_attachment_repo: Arc::new(message_attachment_repo),
            message_thread_repo: Arc::new(thread_repo),
            server_member_repo: Arc::new(server_member_repo),
        });

        let dto = svc
            .send_thread_message(SendThreadMessageCmd {
                actor: Actor::new(actor_id),
                server_id,
                conversation_id: conv_id,
                thread_id,
                text: Some("reply".to_string()),
                parent_message_id: Some(root_id),
                attachments: vec![],
            })
            .await
            .unwrap();

        assert_eq!(dto.thread_id, Some(thread_id));
        assert_eq!(dto.root_thread_id, None);
    }

    #[tokio::test]
    async fn list_thread_messages_calls_repo_with_thread_id() {
        let actor_id = UserId::new();
        let server_id = ServerId::new();
        let conv_id = ConversationId::new();
        let thread_id = MessageThreadId::new();
        let conv = conversation(server_id);

        let mut server_member_repo = MockServerMemberRepo::new();
        server_member_repo
            .expect_is_member()
            .returning(|_, _| Ok(true));

        let mut conversation_repo = MockConversationRepo::new();
        let conv_for_repo = conv.clone();
        conversation_repo
            .expect_get_by_id()
            .returning(move |_| Ok(Some(conv_for_repo.clone())));

        let mut conversation_member_repo = MockConversationMemberRepo::new();
        conversation_member_repo
            .expect_is_member()
            .returning(|_, _| Ok(true));

        let mut thread_repo = MockThreadRepo::new();
        thread_repo.expect_get_by_id().return_once(move |_| {
            Ok(Some(MessageThread {
                id: thread_id,
                conversation_id: conv_id,
                first_message_id: None,
                created_at: chrono::Utc::now(),
                last_message_at: chrono::Utc::now(),
            }))
        });

        let mut message_repo = MockMessageRepo::new();
        let msg = Message {
            id: MessageId::new(),
            conversation_id: conv_id,
            thread_id: Some(thread_id),
            sent_by: actor_id,
            text: Some("reply".to_string()),
            has_attachments: false,
            parent_message_id: None,
            is_deleted: false,
            sent_at: chrono::Utc::now(),
        };
        let msg_for_repo = msg.clone();
        message_repo
            .expect_list_messages()
            .return_once(move |cid, tid, _, _, _, _, _| {
                assert_eq!(cid, conv_id);
                assert_eq!(tid, Some(thread_id));
                Ok(Page::new(vec![msg_for_repo], None, None))
            });
        let mut message_attachment_repo = MockMessageAttachmentRepo::new();
        message_attachment_repo
            .expect_get_attachments_for_messages()
            .return_once(move |_ids| Ok(std::collections::HashMap::new()));

        let svc = DefaultMessageService::new_for_tests(MessageReposPorts {
            conversation_repo: Arc::new(conversation_repo),
            conversation_member_repo: Arc::new(conversation_member_repo),
            message_repo: Arc::new(message_repo),
            message_attachment_repo: Arc::new(message_attachment_repo),
            message_thread_repo: Arc::new(thread_repo),
            server_member_repo: Arc::new(server_member_repo),
        });

        let page = svc
            .list_thread_messages(ListThreadMessagesQuery {
                actor: Actor::new(actor_id),
                server_id,
                conversation_id: conv_id,
                thread_id,
                limit: 10,
                dir: PageDir::After,
                cursor: None,
                include_deleted: false,
            })
            .await
            .unwrap();

        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].thread_id, Some(thread_id));
        assert_eq!(page.items[0].root_thread_id, None);
    }

    #[tokio::test]
    async fn get_message_exposes_root_thread_id_for_timeline_root() {
        let actor_id = UserId::new();
        let server_id = ServerId::new();
        let conv_id = ConversationId::new();
        let conv = conversation(server_id);
        let msg = message(actor_id, conv_id);
        let thread_id = MessageThreadId::new();

        let mut server_member_repo = MockServerMemberRepo::new();
        server_member_repo
            .expect_is_member()
            .returning(|_, _| Ok(true));

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
        let msg_for_repo = msg.clone();
        message_repo
            .expect_get_by_id()
            .return_once(move |_| Ok(Some(msg_for_repo.clone())));
        let mut message_attachment_repo = MockMessageAttachmentRepo::new();
        message_attachment_repo
            .expect_get_attachments_for_message()
            .return_once(move |_| Ok(vec![]));

        let mut thread_repo = MockThreadRepo::new();
        thread_repo
            .expect_get_by_root_message()
            .return_once(move |_| {
                Ok(Some(MessageThread {
                    id: thread_id,
                    conversation_id: conv_id,
                    first_message_id: Some(msg.id),
                    created_at: chrono::Utc::now(),
                    last_message_at: chrono::Utc::now(),
                }))
            });

        let svc = DefaultMessageService::new_for_tests(MessageReposPorts {
            conversation_repo: Arc::new(conversation_repo),
            conversation_member_repo: Arc::new(conversation_member_repo),
            message_repo: Arc::new(message_repo),
            message_attachment_repo: Arc::new(message_attachment_repo),
            message_thread_repo: Arc::new(thread_repo),
            server_member_repo: Arc::new(server_member_repo),
        });

        let dto = svc
            .get_message(GetMessageQuery {
                actor: Actor::new(actor_id),
                server_id,
                conversation_id: conv_id,
                message_id: msg.id,
            })
            .await
            .unwrap();

        assert_eq!(dto.root_thread_id, Some(thread_id));
        assert_eq!(dto.thread_id, None);
    }

    #[tokio::test]
    async fn get_message_reference_returns_timeline_permalink() {
        let actor_id = UserId::new();
        let server_id = ServerId::new();
        let conv_id = ConversationId::new();
        let conv = conversation(server_id);
        let msg = message(actor_id, conv_id);

        let mut server_member_repo = MockServerMemberRepo::new();
        server_member_repo
            .expect_is_member()
            .returning(|_, _| Ok(true));

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
        let msg_for_repo = msg.clone();
        message_repo
            .expect_get_by_id()
            .return_once(move |_| Ok(Some(msg_for_repo.clone())));

        let svc = DefaultMessageService::new_for_tests(MessageReposPorts {
            conversation_repo: Arc::new(conversation_repo),
            conversation_member_repo: Arc::new(conversation_member_repo),
            message_repo: Arc::new(message_repo),
            message_attachment_repo: Arc::new(MockMessageAttachmentRepo::new()),
            message_thread_repo: Arc::new(MockThreadRepo::new()),
            server_member_repo: Arc::new(server_member_repo),
        });

        let reference: MessageReferenceDto = svc
            .get_message_reference(GetMessageReferenceQuery {
                actor: Actor::new(actor_id),
                server_id,
                conversation_id: conv_id,
                message_id: msg.id,
            })
            .await
            .unwrap();

        assert_eq!(reference.message_id, msg.id);
        assert!(reference.thread_id.is_none());
        assert_eq!(
            reference.permalink_path,
            format!(
                "/workspaces/{}/conversations/{}/messages/{}",
                server_id, conv_id, msg.id
            )
        );
        assert_eq!(reference.anchor, format!("message-{}", msg.id));
    }

    #[tokio::test]
    async fn search_messages_requires_non_empty_query() {
        let actor_id = UserId::new();
        let server_id = ServerId::new();
        let conv_id = ConversationId::new();

        let mut server_member_repo = MockServerMemberRepo::new();
        server_member_repo
            .expect_is_member()
            .returning(|_, _| Ok(true));

        let mut conversation_repo = MockConversationRepo::new();
        conversation_repo
            .expect_get_by_id()
            .returning(move |_| Ok(Some(conversation(server_id))));

        let mut conversation_member_repo = MockConversationMemberRepo::new();
        conversation_member_repo
            .expect_is_member()
            .returning(|_, _| Ok(true));

        let svc = DefaultMessageService::new_for_tests(MessageReposPorts {
            conversation_repo: Arc::new(conversation_repo),
            conversation_member_repo: Arc::new(conversation_member_repo),
            message_repo: Arc::new(MockMessageRepo::new()),
            message_attachment_repo: Arc::new(MockMessageAttachmentRepo::new()),
            message_thread_repo: Arc::new(MockThreadRepo::new()),
            server_member_repo: Arc::new(server_member_repo),
        });

        let err = svc
            .search_messages(SearchMessagesQuery {
                actor: Actor::new(actor_id),
                server_id,
                conversation_id: conv_id,
                query: "   ".to_string(),
                limit: 10,
                include_deleted: false,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[tokio::test]
    async fn get_message_not_found_when_message_not_in_conversation() {
        let actor_id = UserId::new();
        let server_id = ServerId::new();
        let conv_id = ConversationId::new();
        let conv = conversation(server_id);
        let msg = message(actor_id, ConversationId::new());

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

        let svc = DefaultMessageService::new_for_tests(MessageReposPorts {
            conversation_repo: Arc::new(conversation_repo),
            conversation_member_repo: Arc::new(conversation_member_repo),
            message_repo: Arc::new(message_repo),
            message_attachment_repo: Arc::new(MockMessageAttachmentRepo::new()),
            message_thread_repo: Arc::new(MockThreadRepo::new()),
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
