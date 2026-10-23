//! The cap on local accounts: every signup path asks it for room inside its transaction, and
//! `serverInfo` and the discovery document ask whether the instance is full.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rim_core::user::repository::UserRepository;
use rim_services::{ServiceError, ServiceResult};

/// How long a "full or not" answer is reused before the accounts are counted again.
const STATUS_TTL: Duration = Duration::from_secs(30);

/// The most local accounts the instance accepts, and the last "full or not" answer. Clones
/// share the answer, so a signup on any path makes the next question count afresh.
#[derive(Clone, Default)]
pub(crate) struct AccountCap {
    cap: Option<u32>,
    status: Arc<Mutex<Option<(Instant, bool)>>>,
}

impl AccountCap {
    /// A cap of `cap` local accounts; `None` accepts any number.
    pub(crate) fn new(cap: Option<u32>) -> Self {
        Self {
            cap,
            status: Arc::default(),
        }
    }

    /// `Ok` when one more local account fits under the cap or there is none. With a cap, takes
    /// the account-creation lock first, so `user_repo` must be the signup's transaction's: the
    /// lock is held until that transaction ends and concurrent signups count one after another.
    ///
    /// # Errors
    /// `RegistrationFull` when the instance holds as many local accounts as the cap allows;
    /// storage errors propagate.
    pub(crate) async fn admit(&self, user_repo: &dyn UserRepository) -> ServiceResult<()> {
        if self.cap.is_none() {
            return Ok(());
        }
        user_repo.lock_account_creation().await?;
        self.check(user_repo).await
    }

    /// Count accounts afresh before signup input is validated, without taking a lock.
    ///
    /// # Errors
    /// `RegistrationFull` at the cap; storage errors propagate.
    pub(crate) async fn check(&self, user_repo: &dyn UserRepository) -> ServiceResult<()> {
        let Some(cap) = self.cap else {
            return Ok(());
        };
        let accounts = user_repo.count_local_accounts(u64::from(cap)).await?;
        if accounts >= u64::from(cap) {
            self.remember(true);
            tracing::info!(
                target: "audit",
                event = "signup_refused_registration_full",
                cap,
                "signup refused: the server holds as many accounts as RIM_LIMIT_ACCOUNTS allows"
            );
            return Err(ServiceError::RegistrationFull);
        }
        Ok(())
    }

    /// Forget the last answer once a signup committed, so the next question counts afresh.
    pub(crate) fn admitted(&self) {
        if let Ok(mut status) = self.status.lock() {
            *status = None;
        }
    }

    /// Whether the instance holds as many local accounts as the cap allows; `false` without a
    /// cap. Reuses an answer younger than half a minute.
    ///
    /// # Errors
    /// Propagates storage errors.
    pub(crate) async fn is_full(&self, user_repo: &dyn UserRepository) -> ServiceResult<bool> {
        let Some(cap) = self.cap else {
            return Ok(false);
        };
        let cached = self
            .status
            .lock()
            .ok()
            .and_then(|status| *status)
            .filter(|(at, _)| at.elapsed() < STATUS_TTL);
        if let Some((_, full)) = cached {
            return Ok(full);
        }
        let full = user_repo.count_local_accounts(u64::from(cap)).await? >= u64::from(cap);
        self.remember(full);
        Ok(full)
    }

    fn remember(&self, full: bool) {
        if let Ok(mut status) = self.status.lock() {
            *status = Some((Instant::now(), full));
        }
    }
}

#[cfg(test)]
mod tests {
    use mockall::Sequence;
    use rim_core::user::repository::MockUserRepository;

    use super::*;

    /// A repository holding `accounts` local accounts that expects the lock before each count.
    fn holding(accounts: u64, counts: usize) -> MockUserRepository {
        let mut repo = MockUserRepository::new();
        let mut seq = Sequence::new();
        for _ in 0..counts {
            repo.expect_lock_account_creation()
                .times(1)
                .in_sequence(&mut seq)
                .returning(|| Ok(()));
            repo.expect_count_local_accounts()
                .times(1)
                .in_sequence(&mut seq)
                .returning(move |at_most| Ok(accounts.min(at_most)));
        }
        repo
    }

    #[tokio::test]
    async fn no_cap_admits_everyone_without_counting_and_is_never_full() {
        let repo = MockUserRepository::new();
        let cap = AccountCap::new(None);
        assert!(cap.check(&repo).await.is_ok());
        assert!(cap.admit(&repo).await.is_ok());
        assert!(!cap.is_full(&repo).await.expect("full"));
    }

    #[tokio::test]
    async fn below_the_cap_admits_after_the_lock_and_at_it_refuses_as_full() {
        let cap = AccountCap::new(Some(3));
        assert!(cap.admit(&holding(2, 1)).await.is_ok());
        assert!(matches!(
            cap.admit(&holding(3, 1)).await,
            Err(ServiceError::RegistrationFull)
        ));
        assert!(matches!(
            cap.admit(&holding(7, 1)).await,
            Err(ServiceError::RegistrationFull)
        ));
    }

    #[tokio::test]
    async fn the_full_answer_is_reused_until_a_signup_is_admitted() {
        let cap = AccountCap::new(Some(2));
        let mut repo = MockUserRepository::new();
        repo.expect_count_local_accounts()
            .times(1)
            .returning(|_| Ok(1));
        assert!(!cap.is_full(&repo).await.expect("full"));
        assert!(!cap.is_full(&repo).await.expect("full"), "reused");

        cap.admitted();
        let mut repo = MockUserRepository::new();
        repo.expect_count_local_accounts()
            .times(1)
            .returning(|_| Ok(2));
        assert!(cap.is_full(&repo).await.expect("full"));

        let shared = cap.clone();
        shared.admitted();
        let mut repo = MockUserRepository::new();
        repo.expect_count_local_accounts()
            .times(1)
            .returning(|_| Ok(1));
        assert!(!cap.is_full(&repo).await.expect("full"), "clones share it");
    }

    #[tokio::test]
    async fn prevalidation_counts_afresh_even_when_discovery_cached_room() {
        let cap = AccountCap::new(Some(2));
        let mut repo = MockUserRepository::new();
        let mut sequence = Sequence::new();
        repo.expect_count_local_accounts()
            .times(1)
            .in_sequence(&mut sequence)
            .returning(|_| Ok(1));
        repo.expect_count_local_accounts()
            .times(1)
            .in_sequence(&mut sequence)
            .returning(|_| Ok(2));
        assert!(!cap.is_full(&repo).await.expect("cached room"));
        assert!(matches!(
            cap.check(&repo).await,
            Err(ServiceError::RegistrationFull)
        ));
        assert!(cap.is_full(&repo).await.expect("full"));
    }

    #[tokio::test]
    async fn a_refusal_marks_the_instance_full() {
        let cap = AccountCap::new(Some(1));
        assert!(cap.admit(&holding(1, 1)).await.is_err());
        let repo = MockUserRepository::new();
        assert!(cap.is_full(&repo).await.expect("full"));
    }
}
