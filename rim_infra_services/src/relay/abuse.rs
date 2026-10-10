//! The operator's abuse tools: blocking principals on this server and the abuse reports.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{Duration, Utc};
use tracing::{info, instrument};

use rim_core::auth::repository::SessionRepository;
use rim_core::key_identity::repository::KeyIdentityRepository;
use rim_core::relay::model::{AbuseReportId, NewAbuseReport};
use rim_core::relay::repository::RelayAbuseRepository;
use rim_identity::is_principal_id;
use rim_services::relay::dto::{
    AbuseReportDto, FileAbuseReport, PrincipalBlockDto, PrincipalBlockOutcome,
};
use rim_services::relay::service::RelayAbuseService;
use rim_services::{ServiceError, ServiceResult};

/// Longest block or report reason, in characters.
const MAX_REASON_CHARS: usize = 1000;
/// How long a request ignored by a block is kept, so its conversation stays dropped.
const IGNORED_REQUEST_TTL: Duration = Duration::days(30);

#[derive(Clone)]
pub struct RelayAbuseRepos {
    pub relay_abuse_repo: Arc<dyn RelayAbuseRepository>,
    pub key_identity_repo: Arc<dyn KeyIdentityRepository>,
    pub session_repo: Arc<dyn SessionRepository>,
}

pub struct DefaultRelayAbuseService {
    repos: RelayAbuseRepos,
}

impl DefaultRelayAbuseService {
    pub fn new(repos: RelayAbuseRepos) -> Self {
        Self { repos }
    }
}

/// A reason trimmed, 1 to 1000 characters.
fn check_reason(reason: &str) -> ServiceResult<String> {
    let reason = reason.trim();
    if reason.is_empty() || reason.chars().count() > MAX_REASON_CHARS {
        return Err(ServiceError::Validation(format!(
            "a reason is 1 to {MAX_REASON_CHARS} characters"
        )));
    }
    Ok(reason.to_string())
}

fn check_principal(principal_id: &str) -> ServiceResult<()> {
    if is_principal_id(principal_id) {
        Ok(())
    } else {
        Err(ServiceError::Validation(format!(
            "{principal_id} is not a principal id"
        )))
    }
}

#[async_trait]
impl RelayAbuseService for DefaultRelayAbuseService {
    #[instrument(name = "relay_abuse.block_principal", skip_all)]
    async fn block_principal(
        &self,
        principal_id: String,
        reason: String,
    ) -> ServiceResult<PrincipalBlockOutcome> {
        check_principal(&principal_id)?;
        let reason = check_reason(&reason)?;
        // Blocked first, so a login racing the session sweep below is refused.
        let newly_blocked = self
            .repos
            .relay_abuse_repo
            .block_principal(&principal_id, &reason)
            .await?;
        let user_id = self
            .repos
            .key_identity_repo
            .find_user_by_principal_id(&principal_id)
            .await?;
        let ended_sessions = match user_id {
            Some(user_id) => self.repos.session_repo.revoke_all_for_user(user_id).await?,
            None => 0,
        };
        let ignored_requests = self
            .repos
            .relay_abuse_repo
            .ignore_requests_from(&principal_id, Utc::now() + IGNORED_REQUEST_TTL)
            .await?;
        info!(
            target: "audit",
            event = "relay.principal_blocked",
            scope = "server",
            principal_id = %principal_id,
            user_id = ?user_id,
            newly_blocked,
            ended_sessions,
            ignored_requests,
            "an operator blocked a principal on this server"
        );
        Ok(PrincipalBlockOutcome {
            newly_blocked,
            user_id,
            ended_sessions,
            ignored_requests,
        })
    }

    #[instrument(name = "relay_abuse.unblock_principal", skip_all)]
    async fn unblock_principal(&self, principal_id: String) -> ServiceResult<bool> {
        check_principal(&principal_id)?;
        let lifted = self
            .repos
            .relay_abuse_repo
            .unblock_principal(&principal_id)
            .await?;
        if lifted {
            info!(
                target: "audit",
                event = "relay.principal_unblocked",
                scope = "server",
                principal_id = %principal_id,
                "an operator unblocked a principal on this server"
            );
        }
        Ok(lifted)
    }

    async fn principal_blocks(&self) -> ServiceResult<Vec<PrincipalBlockDto>> {
        Ok(self
            .repos
            .relay_abuse_repo
            .list_principal_blocks()
            .await?
            .into_iter()
            .map(|b| PrincipalBlockDto {
                principal_id: b.principal_id,
                reason: b.reason,
                created_at: b.created_at,
            })
            .collect())
    }

    #[instrument(name = "relay_abuse.file_report", skip_all)]
    async fn file_report(&self, report: FileAbuseReport) -> ServiceResult<AbuseReportDto> {
        let reason = check_reason(&report.reason)?;
        if report.subject_principal_id.is_none() && report.subject_server_id.is_none() {
            return Err(ServiceError::Validation(
                "a report names a principal, a server or both".to_string(),
            ));
        }
        if let Some(principal_id) = &report.subject_principal_id {
            check_principal(principal_id)?;
        }
        if let Some(server_id) = &report.subject_server_id
            && !rim_s2s::is_server_id(server_id)
        {
            return Err(ServiceError::Validation(format!(
                "{server_id} is not a server id"
            )));
        }
        let stored = self
            .repos
            .relay_abuse_repo
            .insert_report(NewAbuseReport {
                reporter_user_id: None,
                subject_principal_id: report.subject_principal_id,
                subject_server_id: report.subject_server_id,
                request_id: None,
                reason,
            })
            .await?;
        info!(
            target: "audit",
            event = "relay.abuse_report_filed",
            report_id = %stored.id,
            principal_id = ?stored.subject_principal_id,
            peer_server_id = ?stored.subject_server_id,
            "an operator filed an abuse report"
        );
        Ok(stored.into())
    }

