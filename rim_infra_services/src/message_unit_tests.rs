#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use rim_core::conversation::model::{Conversation, ConversationId};
    use rim_core::conversation::repository::MockConversationRepository;
    use rim_core::conversation_member::repository::MockConversationMemberRepository;
    use rim_core::message::model::{Message, MessageId, NewMessage};
    use rim_core::message::repository::{MessageChange, MockMessageRepository};
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
        DeleteMessageCmd, EditMessageCmd, GetMessageQuery, ListMessageChangesQuery,
        ListMessagesQuery, ListThreadMessagesQuery, MessageChangeCursor, SearchMessagesQuery,
        SendMessageCmd,
    };
    use rim_services::message::dto::{ConversationChangesStatus, MessageChangeKind};
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
            slow_mode_seconds: 0,
            encrypted_at: None,
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
            encryption: None,
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
        server_member_repo
            .expect_get_member_standing()
            .returning(|_, _| {
                Ok(Some(rim_core::server_member::model::MemberStanding::new(
                    rim_core::server_member::model::ServerMemberRole::Member,
                )))
            });

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
                        encryption: None,
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
                forwarded_from_message_id: None,
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
        server_member_repo
            .expect_get_member_standing()
            .returning(|_, _| {
                Ok(Some(rim_core::server_member::model::MemberStanding::new(
                    rim_core::server_member::model::ServerMemberRole::Member,
                )))
            });

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
                forwarded_from_message_id: None,
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
        server_member_repo
            .expect_get_member_standing()
            .returning(|_, _| {
                Ok(Some(rim_core::server_member::model::MemberStanding::new(
                    rim_core::server_member::model::ServerMemberRole::Member,
                )))
            });

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
        server_member_repo
            .expect_get_member_standing()
            .returning(|_, _| {
                Ok(Some(rim_core::server_member::model::MemberStanding::new(
                    rim_core::server_member::model::ServerMemberRole::Member,
                )))
            });

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
        server_member_repo
            .expect_get_member_standing()
            .returning(|_, _| {
                Ok(Some(rim_core::server_member::model::MemberStanding::new(
                    rim_core::server_member::model::ServerMemberRole::Member,
                )))
            });

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
        server_member_repo
            .expect_get_member_standing()
            .returning(|_, _| {
                Ok(Some(rim_core::server_member::model::MemberStanding::new(
                    rim_core::server_member::model::ServerMemberRole::Member,
                )))
            });

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
            encryption: None,
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
        server_member_repo
            .expect_get_member_standing()
            .returning(|_, _| {
                Ok(Some(rim_core::server_member::model::MemberStanding::new(
                    rim_core::server_member::model::ServerMemberRole::Member,
                )))
            });

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

    /// An empty/whitespace-only query is not an error —
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
    /// `Before`. The service must drop that row — not the one next to the cursor.
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
            server_member_repo
                .expect_get_member_standing()
                .returning(|_, _| {
                    Ok(Some(rim_core::server_member::model::MemberStanding::new(
                        rim_core::server_member::model::ServerMemberRole::Member,
                    )))
                });
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
        server_member_repo
            .expect_get_member_standing()
            .returning(|_, _| {
                Ok(Some(rim_core::server_member::model::MemberStanding::new(
                    rim_core::server_member::model::ServerMemberRole::Member,
                )))
            });

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

    // ── Slow mode and moderator deletes ──

    #[test]
    fn slow_mode_retry_after_rounds_up_and_expires() {
        use crate::message::slow_mode_retry_after;
        let now = chrono::Utc::now();
        assert_eq!(slow_mode_retry_after(0, now, now), None, "off");
        assert_eq!(slow_mode_retry_after(30, now, now), Some(30));
        assert_eq!(
            slow_mode_retry_after(30, now - chrono::Duration::milliseconds(10_500), now),
            Some(20),
            "19.5 s left rounds up to 20"
        );
        assert_eq!(
            slow_mode_retry_after(30, now - chrono::Duration::milliseconds(29_999), now),
            Some(1)
        );
        assert_eq!(
            slow_mode_retry_after(30, now - chrono::Duration::seconds(30), now),
            None
        );
        assert_eq!(
            slow_mode_retry_after(30, now + chrono::Duration::seconds(5), now),
            Some(30),
            "a clock skewed into the future never shortens the wait below the interval"
        );
    }

    /// A message service for slow-mode tests: one public channel with `slow_mode_seconds`, the
    /// actor's last message there `last_sent_ago` ago, and the actor holding `role`.
    fn slow_mode_service(
        role: rim_core::server_member::model::ServerMemberRole,
        slow_mode_seconds: i32,
        last_sent_ago: chrono::Duration,
    ) -> (DefaultMessageService, ServerId, ConversationId) {
        slow_mode_service_with_users(
            role,
            slow_mode_seconds,
            last_sent_ago,
            MockUserRepository::new(),
        )
    }

    /// [`slow_mode_service`] whose user repository is `user_repo`.
    fn slow_mode_service_with_users(
        role: rim_core::server_member::model::ServerMemberRole,
        slow_mode_seconds: i32,
        last_sent_ago: chrono::Duration,
        user_repo: MockUserRepository,
    ) -> (DefaultMessageService, ServerId, ConversationId) {
        let server_id = ServerId::new();
        let mut conv = conversation(server_id);
        conv.slow_mode_seconds = slow_mode_seconds;
        let conv_id = conv.id;

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_get_member_standing()
            .returning(move |_, _| {
                Ok(Some(rim_core::server_member::model::MemberStanding::new(
                    role,
                )))
            });
        let mut conversation_repo = MockConversationRepository::new();
        conversation_repo.expect_get_by_id().returning(move |_| {
            let c = conv.clone();
            Box::pin(async move { Ok(Some(c)) })
        });
        conversation_repo
            .expect_touch_last_activity_at()
            .returning(|_, _| Box::pin(async { Ok(()) }));
        let mut conversation_member_repo = MockConversationMemberRepository::new();
        conversation_member_repo
            .expect_list_member_ids()
            .returning(|_| Box::pin(async { Ok(vec![]) }));
        let mut message_repo = MockMessageRepository::new();
        message_repo
            .expect_last_sent_at_by_author()
            .returning(move |_, _| {
                Box::pin(async move { Ok(Some(chrono::Utc::now() - last_sent_ago)) })
            });
        message_repo.expect_create_message().returning(|msg| {
            let msg = msg.clone();
            Box::pin(async move {
                Ok(Message {
                    id: MessageId::new(),
                    conversation_id: msg.conversation_id,
                    thread_id: None,
                    sent_by: msg.sent_by,
                    text: msg.text,
                    has_attachments: false,
                    encryption: None,
                    parent_message_id: None,
                    forwarded_from_message_id: None,
                    is_deleted: false,
                    sent_at: chrono::Utc::now(),
                })
            })
        });
        let mut message_attachment_repo = MockMessageAttachmentRepository::new();
        message_attachment_repo
            .expect_get_attachments_for_message()
            .returning(|_| Box::pin(async { Ok(vec![]) }));
        let mut thread_repo = MockMessageThreadRepository::new();
        thread_repo
            .expect_get_by_root_message()
            .returning(|_| Box::pin(async { Ok(None) }));

        let svc = DefaultMessageService::new(
            MessageRepos {
                conversation_repo: Arc::new(conversation_repo),
                conversation_member_repo: Arc::new(conversation_member_repo),
                message_repo: Arc::new(message_repo),
                message_attachment_repo: Arc::new(message_attachment_repo),
                message_mention_repo: Arc::new(MockMessageMentionRepository::new()),
                message_thread_repo: Arc::new(thread_repo),
                server_member_repo: Arc::new(server_member_repo),
                user_repo: Arc::new(user_repo),
            },
            None,
            test_observers(),
        );
        (svc, server_id, conv_id)
    }

    fn send(server_id: ServerId, conversation_id: ConversationId) -> SendMessageCmd {
        SendMessageCmd {
            actor: Actor::new(UserId::new()),
            server_id,
            conversation_id,
            text: Some("hello".to_string()),
            parent_message_id: None,
            forwarded_from_message_id: None,
        }
    }

    #[tokio::test]
    async fn slow_mode_refuses_a_member_with_retry_after() {
        use rim_core::server_member::model::ServerMemberRole;
        let (svc, server_id, conv_id) =
            slow_mode_service(ServerMemberRole::Member, 60, chrono::Duration::seconds(15));
        let err = svc
            .send_message(send(server_id, conv_id))
            .await
            .unwrap_err();
        match err {
            ServiceError::RateLimited {
                retry_after_seconds,
            } => assert!(
                (44..=45).contains(&retry_after_seconds),
                "{retry_after_seconds}"
            ),
            other => panic!("expected RateLimited, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn slow_mode_allows_after_the_interval_and_exempts_moderators() {
        use rim_core::server_member::model::ServerMemberRole;
        let (svc, server_id, conv_id) =
            slow_mode_service(ServerMemberRole::Member, 60, chrono::Duration::seconds(61));
        svc.send_message(send(server_id, conv_id))
            .await
            .expect("interval elapsed");
        for role in [ServerMemberRole::Owner, ServerMemberRole::Moderator] {
            let (svc, server_id, conv_id) =
                slow_mode_service(role, 60, chrono::Duration::seconds(1));
            svc.send_message(send(server_id, conv_id))
                .await
                .unwrap_or_else(|e| panic!("{role:?} must be exempt: {e:?}"));
        }
    }

    #[tokio::test]
    async fn the_message_budget_is_per_user_and_covers_replies_forwards_and_encrypted_sends() {
        use rim_core::server_member::model::ServerMemberRole;
        let (svc, server_id, conv_id) =
            slow_mode_service(ServerMemberRole::Owner, 0, chrono::Duration::seconds(1));
        let rate = crate::token_bucket::BucketRate::new(1, 2).expect("a rate");
        let svc = svc.with_message_budget(rate);
        let sender = send(server_id, conv_id);
        let actor = sender.actor;
        for _ in 0..2 {
            svc.send_message(sender.clone())
                .await
                .expect("within the burst");
        }
        let assert_limited = |result: Result<_, ServiceError>| match result {
            Err(ServiceError::RateLimited {
                retry_after_seconds,
            }) => assert!(
                (59..=60).contains(&retry_after_seconds),
                "{retry_after_seconds}"
            ),
            other => panic!("expected RateLimited, got {other:?}"),
        };
        assert_limited(svc.send_message(sender.clone()).await);
        assert_limited(
            svc.reply_to_message(rim_services::message::command::ReplyToMessageCmd {
                actor,
                server_id,
                conversation_id: conv_id,
                root_message_id: MessageId::new(),
                text: "reply".to_string(),
                parent_message_id: None,
            })
            .await,
        );
        assert_limited(
            svc.forward_message(rim_services::message::command::ForwardMessageCmd {
                actor,
                server_id,
                source_conversation_id: conv_id,
                target_conversation_id: conv_id,
                message_id: MessageId::new(),
            })
            .await,
        );
        assert_limited(
            svc.send_encrypted_message(rim_services::message::command::SendEncryptedMessageCmd {
                caller: rim_services::e2ee::command::MlsCaller {
                    actor,
                    device_key_id: None,
                },
                server_id,
                conversation_id: conv_id,
                message_id: MessageId::new(),
                envelope: String::new(),
                thread_root_message_id: None,
                forwarded_from_message_id: None,
            })
            .await,
        );
        svc.send_message(send(server_id, conv_id))
            .await
            .expect("another user has their own budget");
    }

    #[tokio::test]
    async fn message_text_follows_the_message_bytes_cap_in_bytes() {
        use rim_core::server_member::model::ServerMemberRole;
        let (svc, server_id, conv_id) =
            slow_mode_service(ServerMemberRole::Owner, 0, chrono::Duration::seconds(1));
        let capped = svc
            .clone()
            .with_limits(rim_services::limits::InstanceLimits {
                message_bytes: Some(16),
                ..Default::default()
            });
        let with_text = |text: &str| SendMessageCmd {
            text: Some(text.to_string()),
            ..send(server_id, conv_id)
        };
        let refused = |result: Result<_, ServiceError>| match result {
            Err(ServiceError::Validation(message)) => {
                assert_eq!(message, "this instance allows 16 bytes of text per message")
            }
            other => panic!("expected Validation, got {other:?}"),
        };

        capped
            .send_message(with_text(&"a".repeat(16)))
            .await
            .expect("at the cap");
        refused(capped.send_message(with_text(&"a".repeat(17))).await);
        // Nine characters, eighteen bytes.
        refused(capped.send_message(with_text(&"é".repeat(9))).await);
        refused(
            capped
                .edit_message(rim_services::message::command::EditMessageCmd {
                    actor: Actor::new(UserId::new()),
                    server_id,
                    conversation_id: conv_id,
                    message_id: MessageId::new(),
                    text: "a".repeat(17),
                })
                .await,
        );
        svc.send_message(with_text(&"a".repeat(9_000)))
            .await
            .expect("no cap");
    }

    #[test]
    fn an_encrypted_message_may_exceed_the_text_cap_only_by_its_framing() {
        let (svc, _, _) = slow_mode_service(
            rim_core::server_member::model::ServerMemberRole::Owner,
            0,
            chrono::Duration::seconds(1),
        );
        let capped = svc
            .clone()
            .with_limits(rim_services::limits::InstanceLimits {
                message_bytes: Some(16),
                ..Default::default()
            });
        let framing = 4096;
        assert!(capped.ensure_ciphertext_bytes(16 + framing, 0).is_ok());
        assert!(
            capped
                .ensure_ciphertext_bytes(16 + framing + 100, 100)
                .is_ok(),
            "the authenticated data does not count"
        );
        assert!(matches!(
            capped.ensure_ciphertext_bytes(17 + framing, 0),
            Err(ServiceError::Validation(m)) if m == "this instance allows 16 bytes of text per message"
        ));
        assert!(svc.ensure_ciphertext_bytes(96 * 1024, 0).is_ok());
    }

    /// An account in probation draws on the probation rate instead of the normal one, and
    /// moves to the normal one once probation ends; without probation the normal rate applies.
    #[tokio::test]
    async fn an_account_in_probation_sends_at_the_probation_rate() {
        use rim_core::server_member::model::ServerMemberRole;
        let created = chrono::Utc::now();
        let budget = |probation_hours, now| {
            let author = UserId::new();
            let (svc, server_id, conv_id) = slow_mode_service_with_users(
                ServerMemberRole::Owner,
                0,
                chrono::Duration::seconds(1),
                crate::probation_fixtures::users(vec![crate::probation_fixtures::account(
                    author, created, None,
                )]),
            );
            let cmd = SendMessageCmd {
                actor: Actor::new(author),
                ..send(server_id, conv_id)
            };
            let svc = svc
                .with_message_budget(crate::token_bucket::BucketRate::new(10, 3).expect("rate"))
                .with_probation_budget(crate::token_bucket::BucketRate::new(1, 1).expect("rate"))
                .with_limits(rim_services::limits::InstanceLimits {
                    probation_hours,
                    ..Default::default()
                })
                .with_clock(crate::clock::Clock::fixed(now));
            (svc, cmd)
        };
        let sent_before_limited = |(svc, cmd): (DefaultMessageService, SendMessageCmd)| async move {
            let mut sent = 0;
            while svc.send_message(cmd.clone()).await.is_ok() {
                sent += 1;
                assert!(sent < 10, "never limited");
            }
            sent
        };
        let in_probation = created + chrono::Duration::minutes(10);
        assert_eq!(sent_before_limited(budget(Some(1), in_probation)).await, 1);
        let after = created + chrono::Duration::hours(1);
        assert_eq!(sent_before_limited(budget(Some(1), after)).await, 3);
        assert_eq!(sent_before_limited(budget(None, in_probation)).await, 3);
    }

    #[tokio::test]
    async fn timed_out_member_cannot_send() {
        let server_id = ServerId::new();
        let conv = conversation(server_id);
        let conv_id = conv.id;
        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_get_member_standing()
            .returning(|_, _| {
                Ok(Some(rim_core::server_member::model::MemberStanding {
                    role: rim_core::server_member::model::ServerMemberRole::Member,
                    timed_out_until: Some(chrono::Utc::now() + chrono::Duration::minutes(5)),
                }))
            });
        let mut conversation_repo = MockConversationRepository::new();
        conversation_repo.expect_get_by_id().returning(move |_| {
            let c = conv.clone();
            Box::pin(async move { Ok(Some(c)) })
        });
        let svc = DefaultMessageService::new(
            MessageRepos {
                conversation_repo: Arc::new(conversation_repo),
                conversation_member_repo: Arc::new(MockConversationMemberRepository::new()),
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
            .send_message(send(server_id, conv_id))
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::TimedOut { .. }), "{err:?}");
    }

    #[derive(Default)]
    struct RecordingAudit(std::sync::Mutex<Vec<rim_services::audit::dto::AuditLogEntryDto>>);

    #[async_trait::async_trait]
    impl rim_services::audit::service::AuditLogService for RecordingAudit {
        async fn write_entry(
            &self,
            entry: rim_services::audit::dto::AuditLogEntryDto,
        ) -> rim_services::ServiceResult<()> {
            self.0.lock().expect("lock").push(entry);
            Ok(())
        }

        async fn list_entries(
            &self,
            _caller: Actor,
            _query: rim_services::audit::dto::ListAuditLogQuery,
        ) -> rim_services::ServiceResult<rim_services::audit::dto::AuditLogPage> {
            Err(ServiceError::NotFound)
        }
    }

    /// The author's message, deleted by `actor` holding `role`; returns the result and the
    /// audit rows written.
    async fn delete_as(
        role: rim_core::server_member::model::ServerMemberRole,
        actor_is_author: bool,
    ) -> (
        Result<(), ServiceError>,
        Vec<rim_services::audit::dto::AuditLogEntryDto>,
    ) {
        let server_id = ServerId::new();
        let conv = conversation(server_id);
        let conv_id = conv.id;
        let author = UserId::new();
        let actor = if actor_is_author {
            author
        } else {
            UserId::new()
        };
        let msg = message(author, conv_id);
        let msg_id = msg.id;

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_get_member_standing()
            .returning(move |_, _| {
                Ok(Some(rim_core::server_member::model::MemberStanding::new(
                    role,
                )))
            });
        let mut conversation_repo = MockConversationRepository::new();
        conversation_repo.expect_get_by_id().returning(move |_| {
            let c = conv.clone();
            Box::pin(async move { Ok(Some(c)) })
        });
        let mut conversation_member_repo = MockConversationMemberRepository::new();
        conversation_member_repo
            .expect_list_member_ids()
            .returning(|_| Box::pin(async { Ok(vec![]) }));
        // A guest reads a public channel only as a member; make every actor one.
        conversation_member_repo
            .expect_is_member()
            .returning(|_, _| Box::pin(async { Ok(true) }));
        let mut message_repo = MockMessageRepository::new();
        message_repo.expect_get_by_id().returning(move |_| {
            let m = msg.clone();
            Box::pin(async move { Ok(Some(m)) })
        });
        message_repo
            .expect_soft_delete_message()
            .returning(|_| Box::pin(async { Ok(()) }));

        let audit = Arc::new(RecordingAudit::default());
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
        )
        .with_audit_log_service(
            Arc::clone(&audit) as Arc<dyn rim_services::audit::service::AuditLogService>
        );
        let res = svc
            .delete_message(DeleteMessageCmd {
                actor: Actor::new(actor),
                server_id,
                conversation_id: conv_id,
                message_id: msg_id,
                hard: false,
            })
            .await;
        let rows = audit.0.lock().expect("lock").clone();
        (res, rows)
    }

    #[tokio::test]
    async fn moderators_delete_others_messages_with_an_audit_row() {
        use rim_core::server_member::model::ServerMemberRole;

        for role in [ServerMemberRole::Owner, ServerMemberRole::Moderator] {
            let (res, rows) = delete_as(role, false).await;
            res.unwrap_or_else(|e| panic!("{role:?} delete: {e:?}"));
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].command_id, "admin.delete_message");
            assert!(rows[0].args_redacted.get("author_id").is_some());
            assert!(
                !rows[0].args_redacted.to_string().contains("\"t\""),
                "message text is never copied into the audit log"
            );
        }
        for role in [ServerMemberRole::Member, ServerMemberRole::Guest] {
            let (res, rows) = delete_as(role, false).await;
            assert!(
                matches!(res, Err(ServiceError::PermissionDenied)),
                "{role:?}"
            );
            assert!(rows.is_empty());
        }
        // An author deleting their own message is not a moderation action.
        let (res, rows) = delete_as(ServerMemberRole::Moderator, true).await;
        res.expect("own delete");
        assert!(rows.is_empty());
    }

    /// The author's message in a DM `actor` is not in, deleted by `actor` holding `role`, with
    /// or without a verified franked report of it; returns the result and the audit rows.
    async fn delete_outside_as(
        role: rim_core::server_member::model::ServerMemberRole,
        verified_report: bool,
    ) -> (
        Result<(), ServiceError>,
        Vec<rim_services::audit::dto::AuditLogEntryDto>,
    ) {
        let server_id = ServerId::new();
        let mut conv = conversation(server_id);
        conv.conversation_type = rim_core::conversation::model::ConversationType::Direct;
        conv.encrypted_at = Some(chrono::Utc::now());
        let conv_id = conv.id;
        let author = UserId::new();
        let actor = UserId::new();
        let msg = message(author, conv_id);
        let msg_id = msg.id;

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_get_member_standing()
            .returning(move |_, _| {
                Ok(Some(rim_core::server_member::model::MemberStanding::new(
                    role,
                )))
            });
        let mut conversation_repo = MockConversationRepository::new();
        conversation_repo.expect_get_by_id().returning(move |_| {
            let c = conv.clone();
            Box::pin(async move { Ok(Some(c)) })
        });
        let mut conversation_member_repo = MockConversationMemberRepository::new();
        conversation_member_repo
            .expect_list_member_ids()
            .returning(|_| Box::pin(async { Ok(vec![]) }));
        conversation_member_repo
            .expect_is_member()
            .returning(|_, _| Box::pin(async { Ok(false) }));
        let mut message_repo = MockMessageRepository::new();
        message_repo.expect_get_by_id().returning(move |_| {
            let m = msg.clone();
            Box::pin(async move { Ok(Some(m)) })
        });
        message_repo
            .expect_soft_delete_message()
            .returning(|_| Box::pin(async { Ok(()) }));
        let mut moderation_repo = rim_core::moderation::repository::MockModerationRepository::new();
        moderation_repo
            .expect_has_verified_report()
            .withf(move |s, m| *s == server_id && *m == msg_id)
            .returning(move |_, _| Box::pin(async move { Ok(verified_report) }));

        let audit = Arc::new(RecordingAudit::default());
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
        )
        .with_audit_log_service(
            Arc::clone(&audit) as Arc<dyn rim_services::audit::service::AuditLogService>
        )
        .with_moderation_repo(Arc::new(moderation_repo));
        let res = svc
            .delete_message(DeleteMessageCmd {
                actor: Actor::new(actor),
                server_id,
                conversation_id: conv_id,
                message_id: msg_id,
                hard: false,
            })
            .await;
        let rows = audit.0.lock().expect("lock").clone();
        (res, rows)
    }

    #[tokio::test]
    async fn a_verified_report_lets_moderators_delete_outside_the_conversation() {
        use rim_core::server_member::model::ServerMemberRole;

        for role in [ServerMemberRole::Owner, ServerMemberRole::Moderator] {
            let (res, rows) = delete_outside_as(role, true).await;
            res.unwrap_or_else(|e| panic!("{role:?} delete: {e:?}"));
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].command_id, "admin.delete_message");

            let (res, rows) = delete_outside_as(role, false).await;
            assert!(
                matches!(res, Err(ServiceError::PermissionDenied)),
                "{role:?} without a report"
            );
            assert!(rows.is_empty());
        }
        for role in [ServerMemberRole::Member, ServerMemberRole::Guest] {
            let (res, rows) = delete_outside_as(role, true).await;
            assert!(
                matches!(res, Err(ServiceError::PermissionDenied)),
                "{role:?}"
            );
            assert!(rows.is_empty());
        }
    }

    // ── Notifications after commit, best-effort ──

    /// A failing notification service never fails the send; it is called once, after the
    /// message is stored, with the conversation's member list.
    #[tokio::test]
    async fn send_message_succeeds_when_notifications_fail() {
        let actor_id = UserId::new();
        let bob = UserId::new();
        let server_id = ServerId::new();
        let conv = conversation(server_id);
        let conv_id = conv.id;
        let msg = message(actor_id, conv_id);
        let msg_id = msg.id;

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_is_member()
            .returning(|_sid, _uid| Ok(true));
        server_member_repo
            .expect_get_member_standing()
            .returning(|_, _| {
                Ok(Some(rim_core::server_member::model::MemberStanding::new(
                    rim_core::server_member::model::ServerMemberRole::Member,
                )))
            });
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
            .returning(move |_cid| Box::pin(async move { Ok(vec![actor_id, bob]) }));
        let mut message_repo = MockMessageRepository::new();
        let msg_for_repo = msg.clone();
        message_repo.expect_create_message().returning(move |_| {
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

        let mut notifications = rim_services::notification::service::MockNotificationService::new();
        notifications
            .expect_notify_for_message()
            .withf(move |cmd| {
                cmd.server_id == server_id
                    && cmd.conversation.id == conv_id
                    && cmd.message.id == msg_id
                    && cmd.member_user_ids == vec![actor_id, bob]
            })
            .times(1)
            .returning(|_| Box::pin(async { Err(ServiceError::Unavailable) }));

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
        )
        .with_notification_service(Arc::new(notifications));

        let dto = svc
            .send_message(SendMessageCmd {
                actor: Actor::new(actor_id),
                server_id,
                conversation_id: conv_id,
                text: Some("hi".to_string()),
                parent_message_id: None,
                forwarded_from_message_id: None,
            })
            .await
            .expect("the send succeeds although notifications failed");
        assert_eq!(dto.id, msg_id);
    }

    /// Deleting a message drops its unread notifications (best-effort: a failure is logged).
    #[tokio::test]
    async fn delete_message_forgets_unread_notifications() {
        let server_id = ServerId::new();
        let conv = conversation(server_id);
        let conv_id = conv.id;
        let author = UserId::new();
        let msg = message(author, conv_id);
        let msg_id = msg.id;

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_get_member_standing()
            .returning(move |_, _| {
                Ok(Some(rim_core::server_member::model::MemberStanding::new(
                    rim_core::server_member::model::ServerMemberRole::Member,
                )))
            });
        let mut conversation_repo = MockConversationRepository::new();
        conversation_repo.expect_get_by_id().returning(move |_| {
            let c = conv.clone();
            Box::pin(async move { Ok(Some(c)) })
        });
        let mut conversation_member_repo = MockConversationMemberRepository::new();
        conversation_member_repo
            .expect_list_member_ids()
            .returning(|_| Box::pin(async { Ok(vec![]) }));
        conversation_member_repo
            .expect_is_member()
            .returning(|_, _| Box::pin(async { Ok(true) }));
        let mut message_repo = MockMessageRepository::new();
        message_repo.expect_get_by_id().returning(move |_| {
            let m = msg.clone();
            Box::pin(async move { Ok(Some(m)) })
        });
        message_repo
            .expect_soft_delete_message()
            .returning(|_| Box::pin(async { Ok(()) }));
        let mut notifications = rim_services::notification::service::MockNotificationService::new();
        notifications
            .expect_forget_message()
            .withf(move |id| *id == msg_id)
            .times(1)
            .returning(|_| Box::pin(async { Err(ServiceError::Unavailable) }));

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
        )
        .with_notification_service(Arc::new(notifications));
        svc.delete_message(DeleteMessageCmd {
            actor: Actor::new(author),
            server_id,
            conversation_id: conv_id,
            message_id: msg_id,
            hard: false,
        })
        .await
        .expect("delete succeeds although forgetting notifications failed");
    }

    fn change_feed_server() -> ServerId {
        ServerId::from(uuid::Uuid::from_u128(1))
    }

    /// A service whose access gate admits the actor to every conversation (or to none, with
    /// `member = false`), wired to `message_repo`.
    fn change_feed_service(
        message_repo: MockMessageRepository,
        member: bool,
    ) -> DefaultMessageService {
        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_is_member()
            .returning(|_sid, _uid| Ok(true));
        server_member_repo
            .expect_get_member_standing()
            .returning(|_, _| {
                Ok(Some(rim_core::server_member::model::MemberStanding::new(
                    rim_core::server_member::model::ServerMemberRole::Member,
                )))
            });

        let mut conversation_repo = MockConversationRepository::new();
        conversation_repo.expect_get_by_id().returning(move |id| {
            Box::pin(async move {
                let mut conv = conversation(change_feed_server());
                conv.id = id;
                conv.is_private = true;
                Ok(Some(conv))
            })
        });

        let mut conversation_member_repo = MockConversationMemberRepository::new();
        conversation_member_repo
            .expect_is_member()
            .returning(move |_cid, _uid| Box::pin(async move { Ok(member) }));

        let mut message_attachment_repo = MockMessageAttachmentRepository::new();
        message_attachment_repo
            .expect_get_attachments_for_messages()
            .returning(|_ids| Box::pin(async { Ok(std::collections::HashMap::new()) }));
        let mut thread_repo = MockMessageThreadRepository::new();
        thread_repo
            .expect_get_thread_ids_by_root_messages()
            .returning(|_ids| Box::pin(async { Ok(std::collections::HashMap::new()) }));

        DefaultMessageService::new(
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
        )
    }

    fn change_feed_query(
        conversations: Vec<MessageChangeCursor>,
        limit: u32,
    ) -> ListMessageChangesQuery {
        ListMessageChangesQuery {
            actor: Actor::new(UserId::new()),
            server_id: change_feed_server(),
            conversations,
            limit,
        }
    }

    #[tokio::test]
    async fn list_message_changes_derives_kinds_and_the_next_cursor() {
        let conv_id = ConversationId::new();
        let author = UserId::new();
        let created = message(author, conv_id);
        let edited = message(author, conv_id);
        let mut deleted = message(author, conv_id);
        deleted.is_deleted = true;
        let beyond_the_page = message(author, conv_id);
        let rows = vec![
            MessageChange {
                message: created.clone(),
                version: 8,
                edited_at: None,
            },
            MessageChange {
                message: edited.clone(),
                version: 9,
                edited_at: Some(chrono::Utc::now()),
            },
            // Deleted wins over edited.
            MessageChange {
                message: deleted.clone(),
                version: 11,
                edited_at: Some(chrono::Utc::now()),
            },
            MessageChange {
                message: beyond_the_page,
                version: 12,
                edited_at: None,
            },
        ];

        let mut message_repo = MockMessageRepository::new();
        message_repo
            .expect_latest_change_version()
            .returning(|_| Box::pin(async { Ok(12) }));
        message_repo
            .expect_list_message_changes()
            .withf(move |cid, after, limit| *cid == conv_id && *after == 7 && *limit == 3)
            .return_once(move |_, _, _| Box::pin(async move { Ok(rows) }));
        let svc = change_feed_service(message_repo, true);

        let feed = svc
            .list_message_changes(change_feed_query(
                vec![MessageChangeCursor {
                    conversation_id: conv_id,
                    after_version: 7,
                }],
                3,
            ))
            .await
            .unwrap();

        assert_eq!(feed.len(), 1);
        let entry = &feed[0];
        assert_eq!(entry.conversation_id, conv_id);
        assert!(entry.has_more, "the repo returned limit + 1 rows");
        assert_eq!(
            entry.cursor, 11,
            "the last returned change, not the extra row"
        );
        assert_eq!(
            entry
                .changes
                .iter()
                .map(|c| (c.message.id, c.version, c.kind))
                .collect::<Vec<_>>(),
            vec![
                (created.id, 8, MessageChangeKind::Created),
                (edited.id, 9, MessageChangeKind::Edited),
                (deleted.id, 11, MessageChangeKind::Deleted),
            ]
        );
    }

    #[tokio::test]
    async fn list_message_changes_echoes_the_cursor_when_nothing_changed() {
        let conv_id = ConversationId::new();
        let mut message_repo = MockMessageRepository::new();
        message_repo
            .expect_latest_change_version()
            .returning(|_| Box::pin(async { Ok(42) }));
        message_repo
            .expect_list_message_changes()
            .return_once(|_, _, _| Box::pin(async { Ok(vec![]) }));
        let svc = change_feed_service(message_repo, true);

        let feed = svc
            .list_message_changes(change_feed_query(
                vec![MessageChangeCursor {
                    conversation_id: conv_id,
                    after_version: 42,
                }],
                10,
            ))
            .await
            .unwrap();

        assert_eq!(feed[0].cursor, 42);
        assert!(!feed[0].has_more);
        assert!(feed[0].changes.is_empty());
    }

    #[tokio::test]
    async fn list_message_changes_reports_an_unreadable_conversation_on_its_entry() {
        // No `list_message_changes` expectation: reading the conversation would panic.
        let svc = change_feed_service(MockMessageRepository::new(), false);
        let conv_id = ConversationId::new();

        let feed = svc
            .list_message_changes(change_feed_query(
                vec![MessageChangeCursor {
                    conversation_id: conv_id,
                    after_version: 7,
                }],
                10,
            ))
            .await
            .unwrap();
        assert_eq!(feed.len(), 1);
        assert_eq!(feed[0].conversation_id, conv_id);
        assert_eq!(feed[0].status, ConversationChangesStatus::NoAccess);
        assert!(feed[0].changes.is_empty());
        assert_eq!(feed[0].cursor, 7, "the cursor is echoed");
        assert!(!feed[0].has_more);
    }

    #[tokio::test]
    async fn list_message_changes_rejects_bad_requests_before_reading() {
        let svc = change_feed_service(MockMessageRepository::new(), true);
        let cursor = |after_version| MessageChangeCursor {
            conversation_id: ConversationId::new(),
            after_version,
        };

        let err = svc
            .list_message_changes(change_feed_query(vec![cursor(-1)], 10))
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));

        let err = svc
            .list_message_changes(change_feed_query((0..101).map(|_| cursor(0)).collect(), 10))
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[tokio::test]
    async fn list_message_changes_restarts_a_cursor_ahead_of_the_counter() {
        // The counters were reset (rollback and re-run, or a restored backup): reading on from
        // the stored cursor would skip changes, so the conversation starts over from 0.
        let mut message_repo = MockMessageRepository::new();
        message_repo
            .expect_latest_change_version()
            .returning(|_| Box::pin(async { Ok(5) }));
        message_repo
            .expect_list_message_changes()
            .withf(|_, after, _| *after == 0)
            .returning(|_, _, _| Box::pin(async { Ok(vec![]) }));
        let svc = change_feed_service(message_repo, true);

        let feed = svc
            .list_message_changes(change_feed_query(
                vec![MessageChangeCursor {
                    conversation_id: ConversationId::new(),
                    after_version: 6,
                }],
                10,
            ))
            .await
            .unwrap();
        assert_eq!(feed[0].status, ConversationChangesStatus::Reset);
        assert_eq!(feed[0].cursor, 0);
    }

    #[tokio::test]
    async fn list_message_changes_shares_a_total_budget_across_conversations() {
        let mut message_repo = MockMessageRepository::new();
        message_repo
            .expect_list_message_changes()
            .times(50)
            .withf(|_, _, limit| *limit == 20)
            .returning(|_, _, _| Box::pin(async { Ok(vec![]) }));
        let svc = change_feed_service(message_repo, true);

        let conversations = (0..50)
            .map(|_| MessageChangeCursor {
                conversation_id: ConversationId::new(),
                after_version: 0,
            })
            .collect();
        svc.list_message_changes(change_feed_query(conversations, 200))
            .await
            .unwrap();
    }
}
