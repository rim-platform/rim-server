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
mod model_id_macro {
    use crate::conversation::model::ConversationId;
    use crate::message::model::MessageId;
    use crate::server::model::ServerId;
    use crate::user::model::UserId;

    #[test]
    fn model_id_new_creates_unique_ids() {
        let a = UserId::new();
        let b = UserId::new();
        assert_ne!(a, b);
    }

    #[test]
    fn model_id_default_creates_new_id() {
        let a = UserId::default();
        let b = UserId::default();
        assert_ne!(a, b);
    }

    #[test]
    fn model_id_roundtrip_through_uuid() {
        let original = ServerId::new();
        let uuid: uuid::Uuid = original.into();
        let restored = ServerId::from(uuid);
        assert_eq!(original, restored);
    }

    #[test]
    fn model_id_display_matches_uuid() {
        let id = MessageId::new();
        let uuid: uuid::Uuid = id.into();
        assert_eq!(id.to_string(), uuid.to_string());
    }

    #[test]
    fn model_id_is_copy() {
        let a = ConversationId::new();
        let b = a; // Copy
        assert_eq!(a, b);
    }

    #[test]
    fn model_id_hash_consistent() {
        use std::collections::HashSet;
        let id = UserId::new();
        let mut set = HashSet::new();
        set.insert(id);
        assert!(set.contains(&id));
    }

    #[test]
    fn model_id_debug_format() {
        let id = ServerId::new();
        let debug = format!("{:?}", id);
        assert!(debug.starts_with("ServerId("));
    }
}

#[cfg(test)]
mod principal {
    use crate::principal::model::{Principal, PrincipalId, PrincipalKind};
    use crate::user::model::UserId;

    #[test]
    fn user_principal_has_user_id() {
        let uid = UserId::new();
        let p = Principal::user(uid);
        assert_eq!(p.kind, PrincipalKind::User);
        assert_eq!(p.user_id, Some(uid));
    }

    #[test]
    fn bot_principal_has_no_user_id() {
        let p = Principal::bot(PrincipalId::new());
        assert_eq!(p.kind, PrincipalKind::Bot);
        assert!(p.user_id.is_none());
    }

    #[test]
    fn app_principal_has_no_user_id() {
        let p = Principal::app(PrincipalId::new());
        assert_eq!(p.kind, PrincipalKind::App);
        assert!(p.user_id.is_none());
    }

    #[test]
    fn webhook_principal_has_no_user_id() {
        let p = Principal::webhook(PrincipalId::new());
        assert_eq!(p.kind, PrincipalKind::Webhook);
        assert!(p.user_id.is_none());
    }

    #[test]
    fn user_principal_id_matches_user_id() {
        let uid = UserId::new();
        let p = Principal::user(uid);
        let principal_uuid: uuid::Uuid = p.id.into();
        let user_uuid: uuid::Uuid = uid.into();
        assert_eq!(principal_uuid, user_uuid);
    }
}

#[cfg(test)]
mod repo_error {
    use crate::repository::error::{RepoError, RepoErrorKind};
    use std::error::Error;

    #[test]
    fn repo_error_display_with_message() {
        let err = RepoError::with_message(RepoErrorKind::NotFound, "user not found");
        assert_eq!(format!("{err}"), "NotFound: user not found");
    }

    #[test]
    fn repo_error_display_without_message() {
        let err = RepoError::new(RepoErrorKind::Timeout);
        assert_eq!(format!("{err}"), "Timeout");
    }

    #[test]
    fn repo_error_kind_returns_kind() {
        let err = RepoError::new(RepoErrorKind::Conflict);
        assert_eq!(err.kind(), RepoErrorKind::Conflict);
    }

    #[test]
    fn repo_error_message_returns_message() {
        let err = RepoError::with_message(RepoErrorKind::Internal, "oops");
        assert_eq!(err.message(), Some("oops"));
    }

    #[test]
    fn repo_error_message_returns_none_when_absent() {
        let err = RepoError::new(RepoErrorKind::Internal);
        assert_eq!(err.message(), None);
    }

    #[test]
    fn repo_error_with_source_has_source() {
        let source = std::io::Error::other("disk full");
        let err = RepoError::with_source(RepoErrorKind::Internal, source);
        assert!(err.source().is_some());
    }

    #[test]
    fn repo_error_with_message_and_source() {
        let source = std::io::Error::other("io");
        let err = RepoError::with_message_and_source(RepoErrorKind::Unavailable, "db down", source);
        assert_eq!(err.kind(), RepoErrorKind::Unavailable);
        assert_eq!(err.message(), Some("db down"));
        assert!(err.source().is_some());
    }
}

#[cfg(test)]
mod page_extras {
    use crate::repository::pagination::Page;

    #[test]
    fn page_empty_items() {
        let page: Page<i32, &str> = Page::new(vec![], None, None);
        assert!(page.items.is_empty());
        assert!(page.next.is_none());
        assert!(page.prev.is_none());
    }

    #[test]
    fn page_with_cursors() {
        let page = Page::new(vec![1, 2], Some("cursor_next"), Some("cursor_prev"));
        assert_eq!(page.items.len(), 2);
        assert_eq!(page.next, Some("cursor_next"));
        assert_eq!(page.prev, Some("cursor_prev"));
    }
}
