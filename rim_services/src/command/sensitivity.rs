//! Command argument sensitivity classification and sensitive-data detection.
//!
//! This module is the **single source of truth** for what counts as "sensitive"
//! data in command arguments — both for:
//!
//! 1. **UI-side modal masking** (Phase 4 `ConfirmCommandModal` renders
//!    masked-by-default with optional reveal).
//! 2. **Backend audit-log redaction** (Phase 4 `tracing::info!(target: "audit", …)`
//!    line and Phase 5 DB-backed `AuditLogService`).
//!
//! Both call sites import from here. Eliminates regex-drift risk (Widow N4)
//! that would produce "value masked in UI but logged plain" failure modes.
//!
//! Per Fury v2 §3.1 + Scott F3 / Widow N4 in §0.5 addendum of
//! `docs/design/ai-agent-reframe-design-2026-04-22-v2.md`.

use std::sync::LazyLock;

use regex::Regex;

/// Argument-name regex matching sensitive keys.
///
/// MUST be the only definition anywhere in the workspace. Modal masking
/// and audit redaction both import this constant. Changing the regex here
/// changes both call sites atomically.
///
/// Matches case-insensitively on substring — `apiKey`, `user_token`, and
/// `AUTHORIZATION` all hit.
pub static SENSITIVE_ARG_NAME_RE: LazyLock<Regex> = LazyLock::new(|| {
    // Note: `|` alternation with `(?i)` case-insensitive prefix.
    // Substring match (no anchors) — `refresh_token` matches on `token`.
    Regex::new(r"(?i)password|token|secret|key|credential|auth|api_key|bearer|cookie|session")
        .expect("SENSITIVE_ARG_NAME_RE regex is a compile-time constant")
});

/// JWT detection: `header.payload.signature`, each segment base64url.
///
/// Segment chars: `[A-Za-z0-9_-]`. Each segment 1+ chars, separated by `.`.
/// Kept liberal — we want false positives (mask a non-JWT that looks like one)
/// rather than false negatives.
static JWT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+$")
        .expect("JWT_RE regex is a compile-time constant")
});

/// High-entropy base64-ish token detection. Used as a fallback when no
/// name-based or vendor-prefix match succeeds.
///
/// Checks that a string is ≥ `min_chars`, composed of base64/base64url
/// characters, and has enough character diversity to not be a word.
static B64_LIKE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[A-Za-z0-9+/_=-]+$").expect("B64_LIKE_RE regex is a compile-time constant")
});

/// Minimum length for the high-entropy base64 detector to fire.
/// Shorter than this and we won't guess a value is secret without a name
/// or prefix signal.
const B64_LIKE_MIN_CHARS: usize = 40;

/// Classification tier for a command's execution sensitivity.
///
/// Orthogonal to `CommandDefinitionDto::dangerous` — `dangerous: true`
/// forces a confirm regardless of tier, but tier drives:
///
/// - Audit-log verbosity (higher = more context captured)
/// - Mutating-call caps in the agent runtime (see §3.7)
/// - Default modal vs inline confirm placement in the UX (Ant-Man r2.1)
///
/// Unchanged from v1 per Fury v2 §3.1 ("`Safe, Sensitive, Mutating, Restricted`,
/// unchanged from v1").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum CommandSensitivityDto {
    /// Read-only, no side effects observable to other users.
    /// E.g. `list_channels`, `search_messages`.
    Safe,
    /// Reads or writes user-visible state that should be tracked in audit
    /// but is not high-blast-radius. E.g. `mark_read`, `set_presence`.
    Sensitive,
    /// Mutates durable state other users will see. E.g. `send_message`,
    /// `create_thread`. Counted against agent's
    /// mutating-call cap (3/turn per §3.7).
    Mutating,
    /// Elevated-risk surface that should not be invoked casually — admin
    /// operations, cross-workspace writes, destructive operations.
    /// E.g. `delete_conversation`, `revoke_session`, `uninstall_plugin`.
    /// Typically paired with `dangerous: true` and requires a confirmation
    /// proof (§3.5 `ConfirmationProof`).
    Restricted,
}

