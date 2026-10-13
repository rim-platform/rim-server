//! Argon2 password hashing and verification off the async workers, bounded in concurrency.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Semaphore;
use zeroize::Zeroizing;

use rim_auth::password::{hash_password, verify_dummy_password, verify_password};
use rim_services::{ServiceError, ServiceResult};

/// Most Argon2 computations one server runs at once (`RIM_PASSWORD_HASH_CONCURRENCY`).
pub const MAX_CONCURRENCY: usize = 64;

/// How long a caller waits for a permit before it is answered `Unavailable`.
const PERMIT_WAIT: Duration = Duration::from_secs(10);

/// Runs password hashing and verification on the blocking pool, at most `concurrency` at once.
///
/// Each computation holds a permit for as long as it runs, even when its caller has gone away,
/// so the cap bounds the CPU and memory Argon2 takes (64 MiB each). A caller that waits longer
/// than [`PERMIT_WAIT`] for a permit gets [`ServiceError::Unavailable`].
#[derive(Clone)]
pub struct PasswordHashing {
    permits: Arc<Semaphore>,
    permit_wait: Duration,
}

impl PasswordHashing {
    /// A hasher running at most `concurrency` computations at once, kept within 1 to
    /// [`MAX_CONCURRENCY`].
    ///
    /// # Examples
    /// ```ignore
    /// let hashing = PasswordHashing::new(4);
    /// let phc = hashing.hash("correct horse").await?;
    /// assert!(hashing.verify("correct horse", &phc).await?);
    /// ```
    pub fn new(concurrency: usize) -> Self {
        Self {
            permits: Arc::new(Semaphore::new(concurrency.clamp(1, MAX_CONCURRENCY))),
            permit_wait: PERMIT_WAIT,
        }
    }

    /// The default concurrency: the number of CPUs, within 1 to [`MAX_CONCURRENCY`].
    pub fn default_concurrency() -> usize {
        std::thread::available_parallelism()
            .map_or(1, usize::from)
            .clamp(1, MAX_CONCURRENCY)
    }

    #[cfg(test)]
    fn with_permit_wait(mut self, permit_wait: Duration) -> Self {
        self.permit_wait = permit_wait;
        self
    }

    /// Hash `password` with [`hash_password`].
    ///
    /// # Errors
    /// `Unavailable` when no permit frees up in time; `Internal` when hashing fails.
    pub async fn hash(&self, password: &str) -> ServiceResult<String> {
        let password = Zeroizing::new(password.to_owned());
        self.run(move || hash_password(&password))
            .await?
            .map_err(ServiceError::internal)
    }

    /// Verify `password` against the PHC string `password_hash` with [`verify_password`].
    ///
    /// # Errors
    /// `Unavailable` when no permit frees up in time; `Internal` when the hash is malformed.
    pub async fn verify(&self, password: &str, password_hash: &str) -> ServiceResult<bool> {
        let password = Zeroizing::new(password.to_owned());
        let password_hash = password_hash.to_owned();
        self.run(move || verify_password(&password, &password_hash))
            .await?
            .map_err(ServiceError::internal)
    }

    /// Verify `password` against a fixed hash with [`verify_dummy_password`], so a missing
    /// account costs the same as a wrong password.
    ///
    /// # Errors
    /// `Unavailable` when no permit frees up in time; `Internal` when the dummy hash fails.
    pub async fn verify_dummy(&self, password: &str) -> ServiceResult<()> {
        let password = Zeroizing::new(password.to_owned());
        self.run(move || verify_dummy_password(&password))
            .await?
            .map_err(ServiceError::internal)
    }

    /// Run `work` on the blocking pool once a permit is free, holding the permit until `work`
    /// returns.
    async fn run<T, F>(&self, work: F) -> ServiceResult<T>
    where
        T: Send + 'static,
        F: FnOnce() -> T + Send + 'static,
    {
        let permit =
            tokio::time::timeout(self.permit_wait, Arc::clone(&self.permits).acquire_owned())
                .await
                .map_err(|_| {
                    tracing::warn!(
                        wait_secs = self.permit_wait.as_secs_f64(),
                        "password hashing is saturated; refusing the request"
                    );
                    ServiceError::Unavailable
                })?
                .map_err(ServiceError::internal)?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            work()
        })
        .await
        .map_err(ServiceError::internal)
    }
}

impl Default for PasswordHashing {
    fn default() -> Self {
        Self::new(Self::default_concurrency())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    /// Counts the computations running at once and the most seen together.
    #[derive(Default)]
    struct Overlap {
        running: AtomicUsize,
        most: AtomicUsize,
    }

    impl Overlap {
        fn during<T>(&self, work: impl FnOnce() -> T) -> T {
            let now = self.running.fetch_add(1, Ordering::SeqCst) + 1;
            self.most.fetch_max(now, Ordering::SeqCst);
            let out = work();
            self.running.fetch_sub(1, Ordering::SeqCst);
            out
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn one_permit_runs_two_verifications_one_after_the_other() {
        let hash = hash_password("correct-horse").expect("hash");
        let hashing = PasswordHashing::new(1);
        let overlap = Arc::new(Overlap::default());
        let verify = |password: &'static str| {
            let (hash, overlap) = (hash.clone(), Arc::clone(&overlap));
            hashing.run(move || overlap.during(|| verify_password(password, &hash)))
        };

        let (right, wrong) = tokio::join!(verify("correct-horse"), verify("battery-staple"));

        assert!(right.expect("permit").expect("verify"));
        assert!(!wrong.expect("permit").expect("verify"));
        assert_eq!(overlap.most.load(Ordering::SeqCst), 1);
        assert_eq!(hashing.permits.available_permits(), 1);
    }

    #[tokio::test]
    async fn hash_and_verify_answer_as_the_synchronous_functions_do() {
        let hashing = PasswordHashing::new(1);
        let phc = hashing.hash("correct-horse").await.expect("hash");

        assert!(hashing.verify("correct-horse", &phc).await.expect("verify"));
        assert!(
            !hashing
                .verify("battery-staple", &phc)
                .await
                .expect("verify")
        );
        assert!(hashing.verify_dummy("anything").await.is_ok());
        assert!(matches!(
            hashing.verify("correct-horse", "not-a-phc-string").await,
            Err(ServiceError::Internal(_))
        ));
        assert!(matches!(
            hashing.hash("short").await,
            Err(ServiceError::Internal(_))
        ));
    }

    #[tokio::test]
    async fn a_caller_without_a_permit_in_time_is_answered_unavailable() {
        let hashing = PasswordHashing::new(1).with_permit_wait(Duration::from_millis(20));
        let held = Arc::clone(&hashing.permits)
            .acquire_owned()
            .await
            .expect("permit");

        let result = hashing.verify_dummy("anything").await;

        assert!(matches!(result, Err(ServiceError::Unavailable)));
        drop(held);
        assert!(hashing.verify_dummy("anything").await.is_ok());
    }

    #[test]
    fn concurrency_is_kept_between_one_and_the_maximum() {
        assert_eq!(PasswordHashing::new(0).permits.available_permits(), 1);
        assert_eq!(
            PasswordHashing::new(1000).permits.available_permits(),
            MAX_CONCURRENCY
        );
        assert!((1..=MAX_CONCURRENCY).contains(&PasswordHashing::default_concurrency()));
    }
}
