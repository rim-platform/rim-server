use async_trait::async_trait;

use rim_auth::session_token::{hash_token, session_secret_matches};
use rim_core::auth::model::{Session, SessionId};
use rim_services::ServiceResult;
use rim_services::auth::dto::{RequestAuthResultDto, SessionSecret};
use rim_services::auth::service::RequestAuthenticator;

use crate::RequestAuthRepos;
use crate::clock::Clock;

/// Whether `secret` admits a request of `session`: always for a kept session; for an unkept one
/// only when the request needs no secret or presents the one whose hash the session holds.
fn secret_admits(session: &Session, secret: &SessionSecret) -> bool {
    if session.kept {
        return true;
    }
    let Some(secret_hash) = session.secret_hash.as_deref() else {
        return false;
    };
    match secret {
        SessionSecret::NotRequired => true,
        SessionSecret::Presented(Some(presented)) => session_secret_matches(presented, secret_hash),
        SessionSecret::Presented(None) => false,
    }
}

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
        secret: SessionSecret,
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
        if !secret_admits(&session, &secret) {
            tracing::debug!(session_id = %session.id, "unkept session without its secret");
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
            secret_hash: if session.kept {
                None
            } else {
                session.secret_hash
            },
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
            async fn set_keep(&self, id: SessionId, keep: rim_core::auth::model::SessionKeep) -> RepoResult<()>;
            async fn set_reload_key(&self, id: SessionId, reload_key_sealed: Vec<u8>) -> RepoResult<bool>;
            async fn take_reload_key(&self, id: SessionId, next_sealed: Vec<u8>, now: chrono::DateTime<chrono::Utc>, closed_since: chrono::DateTime<chrono::Utc>) -> RepoResult<Option<Vec<u8>>>;
            async fn socket_opened(&self, id: SessionId) -> RepoResult<()>;
            async fn socket_closed(&self, id: SessionId, at: chrono::DateTime<chrono::Utc>) -> RepoResult<()>;
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
            kept: true,
            secret_hash: None,
            keep_key_sealed: None,
            reload_key_sealed: None,
            ws_closed_at: None,
        }
    }

    #[tokio::test]
    async fn authenticate_session_token_returns_anonymous_for_missing_token() {
        let mut repo = MockSessionRepository::new();
        repo.expect_get_by_token_hash().never();
        let svc = service(repo);

        let result = svc
            .authenticate_session_token(None, SessionSecret::Presented(None))
            .await
            .unwrap();

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
            .authenticate_session_token(Some("session-token"), SessionSecret::Presented(None))
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
            .authenticate_session_token(Some("session-token"), SessionSecret::Presented(None))
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
            .authenticate_session_token(Some("session-token"), SessionSecret::Presented(None))
            .await
            .unwrap();

        assert_eq!(result.user_id, None);
        assert_eq!(result.session_id, None);
    }

    /// An unkept session admits a request presenting its secret, or needing none; a missing or
    /// wrong secret leaves the request anonymous. A kept session needs no secret.
    #[tokio::test]
    async fn an_unkept_session_requires_its_secret_unless_none_is_required() {
        let secret = rim_auth::session_token::generate_session_secret();
        let mut unkept = active_session();
        unkept.kept = false;
        unkept.secret_hash = Some(hash_token(&secret));
        let cases = [
            (
                unkept.clone(),
                SessionSecret::Presented(Some(secret.clone())),
                true,
            ),
            (
                unkept.clone(),
                SessionSecret::Presented(Some("wrong".to_string())),
                false,
            ),
            (unkept.clone(), SessionSecret::Presented(None), false),
            (unkept.clone(), SessionSecret::NotRequired, true),
            (active_session(), SessionSecret::Presented(None), true),
        ];
        for (session, presented, admitted) in cases {
            let expected_hash = (!session.kept).then(|| hash_token(&secret));
            let mut repo = MockSessionRepository::new();
            repo.expect_get_by_token_hash()
                .return_once(move |_| Ok(Some(session)));
            repo.expect_touch_last_seen_at().returning(|_| Ok(()));
            let result = service(repo)
                .authenticate_session_token(Some("session-token"), presented.clone())
                .await
                .unwrap();
            assert_eq!(result.user_id.is_some(), admitted, "{presented:?}");
            if admitted {
                assert_eq!(result.secret_hash, expected_hash);
            }
        }

        let mut no_hash = active_session();
        no_hash.kept = false;
        let mut repo = MockSessionRepository::new();
        repo.expect_get_by_token_hash()
            .return_once(move |_| Ok(Some(no_hash)));
        let result = service(repo)
            .authenticate_session_token(Some("session-token"), SessionSecret::NotRequired)
            .await
            .unwrap();
        assert_eq!(
            result.user_id, None,
            "an unkept session without a hash admits nothing"
        );
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