impl CommandSensitivityDto {
    /// Canonical lowercase string representation for DB storage.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Safe => "safe",
            Self::Sensitive => "sensitive",
            Self::Mutating => "mutating",
            Self::Restricted => "restricted",
        }
    }

    /// Whether this tier counts against the agent's per-turn mutating-call cap.
    pub fn is_mutating(self) -> bool {
        matches!(self, Self::Mutating | Self::Restricted)
    }

    /// Whether this tier requires an explicit confirmation proof before
    /// execution on the AI surface (absent `dangerous: true`).
    ///
    /// Restricted always requires; Mutating requires when combined with
    /// other signals (same-turn tool_result, cross-channel write, etc. —
    /// see §3.5 escalation rules). The base predicate here is just
    /// "tier alone warrants confirm."
    pub fn requires_confirmation_by_tier(self) -> bool {
        matches!(self, Self::Restricted)
    }

    /// Single-source-of-truth mapping from the legacy `dangerous: bool` flag
    /// to a sensitivity tier. Used by every command-registration code path
    /// that has not yet been audited per-command (Phase 1 default policy):
    /// `dangerous: true` → `Restricted`, `dangerous: false` → `Safe`.
    ///
    /// Two call sites depend on this mapping:
    ///
    /// 1. `UserCommandDto::as_command_definition_with_metadata` (user macro path)
    /// 2. Built-in `registry.rs` literals (still hand-written so Phase 4 can
    ///    diverge per-command without touching this helper)
    ///
    /// (Plugin manifests are mapped client-side by the plugin host in
    /// `rim-ui/src-tauri/src/plugin_host/`.)
    ///
    /// Per Hawkeye Gate 2 strong WARN — drift between copies of the
    /// same if/else is exactly the failure mode this helper exists to prevent.
    /// Phase 4 will tune per-command tiers and the call sites here will start
    /// to override; that override happens at the call site, not in this helper.
    pub fn from_dangerous_flag(dangerous: bool) -> Self {
        if dangerous {
            Self::Restricted
        } else {
            Self::Safe
        }
    }

    /// Max sensitivity across a macro's own tier and all its step definitions.
    ///
    /// Prevents a macro declared as `Safe` from silently wrapping a `Mutating`
    /// step — the effective tier enforced at execute time is the worst-case
    /// across the whole expansion.
    pub fn effective(macro_sensitivity: Self, step_sensitivities: &[Self]) -> Self {
        step_sensitivities
            .iter()
            .copied()
            .fold(macro_sensitivity, Self::max)
    }
}

impl From<&str> for CommandSensitivityDto {
    fn from(s: &str) -> Self {
        match s {
            "safe" => Self::Safe,
            "sensitive" => Self::Sensitive,
            "mutating" => Self::Mutating,
            "restricted" => Self::Restricted,
            _ => Self::Safe,
        }
    }
}

/// Returns `true` if `arg_name` matches any of the sensitive-name patterns.
///
/// Case-insensitive substring match against `SENSITIVE_ARG_NAME_RE`.
pub fn arg_name_looks_sensitive(arg_name: &str) -> bool {
    SENSITIVE_ARG_NAME_RE.is_match(arg_name)
}

/// Returns `true` if the string value has a secret-like *shape* regardless
/// of what argument name it was passed under.
///
/// Catches cases like `data: "sk-abc123…"` where the arg name doesn't match
/// the sensitive-name regex but the value is clearly a vendor API key.
///
/// Detection layers (first match wins):
/// 1. Vendor prefixes: `sk-`, `xoxb-`, `xoxp-`, `xoxa-`, `ghp_`, `gho_`,
///    `ghu_`, `ghs_`, `ghr_`, `glpat-`, `ya29.`, `AKIA`.
/// 2. JWT shape: three dot-separated base64url segments.
/// 3. High-entropy base64-ish tokens ≥ `B64_LIKE_MIN_CHARS` chars.
pub fn value_looks_secret(value: &str) -> bool {
    if value.is_empty() {
        return false;
    }

    // Layer 1: vendor prefixes. Ordered roughly by how common they are in
    // the wild and how unambiguous the prefix is.
    const VENDOR_PREFIXES: &[&str] = &[
        "sk-",    // OpenAI, Anthropic (legacy), others
        "xoxb-",  // Slack bot tokens
        "xoxp-",  // Slack user tokens
        "xoxa-",  // Slack workspace tokens
        "ghp_",   // GitHub personal access tokens
        "gho_",   // GitHub OAuth tokens
        "ghu_",   // GitHub user-to-server tokens
        "ghs_",   // GitHub server-to-server tokens
        "ghr_",   // GitHub refresh tokens
        "glpat-", // GitLab personal access tokens
        "ya29.",  // Google OAuth access tokens
        "AKIA",   // AWS access key IDs
        "AIza",   // Google API keys
    ];
    if VENDOR_PREFIXES.iter().any(|p| value.starts_with(p)) {
        return true;
    }

    // Layer 2: JWT shape.
    if JWT_RE.is_match(value) {
        return true;
    }

    // Layer 3: high-entropy-ish base64 tokens.
    if value.len() >= B64_LIKE_MIN_CHARS
        && B64_LIKE_RE.is_match(value)
        && has_sufficient_char_diversity(value)
    {
        return true;
    }

    false
}

