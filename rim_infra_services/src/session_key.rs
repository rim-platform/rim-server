//! What a session keeps for its client: the choice made at sign-in ("Keep me signed in"), the
//! keep key of a kept web session, the secret of an unkept session, and the reload key with the
//! window in which a reloaded tab of an unkept session may take it.

use async_trait::async_trait;
use chrono::Duration;
use zeroize::Zeroizing;

use rim_auth::session_token::{generate_session_secret, hash_token};
use rim_core::auth::model::{Session, SessionId, SessionKeep};
use rim_core::auth::repository::SessionRepository;
use rim_services::auth::dto::SessionKeysDto;
use rim_services::key_auth::command::SessionKeepChoice;
use rim_services::session_key::dto::ReloadKeysDto;
use rim_services::session_key::service::SessionKeyService;
use rim_services::{ServiceError, ServiceResult};

use crate::RequestAuthRepos;
use crate::clock::Clock;
use crate::password_key::PasswordKey;

/// Length of a keep key and of a reload key.
const KEY_LEN: usize = 32;

/// How long after its WebSocket closes an unkept session may take its reload key.
pub(crate) const RELOAD_WINDOW: Duration = Duration::seconds(60);

/// `KEY_LEN` bytes from the OS random generator.
fn random_key() -> ServiceResult<Zeroizing<[u8; KEY_LEN]>> {
    let mut key = Zeroizing::new([0u8; KEY_LEN]);
    getrandom::fill(key.as_mut())
        .map_err(|e| ServiceError::internal(anyhow::anyhow!("entropy failure: {e}")))?;
    Ok(key)
}

fn password_key(key: Option<&PasswordKey>) -> ServiceResult<&PasswordKey> {
    key.ok_or_else(|| ServiceError::internal(anyhow::anyhow!("the password key is not loaded")))
}

/// Record what the new session `session_id` keeps under `choice`, through `sessions` (inside
/// the sign-in's transaction), and return what the client is handed:
///
/// - kept, with the client holding its own keep key: nothing;
/// - kept, for a web client: a new keep key, stored sealed under `key`;
/// - unkept: a new session secret, of which only the SHA-256 hex is stored.
///
/// # Errors
/// `Internal` when a keep key is needed and `key` is `None`, or entropy or sealing fails;
/// storage errors propagate.
pub(crate) async fn keep_new_session(
    sessions: &dyn SessionRepository,
    key: Option<&PasswordKey>,
    session_id: SessionId,
    choice: SessionKeepChoice,
) -> ServiceResult<SessionKeysDto> {
    if !choice.keep_signed_in {
        let secret = generate_session_secret();
        sessions
            .set_keep(
                session_id,
                SessionKeep::Unkept {
                    secret_hash: hash_token(&secret),
                },
            )
            .await?;
        return Ok(SessionKeysDto {
            kept: false,
            session_secret: Some(secret),
            keep_key: None,
        });
    }
    if choice.keep_key_held {
        sessions
            .set_keep(
                session_id,
                SessionKeep::Kept {
                    keep_key_sealed: None,
                },
            )
            .await?;
        return Ok(SessionKeysDto::kept());
    }
    let keep_key = random_key()?;
    let sealed = password_key(key)?
        .seal_keep(session_id, keep_key.as_slice())
        .map_err(ServiceError::internal)?;
    sessions
        .set_keep(
            session_id,
            SessionKeep::Kept {
                keep_key_sealed: Some(sealed),
            },
        )
        .await?;
    Ok(SessionKeysDto {
        kept: true,
        session_secret: None,
        keep_key: Some(keep_key.to_vec()),
    })
}

/// [`SessionKeyService`] over the session repository, sealing keys under the password key.
#[derive(Clone)]
pub struct DefaultSessionKeyService {
    repos: RequestAuthRepos,
    password_key: Option<PasswordKey>,
    clock: Clock,
}

impl DefaultSessionKeyService {
    /// The service over `repos`, sealing and opening keys with `password_key`.
    pub fn new(repos: RequestAuthRepos, password_key: Option<PasswordKey>) -> Self {
        Self {
            repos,
            password_key,
            clock: Clock::default(),
        }
    }

