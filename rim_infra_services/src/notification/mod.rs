//! Server-side notification rules: the pure decision engine, the
//! `NotificationService` implementation and the digest/retention jobs.

pub(crate) mod decide;
// pub: `DefaultNotificationJobs`/`NotificationJobs` are constructed and driven by the
// composition root (rim_server/src/app.rs).
pub mod digest;
pub(crate) mod service;

pub use digest::{DefaultNotificationJobs, NotificationJobRepos, NotificationJobs};
pub use service::{DefaultNotificationService, NotificationRepos};
