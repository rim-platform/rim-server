//! What the server's default workspace, which every account joins, asks of the accounts that
//! store things in it: none of its per-workspace caps on what members own, and an account out
//! of probation.

use std::sync::Arc;

use rim_core::server::model::ServerId;
use rim_core::user::model::UserId;
use rim_core::user::repository::UserRepository;
use rim_services::default_workspace::DefaultWorkspaceService;
use rim_services::limits::InstanceLimits;
use rim_services::{ServiceError, ServiceResult};

use crate::clock::Clock;

/// Tells whether a workspace is the default one and holds back accounts in probation there.
#[derive(Clone)]
pub struct DefaultWorkspaceRules {
    default_workspace: Arc<dyn DefaultWorkspaceService>,
    /// Reads an account's age for probation.
    user_repo: Arc<dyn UserRepository>,
    /// The probation of new accounts.
    limits: InstanceLimits,
    clock: Clock,
}

impl DefaultWorkspaceRules {
    pub fn new(
        default_workspace: Arc<dyn DefaultWorkspaceService>,
        user_repo: Arc<dyn UserRepository>,
        limits: InstanceLimits,
    ) -> Self {
        Self {
            default_workspace,
            user_repo,
            limits,
            clock: Clock::default(),
        }
    }

    /// Rules where `default_workspace` is the default one, `accounts` are the users found, at
    /// the fixed instant `now`.
    #[cfg(test)]
    pub(crate) fn testing(
        default_workspace: ServerId,
        accounts: Vec<rim_core::user::model::User>,
        limits: InstanceLimits,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Self {
        let mut service = rim_services::default_workspace::MockDefaultWorkspaceService::new();
        service
            .expect_current()
            .returning(move || Box::pin(async move { Ok(Some(default_workspace)) }));
        Self {
            default_workspace: Arc::new(service),
            user_repo: Arc::new(crate::probation_fixtures::users(accounts)),
            limits,
            clock: Clock::fixed(now),
        }
    }

    /// Whether `server_id` is the server's default workspace now.
    ///
    /// # Errors
    /// Storage errors reading the default workspace.
    pub(crate) async fn is_default(&self, server_id: ServerId) -> ServiceResult<bool> {
        Ok(self.default_workspace.current().await? == Some(server_id))
    }

    /// `Ok` unless `user_id`'s account is in probation (an instance operator never is).
    ///
    /// # Errors
    /// `Validation` saying the account can `action` once probation ends; `NotFound` for an
    /// unknown account; storage errors.
    pub(crate) async fn ensure_out_of_probation(
        &self,
        user_id: UserId,
        action: &str,
    ) -> ServiceResult<()> {
        if self.limits.probation_hours.is_none() {
            return Ok(());
        }
        let user = self
            .user_repo
            .get_by_id(user_id)
            .await?
            .ok_or(ServiceError::NotFound)?;
        crate::probation::ensure_out_of_probation(
            &self.limits,
            self.user_repo.as_ref(),
            &user,
            self.clock.now(),
            action,
        )
        .await
    }
}
