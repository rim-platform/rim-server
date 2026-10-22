//! Instance operators: the accounts that run this server's moderation queue from the client.

use async_trait::async_trait;
use tracing::{info, instrument};

use rim_core::principal::model::PrincipalKind;
use rim_core::user::model::User;
use rim_core::user::repository::UserRepository;
use rim_services::operator::{InstanceOperatorService, OperatorCaller, OperatorChangeDto};
use rim_services::relay::dto::AccountSummaryDto;
use rim_services::{Actor, ServiceError, ServiceResult};

use crate::relay::abuse::{DefaultRelayAbuseService, RelayAbuseRepos, summary};

/// The message of the `Forbidden` error a user who is not an instance operator gets.
pub(crate) const OPERATORS_ONLY: &str = "instance operators only";

/// Refuse `by` with `Forbidden` unless it is the command line or a user acting for themselves
/// who is an instance operator.
pub(crate) async fn require_operator(
    user_repo: &dyn UserRepository,
    by: OperatorCaller,
) -> ServiceResult<()> {
    match by {
        OperatorCaller::CommandLine => Ok(()),
        OperatorCaller::User(actor) => {
            if is_operator(user_repo, actor).await? {
                Ok(())
            } else {
                Err(ServiceError::Forbidden(OPERATORS_ONLY.to_string()))
            }
        }
    }
}

/// Whether `actor` is a user, not an app or bot acting for one, whose account holds the role.
pub(crate) async fn is_operator(
    user_repo: &dyn UserRepository,
    actor: Actor,
) -> ServiceResult<bool> {
    if actor.principal.kind != PrincipalKind::User {
        return Ok(false);
    }
    Ok(user_repo.is_instance_operator(actor.user_id).await?)
}

pub struct DefaultInstanceOperatorService {
    repos: RelayAbuseRepos,
}

impl DefaultInstanceOperatorService {
    pub fn new(repos: RelayAbuseRepos) -> Self {
        Self { repos }
    }

    async fn account(&self, username: &str) -> ServiceResult<User> {
        self.repos
            .user_repo
            .find_by_username(username)
            .await?
            .ok_or(ServiceError::NotFound)
    }
}

#[async_trait]
impl InstanceOperatorService for DefaultInstanceOperatorService {
    async fn is_operator(&self, actor: Actor) -> ServiceResult<bool> {
        is_operator(self.repos.user_repo.as_ref(), actor).await
    }

    #[instrument(name = "operator.grant", skip_all)]
    async fn grant(&self, username: String) -> ServiceResult<OperatorChangeDto> {
        let user = self.account(&username).await?;
        if user.deletion_scheduled_for.is_some() {
            return Err(ServiceError::Validation(format!(
                "{username} is scheduled for deletion"
            )));
        }
        if DefaultRelayAbuseService::is_blocked(&self.repos, &user).await? {
            return Err(ServiceError::Validation(format!("{username} is blocked")));
        }
        let changed = self
            .repos
            .user_repo
            .set_instance_operator(user.id, true)
            .await?;
        info!(
            target: "audit",
            event = "operator.granted",
            scope = "server",
            actor_id = %OperatorCaller::CommandLine,
            user_id = %user.id,
            changed,
            "an operator made an account an instance operator"
        );
        Ok(OperatorChangeDto {
            account: summary(&user),
            changed,
        })
    }

    #[instrument(name = "operator.revoke", skip_all)]
    async fn revoke(&self, username: String) -> ServiceResult<OperatorChangeDto> {
        let user = self.account(&username).await?;
        let changed = self
            .repos
            .user_repo
            .set_instance_operator(user.id, false)
            .await?;
        info!(
            target: "audit",
            event = "operator.revoked",
            scope = "server",
            actor_id = %OperatorCaller::CommandLine,
            user_id = %user.id,
            changed,
            "an operator took the instance operator role from an account"
        );
        Ok(OperatorChangeDto {
            account: summary(&user),
            changed,
        })
    }