    /// The service reading the time from `clock`.
    #[cfg(test)]
    fn with_clock(mut self, clock: Clock) -> Self {
        self.clock = clock;
        self
    }

    /// The live session `session_id`.
    ///
    /// # Errors
    /// `Unauthenticated` when it is unknown, revoked or expired.
    async fn live(&self, session_id: SessionId) -> ServiceResult<Session> {
        self.repos
            .session_repo
            .get_by_id(session_id)
            .await?
            .filter(|s| s.is_live(self.clock.now()))
            .ok_or(ServiceError::Unauthenticated)
    }

    /// A new reload key for `session_id`, and the same key sealed.
    fn new_reload_key(
        &self,
        session_id: SessionId,
    ) -> ServiceResult<(Zeroizing<[u8; KEY_LEN]>, Vec<u8>)> {
        let reload_key = random_key()?;
        let sealed = password_key(self.password_key.as_ref())?
            .seal_reload(session_id, reload_key.as_slice())
            .map_err(ServiceError::internal)?;
        Ok((reload_key, sealed))
    }
}

#[async_trait]
impl SessionKeyService for DefaultSessionKeyService {
    async fn keep_key(&self, session_id: SessionId) -> ServiceResult<Option<Vec<u8>>> {
        let session = self.live(session_id).await?;
        let Some(sealed) = session.keep_key_sealed.filter(|_| session.kept) else {
            return Ok(None);
        };
        let keep_key = password_key(self.password_key.as_ref())?
            .open_keep(session_id, &sealed)
            .ok_or_else(|| {
                ServiceError::internal(anyhow::anyhow!("a stored keep key does not open"))
            })?;
        Ok(Some(keep_key.to_vec()))
    }

    async fn set_reload_key(&self, session_id: SessionId) -> ServiceResult<Vec<u8>> {
        let session = self.live(session_id).await?;
        if session.kept {
            return Err(ServiceError::Unauthenticated);
        }
        let (reload_key, sealed) = self.new_reload_key(session_id)?;
        if !self
            .repos
            .session_repo
            .set_reload_key(session_id, sealed)
            .await?
        {
            return Err(ServiceError::Unauthenticated);
        }
        Ok(reload_key.to_vec())
    }

    async fn reload(&self, session_id: SessionId) -> ServiceResult<ReloadKeysDto> {
        let (next_reload_key, next_sealed) = self.new_reload_key(session_id)?;
        let now = self.clock.now();
        let Some(sealed) = self
            .repos
            .session_repo
            .take_reload_key(session_id, next_sealed, now, now - RELOAD_WINDOW)
            .await?
        else {
            tracing::info!(
                target: "audit",
                event = "session_reload_refused",
                session_id = %session_id,
                "reload refused: the session is kept or ended, or its reload window is closed"
            );
            return Err(ServiceError::Unauthenticated);
        };
        let reload_key = password_key(self.password_key.as_ref())?
            .open_reload(session_id, &sealed)
            .ok_or_else(|| {
                ServiceError::internal(anyhow::anyhow!("a stored reload key does not open"))
            })?;
        Ok(ReloadKeysDto {
            reload_key: reload_key.to_vec(),
            next_reload_key: next_reload_key.to_vec(),
        })
    }

    async fn socket_opened(&self, session_id: SessionId) -> ServiceResult<()> {
        Ok(self.repos.session_repo.socket_opened(session_id).await?)
    }

