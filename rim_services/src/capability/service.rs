use std::collections::HashMap;

use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;
use rim_core::server::model::ServerId;

use crate::{Actor, ServiceResult};

/// Phase 1 §3.3 — closes Black Widow C1 ("real capability gate from Phase 1,
/// no no-op-allow stub").
///
/// Two-leg gate model:
///
/// 1. **Registry leg** (always evaluated): the capability must exist in the
///    `capability_registry` table AND not be retired. Unknown / retired keys
///    deny by default — eliminates the "type a typo, get a free pass" failure
///    mode Widow named.
/// 2. **Holder leg** (evaluated when leg 1 passes): the actor must be a
///    member of `workspace_id` (when supplied — see below).
///
/// `workspace_id` is `Option<ServerId>` rather than required because some
/// commands (e.g. `auth.who_am_i`) legitimately have no workspace context.
/// When `None`, the holder leg falls back to "actor is authenticated" (which
/// is true by construction — the type system guarantees an `Actor` came from
/// a session-resolved request). Callers with workspace context (i.e. the
/// command service) MUST pass it through so the membership leg actually
/// runs.
///
/// Limitations called out so reviewers don't trust the gate beyond what it
/// does (per design §3.3):
/// - Coarse: workspace-member-AND-known-capability. NO per-actor ACL,
///   role distinctions, or grant revocation. Those land later.
/// - Strict-mode by default: an unknown OR empty capability denies.
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

    /// Bulk variant — used by AI-surface filtering (Phase 4) where the
    /// frontend wants a yes/no map for many capabilities at once. Bulk
    /// short-circuits the registry / membership lookups so each call site
    /// only pays one round-trip's worth of work per distinct workspace.
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
