use std::collections::HashMap;

use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;
use rim_core::server::model::ServerId;

use crate::{Actor, ServiceResult};

/// The capability gate: each of a command's required capabilities passes it before the command
/// runs.
///
/// Two legs:
///
/// 1. **Registry leg** (always evaluated): the capability must exist in the
///    `capability_registry` table and not be retired. Unknown or retired keys deny, so a typo
///    never grants access; an empty key is a validation error.
/// 2. **Holder leg** (evaluated when leg 1 passes): the actor must be a member of
///    `workspace_id`.
///
/// `workspace_id` is optional because self-scoped capabilities (`audit.read.self`) have no
/// workspace context. Without a workspace, a self-scoped capability passes the holder leg and
/// every workspace-scoped capability is denied, so callers with a workspace (the command
/// service) must pass it through.
///
/// The gate is coarse: workspace membership plus a known capability. Roles are enforced by the
/// services behind each command, not here.
#[cfg_attr(any(test, feature = "mocks"), automock)]
#[async_trait]
pub trait CapabilityService: Send + Sync {
    /// Returns `Ok(())` iff the actor is allowed to invoke `capability` in
    /// the optional workspace context. Otherwise `Err(PermissionDenied)`
    /// (or `Validation` for empty-string input).
    async fn check(
        &self,
        actor: &Actor,
        workspace_id: Option<ServerId>,
        capability: &str,
    ) -> ServiceResult<()>;

    /// Bulk variant, used to filter the AI command surface, which needs a yes/no map for many
    /// capabilities at once. The membership lookup runs once per call rather than once per
    /// capability.
    ///
    /// The returned map MUST contain an entry for every input capability
    /// (the bool indicates whether that specific cap is held). Errors
    /// indicate transport / repository failure, not a per-cap denial.
    async fn check_many(
        &self,
        actor: &Actor,
        workspace_id: Option<ServerId>,
        capabilities: &[String],
    ) -> ServiceResult<HashMap<String, bool>>;
}
