#[cfg(test)]
mod repo_error_from {
    use crate::ServiceError;
    use rim_core::repository::error::{RepoError, RepoErrorKind};

    #[test]
    fn maps_not_found() {
        let err = RepoError::new(RepoErrorKind::NotFound);
        assert!(matches!(ServiceError::from(err), ServiceError::NotFound));
    }

    #[test]
    fn maps_conflict_to_conflict_with_message() {
        let err = RepoError::with_message(RepoErrorKind::Conflict, "duplicate");
        assert!(matches!(
            ServiceError::from(err),
            ServiceError::Conflict(m) if m == "duplicate"
        ));
    }

    #[test]
    fn maps_timeout() {
        let err = RepoError::new(RepoErrorKind::Timeout);
        assert!(matches!(ServiceError::from(err), ServiceError::Timeout));
    }

    #[test]
    fn maps_unavailable() {
        let err = RepoError::new(RepoErrorKind::Unavailable);
        assert!(matches!(ServiceError::from(err), ServiceError::Unavailable));
    }

    #[test]
    fn maps_internalish_to_internal() {
        for kind in [
            RepoErrorKind::InvalidData,
            RepoErrorKind::Internal,
            RepoErrorKind::Unsupported,
            RepoErrorKind::Canceled,
        ] {
            let err = RepoError::new(kind);
            assert!(matches!(ServiceError::from(err), ServiceError::Internal(_)));
        }
    }

    #[test]
    fn maps_conflict_without_message_uses_fallback() {
        let err = RepoError::new(RepoErrorKind::Conflict);
        assert!(matches!(
            ServiceError::from(err),
            ServiceError::Conflict(m) if m == "conflict"
        ));
    }

    #[test]
    fn maps_precondition_failed_to_conflict() {
        let err = RepoError::new(RepoErrorKind::PreconditionFailed);
        assert!(matches!(
            ServiceError::from(err),
            ServiceError::Conflict(m) if m == "precondition failed"
        ));
    }
}

#[cfg(test)]
mod actor {
    use crate::Actor;
    use rim_core::principal::model::{Principal, PrincipalId, PrincipalKind};
    use rim_core::user::model::UserId;

    #[test]
    fn actor_new_creates_user_principal() {
        let user_id = UserId::new();
        let actor = Actor::new(user_id);

        assert_eq!(actor.user_id, user_id);
        assert_eq!(actor.principal.kind, PrincipalKind::User);
        assert_eq!(actor.principal.user_id, Some(user_id));
    }

    #[test]
    fn actor_from_principal_webhook_returns_none() {
        let principal = Principal::webhook(PrincipalId::new());
        assert!(Actor::from_principal(principal).is_none());
    }

    #[test]
    fn actor_from_principal_app_returns_none() {
        let principal = Principal::app(PrincipalId::new());
        assert!(Actor::from_principal(principal).is_none());
    }

    #[test]
    fn actor_is_copy() {
        let actor = Actor::new(UserId::new());
        let copy = actor;
        assert_eq!(actor.user_id, copy.user_id);
    }

    #[test]
    fn actor_from_principal_only_allows_user_principals() {
        let user_id = UserId::new();
        let user_principal = Principal::user(user_id);
        let bot_principal = Principal::bot(PrincipalId::new());

        assert_eq!(
            Actor::from_principal(user_principal).map(|actor| actor.user_id),
            Some(user_id)
        );
        assert!(Actor::from_principal(bot_principal).is_none());
    }
}

#[cfg(test)]
mod service_error_display {
    use crate::ServiceError;

    #[test]
    fn not_found_display() {
        assert_eq!(format!("{}", ServiceError::NotFound), "not found");
    }

    #[test]
    fn validation_display() {
        let err = ServiceError::Validation("bad input".to_string());
        assert_eq!(format!("{err}"), "validation error: bad input");
    }

    #[test]
    fn conflict_display() {
        let err = ServiceError::Conflict("dup".to_string());
        assert_eq!(format!("{err}"), "conflict: dup");
    }

    #[test]
    fn unavailable_display() {
        assert_eq!(
            format!("{}", ServiceError::Unavailable),
            "dependency unavailable"
        );
    }

    #[test]
    fn timeout_display() {
        assert_eq!(format!("{}", ServiceError::Timeout), "dependency timed out");
    }

    #[test]
    fn permission_denied_display() {
        assert_eq!(
            format!("{}", ServiceError::PermissionDenied),
            "permission denied"
        );
    }

    #[test]
    fn unauthenticated_display() {
        assert_eq!(
            format!("{}", ServiceError::Unauthenticated),
            "unauthenticated"
        );
    }

    #[test]
    fn internal_display() {
        let err = ServiceError::Internal(None);
        assert_eq!(format!("{err}"), "internal error");
    }

    #[test]
    fn internal_from_helper() {
        let err = ServiceError::internal(std::io::Error::new(std::io::ErrorKind::Other, "oops"));
        assert!(matches!(err, ServiceError::Internal(Some(_))));
    }
}
