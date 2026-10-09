//! Confirmation-proof primitives for dangerous-command execution (H1).
//!
//! Phase 1 / §3.5 of the AI Agent reframe.
//!
//! ## Why this exists (the H1 closure)
//!
//! Pre-reframe, the only gate on a `dangerous: true` command was a single
//! `confirmed_dangerous: bool` flag carried over the wire. Any caller —
//! agent, plugin, malicious frontend, replayed request — could set it to
//! `true` and bypass the modal entirely. That is a TOCTOU bug in disguise:
//! the user OK'd one set of arguments in the modal, but the eventual
//! execution can run a different set, and the server has no way to tell.
//!
//! This module replaces the boolean with a server-issued, server-verified
//! proof bound to:
//!
//! 1. The **command id** the user actually saw in the modal,
//! 2. The **canonical hash of the arguments** the user actually saw, and
//! 3. The **context fingerprint** (workspace / conversation / thread) so a
//!    proof minted for "delete in DM with Alice" cannot be replayed against
//!    "delete in #engineering".
//!
//! Each token is single-use, has a short TTL (5 s by default), and is
//! evicted from the in-memory store on use or on lazy sweep.
//!
//! ### What this module DOES NOT yet do
//!
//! - **Multi-node deployments.** The store is in-memory; a second backend
//!   replica won't see tokens issued on the first. Phase 1 ships single-node
//!   only — Fury §5 risk #3. Phase 5+ will swap the store for a Redis-backed
//!   impl behind the same trait without changing call sites.
//! - **Out-of-band proofs.** The `OutOfBand` variant is a placeholder
//!   (M5 / MCP forward-compat). `verify_in_band` returns `Unsupported` for
//!   it — the actual MCP capability/consent integration ships with Phase 6.
//! - **Audit emission.** Issue/verify success+failure are surfaced via
//!   structured `tracing` events at this layer, but the dedicated
//!   `target = "audit"` sink does not yet exist (N3 / Phase 4).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Duration, Utc};
use sha2::{Digest, Sha256};

use crate::Actor;
use crate::command::dto::CommandContextDto;

/// Default token lifetime (§3.5 — "5 s").
///
/// Short by design: the token only needs to survive the round-trip from
/// challenge → modal click → second mutation. A user staring at the modal
/// long enough for this to expire is almost certainly an attacker replaying
/// a proof scraped from logs or memory.
pub const DEFAULT_CONFIRMATION_TTL_SECS: i64 = 5;

/// Maximum token lifetime accepted by `issue_in_band` regardless of caller
/// preference. Defends against an upstream config bug minting tokens that
/// outlive the user's intent. 60 s is generous; bump only with security
/// review.
pub const MAX_CONFIRMATION_TTL_SECS: i64 = 60;

// ---------------------------------------------------------------------------
// Public types — wire surface
// ---------------------------------------------------------------------------

/// A proof submitted by a caller to attempt execution of a dangerous command.
///
/// `InBandToken` is the standard flow: the server previously issued
/// `token` along with `args_hash`, and the caller now submits both.
///
/// `OutOfBand` is reserved for MCP-style flows where the user's consent
/// arrives via a side channel (e.g. an MCP capability grant). Phase 1 only
/// records the request-id; verification returns `Unsupported`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfirmationProof {
    InBandToken {
        token: String,
        /// Hash of the args the caller _claims_ the user OK'd. The server
        /// recomputes this from the args actually passed to `execute_command`
        /// — mismatch ⇒ TOCTOU attempt ⇒ rejection.
        args_hash: String,
    },
    OutOfBand {
        request_id: String,
    },
}

/// A challenge issued to the caller when a dangerous command needs
/// confirmation. The frontend presents this to the user and re-submits the
/// command together with a matching [`ConfirmationProof::InBandToken`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmationChallengeDto {
    pub token: String,
    pub args_hash: String,
    pub expires_at: DateTime<Utc>,
}

/// High-level outcome of the confirmation gate, surfaced by
/// `execute_command` so the caller knows whether to retry with a proof.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfirmationFlowDto {
    /// Command was executed (either non-dangerous, or proof verified).
    NotRequired,
    /// Caller must re-submit with [`ConfirmationProof::InBandToken`] built
    /// from these challenge parameters.
    ConfirmationRequired(ConfirmationChallengeDto),
    /// Caller submitted a proof but it was expired, single-use-consumed,
    /// args-mismatched, or context-mismatched — re-issue and retry.
    ConfirmationStale,
}

/// Failure modes for `verify_in_band`. Mapped to `ServiceError` at the
/// service-orchestration layer.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConfirmationError {
    #[error("token unknown or already consumed")]
    UnknownToken,
    #[error("token expired")]
    Expired,
    #[error("args hash mismatch (TOCTOU defense)")]
    ArgsMismatch,
    #[error("command id mismatch — proof was issued for a different command")]
    CommandMismatch,
    #[error("context mismatch — proof was issued in a different workspace/conversation/thread")]
    ContextMismatch,
    #[error("actor mismatch — proof was issued for a different user")]
    ActorMismatch,
    #[error("out-of-band proofs are not supported in Phase 1")]
    Unsupported,
}