/// Lightweight character-diversity heuristic — NOT a true entropy estimate.
/// Returns `true` if the string contains at least 12 distinct characters.
///
/// Purpose: reject obvious non-random filler like `"aaaa..."` or
/// `"abcabc..."` while accepting base64-encoded random data (which tends
/// to use most of the 64-char alphabet uniformly).
///
/// Byte-iteration is safe here because this function is only called after
/// [`B64_LIKE_RE`] matches, and that regex restricts the input to ASCII
/// `[A-Za-z0-9+/_=-]` — every byte is one char. If the upstream regex
/// ever loosens to accept multi-byte UTF-8, this must switch to
/// `value.chars()` + `HashSet<char>`.
fn has_sufficient_char_diversity(value: &str) -> bool {
    const MIN_DISTINCT_CHARS: usize = 12;
    let mut seen = [false; 256];
    let mut distinct: usize = 0;
    for b in value.bytes() {
        if !seen[b as usize] {
            seen[b as usize] = true;
            distinct += 1;
            if distinct >= MIN_DISTINCT_CHARS {
                return true;
            }
        }
    }
    false
}

/// Placeholder token emitted in place of sensitive values. Kept consistent
/// between audit and display so operators can grep audit logs and recognize
/// redactions.
pub const REDACTED_PLACEHOLDER: &str = "[REDACTED]";

/// Recursively walk a JSON args object, replacing any value whose *name*
/// matches a sensitive key OR whose *value shape* looks secret with
/// `REDACTED_PLACEHOLDER`.
///
/// Used by the Phase 4 audit log writer to make sure nothing sensitive
/// lands in durable logs. Safe to call on any JSON shape; arrays and nested
/// objects are walked.
///
/// Returns a new `Value`; the input is not mutated.
pub fn redact_for_audit(args: &serde_json::Value) -> serde_json::Value {
    redact_walk(args, /*key_context:*/ None, /*forced:*/ false)
}

/// Same recursive redaction policy, but used at the boundary where a
/// masked view is rendered in the UI. Today these two call sites use
/// *identical* masking — this function exists as a separate name so that
/// if the UI ever wants a different policy (e.g. keep last 4 chars of an
/// API key for user recognition), the change lands here, not in
/// `redact_for_audit`.
///
/// For now, delegates. Keep the alias; it makes greps intentional.
pub fn mask_for_display(args: &serde_json::Value) -> serde_json::Value {
    redact_for_audit(args)
}

/// Audit-side entry point for redacting `ExecuteCommandCmd::args` immediately
/// before it is emitted on the `target: "audit"` tracing line in
/// `command.rs::execute_command`.
///
/// Today this is a thin alias over [`redact_for_audit`] (same key/value
/// regex policy, same propagation rules). It exists as a separately-named
/// entry point because the audit-write boundary may diverge from the
/// generic `redact_for_audit` policy in later phases — Phase 5's DB-backed
/// `AuditLogService` may want structured field-level metadata (which keys
/// were redacted, by which rule) attached to each row, which would change
/// the return type. Pinning the name now keeps the call site at
/// `command.rs::execute_command` stable across that refactor.
///
/// Per Hawkeye Gate 1 strong WARN — three copies of the same redaction
/// policy is exactly the failure mode this single source of truth exists
/// to prevent. Do NOT inline this; route every audit emit through here.
pub fn sanitize_args_for_audit(args: &serde_json::Value) -> serde_json::Value {
    redact_for_audit(args)
}