    async fn reports(&self, open_only: bool) -> ServiceResult<Vec<AbuseReportDto>> {
        Ok(self
            .repos
            .relay_abuse_repo
            .list_reports(open_only)
            .await?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    #[instrument(name = "relay_abuse.resolve_report", skip_all)]
    async fn resolve_report(&self, id: AbuseReportId) -> ServiceResult<Option<AbuseReportDto>> {
        let resolved = self.repos.relay_abuse_repo.resolve_report(id).await?;
        if resolved.is_some() {
            info!(
                target: "audit",
                event = "relay.abuse_report_resolved",
                report_id = %id,
                "an operator resolved an abuse report"
            );
        }
        Ok(resolved.map(Into::into))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use rim_core::auth::repository::MockSessionRepository;
    use rim_core::key_identity::repository::MockKeyIdentityRepository;
    use rim_core::relay::repository::MockRelayAbuseRepository;
    use rim_core::user::model::UserId;

    use super::*;

    const PRINCIPAL: &str = "rimp1_txDLbnipi5i3kCJtJwBOUixzv_MY8OnU0mcy--Tyje0";

    fn service(
        abuse: MockRelayAbuseRepository,
        holder: Option<UserId>,
        ended: Arc<Mutex<Vec<UserId>>>,
    ) -> DefaultRelayAbuseService {
        let mut keys = MockKeyIdentityRepository::new();
        keys.expect_find_user_by_principal_id()
            .returning(move |_| Box::pin(async move { Ok(holder) }));
        let mut sessions = MockSessionRepository::new();
        sessions
            .expect_revoke_all_for_user()
            .returning(move |user| {
                ended.lock().expect("lock").push(user);
                Ok(2)
            });
        DefaultRelayAbuseService::new(RelayAbuseRepos {
            relay_abuse_repo: Arc::new(abuse),
            key_identity_repo: Arc::new(keys),
            session_repo: Arc::new(sessions),
        })
    }

    #[tokio::test]
    async fn blocking_a_principal_ends_its_sessions_and_ignores_its_requests() {
        let mut abuse = MockRelayAbuseRepository::new();
        abuse
            .expect_block_principal()
            .withf(|p, r| p == PRINCIPAL && r == "spam wave")
            .returning(|_, _| Box::pin(async { Ok(true) }));
        abuse
            .expect_ignore_requests_from()
            .returning(|_, _| Box::pin(async { Ok(3) }));
        let holder = UserId::new();
        let ended = Arc::new(Mutex::new(Vec::new()));
        let svc = service(abuse, Some(holder), Arc::clone(&ended));
        let outcome = svc
            .block_principal(PRINCIPAL.to_string(), "  spam wave ".to_string())
            .await
            .expect("blocked");
        assert_eq!(
            outcome,
            PrincipalBlockOutcome {
                newly_blocked: true,
                user_id: Some(holder),
                ended_sessions: 2,
                ignored_requests: 3,
            }
        );
        assert_eq!(*ended.lock().expect("lock"), vec![holder]);
    }

    #[tokio::test]
    async fn a_principal_with_no_account_here_is_blocked_too() {
        let mut abuse = MockRelayAbuseRepository::new();
        abuse
            .expect_block_principal()
            .returning(|_, _| Box::pin(async { Ok(false) }));
        abuse
            .expect_ignore_requests_from()
            .returning(|_, _| Box::pin(async { Ok(0) }));
        let ended = Arc::new(Mutex::new(Vec::new()));
        let svc = service(abuse, None, Arc::clone(&ended));
        let outcome = svc
            .block_principal(PRINCIPAL.to_string(), "again".to_string())
            .await
            .expect("blocked");
        assert!(!outcome.newly_blocked);
        assert_eq!(outcome.ended_sessions, 0);
        assert!(ended.lock().expect("lock").is_empty());
    }

    #[tokio::test]
    async fn bad_principals_reasons_and_reports_are_refused() {
        let svc = service(
            MockRelayAbuseRepository::new(),
            None,
            Arc::new(Mutex::new(Vec::new())),
        );
        for (principal, reason) in [
            ("bob", "spam"),
            (PRINCIPAL, "  "),
            (PRINCIPAL, &"x".repeat(1001)),
        ] {
            assert!(matches!(
                svc.block_principal(principal.to_string(), reason.to_string())
                    .await,
                Err(ServiceError::Validation(_))
            ));
        }
        for report in [
            FileAbuseReport {
                subject_principal_id: None,
                subject_server_id: None,
                reason: "spam".to_string(),
            },
            FileAbuseReport {
                subject_principal_id: None,
                subject_server_id: Some("relay.example".to_string()),
                reason: "spam".to_string(),
            },
        ] {
            assert!(matches!(
                svc.file_report(report).await,
                Err(ServiceError::Validation(_))
            ));
        }
    }
}
