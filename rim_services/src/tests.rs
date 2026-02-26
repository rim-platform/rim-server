#[cfg(test)]
mod map_repo_error {
    use crate::{map_repo_error, ServiceError};
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