impl ConfirmationError {
    /// Returns `true` if the failure is a "stale proof" condition — the
    /// caller can recover by re-issuing and retrying. False for
    /// fundamentally-wrong-proof conditions (command mismatch, context
    /// mismatch, actor mismatch) which indicate misuse or attack.
    pub fn is_stale(&self) -> bool {
        matches!(
            self,
            Self::UnknownToken | Self::Expired | Self::ArgsMismatch
        )
    }
}

// ---------------------------------------------------------------------------
// Internal claim record — lives only in the store
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct IssuedClaims {
    actor: Actor,
    command_id: String,
    args_hash: String,
    context_fingerprint: String,
    expires_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Store trait + in-memory impl
// ---------------------------------------------------------------------------

/// Trait for the confirmation token store. Phase 1 ships
/// [`InMemoryConfirmationStore`]; Phase 5+ may add a Redis-backed impl
/// without touching call sites.
///
/// Implementations MUST be `Send + Sync` (shared across request handlers).
pub trait ConfirmationStore: Send + Sync + std::fmt::Debug {
    /// Stores a freshly-minted claim under `token`. Implementations may
    /// silently overwrite a colliding token (collisions are
    /// cryptographically improbable; explicit replacement is safer than
    /// erroring under load).
    fn put(&self, token: String, claims: ClaimRecord);

    /// Atomically removes the claim for `token` and returns it. Returns
    /// `None` if the token is unknown or already consumed.
    ///
    /// Single-use is enforced here, not in the verify layer, because the
    /// store is the only component that can do it atomically across
    /// concurrent verify calls.
    fn take(&self, token: &str) -> Option<ClaimRecord>;
}

/// Public projection of [`IssuedClaims`] — the trait surface needs a
/// concrete type but we don't want callers building claims by hand. Always
/// construct via [`issue_in_band`].
#[derive(Debug, Clone)]
pub struct ClaimRecord(IssuedClaims);

#[derive(Debug, Default)]
pub struct InMemoryConfirmationStore {
    inner: Mutex<HashMap<String, IssuedClaims>>,
}

impl InMemoryConfirmationStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Lazy-eviction helper: drops every claim with `expires_at <= now`.
    /// Called by `take` on every access; an explicit background sweep is
    /// not needed for Phase 1 (token volume is tiny).
    fn sweep_expired_locked(map: &mut HashMap<String, IssuedClaims>, now: DateTime<Utc>) {
        map.retain(|_, claims| claims.expires_at > now);
    }

    /// Number of *non-expired* claims currently held. Sweeps lazily before
    /// counting so the result reflects observable state (a freshly-expired
    /// token is reported as gone). Intended for **test introspection** —
    /// production callers MUST go through [`ConfirmationStore::take`] to
    /// preserve single-use semantics.
    pub fn len(&self) -> usize {
        let mut guard = match self.inner.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        Self::sweep_expired_locked(&mut guard, Utc::now());
        guard.len()
    }

    /// Convenience for assertions like `assert!(store.is_empty())`.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl ConfirmationStore for InMemoryConfirmationStore {
    fn put(&self, token: String, claims: ClaimRecord) {
        let mut guard = match self.inner.lock() {
            Ok(g) => g,
            // Mutex poisoned ⇒ a previous holder panicked. We recover the
            // guard rather than propagate, because losing the entire token
            // store on a single bad insert would block every dangerous
            // command site-wide. The poisoned data is still valid; we just
            // continue.
            Err(poisoned) => poisoned.into_inner(),
        };
        Self::sweep_expired_locked(&mut guard, Utc::now());
        guard.insert(token, claims.0);
    }

    fn take(&self, token: &str) -> Option<ClaimRecord> {
        let mut guard = match self.inner.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        let now = Utc::now();
        Self::sweep_expired_locked(&mut guard, now);
        guard.remove(token).map(ClaimRecord)
    }
}

// ---------------------------------------------------------------------------
// Hashing — canonical args + context fingerprints
// ---------------------------------------------------------------------------

/// Deterministic SHA-256 hex over the canonical JSON form of `args`.
///
/// Canonicalization is critical: `{"a":1,"b":2}` and `{"b":2,"a":1}` are
/// the same value but `serde_json::to_string` does not guarantee key order.
/// We sort object keys recursively before serializing so the hash is stable
/// across serializer versions and platforms.
pub fn canonical_args_hash(args: &Option<serde_json::Value>) -> String {
    let canonical = match args {
        None => serde_json::Value::Null,
        Some(v) => canonicalize(v),
    };
    let serialized = serde_json::to_string(&canonical)
        // Canonicalize never produces a non-serializable value (it only
        // sorts keys / preserves structure), so this `unwrap_or_default`
        // is unreachable in practice. Defensive default keeps us out of
        // panic territory per the no-unwrap rule.
        .unwrap_or_default();
    let mut hasher = Sha256::new();
    hasher.update(serialized.as_bytes());
    hex(&hasher.finalize())
}

