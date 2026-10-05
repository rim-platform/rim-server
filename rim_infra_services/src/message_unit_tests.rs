#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use rim_core::conversation::model::{Conversation, ConversationId};
    use rim_core::conversation::repository::MockConversationRepository;
    use rim_core::conversation_member::repository::MockConversationMemberRepository;
    use rim_core::message::model::{Message, MessageId, NewMessage};
    use rim_core::message::repository::MockMessageRepository;
    use rim_core::message_attachment::repository::MockMessageAttachmentRepository;
    use rim_core::message_mention::repository::MockMessageMentionRepository;
    use rim_core::message_thread::model::{MessageThread, MessageThreadId};
    use rim_core::message_thread::repository::MockMessageThreadRepository;
    use rim_core::repository::pagination::{Page, PageDir, SortOrder};
    use rim_core::server::model::ServerId;
    use rim_core::server_member::repository::MockServerMemberRepository;
    use rim_core::user::model::UserId;
    use rim_core::user::repository::MockUserRepository;
    use rim_services::message::command::{
        DeleteMessageCmd, EditMessageCmd, GetMessageQuery, GetMessageReferenceQuery,
        ListMessagesQuery, ListThreadMessagesQuery, SearchMessagesQuery, SendMessageCmd,
    };
    use rim_services::message::dto::MessageReferenceDto;
    use rim_services::message::service::MessageService;
    use rim_services::{Actor, ServiceError};

    use crate::MessageRepos;
    use rim_services::events::{NoopBroadcaster, Observers};

    use crate::message::DefaultMessageService;

    fn test_observers() -> Observers {
        Observers::new(Arc::new(NoopBroadcaster) as Arc<dyn rim_services::events::Broadcaster>)
    }

    fn conversation(server_id: ServerId) -> Conversation {
        Conversation {
            id: ConversationId::new(),
            server_id,
            conversation_type: rim_core::conversation::model::ConversationType::Channel,
            title: None,
            topic: None,
            created_at: chrono::Utc::now(),
            last_activity_at: chrono::Utc::now(),
            is_private: false,
            archived_at: None,
            created_by: None,
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
            forwarded_from_message_id: None,
            is_deleted: false,
            sent_at: chrono::Utc::now(),
        }
    }

    #[tokio::test]
    async fn send_message_normalizes_whitespace_only_text_to_none() {
        let actor_id = UserId::new();
        let server_id = ServerId::new();
        let conv_id = ConversationId::new();
        let conv = conversation(server_id);

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_is_member()
            .returning(|_sid, _uid| Ok(true));

        let mut conversation_repo = MockConversationRepository::new();
        let conv_for_repo = conv.clone();
        conversation_repo.expect_get_by_id().returning(move |_id| {
            let c = conv_for_repo.clone();
            Box::pin(async move { Ok(Some(c)) })
        });
        conversation_repo
            .expect_touch_last_activity_at()
            .returning(|_cid, _at| Box::pin(async { Ok(()) }));

        let mut conversation_member_repo = MockConversationMemberRepository::new();
        conversation_member_repo
            .expect_is_member()
            .returning(|_cid, _uid| Box::pin(async { Ok(true) }));
        conversation_member_repo
            .expect_list_member_ids()
            .returning(|_cid| Box::pin(async { Ok(vec![]) }));

        let mut message_repo = MockMessageRepository::new();
        message_repo
            .expect_create_message()
            .withf(|msg| msg.text.is_none())
            .returning(move |msg| {
                let conversation_id = msg.conversation_id;
                let sent_by = msg.sent_by;
                Box::pin(async move {
                    Ok(Message {
                        id: MessageId::new(),
                        conversation_id,
                        thread_id: None,
                        parent_message_id: None,
                        forwarded_from_message_id: None,
                        sent_by,
                        text: None,
                        has_attachments: false,
                        is_deleted: false,
                        sent_at: chrono::Utc::now(),
                    })
                })
            });
        let mut message_attachment_repo = MockMessageAttachmentRepository::new();
        message_attachment_repo
            .expect_get_attachments_for_message()
            .returning(|_mid| Box::pin(async { Ok(vec![]) }));

        let mut thread_repo = MockMessageThreadRepository::new();
        thread_repo
            .expect_get_by_root_message()
            .returning(|_mid| Box::pin(async { Ok(None) }));

        let svc = DefaultMessageService::new(
            MessageRepos {
                conversation_repo: Arc::new(conversation_repo),
                conversation_member_repo: Arc::new(conversation_member_repo),
                message_repo: Arc::new(message_repo),
                message_attachment_repo: Arc::new(message_attachment_repo),
                message_mention_repo: Arc::new(MockMessageMentionRepository::new()),
                message_thread_repo: Arc::new(thread_repo),
                server_member_repo: Arc::new(server_member_repo),
                user_repo: Arc::new(MockUserRepository::new()),
            },
            None,
            test_observers(),
        );

        // Whitespace-only text is normalized to None (empty text for attachment-only messages).
        let dto = svc
            .send_message(SendMessageCmd {
                actor: Actor::new(actor_id),
                server_id,
                conversation_id: conv_id,
                text: Some("   ".to_string()),
                parent_message_id: None,
                attachments: vec![],
            })
            .await
            .unwrap();
        assert!(dto.text.is_none());
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

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_is_member()
            .returning(|_sid, _uid| Ok(true));

        let mut conversation_repo = MockConversationRepository::new();
        let conv_for_repo = conv.clone();
        conversation_repo.expect_get_by_id().returning(move |_id| {
            let c = conv_for_repo.clone();
            Box::pin(async move { Ok(Some(c)) })
        });
        conversation_repo
            .expect_touch_last_activity_at()
            .returning(move |_cid, _at| Box::pin(async { Ok(()) }));

        let mut conversation_member_repo = MockConversationMemberRepository::new();
        conversation_member_repo
            .expect_is_member()
            .returning(|_cid, _uid| Box::pin(async { Ok(true) }));
        conversation_member_repo
            .expect_list_member_ids()
            .returning(move |_cid| Box::pin(async move { Ok(vec![actor_id]) }));

        let mut message_repo = MockMessageRepository::new();
        let msg_for_repo = msg.clone();
        message_repo
            .expect_create_message()
            .returning(move |new: &NewMessage| {
                assert!(new.text.as_ref().unwrap().contains('h'));
                let m = msg_for_repo.clone();
                Box::pin(async move { Ok(m) })
            });
        let mut message_attachment_repo = MockMessageAttachmentRepository::new();
        message_attachment_repo
            .expect_get_attachments_for_message()
            .return_once(move |_| Box::pin(async { Ok(vec![]) }));

        let mut thread_repo = MockMessageThreadRepository::new();
        thread_repo
            .expect_get_by_root_message()
            .return_once(move |_| Box::pin(async { Ok(None) }));

        let svc = DefaultMessageService::new(
            MessageRepos {
                conversation_repo: Arc::new(conversation_repo),
                conversation_member_repo: Arc::new(conversation_member_repo),
                message_repo: Arc::new(message_repo),
                message_attachment_repo: Arc::new(message_attachment_repo),
                message_mention_repo: Arc::new(MockMessageMentionRepository::new()),
                message_thread_repo: Arc::new(thread_repo),
                server_member_repo: Arc::new(server_member_repo),
                user_repo: Arc::new(MockUserRepository::new()),
            },
            None,
            test_observers(),
        );

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

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_is_member()
            .returning(|_sid, _uid| Ok(true));

        let mut conversation_repo = MockConversationRepository::new();
        let conv_for_repo = conv.clone();
        conversation_repo.expect_get_by_id().returning(move |_id| {
            let c = conv_for_repo.clone();
            Box::pin(async move { Ok(Some(c)) })
        });

        let mut conversation_member_repo = MockConversationMemberRepository::new();
        conversation_member_repo
            .expect_is_member()
            .returning(|_cid, _uid| Box::pin(async { Ok(true) }));

        let msg = message(other_id, conv_id);
        let msg_id = msg.id;
        let mut message_repo = MockMessageRepository::new();
        let msg_for_repo = msg.clone();
        message_repo.expect_get_by_id().returning(move |_id| {
            let m = msg_for_repo.clone();
            Box::pin(async move { Ok(Some(m)) })
        });
        message_repo.expect_update_message_text().never();

        let svc = DefaultMessageService::new(
            MessageRepos {
                conversation_repo: Arc::new(conversation_repo),
                conversation_member_repo: Arc::new(conversation_member_repo),
                message_repo: Arc::new(message_repo),
                message_attachment_repo: Arc::new(MockMessageAttachmentRepository::new()),
                message_mention_repo: Arc::new(MockMessageMentionRepository::new()),
                message_thread_repo: Arc::new(MockMessageThreadRepository::new()),
                server_member_repo: Arc::new(server_member_repo),
                user_repo: Arc::new(MockUserRepository::new()),
            },
            None,
            test_observers(),
        );

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

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_is_member()
            .returning(|_sid, _uid| Ok(true));

        let mut conversation_repo = MockConversationRepository::new();
        conversation_repo.expect_get_by_id().returning(move |_id| {
            let c = conversation(server_id);
            Box::pin(async move { Ok(Some(c)) })
        });

        let mut conversation_member_repo = MockConversationMemberRepository::new();
        conversation_member_repo
            .expect_is_member()
            .returning(|_cid, _uid| Box::pin(async { Ok(true) }));

        let svc = DefaultMessageService::new(
            MessageRepos {
                conversation_repo: Arc::new(conversation_repo),
                conversation_member_repo: Arc::new(conversation_member_repo),
                message_repo: Arc::new(MockMessageRepository::new()),
                message_attachment_repo: Arc::new(MockMessageAttachmentRepository::new()),
                message_mention_repo: Arc::new(MockMessageMentionRepository::new()),
                message_thread_repo: Arc::new(MockMessageThreadRepository::new()),
                server_member_repo: Arc::new(server_member_repo),
                user_repo: Arc::new(MockUserRepository::new()),
            },
            None,
            test_observers(),
        );

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

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_is_member()
            .returning(|_sid, _uid| Ok(true));

        let mut conversation_repo = MockConversationRepository::new();
        let conv_for_repo = conv.clone();
        conversation_repo.expect_get_by_id().returning(move |_id| {
            let c = conv_for_repo.clone();
            Box::pin(async move { Ok(Some(c)) })
        });

        let mut conversation_member_repo = MockConversationMemberRepository::new();
        conversation_member_repo
            .expect_is_member()
            .returning(|_cid, _uid| Box::pin(async { Ok(true) }));

        let msg = message(actor_id, conv_id);
        let mut message_repo = MockMessageRepository::new();
        let msg_for_repo = msg.clone();
        message_repo
            .expect_list_messages()
            .returning(move |params| {
                assert_eq!(params.conversation_id, conv_id);
                assert_eq!(params.thread_id, None);
                assert_eq!(params.order, SortOrder::Desc);
                let m = msg_for_repo.clone();
                Box::pin(async move { Ok(Page::new(vec![m], None, None)) })
            });
        let mut message_attachment_repo = MockMessageAttachmentRepository::new();
        message_attachment_repo
            .expect_get_attachments_for_messages()
            .return_once(move |_ids| Box::pin(async { Ok(std::collections::HashMap::new()) }));

        let mut thread_repo = MockMessageThreadRepository::new();
        thread_repo
            .expect_get_thread_ids_by_root_messages()
            .return_once(move |message_ids| {
                let mut ids = std::collections::HashMap::new();
                ids.insert(message_ids[0], MessageThreadId::new());
                Box::pin(async move { Ok(ids) })
            });

        let svc = DefaultMessageService::new(
            MessageRepos {
                conversation_repo: Arc::new(conversation_repo),
                conversation_member_repo: Arc::new(conversation_member_repo),
                message_repo: Arc::new(message_repo),
                message_attachment_repo: Arc::new(message_attachment_repo),
                message_mention_repo: Arc::new(MockMessageMentionRepository::new()),
                message_thread_repo: Arc::new(thread_repo),
                server_member_repo: Arc::new(server_member_repo),
                user_repo: Arc::new(MockUserRepository::new()),
            },
            None,
            test_observers(),
        );

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
    async fn list_thread_messages_calls_repo_with_thread_id() {
        let actor_id = UserId::new();
        let server_id = ServerId::new();
        let conv_id = ConversationId::new();
        let thread_id = MessageThreadId::new();
        let conv = conversation(server_id);

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_is_member()
            .returning(|_, _| Ok(true));

        let mut conversation_repo = MockConversationRepository::new();
        let conv_for_repo = conv.clone();
        conversation_repo.expect_get_by_id().returning(move |_| {
            let c = conv_for_repo.clone();
            Box::pin(async move { Ok(Some(c)) })
        });

        let mut conversation_member_repo = MockConversationMemberRepository::new();
        conversation_member_repo
            .expect_is_member()
            .returning(|_, _| Box::pin(async { Ok(true) }));

        let mut thread_repo = MockMessageThreadRepository::new();
        thread_repo.expect_get_by_id().return_once(move |_| {
            Box::pin(async move {
                Ok(Some(MessageThread {
                    id: thread_id,
                    conversation_id: conv_id,
                    first_message_id: None,
                    created_at: chrono::Utc::now(),
                    last_message_at: chrono::Utc::now(),
                }))
            })
        });
        thread_repo
            .expect_get_by_ids()
            .return_once(move |_| Box::pin(async move { Ok(vec![]) }));

        let mut message_repo = MockMessageRepository::new();
        let msg = Message {
            id: MessageId::new(),
            conversation_id: conv_id,
            thread_id: Some(thread_id),
            sent_by: actor_id,
            text: Some("reply".to_string()),
            has_attachments: false,
            parent_message_id: None,
            forwarded_from_message_id: None,
            is_deleted: false,
            sent_at: chrono::Utc::now(),
        };
        let msg_for_repo = msg.clone();
        message_repo
            .expect_list_messages()
            .return_once(move |params| {
                assert_eq!(params.conversation_id, conv_id);
                assert_eq!(params.thread_id, Some(thread_id));
                Box::pin(async move { Ok(Page::new(vec![msg_for_repo], None, None)) })
            });
        let mut message_attachment_repo = MockMessageAttachmentRepository::new();
        message_attachment_repo
            .expect_get_attachments_for_messages()
            .return_once(move |_ids| Box::pin(async { Ok(std::collections::HashMap::new()) }));

        let svc = DefaultMessageService::new(
            MessageRepos {
                conversation_repo: Arc::new(conversation_repo),
                conversation_member_repo: Arc::new(conversation_member_repo),
                message_repo: Arc::new(message_repo),
                message_attachment_repo: Arc::new(message_attachment_repo),
                message_mention_repo: Arc::new(MockMessageMentionRepository::new()),
                message_thread_repo: Arc::new(thread_repo),
                server_member_repo: Arc::new(server_member_repo),
                user_repo: Arc::new(MockUserRepository::new()),
            },
            None,
            test_observers(),
        );

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

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_is_member()
            .returning(|_, _| Ok(true));

        let mut conversation_repo = MockConversationRepository::new();
        let conv_for_repo = conv.clone();
        conversation_repo.expect_get_by_id().returning(move |_| {
            let c = conv_for_repo.clone();
            Box::pin(async move { Ok(Some(c)) })
        });

        let mut conversation_member_repo = MockConversationMemberRepository::new();
        conversation_member_repo
            .expect_is_member()
            .returning(|_, _| Box::pin(async { Ok(true) }));

        let mut message_repo = MockMessageRepository::new();
        let msg_for_repo = msg.clone();
        message_repo
            .expect_get_by_id()
            .return_once(move |_| Box::pin(async move { Ok(Some(msg_for_repo)) }));
        let mut message_attachment_repo = MockMessageAttachmentRepository::new();
        message_attachment_repo
            .expect_get_attachments_for_message()
            .return_once(move |_| Box::pin(async { Ok(vec![]) }));

        let mut thread_repo = MockMessageThreadRepository::new();
        thread_repo
            .expect_get_by_root_message()
            .return_once(move |_| {
                Box::pin(async move {
                    Ok(Some(MessageThread {
                        id: thread_id,
                        conversation_id: conv_id,
                        first_message_id: Some(msg.id),
                        created_at: chrono::Utc::now(),
                        last_message_at: chrono::Utc::now(),
                    }))
                })
            });

        let svc = DefaultMessageService::new(
            MessageRepos {
                conversation_repo: Arc::new(conversation_repo),
                conversation_member_repo: Arc::new(conversation_member_repo),
                message_repo: Arc::new(message_repo),
                message_attachment_repo: Arc::new(message_attachment_repo),
                message_mention_repo: Arc::new(MockMessageMentionRepository::new()),
                message_thread_repo: Arc::new(thread_repo),
                server_member_repo: Arc::new(server_member_repo),
                user_repo: Arc::new(MockUserRepository::new()),
            },
            None,
            test_observers(),
        );

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

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_is_member()
            .returning(|_, _| Ok(true));

        let mut conversation_repo = MockConversationRepository::new();
        let conv_for_repo = conv.clone();
        conversation_repo.expect_get_by_id().returning(move |_| {
            let c = conv_for_repo.clone();
            Box::pin(async move { Ok(Some(c)) })
        });

        let mut conversation_member_repo = MockConversationMemberRepository::new();
        conversation_member_repo
            .expect_is_member()
            .returning(|_, _| Box::pin(async { Ok(true) }));

        let mut message_repo = MockMessageRepository::new();
        let msg_for_repo = msg.clone();
        message_repo
            .expect_get_by_id()
            .return_once(move |_| Box::pin(async move { Ok(Some(msg_for_repo)) }));

        let svc = DefaultMessageService::new(
            MessageRepos {
                conversation_repo: Arc::new(conversation_repo),
                conversation_member_repo: Arc::new(conversation_member_repo),
                message_repo: Arc::new(message_repo),
                message_attachment_repo: Arc::new(MockMessageAttachmentRepository::new()),
                message_mention_repo: Arc::new(MockMessageMentionRepository::new()),
                message_thread_repo: Arc::new(MockMessageThreadRepository::new()),
                server_member_repo: Arc::new(server_member_repo),
                user_repo: Arc::new(MockUserRepository::new()),
            },
            None,
            test_observers(),
        );

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

    /// Unlike the pre-FTS implementation, an empty/whitespace-only query is no longer an error —
    /// `parse_search_query` accepts it (operator-only or fully-empty searches are valid; see
    /// `rim_services::message::search_query`). What the service still must reject, *before*
    /// touching any repo, is an invalid parsed operator — proving `search_messages` propagates
    /// `parse_search_query`'s `Validation` error ahead of the access gate. No mock expectations
    /// are set on any repo here, so a repo call would panic — that's the proof parsing runs first.
    #[tokio::test]
    async fn search_messages_propagates_parser_validation_error() {
        let actor_id = UserId::new();
        let server_id = ServerId::new();
        let conv_id = ConversationId::new();

        let svc = DefaultMessageService::new(
            MessageRepos {
                conversation_repo: Arc::new(MockConversationRepository::new()),
                conversation_member_repo: Arc::new(MockConversationMemberRepository::new()),
                message_repo: Arc::new(MockMessageRepository::new()),
                message_attachment_repo: Arc::new(MockMessageAttachmentRepository::new()),
                message_mention_repo: Arc::new(MockMessageMentionRepository::new()),
                message_thread_repo: Arc::new(MockMessageThreadRepository::new()),
                server_member_repo: Arc::new(MockServerMemberRepository::new()),
                user_repo: Arc::new(MockUserRepository::new()),
            },
            None,
            test_observers(),
        );

        let err = svc
            .search_messages(SearchMessagesQuery {
                actor: Actor::new(actor_id),
                server_id,
                conversation_id: Some(conv_id),
                query: "has:bogus".to_string(),
                limit: 10,
                cursor: None,
                dir: PageDir::After,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }

    /// The repository returns `limit + 1` search rows nearest the cursor, newest first, so the
    /// overflow row (farthest from the cursor) is the last one for `After` but the first one for
    /// `Before`. The service must drop that row — not the one next to the cursor (finding A-H1).
    #[tokio::test]
    async fn search_messages_drops_the_overflow_row_farthest_from_the_cursor() {
        use rim_core::message::repository::MessageSearchHit;
        use rim_services::message::dto::MessagePageCursor;

        let actor_id = UserId::new();
        let server_id = ServerId::new();
        let conv_id = ConversationId::new();
        let base = chrono::Utc::now();
        // Newest first, as the repository returns them.
        let msgs: Vec<Message> = (0..3)
            .map(|i| Message {
                sent_at: base - chrono::Duration::minutes(i),
                ..message(actor_id, conv_id)
            })
            .collect();
        let key = |m: &Message| (m.sent_at, m.id);
        let cursor_key = |c: Option<MessagePageCursor>| c.map(|c| (c.sent_at, c.message_id));

        for (dir, want) in [
            (PageDir::After, vec![msgs[0].id, msgs[1].id]),
            (PageDir::Before, vec![msgs[1].id, msgs[2].id]),
        ] {
            let mut server_member_repo = MockServerMemberRepository::new();
            server_member_repo
                .expect_is_member()
                .returning(|_sid, _uid| Ok(true));
            let mut conversation_repo = MockConversationRepository::new();
            conversation_repo
                .expect_list_accessible_conversation_ids()
                .returning(move |_sid, _uid| Box::pin(async move { Ok(vec![conv_id]) }));
            let mut message_repo = MockMessageRepository::new();
            let hits: Vec<MessageSearchHit> = msgs
                .iter()
                .map(|m| MessageSearchHit {
                    message: m.clone(),
                    rank: 0.1,
                    headline: None,
                })
                .collect();
            message_repo
                .expect_search_messages_fts()
                .return_once(move |_params| Box::pin(async move { Ok(hits) }));
            let mut attachment_repo = MockMessageAttachmentRepository::new();
            attachment_repo
                .expect_get_attachments_for_messages()
                .return_once(|_ids| Box::pin(async { Ok(std::collections::HashMap::new()) }));
            let mut thread_repo = MockMessageThreadRepository::new();
            thread_repo
                .expect_get_thread_ids_by_root_messages()
                .return_once(|_ids| Box::pin(async { Ok(std::collections::HashMap::new()) }));

            let svc = DefaultMessageService::new(
                MessageRepos {
                    conversation_repo: Arc::new(conversation_repo),
                    conversation_member_repo: Arc::new(MockConversationMemberRepository::new()),
                    message_repo: Arc::new(message_repo),
                    message_attachment_repo: Arc::new(attachment_repo),
                    message_mention_repo: Arc::new(MockMessageMentionRepository::new()),
                    message_thread_repo: Arc::new(thread_repo),
                    server_member_repo: Arc::new(server_member_repo),
                    user_repo: Arc::new(MockUserRepository::new()),
                },
                None,
                test_observers(),
            );

            // The mock ignores the cursor; the service only needs to know one was sent.
            let cursor = MessagePageCursor {
                sent_at: base - chrono::Duration::minutes(10),
                message_id: MessageId::new(),
            };
            let page = svc
                .search_messages(SearchMessagesQuery {
                    actor: Actor::new(actor_id),
                    server_id,
                    conversation_id: None,
                    query: "needle".to_string(),
                    limit: 2,
                    cursor: Some(cursor),
                    dir,
                })
                .await
                .unwrap();

            let got: Vec<MessageId> = page.items.iter().map(|hit| hit.message.id).collect();
            assert_eq!(got, want, "{dir:?}: kept rows");
            let (next, prev) = match dir {
                PageDir::After => (key(&msgs[1]), key(&msgs[0])),
                PageDir::Before => (key(&msgs[1]), key(&msgs[2])),
            };
            assert_eq!(cursor_key(page.next), Some(next), "{dir:?}: next");
            assert_eq!(cursor_key(page.prev), Some(prev), "{dir:?}: prev");
        }
    }

    #[tokio::test]
    async fn get_message_not_found_when_message_not_in_conversation() {
        let actor_id = UserId::new();
        let server_id = ServerId::new();
        let conv_id = ConversationId::new();
        let conv = conversation(server_id);
        let msg = message(actor_id, ConversationId::new());

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_is_member()
            .returning(|_sid, _uid| Ok(true));

        let mut conversation_repo = MockConversationRepository::new();
        let conv_for_repo = conv.clone();
        conversation_repo.expect_get_by_id().returning(move |_id| {
            let c = conv_for_repo.clone();
            Box::pin(async move { Ok(Some(c)) })
        });

        let mut conversation_member_repo = MockConversationMemberRepository::new();
        conversation_member_repo
            .expect_is_member()
            .returning(|_cid, _uid| Box::pin(async { Ok(true) }));

        let mut message_repo = MockMessageRepository::new();
        let msg_for_repo = msg.clone();
        message_repo.expect_get_by_id().returning(move |_id| {
            let m = msg_for_repo.clone();
            Box::pin(async move { Ok(Some(m)) })
        });

        let svc = DefaultMessageService::new(
            MessageRepos {
                conversation_repo: Arc::new(conversation_repo),
                conversation_member_repo: Arc::new(conversation_member_repo),
                message_repo: Arc::new(message_repo),
                message_attachment_repo: Arc::new(MockMessageAttachmentRepository::new()),
                message_mention_repo: Arc::new(MockMessageMentionRepository::new()),
                message_thread_repo: Arc::new(MockMessageThreadRepository::new()),
                server_member_repo: Arc::new(server_member_repo),
                user_repo: Arc::new(MockUserRepository::new()),
            },
            None,
            test_observers(),
        );

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
