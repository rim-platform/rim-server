//! Device linking: the server is a courier. It stores the new device's sealed offer and the
//! old device's sealed answer, checks only their encoding and size, and never reads them. The state machine lives
//! in the repository's conditional transitions; this module decides who may call what, turns
//! a refused transition into the right error, and rate-limits anonymous callers. It does not
//! depend on key login; it reads only `user.principal_id`.

use std::future::Future;
use std::sync::Arc;

use async_trait::async_trait;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Duration, Utc};
use sea_orm::DatabaseConnection;
use sha2::{Digest, Sha256};
use tracing::{info, instrument, warn};

use rim_auth::session_token::generate_link_id;
use rim_core::device_link::model::{
    DeviceLink, DeviceLinkId, DeviceLinkState, NewDeviceLink, SealedBytes,
};
use rim_core::device_link::repository::DeviceLinkRepository;
use rim_core::user::model::UserId;
use rim_core::user::repository::UserRepository;
use rim_infra_db::{TransactionScope, begin_transaction};
use rim_services::command::rate_limit::RateLimitDecision;
use rim_services::device_link::command::{
    CancelDeviceLinkCmd, CompleteDeviceLinkCmd, GetDeviceLinkAnswerQuery, GetDeviceLinkQuery,
    SealedPayload, SendDeviceLinkAnswerCmd, SendDeviceLinkOfferCmd, StartDeviceLinkCmd,
};
use rim_services::device_link::dto::{DeviceLinkDto, DeviceLinkStateDto, StartedDeviceLinkDto};
use rim_services::device_link::service::DeviceLinkService;
use rim_services::{ServiceError, ServiceResult};

use crate::registration_rate_limiter::RegistrationRateLimiter;
use crate::repos::AppRepos;

/// How long a link lives after `startDeviceLink`, and after `sendDeviceLinkAnswer`.
const LINK_TTL: Duration = Duration::minutes(10);

/// How long an expired link stays readable (as `EXPIRED`, `CONSUMED` or `CANCELLED`) before
/// `startDeviceLink` deletes it.
const CLEANUP_GRACE: Duration = Duration::hours(1);

/// Link id length in bytes, and as unpadded base64url text.
const LINK_ID_LEN: usize = 16;
const LINK_ID_TEXT_LEN: usize = 22;

const NONCE_LEN: usize = 12;
/// The nonce as unpadded base64url text.
const NONCE_TEXT_LEN: usize = 16;
/// One byte of plaintext plus the 16-byte tag.
const MIN_CIPHERTEXT_LEN: usize = 17;
/// 16 KiB of plaintext plus the tag.
pub(crate) const MAX_OFFER_CIPHERTEXT_LEN: usize = 16_400;
/// 64 KiB of plaintext plus the tag.
pub(crate) const MAX_ANSWER_CIPHERTEXT_LEN: usize = 65_552;

/// Limiter keys.
const START_KEY: &str = "device_link_start";
const OFFER_KEY: &str = "device_link_offer";
const ANSWER_KEY: &str = "device_link_answer";
const REFUSAL_KEY: &str = "device_link_refusal";

#[derive(Clone)]
pub struct DeviceLinkRepos {
    pub user_repo: Arc<dyn UserRepository>,
    pub device_link_repo: Arc<dyn DeviceLinkRepository>,
}

pub struct DefaultDeviceLinkService {
    repos: DeviceLinkRepos,
    /// When set, `start` runs in one transaction with the user row locked.
    db: Option<DatabaseConnection>,
    /// Links started per user.
    start_rate_limiter: RegistrationRateLimiter,
    /// Offers and anonymous cancels per IP.
    offer_rate_limiter: RegistrationRateLimiter,
    /// Answer polls per IP.
    answer_rate_limiter: RegistrationRateLimiter,
    /// Anonymous refusals per IP. Separate from key login's, so a botched link does not lock
    /// the same network out of `loginWithKey`.
    refusal_rate_limiter: RegistrationRateLimiter,
}

