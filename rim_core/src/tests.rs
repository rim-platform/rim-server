#[cfg(test)]
mod pagination {
    use crate::repository::pagination::{Page, PageDir, SortOrder};

    #[test]
    fn sort_order_is_copy_and_comparable() {
        let a = SortOrder::Asc;
        let b = a;
        assert_eq!(a, b);
    }

    #[test]
    fn page_dir_is_copy_and_comparable() {
        let a = PageDir::Before;
        let b = a;
        assert_eq!(a, b);
    }

    #[test]
    fn page_new_sets_fields() {
        let page = Page::new(vec![1, 2, 3], Some("n"), Some("p"));
        assert_eq!(page.items, vec![1, 2, 3]);
        assert_eq!(page.next, Some("n"));
        assert_eq!(page.prev, Some("p"));
    }
}

#[cfg(test)]
mod domain_helpers {
    use crate::command::model::is_user_command_key;
    use crate::conversation::model::ConversationId;
    use crate::message::model::{Message, MessageId, NewMessage};
    use crate::message_thread::model::{MessageThread, MessageThreadId};
    use crate::user::model::UserId;

    #[test]
    fn message_helpers_distinguish_timeline_and_thread_reply() {
        let timeline = Message {
            id: MessageId::new(),
            conversation_id: ConversationId::new(),
            thread_id: None,
            parent_message_id: None,
            sent_at: chrono::Utc::now(),
            sent_by: UserId::new(),
            text: Some("hello".to_string()),
            is_deleted: false,
            has_attachments: false,
        };
        let reply = Message {
            thread_id: Some(MessageThreadId::new()),
            ..timeline.clone()
        };

        assert!(timeline.is_timeline_message());
        assert!(!timeline.is_thread_reply());
        assert!(reply.is_thread_reply());
        assert!(!reply.is_timeline_message());
    }

    #[test]
    fn new_message_helpers_distinguish_timeline_and_thread_reply() {
        let timeline = NewMessage {
            conversation_id: ConversationId::new(),
            text: Some("hello".to_string()),
            sent_by: UserId::new(),
            thread_id: None,
            parent_message_id: None,
            attachments: vec![],
        };
        let reply = NewMessage {
            thread_id: Some(MessageThreadId::new()),
            ..timeline.clone()
        };

        assert!(timeline.is_timeline_message());
        assert!(reply.is_thread_reply());
    }

    #[test]
    fn message_thread_helper_reports_root_message_presence() {
        let without_root = MessageThread {
            id: MessageThreadId::new(),
            conversation_id: ConversationId::new(),
            first_message_id: None,
            created_at: chrono::Utc::now(),
            last_message_at: chrono::Utc::now(),
        };
        let with_root = MessageThread {
            first_message_id: Some(MessageId::new()),
            ..without_root.clone()
        };

        assert!(!without_root.has_root_message());
        assert!(with_root.has_root_message());
    }

    #[test]
    fn user_command_key_helper_requires_user_namespace() {
        assert!(is_user_command_key("user.quick_group"));
        assert!(!is_user_command_key("chat.post_message"));
    }
}
