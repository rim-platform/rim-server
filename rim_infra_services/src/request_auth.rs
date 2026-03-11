use async_trait::async_trait;

use rim_auth::session_token::hash_token;
use rim_services::ServiceResult;
use rim_services::auth::dto::RequestAuthResultDto;
use rim_services::auth::service::RequestAuthenticator;

use crate::RequestAuthRepos;

#[derive(Clone)]
pub struct DefaultRequestAuthenticator {
    repos: RequestAuthRepos,
}

impl DefaultRequestAuthenticator {
    pub fn new(repos: RequestAuthRepos) -> Self {
        Self { repos }
    }
}

#[async_trait]
impl RequestAuthenticator for DefaultRequestAuthenticator {
    async fn authenticate_session_token(
        &self,
        session_token: Option<&str>,
    ) -> ServiceResult<RequestAuthResultDto> {
        let Some(session_token) = session_token else {
            return Ok(RequestAuthResultDto::default());
        };

        let token_hash = hash_token(session_token);
        let session = self
            .repos
            .session_repo
            .get_by_token_hash(&token_hash)
            .await
            .map_err(rim_services::map_repo_error)?;

        let Some(session) = session else {
            return Ok(RequestAuthResultDto::default());
        };

        if session.revoked_at.is_some() || session.expires_at <= chrono::Utc::now() {
            return Ok(RequestAuthResultDto::default());
        }

        Ok(RequestAuthResultDto {
            user_id: Some(session.user_id),
            session_id: Some(session.id),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use rim_core::auth::model::{NewSession, Session, SessionId};
    use rim_core::auth::repository::SessionRepository;
    use rim_core::repository::error::{RepoError, RepoErrorKind, RepoResult};
    use rim_core::user::model::UserId;
    use rim_services::ServiceError;
    use std::sync::Arc;
    use std::sync::Mutex;

    struct FakeSessionRepository {
        result: Mutex<Option<RepoResult<Option<Session>>>>,
    }

    impl FakeSessionRepository {
        fn new(result: RepoResult<Option<Session>>) -> Self {
            Self {
                result: Mutex::new(Some(result)),
            }
        }
    }

    #[async_trait]
    impl SessionRepository for FakeSessionRepository {
        async fn get_by_id(&self, _id: SessionId) -> RepoResult<Option<Session>> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn get_by_token_hash(&self, _token_hash: &str) -> RepoResult<Option<Session>> {
            self.result
                .lock()
                .unwrap()
                .take()
                .expect("fake session result not configured")
        }

        async fn create_session(&self, _session: NewSession) -> RepoResult<Session> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn touch_last_seen_at(&self, _id: SessionId) -> RepoResult<()> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn revoke_session(&self, _id: SessionId) -> RepoResult<()> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn revoke_all_for_user(&self, _user_id: UserId) -> RepoResult<u64> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }
    }

    fn active_session() -> Session {
        Session {
            id: SessionId::new(),
            user_id: UserId::new(),
            token_hash: hash_token("session-token"),
            expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
            revoked_at: None,
            created_at: chrono::Utc::now(),
            last_seen_at: Some(chrono::Utc::now()),
            user_agent: None,
            ip: None,
        }
    }

    #[tokio::test]
    async fn authenticate_session_token_returns_anonymous_for_missing_token() {
        let svc = DefaultRequestAuthenticator::new(RequestAuthRepos {
            session_repo: Arc::new(FakeSessionRepository::new(Ok(None))),
        });

        let result = svc.authenticate_session_token(None).await.unwrap();

        assert_eq!(result.user_id, None);
        assert_eq!(result.session_id, None);
    }

    #[tokio::test]
    async fn authenticate_session_token_maps_repo_error() {
        let repo = FakeSessionRepository::new(Err(RepoError::with_message(
            RepoErrorKind::Unavailable,
            "db unavailable",
        )));
        let svc = DefaultRequestAuthenticator::new(RequestAuthRepos {
            session_repo: Arc::new(repo),
        });

        let err = svc
            .authenticate_session_token(Some("session-token"))
            .await
            .unwrap_err();

        assert!(matches!(err, ServiceError::Unavailable));
    }

    #[tokio::test]
    async fn authenticate_session_token_returns_actor_for_active_session() {
        let session = active_session();
        let expected_user_id = session.user_id;
        let expected_session_id = session.id;

        let repo = FakeSessionRepository::new(Ok(Some(session)));
        let svc = DefaultRequestAuthenticator::new(RequestAuthRepos {
            session_repo: Arc::new(repo),
        });

        let result = svc
            .authenticate_session_token(Some("session-token"))
            .await
            .unwrap();

        assert_eq!(result.user_id, Some(expected_user_id));
        assert_eq!(result.session_id, Some(expected_session_id));
    }

    #[tokio::test]
    async fn authenticate_session_token_returns_anonymous_for_expired_session() {
        let mut session = active_session();
        session.expires_at = chrono::Utc::now() - chrono::Duration::minutes(1);

        let repo = FakeSessionRepository::new(Ok(Some(session)));
        let svc = DefaultRequestAuthenticator::new(RequestAuthRepos {
            session_repo: Arc::new(repo),
        });

        let result = svc
            .authenticate_session_token(Some("session-token"))
            .await
            .unwrap();

        assert_eq!(result.user_id, None);
        assert_eq!(result.session_id, None);
    }
}