/// SHA-256 of the link id's bytes, or `None` when the text is not 16 bytes of strict
/// unpadded base64url.
fn link_hash(link_id: &str) -> Option<Vec<u8>> {
    if link_id.len() != LINK_ID_TEXT_LEN {
        return None;
    }
    let bytes = URL_SAFE_NO_PAD.decode(link_id).ok()?;
    (bytes.len() == LINK_ID_LEN).then(|| Sha256::digest(&bytes).to_vec())
}

/// Decode a sealed message, checking only its encoding and size. The engine refuses padding
/// and non-zero trailing bits, so the bytes re-encode to exactly the text received.
fn decode_sealed(field: &str, sealed: &SealedPayload, max: usize) -> ServiceResult<SealedBytes> {
    let nonce = Some(&sealed.nonce)
        .filter(|text| text.len() == NONCE_TEXT_LEN)
        .and_then(|text| URL_SAFE_NO_PAD.decode(text).ok())
        .filter(|n| n.len() == NONCE_LEN)
        .ok_or_else(|| {
            ServiceError::Validation(format!(
                "{field}.nonce must be {NONCE_LEN} bytes of unpadded base64url"
            ))
        })?;
    let ciphertext_error = || {
        ServiceError::Validation(format!(
            "{field}.ciphertext must be {MIN_CIPHERTEXT_LEN} to {max} bytes of unpadded base64url"
        ))
    };
    // Refuse oversized text before decoding it.
    if sealed.ciphertext.len() > max.div_ceil(3) * 4 {
        return Err(ciphertext_error());
    }
    let ciphertext = URL_SAFE_NO_PAD
        .decode(&sealed.ciphertext)
        .ok()
        .filter(|c| (MIN_CIPHERTEXT_LEN..=max).contains(&c.len()))
        .ok_or_else(ciphertext_error)?;
    Ok(SealedBytes { nonce, ciphertext })
}

fn encode_sealed(sealed: &SealedBytes) -> SealedPayload {
    SealedPayload {
        nonce: URL_SAFE_NO_PAD.encode(&sealed.nonce),
        ciphertext: URL_SAFE_NO_PAD.encode(&sealed.ciphertext),
    }
}

fn link_dto(link: &DeviceLink) -> DeviceLinkDto {
    let state = match link.state {
        state if state.is_live() && link.expired => DeviceLinkStateDto::Expired,
        DeviceLinkState::Open => DeviceLinkStateDto::Open,
        DeviceLinkState::Offered => DeviceLinkStateDto::Offered,
        DeviceLinkState::Answered => DeviceLinkStateDto::Answered,
        DeviceLinkState::Consumed => DeviceLinkStateDto::Consumed,
        DeviceLinkState::Cancelled => DeviceLinkStateDto::Cancelled,
    };
    let offer = match state {
        DeviceLinkStateDto::Offered | DeviceLinkStateDto::Answered => {
            link.offer.as_ref().map(encode_sealed)
        }
        _ => None,
    };
    DeviceLinkDto {
        state,
        expires_at: link.expires_at,
        offer,
    }
}

/// Why a transition with a session was refused, for a link that is the caller's.
fn conflict(link: &DeviceLink, wanted: &str) -> ServiceError {
    ServiceError::Conflict(if link.state.is_live() && link.expired {
        "the link has expired".to_string()
    } else {
        format!(
            "the link is {}, not waiting for {wanted}",
            link.state.as_str()
        )
    })
}

/// Log an audit event with the link's row id and user, never its link id or sealed bytes.
fn audit(event: &str, link_row_id: DeviceLinkId, user_id: UserId, message: &str) {
    info!(
        target = "audit",
        event,
        link_row_id = %link_row_id,
        user_id = %user_id,
        "{message}"
    );
}

impl DefaultDeviceLinkService {
    /// The service with the default limits; `db` makes `start` transactional.
    pub fn new(repos: DeviceLinkRepos, db: Option<DatabaseConnection>) -> Self {
        Self {
            repos,
            db,
            start_rate_limiter: RegistrationRateLimiter::new(3600, 10),
            offer_rate_limiter: RegistrationRateLimiter::new(900, 30),
            answer_rate_limiter: RegistrationRateLimiter::new(300, 300),
            refusal_rate_limiter: RegistrationRateLimiter::new(900, 20),
        }
    }

