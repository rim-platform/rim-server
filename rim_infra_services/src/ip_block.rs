//! The operator's IP deny list: blocking and lifting addresses and ranges, and the blocks in
//! force for the server's edge.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{Duration, Utc};
use tracing::{info, instrument, warn};

use rim_core::ip_block::model::{IpBlock, IpRange, NewIpBlock};
use rim_core::ip_block::repository::IpBlockRepository;
use rim_core::user::repository::UserRepository;
use rim_services::ip_block::dto::{BlockIpCmd, IpBlockDto, IpBlockOutcome};
use rim_services::ip_block::service::{IpBlockService, MAX_BLOCK_DAYS};
use rim_services::operator::OperatorCaller;
use rim_services::{ServiceError, ServiceResult};

use crate::operator::require_operator;

/// Longest block reason, in characters.
const MAX_REASON_CHARS: usize = 1000;

pub struct DefaultIpBlockService {
    repo: Arc<dyn IpBlockRepository>,
    /// Who is an instance operator.
    user_repo: Arc<dyn UserRepository>,
}

impl DefaultIpBlockService {
    pub fn new(repo: Arc<dyn IpBlockRepository>, user_repo: Arc<dyn UserRepository>) -> Self {
        Self { repo, user_repo }
    }
}

fn parse_range(cidr: &str) -> ServiceResult<IpRange> {
    cidr.parse().map_err(ServiceError::Validation)
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

/// A stored block as the edge uses it; `None`, with a warning, for a row whose range does not
/// parse, which no command writes.
fn to_dto(block: IpBlock) -> Option<IpBlockDto> {
    match block.cidr.parse::<IpRange>() {
        Ok(range) => Some(IpBlockDto {
            range,
            reason: block.reason,
            expires_at: block.expires_at,
            created_at: block.created_at,
        }),
        Err(e) => {
            warn!(cidr = %block.cidr, error = %e, "skipped an IP block whose range does not parse");
            None
        }
    }
}

#[async_trait]
impl IpBlockService for DefaultIpBlockService {
    #[instrument(name = "ip_block.block", skip_all)]
    async fn block(&self, by: OperatorCaller, cmd: BlockIpCmd) -> ServiceResult<IpBlockOutcome> {
        require_operator(self.user_repo.as_ref(), by).await?;
        let range = parse_range(&cmd.cidr)?;
        let reason = check_reason(&cmd.reason)?;
        let expires_at = match cmd.expires_in_days {
            None => None,
            Some(days) if (1..=MAX_BLOCK_DAYS).contains(&days) => {
                Some(Utc::now() + Duration::days(i64::from(days)))
            }
            Some(_) => {
                return Err(ServiceError::Validation(format!(
                    "a block lasts 1 to {MAX_BLOCK_DAYS} days"
                )));
            }
        };
        let (stored, newly_blocked) = self
            .repo
            .upsert(NewIpBlock {
                cidr: range.to_string(),
                reason,
                expires_at,
            })
            .await?;
        info!(
            target: "audit",
            event = "ip.block",
            scope = "server",
            actor_id = %by,
            cidr = %range,
            reason = %stored.reason,
            expires_at = ?stored.expires_at,
            newly_blocked,
            "an operator blocked an IP address or range"
        );
        Ok(IpBlockOutcome {
            block: IpBlockDto {
                range,
                reason: stored.reason,
                expires_at: stored.expires_at,
                created_at: stored.created_at,
            },
            newly_blocked,
        })
    }

    #[instrument(name = "ip_block.unblock", skip_all)]
    async fn unblock(&self, by: OperatorCaller, cidr: String) -> ServiceResult<bool> {
        require_operator(self.user_repo.as_ref(), by).await?;
        let range = parse_range(&cidr)?;
        let lifted = self.repo.delete(&range.to_string()).await?;
        if lifted {
            info!(
                target: "audit",
                event = "ip.unblock",
                scope = "server",
                actor_id = %by,
                cidr = %range,
                "an operator unblocked an IP address or range"
            );
        }
        Ok(lifted)
    }

    async fn list(&self, by: OperatorCaller) -> ServiceResult<Vec<IpBlockDto>> {
        require_operator(self.user_repo.as_ref(), by).await?;
        Ok(self
            .repo
            .list()
            .await?
            .into_iter()
            .filter_map(to_dto)
            .collect())
    }

    async fn active(&self) -> ServiceResult<Vec<IpBlockDto>> {
        Ok(self
            .repo
            .list_active(Utc::now())
            .await?
            .into_iter()
            .filter_map(to_dto)
            .collect())
    }

    async fn purge_expired(&self) -> ServiceResult<u64> {
        Ok(self.repo.delete_expired(Utc::now()).await?)
    }
}

#[cfg(test)]
mod tests {
    use rim_core::ip_block::model::IpBlockId;
    use rim_core::ip_block::repository::MockIpBlockRepository;
    use rim_core::user::repository::MockUserRepository;

    use super::*;

    const CLI: OperatorCaller = OperatorCaller::CommandLine;

    /// The service over `repo`, for callers the user repository is never asked about.
    fn service(repo: MockIpBlockRepository) -> DefaultIpBlockService {
        DefaultIpBlockService::new(Arc::new(repo), Arc::new(MockUserRepository::new()))
    }

    fn stored(new: &NewIpBlock) -> IpBlock {
        IpBlock {
            id: IpBlockId::new(),
            cidr: new.cidr.clone(),
            reason: new.reason.clone(),
            expires_at: new.expires_at,
            created_at: Utc::now(),
        }
    }

    fn cmd(cidr: &str, reason: &str, expires_in_days: Option<u32>) -> BlockIpCmd {
        BlockIpCmd {
            cidr: cidr.to_string(),
            reason: reason.to_string(),
            expires_in_days,
        }
    }

    #[tokio::test]
    async fn a_block_stores_the_canonical_range_and_its_expiry() {
        let mut repo = MockIpBlockRepository::new();
        repo.expect_upsert()
            .withf(|new| {
                new.cidr == "203.0.113.0/24"
                    && new.reason == "scraping"
                    && new.expires_at.is_some_and(|at| {
                        let days = (at - Utc::now()).num_hours();
                        (7 * 24 - 1..=7 * 24).contains(&days)
                    })
            })
            .times(1)
            .returning(|new| Box::pin(async move { Ok((stored(&new), true)) }));
        let service = service(repo);

        let outcome = service
            .block(CLI, cmd("203.0.113.77/24", "  scraping ", Some(7)))
            .await
            .expect("blocked");
        assert!(outcome.newly_blocked);
        assert_eq!(outcome.block.range.to_string(), "203.0.113.0/24");
        assert_eq!(outcome.block.reason, "scraping");
    }

    #[tokio::test]
    async fn a_malformed_range_reason_or_expiry_is_refused_before_the_store() {
        let mut repo = MockIpBlockRepository::new();
        repo.expect_upsert().never();
        repo.expect_delete().never();
        let service = service(repo);
        for bad in [
            cmd("not-an-ip", "spam", None),
            cmd("192.0.2.0/33", "spam", None),
            cmd("192.0.2.1", "   ", None),
            cmd("192.0.2.1", &"x".repeat(1001), None),
            cmd("192.0.2.1", "spam", Some(0)),
            cmd("192.0.2.1", "spam", Some(366)),
        ] {
            assert!(
                matches!(
                    service.block(CLI, bad.clone()).await,
                    Err(ServiceError::Validation(_))
                ),
                "{bad:?}"
            );
        }
        assert!(matches!(
            service.unblock(CLI, "192.0.2".to_string()).await,
            Err(ServiceError::Validation(_))
        ));
    }

    #[tokio::test]
    async fn unblock_names_the_range_in_canonical_form() {
        let mut repo = MockIpBlockRepository::new();
        repo.expect_delete()
            .withf(|cidr| cidr == "2001:db8::/32")
            .times(1)
            .returning(|_| Box::pin(async { Ok(true) }));
        repo.expect_delete()
            .withf(|cidr| cidr == "192.0.2.1/32")
            .times(1)
            .returning(|_| Box::pin(async { Ok(false) }));
        let service = service(repo);
        assert!(
            service
                .unblock(CLI, "2001:DB8::1/32".to_string())
                .await
                .unwrap()
        );
        assert!(!service.unblock(CLI, "192.0.2.1".to_string()).await.unwrap());
    }

    #[tokio::test]
    async fn the_active_list_skips_rows_whose_range_does_not_parse() {
        let mut repo = MockIpBlockRepository::new();
        repo.expect_list_active().returning(|_| {
            Box::pin(async {
                Ok(["198.51.100.0/24", "garbage"]
                    .into_iter()
                    .map(|cidr| IpBlock {
                        id: IpBlockId::new(),
                        cidr: cidr.to_string(),
                        reason: "r".to_string(),
                        expires_at: None,
                        created_at: Utc::now(),
                    })
                    .collect())
            })
        });
        let service = service(repo);
        let active = service.active().await.unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].range.to_string(), "198.51.100.0/24");
    }

    #[tokio::test]
    async fn a_user_who_is_not_an_operator_cannot_touch_the_deny_list() {
        let mut repo = MockIpBlockRepository::new();
        repo.expect_upsert().never();
        repo.expect_delete().never();
        repo.expect_list().never();
        let mut users = MockUserRepository::new();
        users.expect_is_instance_operator().returning(|_| Ok(false));
        let service = DefaultIpBlockService::new(Arc::new(repo), Arc::new(users));
        let member = OperatorCaller::User(rim_services::Actor::new(
            rim_core::user::model::UserId::new(),
        ));

        let refusals = [
            service
                .block(member, cmd("192.0.2.1", "spam", None))
                .await
                .err(),
            service.unblock(member, "192.0.2.1".to_string()).await.err(),
            service.list(member).await.err(),
        ];
        for refusal in refusals {
            assert!(
                matches!(refusal, Some(ServiceError::Forbidden(_))),
                "{refusal:?}"
            );
        }
    }
}
