#[cfg(test)]
mod map_repo_error {
    use crate::{ServiceError, map_repo_error};
    use rim_core::repository::error::{RepoError, RepoErrorKind};

    #[test]
    fn maps_not_found() {
        let err = RepoError::new(RepoErrorKind::NotFound);
        assert!(matches!(map_repo_error(err), ServiceError::NotFound));
    }

    #[test]
    fn maps_conflict_to_conflict_with_message() {
        let err = RepoError::with_message(RepoErrorKind::Conflict, "duplicate");
        assert!(matches!(
            map_repo_error(err),
            ServiceError::Conflict(m) if m == "duplicate"
        ));
    }

    #[test]
    fn maps_timeout() {
        let err = RepoError::new(RepoErrorKind::Timeout);
        assert!(matches!(map_repo_error(err), ServiceError::Timeout));
    }

    #[test]
    fn maps_unavailable() {
        let err = RepoError::new(RepoErrorKind::Unavailable);
        assert!(matches!(map_repo_error(err), ServiceError::Unavailable));
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
            assert!(matches!(map_repo_error(err), ServiceError::Internal(_)));
        }
    }

    #[test]
    fn maps_precondition_failed_to_conflict() {
        let err = RepoError::new(RepoErrorKind::PreconditionFailed);
        assert!(matches!(
            map_repo_error(err),
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