    /// Replace the limiters (starts per user; offers and anonymous cancels, answer polls, and
    /// anonymous refusals per IP).
    #[cfg(test)]
    pub(crate) fn with_rate_limiters(
        mut self,
        start: RegistrationRateLimiter,
        offer: RegistrationRateLimiter,
        answer: RegistrationRateLimiter,
        refusal: RegistrationRateLimiter,
    ) -> Self {
        self.start_rate_limiter = start;
        self.offer_rate_limiter = offer;
        self.answer_rate_limiter = answer;
        self.refusal_rate_limiter = refusal;
        self
    }

    /// Open a transaction when a database is configured, and the repos that run inside it.
    async fn begin(&self) -> ServiceResult<(Option<TransactionScope>, DeviceLinkRepos)> {
        let scope = match &self.db {
            Some(db) => Some(
                begin_transaction(db)
                    .await
                    .map_err(ServiceError::internal)?,
            ),
            None => None,
        };
        let repos = match &scope {
            Some(s) => AppRepos::with_handle(&s.handle()).for_device_link(),
            None => self.repos.clone(),
        };
        Ok((scope, repos))
    }

    async fn commit(scope: Option<TransactionScope>) -> ServiceResult<()> {
        if let Some(scope) = scope {
            scope.commit().await.map_err(ServiceError::internal)?;
        }
        Ok(())
    }

    /// Record one call under `key`; `RateLimited` once over the limit. Returns the instant
    /// recorded, which a refund must name.
    fn limit(
        limiter: &RegistrationRateLimiter,
        bucket: &str,
        key: &str,
    ) -> ServiceResult<DateTime<Utc>> {
        let now = Utc::now();
        match limiter.check_and_record(bucket, key, now) {
            RateLimitDecision::Allowed => Ok(now),
            RateLimitDecision::Limited {
                retry_after_seconds,
            } => Err(ServiceError::RateLimited {
                retry_after_seconds,
            }),
        }
    }

    /// Run an anonymous call. A refusal slot is reserved up front, so concurrent attempts
    /// can't all pass the check before any of them is refused, and refunded unless the call
    /// ends in `Unauthenticated`.
    async fn anonymous<T>(
        &self,
        remote_ip: &str,
        call: impl Future<Output = ServiceResult<T>>,
    ) -> ServiceResult<T> {
        let reserved_at = Self::limit(&self.refusal_rate_limiter, remote_ip, REFUSAL_KEY)?;
        let result = call.await;
        if !matches!(result, Err(ServiceError::Unauthenticated)) {
            self.refusal_rate_limiter
                .refund(remote_ip, REFUSAL_KEY, reserved_at);
        }
        result
    }

    /// The caller's link; `NotFound` for an unknown link and for another user's link.
    async fn find_own(&self, link_hash: &[u8], user_id: UserId) -> ServiceResult<DeviceLink> {
        self.repos
            .device_link_repo
            .find(link_hash)
            .await?
            .filter(|link| link.user_id == user_id)
            .ok_or(ServiceError::NotFound)
    }

    /// Cancel as a holder of the link id: any live link, `Unauthenticated` for anything else.
    async fn cancel_anonymously(&self, link_id: &str, remote_ip: &str) -> ServiceResult<()> {
        Self::limit(&self.offer_rate_limiter, remote_ip, OFFER_KEY)?;
        self.anonymous(remote_ip, async {
            let link_hash = link_hash(link_id).ok_or(ServiceError::Unauthenticated)?;
            let link = self
                .repos
                .device_link_repo
                .cancel(&link_hash, None)
                .await?
                .ok_or(ServiceError::Unauthenticated)?;
            audit(
                "device_link_cancelled",
                link.id,
                link.user_id,
                "device link cancelled by a holder of the link id",
            );
            Ok(())
        })
        .await
    }
}