fn canonicalize(v: &serde_json::Value) -> serde_json::Value {
    use serde_json::Value;
    match v {
        Value::Object(map) => {
            let mut entries: Vec<(&String, &Value)> = map.iter().collect();
            entries.sort_by(|a, b| a.0.cmp(b.0));
            let mut out = serde_json::Map::with_capacity(entries.len());
            for (k, val) in entries {
                out.insert(k.clone(), canonicalize(val));
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(canonicalize).collect()),
        Value::Number(n) => canonicalize_number(n),
        other => other.clone(),
    }
}

/// Number normalization (Widow §B2 closure).
///
/// `serde_json::Number` is internally tagged `PosInt | NegInt | Float`;
/// `1` and `1.0` are distinct variants and serialize to distinct strings.
/// Without normalization a caller could issue with `{"limit": 100}` and
/// re-submit verify with `{"limit": 100.0}`: hashes differ, verify
/// rejects with `ArgsMismatch`. The reverse — issue with `100.0`,
/// re-submit with `100` — would be **silently coerced** by downstream
/// `as_i64()` paths in builtin executors, so the user OK'd a float and
/// the server runs an integer.
///
/// Normalization rule:
/// - Float with `fract() == 0` AND fits `i64` → coerce to integer.
/// - Float NaN / ±Inf → cannot occur (serde_json rejects at parse).
/// - All other floats → stringify with `{:?}` (Rust's debug-float
///   format), which is the only stable, lossless f64 → string mapping
///   short of pulling a `ryu` crate. Wrapped in `Value::String` so two
///   different f64 bit-patterns producing the same `{:?}` output also
///   normalize identically across issue and verify.
/// - Integers → preserved as-is.
fn canonicalize_number(n: &serde_json::Number) -> serde_json::Value {
    use serde_json::Value;
    if let Some(i) = n.as_i64() {
        return Value::Number(i.into());
    }
    if let Some(u) = n.as_u64() {
        return Value::Number(u.into());
    }
    if let Some(f) = n.as_f64() {
        if f.is_finite() && f.fract() == 0.0 && f >= i64::MIN as f64 && f <= i64::MAX as f64 {
            // Round-tripped via i64 — the integer-equivalent canonical form.
            return Value::Number((f as i64).into());
        }
        // Lossless f64 string repr; not parseable back as a JSON number
        // but we never re-parse — only re-hash. Wrapping as String is
        // safe because this canonical form never leaves the hasher.
        return Value::String(format!("__f64__{f:?}"));
    }
    // Unreachable in practice (serde_json::Number is exhaustively i64/u64/f64).
    Value::Null
}

/// Deterministic fingerprint of the execution context.
///
/// **v2 (Widow §B1 closure):** includes `selected_message_id` and
/// `selected_message_ids` because dangerous built-ins like
/// `chat.delete_message` and `chat.edit_message` consume the target
/// message id from `context.selected_message_id` (NOT from `args`). If
/// these were excluded from the fingerprint, an attacker who held the
/// issued token could swap `selected_message_id` between issue and
/// verify and delete a different message than the one the user OK'd —
/// the worked example from the design doc.
///
/// `selected_text`, `focused_surface`, and `active_panel` remain
/// excluded: they're presentation-layer state that no dispatchable
/// command consumes for security-relevant decisions. Adding any new
/// command that does should re-bump the prefix to `v3|` and update this
/// docstring + the corresponding test.
pub fn context_fingerprint(ctx: &Option<CommandContextDto>) -> String {
    let mut hasher = Sha256::new();
    match ctx {
        None => hasher.update(b"none"),
        Some(c) => {
            hasher.update(b"v2|");
            hasher.update(opt_to_bytes(c.workspace_id.as_ref().map(|w| w.to_string())));
            hasher.update(b"|");
            hasher.update(opt_to_bytes(
                c.conversation_id.as_ref().map(|c| c.to_string()),
            ));
            hasher.update(b"|");
            hasher.update(opt_to_bytes(c.thread_id.as_ref().map(|t| t.to_string())));
            hasher.update(b"|");
            hasher.update(opt_to_bytes(
                c.selected_message_id.as_ref().map(|m| m.to_string()),
            ));
            hasher.update(b"|");
            // Order-stable list — sort first so client-side reordering
            // doesn't trip the fingerprint.
            let mut ids: Vec<String> = c
                .selected_message_ids
                .iter()
                .map(|m| m.to_string())
                .collect();
            ids.sort();
            for id in ids {
                hasher.update(id.as_bytes());
                hasher.update(b",");
            }
        }
    }
    hex(&hasher.finalize())
}

fn opt_to_bytes(s: Option<String>) -> Vec<u8> {
    match s {
        Some(v) => v.into_bytes(),
        None => b"-".to_vec(),
    }
}

fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

// ---------------------------------------------------------------------------
// Issue / verify — the core API
// ---------------------------------------------------------------------------

/// Issues a fresh in-band confirmation token bound to
/// `(actor, command_id, args, context)` and stores it.
///
/// Returns the [`ConfirmationChallengeDto`] the caller must hand to the
/// frontend. The frontend presents the modal, then re-issues the original
/// mutation with `ConfirmationProof::InBandToken { token, args_hash }`.
pub fn issue_in_band(
    store: &dyn ConfirmationStore,
    actor: &Actor,
    command_id: &str,
    args: &Option<serde_json::Value>,
    context: &Option<CommandContextDto>,
) -> ConfirmationChallengeDto {
    issue_in_band_with_ttl(
        store,
        actor,
        command_id,
        args,
        context,
        Duration::seconds(DEFAULT_CONFIRMATION_TTL_SECS),
    )
}

/// Same as [`issue_in_band`] but with an explicit TTL — only intended for
/// tests that need a sub-second TTL to exercise expiry paths. TTL is
/// clamped to [`MAX_CONFIRMATION_TTL_SECS`] regardless.
pub fn issue_in_band_with_ttl(
    store: &dyn ConfirmationStore,
    actor: &Actor,
    command_id: &str,
    args: &Option<serde_json::Value>,
    context: &Option<CommandContextDto>,
    ttl: Duration,
) -> ConfirmationChallengeDto {
    let max = Duration::seconds(MAX_CONFIRMATION_TTL_SECS);
    let effective_ttl = if ttl > max { max } else { ttl };
    let now = Utc::now();
    let expires_at = now + effective_ttl;

    let token = uuid::Uuid::new_v4().to_string();
    let args_hash = canonical_args_hash(args);
    let context_fingerprint = context_fingerprint(context);

    store.put(
        token.clone(),
        ClaimRecord(IssuedClaims {
            actor: *actor,
            command_id: command_id.to_string(),
            args_hash: args_hash.clone(),
            context_fingerprint,
            expires_at,
        }),
    );

    ConfirmationChallengeDto {
        token,
        args_hash,
        expires_at,
    }
}

/// Verifies that `proof` was issued for this exact
/// `(actor, command_id, args, context)` tuple, is still within TTL, and
/// has not been consumed.
///
/// **Single-use semantics:** for `InBandToken` proofs, `store.take(...)`
/// is called BEFORE any other check, so the token is consumed even if
/// verification subsequently fails on args / command / context / actor /
/// expiry. This defeats probe attacks that try to enumerate token
/// validity without spending it.
///
/// `OutOfBand` proofs short-circuit at the top with `Unsupported` — no
/// token exists to consume because none was ever issued (the
/// `OutOfBand` flow doesn't go through `issue_in_band`). Phase 6 / MCP
/// will add a separate `verify_out_of_band` entry point with its own
/// store backing.
pub fn verify_in_band(
    store: &dyn ConfirmationStore,
    proof: &ConfirmationProof,
    actor: &Actor,
    command_id: &str,
    args: &Option<serde_json::Value>,
    context: &Option<CommandContextDto>,
) -> Result<(), ConfirmationError> {
    let (token, claimed_args_hash) = match proof {
        ConfirmationProof::InBandToken { token, args_hash } => (token, args_hash),
        ConfirmationProof::OutOfBand { .. } => return Err(ConfirmationError::Unsupported),
    };

    let claims = store.take(token).ok_or(ConfirmationError::UnknownToken)?;
    let claims = claims.0;

    if Utc::now() >= claims.expires_at {
        return Err(ConfirmationError::Expired);
    }

    // Recompute args_hash from the *actual* args being executed. This is
    // the TOCTOU closure: a caller can lie about `claimed_args_hash` all
    // day, but it must match what we recompute.
    let actual_args_hash = canonical_args_hash(args);
    if actual_args_hash != claims.args_hash || claimed_args_hash != &claims.args_hash {
        return Err(ConfirmationError::ArgsMismatch);
    }

    if claims.command_id != command_id {
        return Err(ConfirmationError::CommandMismatch);
    }

    let actual_ctx_fp = context_fingerprint(context);
    if actual_ctx_fp != claims.context_fingerprint {
        return Err(ConfirmationError::ContextMismatch);
    }

    if claims.actor != *actor {
        return Err(ConfirmationError::ActorMismatch);
    }

    Ok(())
}

/// Convenience: shareable handle to an in-memory store, the shape held by
/// the service provider.
pub fn shared_in_memory_store() -> Arc<dyn ConfirmationStore> {
    Arc::new(InMemoryConfirmationStore::new())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use rim_core::conversation::model::ConversationId;
    use rim_core::principal::model::Principal;
    use rim_core::server::model::ServerId;
    use rim_core::user::model::UserId;
    use serde_json::json;
    use std::thread::sleep;
    use std::time::Duration as StdDuration;

    fn actor() -> Actor {
        let user_id = UserId::new();
        Actor {
            principal: Principal::user(user_id),
            user_id,
        }
    }

    fn ctx_in_workspace_and_convo() -> Option<CommandContextDto> {
        Some(CommandContextDto {
            workspace_id: Some(ServerId::new()),
            conversation_id: Some(ConversationId::new()),
            thread_id: None,
            selected_text: None,
            selected_message_id: None,
            selected_message_ids: vec![],
            focused_surface: None,
            active_panel: None,
        })
    }

    #[test]
    fn round_trip_issue_then_verify_succeeds() {
        let store = InMemoryConfirmationStore::new();
        let actor = actor();
        let args = Some(json!({"target_id": "msg_42", "force": true}));
        let ctx = ctx_in_workspace_and_convo();

        let challenge = issue_in_band(&store, &actor, "chat.delete_message", &args, &ctx);
        let proof = ConfirmationProof::InBandToken {
            token: challenge.token.clone(),
            args_hash: challenge.args_hash.clone(),
        };
        let result = verify_in_band(&store, &proof, &actor, "chat.delete_message", &args, &ctx);
        assert!(result.is_ok(), "expected Ok, got {result:?}");
    }

    #[test]
    fn verify_consumes_token_single_use() {
        let store = InMemoryConfirmationStore::new();
        let actor = actor();
        let args = Some(json!({"x": 1}));
        let ctx = None;

        let challenge = issue_in_band(&store, &actor, "test.cmd", &args, &ctx);
        let proof = ConfirmationProof::InBandToken {
            token: challenge.token.clone(),
            args_hash: challenge.args_hash.clone(),
        };

        assert!(verify_in_band(&store, &proof, &actor, "test.cmd", &args, &ctx).is_ok());
        assert_eq!(
            verify_in_band(&store, &proof, &actor, "test.cmd", &args, &ctx),
            Err(ConfirmationError::UnknownToken),
            "second verify must fail — single-use",
        );
    }

    #[test]
    fn ttl_expiry_yields_expired() {
        let store = InMemoryConfirmationStore::new();
        let actor = actor();
        let args = None;
        let ctx = None;

        // 1ms TTL — short enough to expire while we sleep, but
        // representable as chrono::Duration.
        let challenge = issue_in_band_with_ttl(
            &store,
            &actor,
            "test.cmd",
            &args,
            &ctx,
            Duration::milliseconds(1),
        );
        sleep(StdDuration::from_millis(20));

        let proof = ConfirmationProof::InBandToken {
            token: challenge.token,
            args_hash: challenge.args_hash,
        };
        // Either Expired (token still in map but past expires_at) or
        // UnknownToken (lazy sweep already evicted it on the next access).
        // Both are acceptable observable behaviors of a TTL'd store.
        let err = verify_in_band(&store, &proof, &actor, "test.cmd", &args, &ctx).unwrap_err();
        assert!(
            matches!(
                err,
                ConfirmationError::Expired | ConfirmationError::UnknownToken
            ),
            "expected Expired or UnknownToken, got {err:?}",
        );
        assert!(err.is_stale(), "TTL-expiry must be classified as stale");
    }

    #[test]
    fn args_mismatch_rejects_toctou_attempt() {
        // The core H1 closure: a caller mints a token for args A but
        // submits args B at execution time. Verify must reject.
        let store = InMemoryConfirmationStore::new();
        let actor = actor();
        let args_a = Some(json!({"target_id": "msg_alpha"}));
        let args_b = Some(json!({"target_id": "msg_BRAVO"}));
        let ctx = ctx_in_workspace_and_convo();

        let challenge = issue_in_band(&store, &actor, "chat.delete_message", &args_a, &ctx);
        let proof = ConfirmationProof::InBandToken {
            token: challenge.token,
            args_hash: challenge.args_hash, // hash of args_a, but we'll execute with args_b
        };

        let err = verify_in_band(&store, &proof, &actor, "chat.delete_message", &args_b, &ctx)
            .unwrap_err();
        assert_eq!(err, ConfirmationError::ArgsMismatch);
    }

    #[test]
    fn args_hash_canonical_across_key_order() {
        // {"a":1,"b":2} and {"b":2,"a":1} must hash identically.
        let h1 = canonical_args_hash(&Some(json!({"a": 1, "b": 2})));
        let h2 = canonical_args_hash(&Some(json!({"b": 2, "a": 1})));
        assert_eq!(h1, h2);
        // But different values must differ.
        let h3 = canonical_args_hash(&Some(json!({"a": 1, "b": 3})));
        assert_ne!(h1, h3);
        // None and Null hash identically (both serialize to "null").
        let h4 = canonical_args_hash(&None);
        let h5 = canonical_args_hash(&Some(serde_json::Value::Null));
        assert_eq!(h4, h5);
    }

    /// **N2 — mandatory cross-command-replay test (Widow §7.5).**
    /// A token issued for `chat.delete_message` must NOT verify against
    /// `chat.send_message`, even with identical args + context + actor.
    #[test]
    fn n2_token_for_one_command_cannot_replay_against_another() {
        let store = InMemoryConfirmationStore::new();
        let actor = actor();
        // Identical args — the only thing that should distinguish the two
        // calls is the command_id.
        let args = Some(json!({"id": "shared_id"}));
        let ctx = ctx_in_workspace_and_convo();

        let challenge = issue_in_band(&store, &actor, "chat.delete_message", &args, &ctx);
        let proof = ConfirmationProof::InBandToken {
            token: challenge.token,
            args_hash: challenge.args_hash,
        };

        let err =
            verify_in_band(&store, &proof, &actor, "chat.send_message", &args, &ctx).unwrap_err();
        assert_eq!(
            err,
            ConfirmationError::CommandMismatch,
            "a token minted for one command MUST NOT verify against another",
        );
    }

    #[test]
    fn context_mismatch_rejects_cross_workspace_replay() {
        // Token minted in workspace A cannot be used to confirm a delete
        // in workspace B, even with identical args + command + actor.
        let store = InMemoryConfirmationStore::new();
        let actor = actor();
        let args = Some(json!({"id": "x"}));
        let ctx_a = ctx_in_workspace_and_convo();
        let ctx_b = ctx_in_workspace_and_convo(); // different ServerId

        let challenge = issue_in_band(&store, &actor, "chat.delete_message", &args, &ctx_a);
        let proof = ConfirmationProof::InBandToken {
            token: challenge.token,
            args_hash: challenge.args_hash,
        };
        let err = verify_in_band(&store, &proof, &actor, "chat.delete_message", &args, &ctx_b)
            .unwrap_err();
        assert_eq!(err, ConfirmationError::ContextMismatch);
    }

    #[test]
    fn actor_mismatch_rejects_proof_from_another_user() {
        let store = InMemoryConfirmationStore::new();
        let actor_a = actor();
        let actor_b = actor();
        let args = None;
        let ctx = None;

        let challenge = issue_in_band(&store, &actor_a, "test.cmd", &args, &ctx);
        let proof = ConfirmationProof::InBandToken {
            token: challenge.token,
            args_hash: challenge.args_hash,
        };
        let err = verify_in_band(&store, &proof, &actor_b, "test.cmd", &args, &ctx).unwrap_err();
        assert_eq!(err, ConfirmationError::ActorMismatch);
    }

    #[test]
    fn out_of_band_proof_returns_unsupported_in_phase1() {
        let store = InMemoryConfirmationStore::new();
        let actor = actor();
        let proof = ConfirmationProof::OutOfBand {
            request_id: "mcp_grant_123".to_string(),
        };
        let err = verify_in_band(&store, &proof, &actor, "test.cmd", &None, &None).unwrap_err();
        assert_eq!(err, ConfirmationError::Unsupported);
        assert!(
            !err.is_stale(),
            "Unsupported is fundamental, not stale — caller cannot recover by retry",
        );
    }

    #[test]
    fn unknown_token_rejected() {
        let store = InMemoryConfirmationStore::new();
        let actor = actor();
        let proof = ConfirmationProof::InBandToken {
            token: "i-was-never-issued".to_string(),
            args_hash: canonical_args_hash(&None),
        };
        let err = verify_in_band(&store, &proof, &actor, "test.cmd", &None, &None).unwrap_err();
        assert_eq!(err, ConfirmationError::UnknownToken);
        assert!(err.is_stale());
    }

    #[test]
    fn caller_supplied_args_hash_is_also_compared() {
        // Defense-in-depth: even if the server's recompute of args_hash
        // matches the stored hash, a caller submitting a wrong
        // claimed_args_hash should still fail. This catches
        // implementation drift where someone bypasses the recompute.
        let store = InMemoryConfirmationStore::new();
        let actor = actor();
        let args = Some(json!({"x": 1}));

        let challenge = issue_in_band(&store, &actor, "test.cmd", &args, &None);
        let proof = ConfirmationProof::InBandToken {
            token: challenge.token,
            args_hash: "deadbeef".to_string(), // wrong claimed hash
        };
        let err = verify_in_band(&store, &proof, &actor, "test.cmd", &args, &None).unwrap_err();
        assert_eq!(err, ConfirmationError::ArgsMismatch);
        assert!(
            err.is_stale(),
            "args_hash mismatch (caller-claimed) should be classified stale so the caller can recover",
        );
    }

    #[test]
    fn confirmation_error_is_stale_classification() {
        assert!(ConfirmationError::UnknownToken.is_stale());
        assert!(ConfirmationError::Expired.is_stale());
        assert!(ConfirmationError::ArgsMismatch.is_stale());
        assert!(!ConfirmationError::CommandMismatch.is_stale());
        assert!(!ConfirmationError::ContextMismatch.is_stale());
        assert!(!ConfirmationError::ActorMismatch.is_stale());
        assert!(!ConfirmationError::Unsupported.is_stale());
    }

    #[test]
    fn challenge_expires_at_is_in_future() {
        let store = InMemoryConfirmationStore::new();
        let actor = actor();
        let before = Utc::now();
        let challenge = issue_in_band(&store, &actor, "test.cmd", &None, &None);
        assert!(challenge.expires_at > before);
        // Should be ≤ DEFAULT_TTL after `before`.
        let max_expected =
            before + Duration::seconds(DEFAULT_CONFIRMATION_TTL_SECS) + Duration::seconds(1);
        assert!(challenge.expires_at <= max_expected);
    }

    /// **Widow §B1 closure** — context_fingerprint must include
    /// `selected_message_id`. `chat.delete_message` and
    /// `chat.edit_message` consume this from context, NOT args. Without
    /// this binding, an attacker holding a token could swap the target
    /// message between issue and verify.
    #[test]
    fn b1_selected_message_id_swap_rejected() {
        let store = InMemoryConfirmationStore::new();
        let actor = actor();
        let workspace_id = ServerId::new();
        let conv_id = ConversationId::new();
        let make_ctx = |msg_id| {
            Some(CommandContextDto {
                workspace_id: Some(workspace_id),
                conversation_id: Some(conv_id),
                thread_id: None,
                selected_text: None,
                selected_message_id: Some(msg_id),
                selected_message_ids: vec![],
                focused_surface: None,
                active_panel: None,
            })
        };
        let msg_a = rim_core::message::model::MessageId::new();
        let msg_b = rim_core::message::model::MessageId::new();
        let ctx_a = make_ctx(msg_a);
        let ctx_b = make_ctx(msg_b);

        // Issue with selected message A — execute against selected message B.
        // Args, command, workspace, conversation, actor are all identical.
        let challenge = issue_in_band(&store, &actor, "chat.delete_message", &None, &ctx_a);
        let proof = ConfirmationProof::InBandToken {
            token: challenge.token,
            args_hash: challenge.args_hash,
        };
        let err = verify_in_band(&store, &proof, &actor, "chat.delete_message", &None, &ctx_b)
            .unwrap_err();
        assert_eq!(
            err,
            ConfirmationError::ContextMismatch,
            "swapping selected_message_id MUST invalidate the proof — would otherwise let an attacker delete a message the user did not OK",
        );
    }

    /// **Widow §B1 closure (multi-select)** — same defense for
    /// `selected_message_ids` (used by bulk operations like
    /// `chat.delete_messages`).
    #[test]
    fn b1_selected_message_ids_swap_rejected() {
        let store = InMemoryConfirmationStore::new();
        let actor = actor();
        let workspace_id = ServerId::new();
        let conv_id = ConversationId::new();
        let make_ctx = |ids: Vec<rim_core::message::model::MessageId>| {
            Some(CommandContextDto {
                workspace_id: Some(workspace_id),
                conversation_id: Some(conv_id),
                thread_id: None,
                selected_text: None,
                selected_message_id: None,
                selected_message_ids: ids,
                focused_surface: None,
                active_panel: None,
            })
        };
        let id_a = rim_core::message::model::MessageId::new();
        let id_b = rim_core::message::model::MessageId::new();
        let id_c = rim_core::message::model::MessageId::new();

        // Issue with [a, b] — execute with [a, c] (one swapped).
        let ctx_orig = make_ctx(vec![id_a, id_b]);
        let ctx_swapped = make_ctx(vec![id_a, id_c]);

        let challenge = issue_in_band(&store, &actor, "chat.bulk.delete", &None, &ctx_orig);
        let proof = ConfirmationProof::InBandToken {
            token: challenge.token,
            args_hash: challenge.args_hash,
        };
        let err = verify_in_band(
            &store,
            &proof,
            &actor,
            "chat.bulk.delete",
            &None,
            &ctx_swapped,
        )
        .unwrap_err();
        assert_eq!(err, ConfirmationError::ContextMismatch);
    }

    /// **Widow §B1 closure (order-stable)** — re-issuing with the same
    /// set of selected_message_ids in different order MUST verify
    /// (legitimate UX: drag-reorder a multi-select). Otherwise the
    /// fingerprint would reject benign client-side reordering.
    #[test]
    fn b1_selected_message_ids_order_stable() {
        let store = InMemoryConfirmationStore::new();
        let actor = actor();
        let id_a = rim_core::message::model::MessageId::new();
        let id_b = rim_core::message::model::MessageId::new();
        let make_ctx = |ids: Vec<rim_core::message::model::MessageId>| {
            Some(CommandContextDto {
                workspace_id: None,
                conversation_id: None,
                thread_id: None,
                selected_text: None,
                selected_message_id: None,
                selected_message_ids: ids,
                focused_surface: None,
                active_panel: None,
            })
        };
        let challenge = issue_in_band(&store, &actor, "x", &None, &make_ctx(vec![id_a, id_b]));
        let proof = ConfirmationProof::InBandToken {
            token: challenge.token,
            args_hash: challenge.args_hash,
        };
        let result = verify_in_band(
            &store,
            &proof,
            &actor,
            "x",
            &None,
            &make_ctx(vec![id_b, id_a]),
        );
        assert!(
            result.is_ok(),
            "selected_message_ids should be order-stable — got {result:?}",
        );
    }

    /// **Widow §B2 closure** — `{"limit": 100}` and `{"limit": 100.0}`
    /// must hash identically. Without integer-equivalent normalization
    /// a downstream `as_i64()` coercion silently lets the attacker
    /// downgrade a user-confirmed `100.0` to a runtime `100` (or vice
    /// versa) without the gate noticing.
    #[test]
    fn b2_int_and_int_valued_float_normalize_identically() {
        let int_args = Some(json!({"limit": 100}));
        let float_args = Some(json!({"limit": 100.0}));
        let h1 = canonical_args_hash(&int_args);
        let h2 = canonical_args_hash(&float_args);
        assert_eq!(
            h1, h2,
            "int 100 and int-valued float 100.0 MUST canonicalize to the same hash",
        );
    }

    /// **Widow §B2 closure (negative case)** — actual fractional floats
    /// must NOT collapse onto integers (would erase legitimate
    /// precision). 100.5 stays distinct from 100.
    #[test]
    fn b2_fractional_float_does_not_collapse_to_int() {
        let int_args = Some(json!({"limit": 100}));
        let float_args = Some(json!({"limit": 100.5}));
        assert_ne!(
            canonical_args_hash(&int_args),
            canonical_args_hash(&float_args),
            "100 and 100.5 must hash differently — collapsing fractional floats would erase precision",
        );
    }

    /// **Widow §B2 closure (TOCTOU integration)** — issue with int args,
    /// verify with int-valued-float args. The downstream coercion would
    /// otherwise silently apply the wrong value.
    #[test]
    fn b2_toctou_int_vs_int_valued_float_verify_succeeds() {
        let store = InMemoryConfirmationStore::new();
        let actor = actor();
        let int_args = Some(json!({"limit": 100, "id": 1}));
        let float_args = Some(json!({"limit": 100.0, "id": 1.0}));

        let challenge = issue_in_band(&store, &actor, "x", &int_args, &None);
        let proof = ConfirmationProof::InBandToken {
            token: challenge.token,
            args_hash: challenge.args_hash,
        };
        // Caller submits int-valued-floats at verify time — must succeed
        // because we normalize.
        let result = verify_in_band(&store, &proof, &actor, "x", &float_args, &None);
        assert!(
            result.is_ok(),
            "int-equivalent float at verify must accept (canonical normalization); got {result:?}",
        );
    }

    /// **Widow §B2 closure (very-large i64)** — preserved exactly even
    /// when serde_json parses identical-looking literals as distinct
    /// internal variants.
    #[test]
    fn b2_large_integers_preserved() {
        let h_pos = canonical_args_hash(&Some(json!({"n": i64::MAX})));
        let h_neg = canonical_args_hash(&Some(json!({"n": i64::MIN})));
        assert_ne!(h_pos, h_neg);
        // Re-canonicalizing the same value yields the same hash.
        assert_eq!(h_pos, canonical_args_hash(&Some(json!({"n": i64::MAX}))));
    }

    #[test]
    fn ttl_clamped_to_max() {
        let store = InMemoryConfirmationStore::new();
        let actor = actor();
        let now_before = Utc::now();
        let challenge = issue_in_band_with_ttl(
            &store,
            &actor,
            "test.cmd",
            &None,
            &None,
            Duration::hours(24), // way past MAX_CONFIRMATION_TTL_SECS
        );
        let max_ceiling =
            Utc::now() + Duration::seconds(MAX_CONFIRMATION_TTL_SECS) + Duration::seconds(1);
        // Lower bound: must be at least *almost* MAX seconds past
        // `now_before` — a buggy clamp to 0 / negative / 1s would fail
        // here. Ceiling: must not exceed MAX (defense against the
        // clamp leaking the original 24h).
        let min_floor =
            now_before + Duration::seconds(MAX_CONFIRMATION_TTL_SECS) - Duration::seconds(1);
        assert!(
            challenge.expires_at >= min_floor,
            "TTL clamp should land *at* MAX_CONFIRMATION_TTL_SECS, not below it; got {} (expected ≥ {})",
            challenge.expires_at,
            min_floor,
        );
        assert!(
            challenge.expires_at <= max_ceiling,
            "TTL should have been clamped to MAX_CONFIRMATION_TTL_SECS; got {} (expected ≤ {})",
            challenge.expires_at,
            max_ceiling,
        );
    }
}
