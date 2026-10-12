//! The current time a service reads, fixed in tests.

use std::sync::Arc;

use chrono::{DateTime, Utc};

/// The system clock, or in tests a fixed instant.
#[derive(Clone)]
pub(crate) struct Clock(Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>);

impl Clock {
    pub(crate) fn now(&self) -> DateTime<Utc> {
        (self.0)()
    }

    /// A clock that always reads `at`.
    #[cfg(test)]
    pub(crate) fn fixed(at: DateTime<Utc>) -> Self {
        Self(Arc::new(move || at))
    }
}

impl Default for Clock {
    fn default() -> Self {
        Self(Arc::new(Utc::now))
    }
}
