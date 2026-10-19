//! Instance invites: the terms every issuer is held to and the issuing itself, used by the
//! operator's `rim_server invite` command. Workspace invites share the expiry rule.

use chrono::{DateTime, Duration, Utc};

use rim_auth::session_token::{generate_token, hash_token};
use rim_core::auth::model::{InstanceInvite, NewInstanceInvite};
use rim_core::auth::repository::InstanceInviteRepository;
use rim_core::user::model::UserId;
use rim_services::{ServiceError, ServiceResult};

use crate::auth::DefaultAuthService;

/// Lifetime of an invite (instance or workspace) issued without an expiry.
pub const DEFAULT_TTL_DAYS: i64 = 7;
/// Longest lifetime an invite (instance or workspace) may have.
pub const MAX_TTL_DAYS: i64 = 30;
/// Most signups one instance invite may admit.
pub const MAX_USES: i32 = 50;

/// An invite as stored, with its raw token, which is shown once and never stored.
#[derive(Debug, Clone)]
pub struct IssuedInvite {
    pub invite: InstanceInvite,
    pub token: String,
}

/// A fresh raw invite token, `rim_iinv_` followed by the session token's random part.
fn new_token() -> String {
    generate_token("rim_iinv_")
}

/// The expiry an invite gets, instance and workspace invites alike: `expires_at` defaults to
/// [`DEFAULT_TTL_DAYS`] from `now` and must lie after `now` and at most [`MAX_TTL_DAYS`] out.
///
/// # Errors
/// [`ServiceError::Validation`] when the expiry is not after `now` or too far out.
pub fn expiry(
    expires_at: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> ServiceResult<DateTime<Utc>> {
    let expires_at = expires_at.unwrap_or_else(|| now + Duration::days(DEFAULT_TTL_DAYS));
    if expires_at <= now {
        return Err(ServiceError::Validation(
            "expires_at must be in the future".to_string(),
        ));
    }
    if expires_at > now + Duration::days(MAX_TTL_DAYS) {
        return Err(ServiceError::Validation(format!(
            "expires_at may not be more than {MAX_TTL_DAYS} days from now"
        )));
    }
    Ok(expires_at)
}

/// The expiry and use count an instance invite gets: the expiry as [`expiry`] gives it;
/// `max_uses` defaults to 1 and is 1 to [`MAX_USES`].
///
/// # Errors
/// [`ServiceError::Validation`] naming the field out of range.
pub fn terms(
    expires_at: Option<DateTime<Utc>>,
    max_uses: Option<i32>,
    now: DateTime<Utc>,
) -> ServiceResult<(DateTime<Utc>, i32)> {
    let expires_at = expiry(expires_at, now)?;
    let max_uses = max_uses.unwrap_or(1);
    if !(1..=MAX_USES).contains(&max_uses) {
        return Err(ServiceError::Validation(format!(
            "max_uses must be between 1 and {MAX_USES} inclusive"
        )));
    }
    Ok((expires_at, max_uses))
}

/// What an invite admits, besides its expiry and use count.
#[derive(Debug, Clone, Default)]
pub struct InviteTerms {
    pub expires_at: Option<DateTime<Utc>>,
    pub max_uses: Option<i32>,
    /// The only address a signup through the invite may use.
    pub email: Option<String>,
}

/// Store a new invite under [`terms`], issued by `created_by` (`None`: the operator). An email
/// binding is checked like a signup's address and stored trimmed and lowercased.
///
/// # Errors
/// [`ServiceError::Validation`] when the terms are out of range or the email is malformed; the
/// repository's error when storing fails.
pub async fn issue(
    repo: &dyn InstanceInviteRepository,
    created_by: Option<UserId>,
    invite_terms: InviteTerms,
    now: DateTime<Utc>,
) -> ServiceResult<IssuedInvite> {
    let (expires_at, max_uses) = terms(invite_terms.expires_at, invite_terms.max_uses, now)?;
    let email = invite_terms
        .email
        .as_deref()
        .map(DefaultAuthService::validate_email)
        .transpose()?;
    let token = new_token();
    let invite = repo
        .create(NewInstanceInvite {
            created_by_user_id: created_by,
            token_hash: hash_token(&token),
            expires_at,
            max_uses,
            email,
        })
        .await?;
    Ok(IssuedInvite { invite, token })
}

#[cfg(test)]
mod tests {
    use rim_core::auth::model::InstanceInviteId;
    use rim_core::auth::repository::MockInstanceInviteRepository;

    use super::*;

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-10-10T12:00:00Z")
            .map(|t| t.with_timezone(&Utc))
            .unwrap_or_default()
    }

    fn validation(result: ServiceResult<(DateTime<Utc>, i32)>) -> String {
        match result {
            Err(ServiceError::Validation(message)) => message,
            other => panic!("expected a validation error, got {other:?}"),
        }
    }

    #[test]
    fn terms_default_to_a_week_and_one_use() {
        let (expires_at, max_uses) = terms(None, None, now()).expect("terms");
        assert_eq!(expires_at, now() + Duration::days(7));
        assert_eq!(max_uses, 1);
    }

    #[test]
    fn terms_refuse_past_far_expiries_and_use_counts_out_of_range() {
        assert!(validation(terms(Some(now()), None, now())).contains("future"));
        assert!(validation(terms(Some(now() + Duration::days(31)), None, now())).contains("30"));
        assert!(validation(terms(None, Some(0), now())).contains("max_uses"));
        assert!(validation(terms(None, Some(51), now())).contains("max_uses"));
        assert_eq!(
            terms(Some(now() + Duration::days(30)), Some(50), now()).expect("edges"),
            (now() + Duration::days(30), 50)
        );
    }

    #[tokio::test]
    async fn the_operator_issues_an_invite_with_no_creating_user() {
        let mut repo = MockInstanceInviteRepository::new();
        repo.expect_create()
            .withf(|new| {
                new.created_by_user_id.is_none()
                    && new.max_uses == 3
                    && new.token_hash.len() == 64
                    && new.email.is_none()
            })
            .returning(|new| {
                Ok(InstanceInvite {
                    id: InstanceInviteId::new(),
                    created_by_user_id: new.created_by_user_id,
                    token_hash: new.token_hash,
                    created_at: now(),
                    expires_at: new.expires_at,
                    max_uses: new.max_uses,
                    use_count: 0,
                    revoked_at: None,
                    email: new.email,
                })
            });

        let issued = issue(
            &repo,
            None,
            InviteTerms {
                max_uses: Some(3),
                ..Default::default()
            },
            now(),
        )
        .await
        .expect("issued");
        assert!(issued.token.starts_with("rim_iinv_"));
        assert_eq!(issued.invite.token_hash, hash_token(&issued.token));
        assert_eq!(issued.invite.created_by_user_id, None);
    }

    #[tokio::test]
    async fn out_of_range_terms_store_nothing() {
        let mut repo = MockInstanceInviteRepository::new();
        repo.expect_create().never();
        for invite_terms in [
            InviteTerms {
                max_uses: Some(51),
                ..Default::default()
            },
            InviteTerms {
                email: Some("not an address".to_string()),
                ..Default::default()
            },
        ] {
            assert!(matches!(
                issue(&repo, None, invite_terms, now()).await,
                Err(ServiceError::Validation(_))
            ));
        }
    }

    #[tokio::test]
    async fn an_email_binding_is_stored_lowercased() {
        let mut repo = MockInstanceInviteRepository::new();
        repo.expect_create()
            .withf(|new| new.email.as_deref() == Some("ops@example.com"))
            .returning(|new| {
                Ok(InstanceInvite {
                    id: InstanceInviteId::new(),
                    created_by_user_id: None,
                    token_hash: new.token_hash,
                    created_at: now(),
                    expires_at: new.expires_at,
                    max_uses: new.max_uses,
                    use_count: 0,
                    revoked_at: None,
                    email: new.email,
                })
            });
        let issued = issue(
            &repo,
            None,
            InviteTerms {
                email: Some(" Ops@Example.COM ".to_string()),
                ..Default::default()
            },
            now(),
        )
        .await
        .expect("issued");
        assert_eq!(issued.invite.email.as_deref(), Some("ops@example.com"));
    }
}