    async fn socket_closed(&self, session_id: SessionId) -> ServiceResult<()> {
        Ok(self
            .repos
            .session_repo
            .socket_closed(session_id, self.clock.now())
            .await?)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use chrono::{DateTime, Utc};
    use rim_core::auth::repository::MockSessionRepository;
    use rim_core::user::model::UserId;

    use super::*;

    fn session(id: SessionId, kept: bool) -> Session {
        let now = Utc::now();
        Session {
            id,
            user_id: UserId::new(),
            token_hash: "hash".to_string(),
            created_at: now,
            expires_at: now + Duration::hours(1),
            last_seen_at: None,
            revoked_at: None,
            user_agent: None,
            ip: None,
            device_key_id: None,
            must_set_password: false,
            kept,
            secret_hash: None,
            keep_key_sealed: None,
            reload_key_sealed: None,
            ws_closed_at: None,
        }
    }

    /// A repository that records what [`keep_new_session`] stores.
    fn recording(stored: Arc<Mutex<Option<SessionKeep>>>) -> MockSessionRepository {
        let mut repo = MockSessionRepository::new();
        repo.expect_set_keep().times(1).returning(move |_, keep| {
            *stored.lock().unwrap() = Some(keep);
            Ok(())
        });
        repo
    }

    #[tokio::test]
    async fn an_unkept_session_gets_a_secret_of_which_only_the_hash_is_stored() {
        let stored = Arc::new(Mutex::new(None));
        let repo = recording(Arc::clone(&stored));
        let keys = keep_new_session(
            &repo,
            None,
            SessionId::new(),
            SessionKeepChoice {
                keep_signed_in: false,
                keep_key_held: true,
            },
        )
        .await
        .unwrap();
        assert!(!keys.kept);
        assert!(keys.keep_key.is_none());
        let secret = keys.session_secret.expect("a secret");
        assert_eq!(
            stored.lock().unwrap().take(),
            Some(SessionKeep::Unkept {
                secret_hash: hash_token(&secret)
            })
        );
    }

    #[tokio::test]
    async fn a_kept_web_session_gets_a_keep_key_stored_sealed_for_it_and_a_desktop_one_none() {
        let key = PasswordKey::generate().unwrap();
        let id = SessionId::new();
        let stored = Arc::new(Mutex::new(None));
        let repo = recording(Arc::clone(&stored));
        let web = SessionKeepChoice {
            keep_signed_in: true,
            keep_key_held: false,
        };
        let keys = keep_new_session(&repo, Some(&key), id, web).await.unwrap();
        assert!(keys.kept);
        assert!(keys.session_secret.is_none());
        let keep_key = keys.keep_key.expect("a keep key");
        assert_eq!(keep_key.len(), KEY_LEN);
        let Some(SessionKeep::Kept {
            keep_key_sealed: Some(sealed),
        }) = stored.lock().unwrap().take()
        else {
            panic!("the keep key is stored sealed");
        };
        assert_eq!(
            key.open_keep(id, &sealed).as_deref().map(Vec::as_slice),
            Some(keep_key.as_slice())
        );

        let repo = recording(Arc::clone(&stored));
        let desktop = SessionKeepChoice {
            keep_signed_in: true,
            keep_key_held: true,
        };
        let keys = keep_new_session(&repo, None, id, desktop).await.unwrap();
        assert_eq!(keys, SessionKeysDto::kept());
        assert_eq!(
            stored.lock().unwrap().take(),
            Some(SessionKeep::Kept {
                keep_key_sealed: None
            })
        );
    }

    #[tokio::test]
    async fn a_web_keep_key_without_the_password_key_is_an_internal_error() {
        let mut repo = MockSessionRepository::new();
        repo.expect_set_keep().never();
        let err = keep_new_session(
            &repo,
            None,
            SessionId::new(),
            SessionKeepChoice {
                keep_signed_in: true,
                keep_key_held: false,
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(err, ServiceError::Internal(_)), "{err:?}");
    }

    fn service(repo: MockSessionRepository, key: &PasswordKey) -> DefaultSessionKeyService {
        DefaultSessionKeyService::new(
            RequestAuthRepos {
                session_repo: Arc::new(repo),
            },
            Some(key.clone()),
        )
    }

    #[tokio::test]
    async fn the_keep_key_opens_only_for_a_live_kept_session() {
        let key = PasswordKey::generate().unwrap();
        let id = SessionId::new();
        let mut kept = session(id, true);
        kept.keep_key_sealed = Some(key.seal_keep(id, &[7; KEY_LEN]).unwrap());
        let mut unkept = kept.clone();
        unkept.kept = false;
        let mut desktop = kept.clone();
        desktop.keep_key_sealed = None;
        let mut revoked = kept.clone();
        revoked.revoked_at = Some(Utc::now());
        for (stored, expected) in [
            (kept, Ok(Some(vec![7; KEY_LEN]))),
            (unkept, Ok(None)),
            (desktop, Ok(None)),
            (revoked, Err(())),
        ] {
            let mut repo = MockSessionRepository::new();
            repo.expect_get_by_id()
                .return_once(move |_| Ok(Some(stored)));
            let answer = service(repo, &key).keep_key(id).await;
            match expected {
                Ok(expected) => assert_eq!(answer.unwrap(), expected),
                Err(()) => assert!(matches!(answer, Err(ServiceError::Unauthenticated))),
            }
        }
    }

    #[tokio::test]
    async fn set_reload_key_stores_the_returned_key_sealed_and_refuses_a_kept_session() {
        let key = PasswordKey::generate().unwrap();
        let id = SessionId::new();
        let stored = Arc::new(Mutex::new(None));
        let mut repo = MockSessionRepository::new();
        repo.expect_get_by_id()
            .return_once(move |_| Ok(Some(session(id, false))));
        let into = Arc::clone(&stored);
        repo.expect_set_reload_key().return_once(move |_, sealed| {
            *into.lock().unwrap() = Some(sealed);
            Ok(true)
        });
        let reload_key = service(repo, &key).set_reload_key(id).await.unwrap();
        let sealed = stored.lock().unwrap().take().expect("stored");
        assert_eq!(
            key.open_reload(id, &sealed).as_deref().map(Vec::as_slice),
            Some(reload_key.as_slice())
        );

        let mut repo = MockSessionRepository::new();
        repo.expect_get_by_id()
            .return_once(move |_| Ok(Some(session(id, true))));
        repo.expect_set_reload_key().never();
        let err = service(repo, &key).set_reload_key(id).await.unwrap_err();
        assert!(matches!(err, ServiceError::Unauthenticated), "{err:?}");
    }

    #[tokio::test]
    async fn a_reload_hands_out_the_stored_key_and_stores_the_next_within_the_window() {
        let key = PasswordKey::generate().unwrap();
        let id = SessionId::new();
        let now = Utc::now();
        let current = key.seal_reload(id, &[3; KEY_LEN]).unwrap();
        let next_stored = Arc::new(Mutex::new(None));
        let mut repo = MockSessionRepository::new();
        let into = Arc::clone(&next_stored);
        repo.expect_take_reload_key()
            .withf(move |session, _, at, since| {
                *session == id && *at == now && *since == now - RELOAD_WINDOW
            })
            .return_once(move |_, next, _, _| {
                *into.lock().unwrap() = Some(next);
                Ok(Some(current))
            });
        let keys = service(repo, &key)
            .with_clock(Clock::fixed(now))
            .reload(id)
            .await
            .unwrap();
        assert_eq!(keys.reload_key, vec![3; KEY_LEN]);
        let next = next_stored.lock().unwrap().take().expect("next stored");
        assert_eq!(
            key.open_reload(id, &next).as_deref().map(Vec::as_slice),
            Some(keys.next_reload_key.as_slice())
        );

        let mut repo = MockSessionRepository::new();
        repo.expect_take_reload_key()
            .return_once(|_, _, _, _| Ok(None));
        let err = service(repo, &key).reload(id).await.unwrap_err();
        assert!(matches!(err, ServiceError::Unauthenticated), "{err:?}");
    }

    #[tokio::test]
    async fn a_closing_is_recorded_at_the_clocks_now() {
        let key = PasswordKey::generate().unwrap();
        let id = SessionId::new();
        let at: DateTime<Utc> = Utc::now() - Duration::seconds(5);
        let mut repo = MockSessionRepository::new();
        repo.expect_socket_opened()
            .withf(move |session| *session == id)
            .times(1)
            .returning(|_| Ok(()));
        repo.expect_socket_closed()
            .withf(move |session, closed| *session == id && *closed == at)
            .times(1)
            .returning(|_, _| Ok(()));
        let svc = service(repo, &key).with_clock(Clock::fixed(at));
        svc.socket_opened(id).await.unwrap();
        svc.socket_closed(id).await.unwrap();
    }
}
