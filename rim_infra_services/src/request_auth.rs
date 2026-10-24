use async_trait::async_trait;

use rim_auth::session_token::hash_token;
use rim_core::auth::model::SessionId;
use rim_services::ServiceResult;
use rim_services::auth::dto::RequestAuthResultDto;
use rim_services::auth::service::RequestAuthenticator;

use crate::RequestAuthRepos;
use crate::clock::Clock;

#[derive(Clone)]
pub struct DefaultRequestAuthenticator {
    repos: RequestAuthRepos,
    clock: Clock,
}

impl DefaultRequestAuthenticator {
    pub fn new(repos: RequestAuthRepos) -> Self {
        Self {
            repos,
            clock: Clock::default(),
        }
    }

    /// The authenticator reading the time from `clock`.
    #[cfg(test)]
    fn with_clock(mut self, clock: Clock) -> Self {
        self.clock = clock;
        self
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
            .await?;

        let Some(session) = session else {
            return Ok(RequestAuthResultDto::default());
        };

        if !session.is_live(self.clock.now()) {
            return Ok(RequestAuthResultDto::default());
        }

        // Best-effort: update last_seen_at for activity tracking.
        if let Err(error) = self.repos.session_repo.touch_last_seen_at(session.id).await {
            tracing::warn!(%error, "could not record authenticated activity");
        }

        Ok(RequestAuthResultDto {
            user_id: Some(session.user_id),
            session_id: Some(session.id),
            device_key_id: session.device_key_id,
            expires_at: Some(session.expires_at),
            must_set_password: session.must_set_password,
        })
    }

    async fn live_sessions(&self, ids: &[SessionId]) -> ServiceResult<Vec<SessionId>> {
        let now = self.clock.now();
        let sessions = self.repos.session_repo.get_many(ids).await?;
        Ok(sessions
            .into_iter()
            .filter(|s| s.is_live(now))
            .map(|s| s.id)
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use rim_core::auth::model::{NewSession, Session, SessionId};
    use rim_core::auth::repository::SessionRepository;
    use rim_core::key_identity::model::DeviceKeyId;
    use rim_core::repository::error::{RepoError, RepoErrorKind, RepoResult};
    use rim_core::user::model::UserId;
    use rim_services::ServiceError;
    use std::sync::Arc;

    mockall::mock! {
        SessionRepository {}
        #[async_trait]
        impl SessionRepository for SessionRepository {
            async fn get_by_id(&self, id: SessionId) -> RepoResult<Option<Session>>;
            async fn get_by_token_hash(&self, token_hash: &str) -> RepoResult<Option<Session>>;
            async fn get_many(&self, ids: &[SessionId]) -> RepoResult<Vec<Session>>;
            async fn create_session(&self, session: NewSession) -> RepoResult<Session>;
            async fn touch_last_seen_at(&self, id: SessionId) -> RepoResult<()>;
            async fn revoke_session(&self, id: SessionId) -> RepoResult<()>;
            async fn revoke_all_for_user(&self, user_id: UserId) -> RepoResult<u64>;
            async fn revoke_all_for_user_except(&self, user_id: UserId, keep: SessionId) -> RepoResult<u64>;
            async fn bind_device(&self, id: SessionId, device_key_id: DeviceKeyId) -> RepoResult<()>;
            async fn revoke_all_for_device(&self, device_key_id: DeviceKeyId) -> RepoResult<u64>;
            async fn require_password(&self, id: SessionId) -> RepoResult<()>;
            async fn clear_password_requirement(&self, user_id: UserId) -> RepoResult<u64>;
            async fn delete_ended(&self, now: chrono::DateTime<chrono::Utc>) -> RepoResult<u64>;
        }
    }

    fn service(session_repo: MockSessionRepository) -> DefaultRequestAuthenticator {
        DefaultRequestAuthenticator::new(RequestAuthRepos {
            session_repo: Arc::new(session_repo),
        })
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
            device_key_id: None,
            must_set_password: false,
        }
    }

    #[tokio::test]
    async fn authenticate_session_token_returns_anonymous_for_missing_token() {
        let mut repo = MockSessionRepository::new();
        repo.expect_get_by_token_hash().never();
        let svc = service(repo);

        let result = svc.authenticate_session_token(None).await.unwrap();

        assert_eq!(result.user_id, None);
        assert_eq!(result.session_id, None);
    }

    #[tokio::test]
    async fn authenticate_session_token_maps_repo_error() {
        let mut repo = MockSessionRepository::new();
        repo.expect_get_by_token_hash().return_once(|token_hash| {
            assert_eq!(token_hash, hash_token("session-token"));
            Err(RepoError::with_message(
                RepoErrorKind::Unavailable,
                "db unavailable",
            ))
        });
        let svc = service(repo);

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

        let mut repo = MockSessionRepository::new();
        repo.expect_get_by_token_hash().return_once(|token_hash| {
            assert_eq!(token_hash, hash_token("session-token"));
            Ok(Some(session))
        });
        repo.expect_touch_last_seen_at().return_once(|_| Ok(()));
        let svc = service(repo);

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

        let mut repo = MockSessionRepository::new();
        repo.expect_get_by_token_hash().return_once(|token_hash| {
            assert_eq!(token_hash, hash_token("session-token"));
            Ok(Some(session))
        });
        let svc = service(repo);

        let result = svc
            .authenticate_session_token(Some("session-token"))
            .await
            .unwrap();

        assert_eq!(result.user_id, None);
        assert_eq!(result.session_id, None);
    }

    /// A session is live until it is revoked or its expiry is reached, read on the injected
    /// clock; a session the repository no longer has is not live.
    #[tokio::test]
    async fn live_sessions_keeps_those_neither_revoked_nor_expired_at_the_clocks_now() {
        let now = chrono::Utc::now();
        let mut live = active_session();
        live.expires_at = now + chrono::Duration::seconds(1);
        let mut expiring_now = active_session();
        expiring_now.expires_at = now;
        let mut revoked = active_session();
        revoked.revoked_at = Some(now - chrono::Duration::minutes(1));
        let asked = vec![live.id, expiring_now.id, revoked.id, SessionId::new()];
        let stored = vec![live.clone(), expiring_now, revoked];

        let mut repo = MockSessionRepository::new();
        let expected = asked.clone();
        repo.expect_get_many()
            .withf(move |ids| ids == expected.as_slice())
            .return_once(move |_| Ok(stored));
        let svc = service(repo).with_clock(Clock::fixed(now));

        assert_eq!(svc.live_sessions(&asked).await.unwrap(), vec![live.id]);
    }
}