/// Recursive walker. `key_context` is the key this value was looked up
/// under (`None` at the root, or inside an array). `forced` is a sticky
/// flag that becomes `true` as soon as any ancestor key is sensitive and
/// stays `true` for every descendant — so
/// `{"tokens": ["v1", "v2"]}` redacts *both* array items by inheritance,
/// and `{"auth": {"secret": "plain"}}` redacts the inner string even if
/// the inner key `secret` weren't itself sensitive.
///
/// Preserves container shape: arrays stay arrays, objects stay objects.
/// Scalars (strings, numbers, bools) under a forced path become the
/// `[REDACTED]` string — shape change is accepted as the cost of not
/// lying to audit (a number-shaped secret must not survive).
fn redact_walk(
    value: &serde_json::Value,
    key_context: Option<&str>,
    forced: bool,
) -> serde_json::Value {
    let is_forced = forced || matches!(key_context, Some(k) if arg_name_looks_sensitive(k));

    match value {
        serde_json::Value::String(s) => {
            if is_forced || value_looks_secret(s) {
                serde_json::Value::String(REDACTED_PLACEHOLDER.to_string())
            } else {
                value.clone()
            }
        }
        serde_json::Value::Array(items) => serde_json::Value::Array(
            items
                .iter()
                // Array items inherit the parent key's sensitivity via
                // `is_forced`. Pass `None` as their own key_context (they
                // have no key) — the forced flag carries the signal.
                .map(|v| redact_walk(v, None, is_forced))
                .collect(),
        ),
        serde_json::Value::Object(map) => {
            let mut out = serde_json::Map::with_capacity(map.len());
            for (k, v) in map {
                out.insert(k.clone(), redact_walk(v, Some(k), is_forced));
            }
            serde_json::Value::Object(out)
        }
        // Numbers / bools / null: preserved at benign call sites, redacted
        // under a sensitive path. A caller that stores `api_key: 12345`
        // gets the 12345 masked — surprising, but correct. The alternative
        // (leaving numeric secrets in audit) is worse.
        _ => {
            if is_forced {
                serde_json::Value::String(REDACTED_PLACEHOLDER.to_string())
            } else {
                value.clone()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // --- SENSITIVE_ARG_NAME_RE --- //

    #[test]
    fn arg_name_matches_each_keyword() {
        for name in [
            "password",
            "Password",
            "user_password",
            "token",
            "access_token",
            "refresh_token",
            "secret",
            "secret_key",
            "api_key",
            "apiKey",
            "API_KEY",
            "credential",
            "credentials",
            "authorization",
            "AUTH",
            "bearer",
            "cookie",
            "session",
            "session_id",
        ] {
            assert!(
                arg_name_looks_sensitive(name),
                "expected {name} to match SENSITIVE_ARG_NAME_RE"
            );
        }
    }

    #[test]
    fn arg_name_does_not_match_innocuous_names() {
        for name in [
            "message",
            "channel",
            "text",
            "user_id",
            "conversation_id",
            "created_at",
            "count",
            "query",
            "limit",
            "offset",
        ] {
            assert!(
                !arg_name_looks_sensitive(name),
                "did not expect {name} to match SENSITIVE_ARG_NAME_RE"
            );
        }
    }

    // --- value_looks_secret --- //

    #[test]
    fn detects_openai_style_keys() {
        assert!(value_looks_secret(
            "sk-proj-abcdefghijklmnopqrstuvwxyz0123456789"
        ));
        assert!(value_looks_secret("sk-ant-api03-xyz"));
    }

    #[test]
    fn detects_slack_tokens() {
        assert!(value_looks_secret(
            "xoxb-1234567890-1234567890-abcdefghijklmnopqrstuvwx"
        ));
        assert!(value_looks_secret("xoxp-ABC123"));
        assert!(value_looks_secret("xoxa-XYZ"));
    }

    #[test]
    fn detects_github_tokens() {
        for prefix in ["ghp_", "gho_", "ghu_", "ghs_", "ghr_"] {
            let v = format!("{prefix}abcdefghijklmnopqrstuvwxyz012345");
            assert!(value_looks_secret(&v), "expected {v} to look secret");
        }
    }

    #[test]
    fn detects_gitlab_and_google_and_aws() {
        assert!(value_looks_secret("glpat-1234567890abcdefghij"));
        assert!(value_looks_secret(
            "ya29.A0AVA9y1u_abcdefghijklmnopqrstuvwxyz"
        ));
        assert!(value_looks_secret("AKIAIOSFODNN7EXAMPLE"));
        assert!(value_looks_secret("AIzaSyA-example-google-api-key-here"));
    }

    #[test]
    fn detects_jwts() {
        // A JWT-shape string — three base64url segments separated by dots.
        assert!(value_looks_secret(
            "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjMifQ.abc123"
        ));
    }

    #[test]
    fn detects_high_entropy_base64() {
        // 48 chars of pseudo-random base64 — should trip the fallback.
        assert!(value_looks_secret(
            "AbCdEf0123456789XyZwVuTsRqPoNmLkJiHgFeDcBa9876"
        ));
    }

    #[test]
    fn does_not_detect_short_base64() {
        // Short strings fall below B64_LIKE_MIN_CHARS (40) and don't hit.
        assert!(!value_looks_secret("abc123"));
        assert!(!value_looks_secret("short-token"));
    }

    #[test]
    fn does_not_detect_normal_prose() {
        assert!(!value_looks_secret("hello world"));
        assert!(!value_looks_secret(
            "The quick brown fox jumps over the lazy dog"
        ));
        assert!(!value_looks_secret("status: ok"));
    }

    #[test]
    fn does_not_detect_low_entropy_repetition() {
        // Long but low-diversity string — avoids false-positive on
        // `aaaaaa...` or other repetitive content even though it's base64-ish.
        assert!(!value_looks_secret(&"a".repeat(80)));
        assert!(!value_looks_secret(&"abc".repeat(50)));
    }

    #[test]
    fn does_not_detect_empty() {
        assert!(!value_looks_secret(""));
    }

    // --- redact_for_audit / mask_for_display --- //

    #[test]
    fn redacts_top_level_sensitive_key() {
        let args = json!({
            "message": "hello",
            "password": "hunter2",
        });
        let masked = redact_for_audit(&args);
        assert_eq!(masked["message"], json!("hello"));
        assert_eq!(masked["password"], json!(REDACTED_PLACEHOLDER));
    }

    #[test]
    fn redacts_value_shape_even_under_innocuous_key() {
        // Key name doesn't match sensitive regex, but value is a vendor
        // API key — must still redact.
        let args = json!({
            "data": "sk-proj-abcdefghijklmnopqrstuvwxyz0123456789",
        });
        let masked = redact_for_audit(&args);
        assert_eq!(masked["data"], json!(REDACTED_PLACEHOLDER));
    }

    #[test]
    fn redacts_nested_sensitive_key() {
        let args = json!({
            "user": {
                "id": "u1",
                "auth": "bearer token-here",
            },
        });
        let masked = redact_for_audit(&args);
        assert_eq!(masked["user"]["id"], json!("u1"));
        assert_eq!(masked["user"]["auth"], json!(REDACTED_PLACEHOLDER));
    }

    #[test]
    fn redacts_array_items_by_shape() {
        let args = json!([
            "hello",
            "sk-proj-abcdefghijklmnopqrstuvwxyz0123456789",
            "world"
        ]);
        let masked = redact_for_audit(&args);
        assert_eq!(masked[0], json!("hello"));
        assert_eq!(masked[1], json!(REDACTED_PLACEHOLDER));
        assert_eq!(masked[2], json!("world"));
    }

    #[test]
    fn redacts_array_items_under_sensitive_key_even_if_short() {
        // Hawkeye's BLOCK: without key-context propagation into arrays,
        // short-but-harmless-looking strings inside `tokens: [...]` would
        // survive unredacted. We preserve array shape and redact every
        // item by inheritance from the parent key.
        let args = json!({
            "tokens": ["short1", "plain-value", "abc"],
            "message": "hi",
        });
        let masked = redact_for_audit(&args);
        assert_eq!(masked["message"], json!("hi"));
        assert_eq!(
            masked["tokens"],
            json!([
                REDACTED_PLACEHOLDER,
                REDACTED_PLACEHOLDER,
                REDACTED_PLACEHOLDER
            ]),
            "array shape must be preserved but every item redacted by inherited sensitivity"
        );
    }

    #[test]
    fn redacts_nested_object_under_sensitive_key() {
        // Sensitivity propagates through ALL descendants of a sensitive
        // key, not just the immediate child. Even an "innocuous" inner
        // field name like `value` should be redacted because it lives
        // under `credentials`.
        let args = json!({
            "credentials": {
                "value": "not-a-known-shape",
                "region": "us-east-1",
            },
        });
        let masked = redact_for_audit(&args);
        assert_eq!(masked["credentials"]["value"], json!(REDACTED_PLACEHOLDER));
        assert_eq!(masked["credentials"]["region"], json!(REDACTED_PLACEHOLDER));
    }

    #[test]
    fn redacts_numeric_secret_under_sensitive_key() {
        // A numeric value under a sensitive key name must be redacted
        // even though the scalar branch of redact_walk normally
        // preserves numbers — numeric secrets (PINs, numeric OTPs,
        // IDs that embed sensitive info) must not survive audit.
        let args = json!({
            "api_key": 12345,
            "count": 42,
        });
        let masked = redact_for_audit(&args);
        assert_eq!(masked["api_key"], json!(REDACTED_PLACEHOLDER));
        assert_eq!(masked["count"], json!(42));
    }

    #[test]
    fn preserves_non_string_scalars() {
        let args = json!({
            "count": 42,
            "enabled": true,
            "missing": null,
        });
        let masked = redact_for_audit(&args);
        assert_eq!(masked["count"], json!(42));
        assert_eq!(masked["enabled"], json!(true));
        assert_eq!(masked["missing"], json!(null));
    }

    #[test]
    fn mask_for_display_delegates_to_redact() {
        // Today these two entry points have identical policy. Pinning
        // the behavior so a future divergence is an intentional edit
        // rather than an accident.
        let args = json!({
            "token": "abc",
            "other": "hello",
        });
        assert_eq!(mask_for_display(&args), redact_for_audit(&args));
    }

    #[test]
    fn sanitize_args_for_audit_delegates_to_redact() {
        // Same alias-parity guard as `mask_for_display_delegates_to_redact`,
        // but for the audit-side entry point. If Phase 5's structured
        // metadata work diverges this from `redact_for_audit`, the test
        // updates intentionally.
        let args = json!({
            "password": "hunter2",
            "data": "sk-proj-abcdefghijklmnopqrstuvwxyz0123456789",
            "message": "hi",
            "tokens": ["v1", "v2"],
        });
        assert_eq!(sanitize_args_for_audit(&args), redact_for_audit(&args));
    }

    // --- CommandSensitivityDto --- //

    #[test]
    fn sensitivity_ord_ordering() {
        assert!(CommandSensitivityDto::Safe < CommandSensitivityDto::Sensitive);
        assert!(CommandSensitivityDto::Sensitive < CommandSensitivityDto::Mutating);
        assert!(CommandSensitivityDto::Mutating < CommandSensitivityDto::Restricted);
    }

    #[test]
    fn effective_sensitivity_uses_max() {
        use CommandSensitivityDto::*;
        assert_eq!(CommandSensitivityDto::effective(Safe, &[]), Safe);
        assert_eq!(
            CommandSensitivityDto::effective(Safe, &[Mutating, Safe]),
            Mutating
        );
        assert_eq!(
            CommandSensitivityDto::effective(Restricted, &[Safe]),
            Restricted
        );
        assert_eq!(
            CommandSensitivityDto::effective(Sensitive, &[Mutating, Restricted]),
            Restricted
        );
    }

    #[test]
    fn sensitivity_tier_classification() {
        assert!(!CommandSensitivityDto::Safe.is_mutating());
        assert!(!CommandSensitivityDto::Sensitive.is_mutating());
        assert!(CommandSensitivityDto::Mutating.is_mutating());
        assert!(CommandSensitivityDto::Restricted.is_mutating());

        assert!(!CommandSensitivityDto::Safe.requires_confirmation_by_tier());
        assert!(!CommandSensitivityDto::Sensitive.requires_confirmation_by_tier());
        assert!(!CommandSensitivityDto::Mutating.requires_confirmation_by_tier());
        assert!(CommandSensitivityDto::Restricted.requires_confirmation_by_tier());
    }
}