    async fn operators(&self) -> ServiceResult<Vec<AccountSummaryDto>> {
        Ok(self
            .repos
            .user_repo
            .instance_operators()
            .await?
            .iter()
            .map(summary)
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use chrono::Utc;
    use rim_core::auth::repository::MockSessionRepository;
    use rim_core::key_identity::repository::MockKeyIdentityRepository;
    use rim_core::relay::repository::MockRelayAbuseRepository;
    use rim_core::user::model::UserId;
    use rim_core::user::repository::MockUserRepository;

    use super::*;
    use crate::probation_fixtures::account;

    const PRINCIPAL: &str = "rimp1_txDLbnipi5i3kCJtJwBOUixzv_MY8OnU0mcy--Tyje0";

    /// Role changes the user repository received.
    type Changes = Arc<Mutex<Vec<(UserId, bool)>>>;

    /// The service over the one account `user`, the role holder, whose principal is blocked
    /// when `blocked`.
    fn service(user: &User, blocked: bool, changes: &Changes) -> DefaultInstanceOperatorService {
        let mut users = MockUserRepository::new();
        let found = user.clone();
        users
            .expect_find_by_username()
            .returning(move |name| Ok((name == found.username).then(|| found.clone())));
        users.expect_is_email_blocked().returning(|_| Ok(false));
        let holder = user.id;
        users
            .expect_is_instance_operator()
            .returning(move |id| Ok(id == holder));
        let log = Arc::clone(changes);
        users
            .expect_set_instance_operator()
            .returning(move |id, operator| {
                log.lock().unwrap().push((id, operator));
                Ok(true)
            });
        let mut abuse = MockRelayAbuseRepository::new();
        abuse
            .expect_is_principal_blocked()
            .returning(move |_| Box::pin(async move { Ok(blocked) }));
        DefaultInstanceOperatorService::new(RelayAbuseRepos {
            relay_abuse_repo: Arc::new(abuse),
            key_identity_repo: Arc::new(MockKeyIdentityRepository::new()),
            session_repo: Arc::new(MockSessionRepository::new()),
            user_repo: Arc::new(users),
        })
    }

    fn ada() -> User {
        account(UserId::new(), Utc::now(), Some(PRINCIPAL))
    }

    #[tokio::test]
    async fn granting_and_revoking_set_the_role_on_the_named_account() {
        let user = ada();
        let changes = Changes::default();
        let svc = service(&user, false, &changes);

        let granted = svc.grant(user.username.clone()).await.expect("granted");
        assert!(granted.changed);
        assert_eq!(granted.account.user_id, user.id);
        let revoked = svc.revoke(user.username.clone()).await.expect("revoked");
        assert!(revoked.changed);
        assert_eq!(
            *changes.lock().unwrap(),
            vec![(user.id, true), (user.id, false)]
        );
    }

    #[tokio::test]
    async fn a_blocked_account_or_one_scheduled_for_deletion_cannot_be_granted() {
        let changes = Changes::default();
        let blocked = ada();
        assert!(matches!(
            service(&blocked, true, &changes)
                .grant(blocked.username.clone())
                .await,
            Err(ServiceError::Validation(_))
        ));
        let mut departing = ada();
        departing.deletion_scheduled_for = Some(Utc::now());
        assert!(matches!(
            service(&departing, false, &changes)
                .grant(departing.username.clone())
                .await,
            Err(ServiceError::Validation(_))
        ));
        assert!(changes.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn an_unknown_username_is_not_found() {
        let changes = Changes::default();
        let svc = service(&ada(), false, &changes);
        assert!(matches!(
            svc.grant("nobody".to_string()).await,
            Err(ServiceError::NotFound)
        ));
        assert!(matches!(
            svc.revoke("nobody".to_string()).await,
            Err(ServiceError::NotFound)
        ));
    }

    #[tokio::test]
    async fn only_the_role_holder_is_an_operator() {
        let user = ada();
        let svc = service(&user, false, &Changes::default());
        assert!(svc.is_operator(Actor::new(user.id)).await.expect("checked"));
        assert!(
            !svc.is_operator(Actor::new(UserId::new()))
                .await
                .expect("checked")
        );
    }
}