#[async_trait]
impl DeviceLinkService for DefaultDeviceLinkService {
    #[instrument(name = "device_link.start", skip_all, fields(actor_id = %cmd.actor.user_id))]
    async fn start(&self, cmd: StartDeviceLinkCmd) -> ServiceResult<StartedDeviceLinkDto> {
        let user_id = cmd.actor.user_id;
        Self::limit(&self.start_rate_limiter, &user_id.to_string(), START_KEY)?;
        // Best effort: an old row left behind costs nothing but space.
        if let Err(e) = self
            .repos
            .device_link_repo
            .delete_expired(CLEANUP_GRACE)
            .await
        {
            warn!(error = %e, "could not delete expired device links");
        }

        let link_id = generate_link_id();
        let link_hash = link_hash(&link_id).ok_or_else(|| {
            ServiceError::internal(anyhow::anyhow!("generated link id does not decode"))
        })?;
        let (scope, repos) = self.begin().await?;
        // The lock serializes two starts of one account, so the cancel below always sees the
        // previous link and the one-live-link index never refuses the insert.
        if !repos.device_link_repo.lock_user(user_id).await? {
            return Err(ServiceError::NotFound);
        }
        let user = repos
            .user_repo
            .get_by_id(user_id)
            .await?
            .ok_or(ServiceError::NotFound)?;
        if user.principal_id.is_none() {
            return Err(ServiceError::Validation(
                "this account has no key identity yet".to_string(),
            ));
        }
        let cancelled = repos.device_link_repo.cancel_live_links(user_id).await?;
        let link = repos
            .device_link_repo
            .create(NewDeviceLink {
                link_hash,
                user_id,
                ttl: LINK_TTL,
            })
            .await?;
        Self::commit(scope).await?;

        for link_row_id in cancelled {
            audit(
                "device_link_cancelled",
                link_row_id,
                user_id,
                "device link cancelled by a new link",
            );
        }
        audit(
            "device_link_started",
            link.id,
            user_id,
            "device link started",
        );
        Ok(StartedDeviceLinkDto {
            link_id,
            expires_at: link.expires_at,
        })
    }

    #[instrument(name = "device_link.send_offer", skip_all)]
    async fn send_offer(&self, cmd: SendDeviceLinkOfferCmd) -> ServiceResult<()> {
        Self::limit(&self.offer_rate_limiter, &cmd.remote_ip, OFFER_KEY)?;
        self.anonymous(&cmd.remote_ip, async {
            let offer = decode_sealed("offer", &cmd.offer, MAX_OFFER_CIPHERTEXT_LEN)?;
            let link_hash = link_hash(&cmd.link_id).ok_or(ServiceError::Unauthenticated)?;
            if let Some(link) = self
                .repos
                .device_link_repo
                .offer(&link_hash, offer.clone())
                .await?
            {
                audit(
                    "device_link_offered",
                    link.id,
                    link.user_id,
                    "device link offer received",
                );
                return Ok(());
            }
            // A retry of the offer that won (its answer was lost) succeeds again.
            match self.repos.device_link_repo.find(&link_hash).await? {
                Some(link)
                    if link.state == DeviceLinkState::Offered
                        && link.offer.as_ref() == Some(&offer) =>
                {
                    Ok(())
                }
                _ => Err(ServiceError::Unauthenticated),
            }
        })
        .await
    }

    #[instrument(name = "device_link.get", skip_all, fields(actor_id = %query.actor.user_id))]
    async fn get(&self, query: GetDeviceLinkQuery) -> ServiceResult<DeviceLinkDto> {
        let link_hash = link_hash(&query.link_id).ok_or(ServiceError::NotFound)?;
        let link = self.find_own(&link_hash, query.actor.user_id).await?;
        Ok(link_dto(&link))
    }

    #[instrument(name = "device_link.send_answer", skip_all, fields(actor_id = %cmd.actor.user_id))]
    async fn send_answer(&self, cmd: SendDeviceLinkAnswerCmd) -> ServiceResult<DeviceLinkDto> {
        let user_id = cmd.actor.user_id;
        let answer = decode_sealed("answer", &cmd.answer, MAX_ANSWER_CIPHERTEXT_LEN)?;
        let link_hash = link_hash(&cmd.link_id).ok_or(ServiceError::NotFound)?;
        if let Some(link) = self
            .repos
            .device_link_repo
            .answer(&link_hash, user_id, answer.clone(), LINK_TTL)
            .await?
        {
            audit(
                "device_link_answered",
                link.id,
                link.user_id,
                "device link answer received",
            );
            return Ok(link_dto(&link));
        }
        let link = self.find_own(&link_hash, user_id).await?;
        // A retry of the answer that was stored succeeds again.
        if link.state == DeviceLinkState::Answered && link.answer.as_ref() == Some(&answer) {
            return Ok(link_dto(&link));
        }
        if link.state == DeviceLinkState::Answered && !link.expired {
            return Err(ServiceError::Conflict(
                "another answer is already stored".to_string(),
            ));
        }
        Err(conflict(&link, "an answer"))
    }

    #[instrument(name = "device_link.get_answer", skip_all)]
    async fn get_answer(
        &self,
        query: GetDeviceLinkAnswerQuery,
    ) -> ServiceResult<Option<SealedPayload>> {
        Self::limit(&self.answer_rate_limiter, &query.remote_ip, ANSWER_KEY)?;
        self.anonymous(&query.remote_ip, async {
            let link_hash = link_hash(&query.link_id).ok_or(ServiceError::Unauthenticated)?;
            let link = self
                .repos
                .device_link_repo
                .find(&link_hash)
                .await?
                .filter(|link| !link.expired)
                .ok_or(ServiceError::Unauthenticated)?;
            match link.state {
                DeviceLinkState::Open | DeviceLinkState::Offered => Ok(None),
                DeviceLinkState::Answered => link
                    .answer
                    .as_ref()
                    .map(encode_sealed)
                    .map(Some)
                    .ok_or_else(|| {
                        ServiceError::internal(anyhow::anyhow!(
                            "answered device link has no answer"
                        ))
                    }),
                DeviceLinkState::Consumed | DeviceLinkState::Cancelled => {
                    Err(ServiceError::Unauthenticated)
                }
            }
        })
        .await
    }

    #[instrument(name = "device_link.complete", skip_all, fields(actor_id = %cmd.actor.user_id))]
    async fn complete(&self, cmd: CompleteDeviceLinkCmd) -> ServiceResult<()> {
        let user_id = cmd.actor.user_id;
        let link_hash = link_hash(&cmd.link_id).ok_or(ServiceError::NotFound)?;
        if let Some(link) = self
            .repos
            .device_link_repo
            .complete(&link_hash, user_id)
            .await?
        {
            audit(
                "device_link_completed",
                link.id,
                link.user_id,
                "device link completed",
            );
            return Ok(());
        }
        let link = self.find_own(&link_hash, user_id).await?;
        if link.state == DeviceLinkState::Consumed {
            return Ok(());
        }
        Err(conflict(&link, "completion"))
    }

    #[instrument(name = "device_link.cancel", skip_all)]
    async fn cancel(&self, cmd: CancelDeviceLinkCmd) -> ServiceResult<()> {
        if let (Some(actor), Some(link_hash)) = (&cmd.actor, link_hash(&cmd.link_id)) {
            let repo = &self.repos.device_link_repo;
            if let Some(link) = repo.cancel(&link_hash, Some(actor.user_id)).await? {
                audit(
                    "device_link_cancelled",
                    link.id,
                    link.user_id,
                    "device link cancelled by its user",
                );
                return Ok(());
            }
            if let Some(link) = repo.find(&link_hash).await?
                && link.user_id == actor.user_id
            {
                // A live link that has not expired would have been cancelled above.
                return match link.state {
                    DeviceLinkState::Consumed => Err(ServiceError::Conflict(
                        "the link was already completed".to_string(),
                    )),
                    _ => Ok(()),
                };
            }
        }
        self.cancel_anonymously(&cmd.link_id, &cmd.remote_ip).await
    }
}
