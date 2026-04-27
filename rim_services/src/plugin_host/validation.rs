//! Plugin manifest validation — Phase 1 Task 7 (closes Black Widow N1 +
//! Scott F2). The validators here MUST be called by the plugin install
//! pipeline before any manifest is persisted into `capability_registry` or
//! the command registry; without them an attacker (or a buggy plugin
//! author) can:
//!
//! - **N1**: declare `capability_key = "*"` / `"system.evil"` /
//!   `"plugin.other_plugin.foo"` and confuse the strict-default-deny gate
//!   into allowing actions that don't belong to this plugin's surface.
//! - **F2**: declare `ai_callable: true` on a command whose `output_schema`
//!   leaks `plugin_install_id` (a per-install opaque identifier the agent
//!   could feed back as a forged actor hint into a separate `installAs`
//!   pathway). The validator catches the typed-schema case directly and
//!   forces a manual review when the schema is opaque.
//!
//! The install path (when it lands later in Phase 2+) calls
//! [`validate_plugin_manifest`] as the **single** entry point — orchestrating
//! both the N1 capability-key validator and the F2 install-ID-leak walker.
//! Phase 1 ships the validators with full unit test coverage so the install
//! pipeline only needs to wire the call when it arrives.

use crate::plugin_host::manifest::{LocalPluginManifestCommandDto, LocalPluginManifestDto};

/// Reserved namespace prefixes a plugin MAY NOT register caps under,
/// regardless of the `plugin.<plugin_id>.` enforced prefix. Defense-in-depth
/// against future relaxation of the prefix check (or against a buggy code
/// path that bypasses it). A key like `plugin.x.audit.something` would still
/// be rejected here because it CONTAINS `audit.` as a non-root segment that
/// could be misread by a sloppy log filter; we err strict.
const RESERVED_PREFIXES: &[&str] = &["system.", "internal.", "admin.", "audit."];

/// Reserved segment names that must not appear as a non-root segment of a
/// plugin capability key (defense-in-depth against `plugin.<id>.audit.read`
/// being mistaken by a Phase 5+ audit filter for the builtin `audit.read`).
const RESERVED_SEGMENTS: &[&str] = &["system", "internal", "admin", "audit"];

const MIN_KEY_LEN: usize = 4;
const MAX_KEY_LEN: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginManifestError {
    /// Capability key rejected by N1 validator. `reason` is operator-facing
    /// (NOT user-facing — install errors go to plugin authors who can read
    /// the spec). `key` is echoed for debuggability.
    InvalidCapabilityKey { key: String, reason: String },

    /// F2 closure: an `ai_callable: true` command's typed `output_schema`
    /// declares a property whose name matches the install-ID leak heuristic.
    /// `field_path` is a dotted path into the schema for the offending
    /// property name.
    InstallIdLeakInAiSurface {
        command_id: String,
        field_path: String,
    },

    /// F2 fallback: an `ai_callable: true` command did NOT declare a typed
    /// `output_schema` (or declared one that's structurally untyped, e.g.
    /// `{"type":"object","additionalProperties":true}`). The install path
    /// MUST escalate to a human review gate before persisting; we cannot
    /// statically prove the absence of an install-ID leak through opaque
    /// output. Documented in `docs/plugin_model.md` (Phase 5 review-gate).
    RequiresManualReview { command_id: String, reason: String },
}

impl std::fmt::Display for PluginManifestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidCapabilityKey { key, reason } => {
                write!(f, "invalid capability key `{key}`: {reason}")
            }
            Self::InstallIdLeakInAiSurface {
                command_id,
                field_path,
            } => write!(
                f,
                "command `{command_id}` is ai_callable but its output_schema \
                 declares `{field_path}` which matches the plugin-install-id \
                 leak heuristic — install rejected"
            ),
            Self::RequiresManualReview { command_id, reason } => write!(
                f,
                "command `{command_id}` requires manual install review: {reason}"
            ),
        }
    }
}

impl std::error::Error for PluginManifestError {}

/// **N1 closure.** Validates a single capability key declared by `plugin_id`.
///
/// Rules (intersect of all):
/// 1. Length 4..=128 bytes.
/// 2. Charset `[a-z0-9_.]` only (lowercase ASCII alnum, underscore, dot).
/// 3. Prefix `plugin.<plugin_id>.` (literal, dot-terminated).
/// 4. No empty segments (no `..`, no leading/trailing dot).
/// 5. No reserved segment as a non-root component
///    (`system|internal|admin|audit`).
/// 6. The suffix after the prefix (the verb) must be at least 1 byte AND
///    must not itself begin with a reserved-prefix string.
///
/// Returns `Ok(())` on a valid key; otherwise [`PluginManifestError::InvalidCapabilityKey`]
/// with a reason string suitable for surfacing in install-rejection logs.
///
/// # Examples
///
/// Valid: `plugin.dev.example.k8s.restart_pod`
/// Invalid: `*`, `system.evil`, `plugin.other_plugin.foo` (when called with
/// `plugin_id = "self"`), `plugin.x..bad`, `plugin.x.audit.read`.
pub fn validate_plugin_capability(
    plugin_id: &str,
    capability_key: &str,
) -> Result<(), PluginManifestError> {
    let reject = |reason: &str| {
        Err(PluginManifestError::InvalidCapabilityKey {
            key: capability_key.to_string(),
            reason: reason.to_string(),
        })
    };

    // Rule 1: length.
    let len = capability_key.len();
    if len < MIN_KEY_LEN {
        return reject(&format!(
            "capability key must be at least {MIN_KEY_LEN} bytes (got {len})"
        ));
    }
    if len > MAX_KEY_LEN {
        return reject(&format!(
            "capability key must be at most {MAX_KEY_LEN} bytes (got {len})"
        ));
    }

    // Rule 2: charset.
    if !capability_key
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'.')
    {
        return reject(
            "capability key must match [a-z0-9_.]+ (lowercase ASCII alnum, underscore, dot)",
        );
    }

    // Rule 4 (early): no leading/trailing dot, no `..` empty segment.
    if capability_key.starts_with('.') || capability_key.ends_with('.') {
        return reject("capability key may not start or end with `.`");
    }
    if capability_key.contains("..") {
        return reject("capability key may not contain `..` (empty segment)");
    }

    // Rule 5 (defense-in-depth): reject reserved prefixes outright. With the
    // forced `plugin.<id>.` prefix this is covered, but a future relaxation
    // would expose this hole — keep the check.
    for reserved in RESERVED_PREFIXES {
        if capability_key.starts_with(reserved) {
            return reject(&format!(
                "capability key starts with reserved namespace `{reserved}` \
                 (reserved for builtin caps)"
            ));
        }
    }

    // Rule 3: prefix `plugin.<plugin_id>.`. We assert this AFTER the
    // reserved-prefix check so a key beginning with a reserved root
    // (`system.`, `audit.`, …) returns the more specific reserved-namespace
    // error before the prefix-mismatch error fires.
    //
    // Note on overlap: a key like `plugin.victim.system.foo` from
    // `attacker_plugin` is rejected by the wrong-plugin prefix check below
    // (reaches it because `plugin.victim.…` doesn't match any reserved-root
    // string), NOT by the segment scan further down. Wrong-plugin always
    // wins over reserved-segment when both apply. Tests
    // `n1_rejects_capability_for_other_plugin` + `n1_rejects_dispatch_plan_garbage_set`
    // pin both legs.
    let required_prefix = format!("plugin.{plugin_id}.");
    if !capability_key.starts_with(&required_prefix) {
        return reject(&format!(
            "capability key must start with `{required_prefix}` so the gate \
             can attribute the cap back to its declaring plugin"
        ));
    }

    // Rule 6: the suffix verb must be non-empty and not start with a
    // reserved keyword segment.
    let verb = &capability_key[required_prefix.len()..];
    if verb.is_empty() {
        return reject("capability key must declare a verb after `plugin.<id>.`");
    }

    // Rule 5 (continued): scan every segment for reserved names so a key
    // like `plugin.<id>.audit.read` is rejected — an audit log filter
    // doing a substring match could otherwise mistake it for the builtin.
    for segment in capability_key.split('.') {
        if RESERVED_SEGMENTS.contains(&segment) {
            // The literal `plugin` segment in the prefix is fine; the
            // reserved list excludes it.
            return reject(&format!(
                "capability key segment `{segment}` is reserved and may not \
                 appear as a component of a plugin cap"
            ));
        }
    }

    Ok(())
}

/// **F2 closure.** Walks every command in `manifest` flagged `ai_callable: true`
/// and inspects its `output_schema` for fields whose names match the
/// install-ID leak heuristic. Returns:
///
/// - `Ok(())` when no `ai_callable` command is present, or every present
///   `ai_callable` command's typed schema is leak-free.
/// - `Err(InstallIdLeakInAiSurface)` when a typed schema property name
///   matches the heuristic (rejects the install).
/// - `Err(RequiresManualReview)` when an `ai_callable` command has no
///   declared `output_schema` or its schema is structurally opaque
///   (`additionalProperties: true` at the root, or non-`object` root).
///   The install pipeline is expected to escalate this to a human review
///   step (Phase 5 — see `docs/plugin_model.md`).
///
/// The check is a registry-time static analysis only. Runtime output
/// scrubbing (Phase 5 H3) is a separate, complementary defense.
pub fn walk_output_schema_for_install_id_leak(
    manifest: &LocalPluginManifestDto,
) -> Result<(), PluginManifestError> {
    for command in &manifest.commands {
        if !command.ai_callable {
            continue;
        }
        check_ai_command_output_schema(command)?;
    }
    Ok(())
}

fn check_ai_command_output_schema(
    command: &LocalPluginManifestCommandDto,
) -> Result<(), PluginManifestError> {
    let Some(schema) = command.output_schema.as_ref() else {
        return Err(PluginManifestError::RequiresManualReview {
            command_id: command.id.clone(),
            reason: "ai_callable: true requires a declared output_schema for \
                     static install-ID leak analysis"
                .to_string(),
        });
    };

    // Schema must be a JSON object describing an object envelope.
    let Some(root) = schema.as_object() else {
        return Err(PluginManifestError::RequiresManualReview {
            command_id: command.id.clone(),
            reason: "output_schema must be a JSON object describing an object envelope".to_string(),
        });
    };

    let root_type = root.get("type").and_then(|v| v.as_str());
    if root_type != Some("object") {
        return Err(PluginManifestError::RequiresManualReview {
            command_id: command.id.clone(),
            reason: format!(
                "output_schema root type must be `object` (got {:?})",
                root_type.unwrap_or("none")
            ),
        });
    }

    // Opaque-property bypass: if `additionalProperties: true` or the
    // properties map is missing, we cannot statically prove the absence of
    // a leak — escalate to manual review.
    let additional_props_open = root
        .get("additionalProperties")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if additional_props_open {
        return Err(PluginManifestError::RequiresManualReview {
            command_id: command.id.clone(),
            reason: "output_schema declares `additionalProperties: true` — \
                     opaque shape cannot be statically scanned for install-ID leak"
                .to_string(),
        });
    }

    if root.get("properties").and_then(|v| v.as_object()).is_none()
        && !contains_compositional_keyword(root)
    {
        // No `properties`, no `oneOf`/`anyOf`/`allOf`/`$defs` either — we
        // have nothing static to scan even though the root claims `object`.
        // Escalate.
        return Err(PluginManifestError::RequiresManualReview {
            command_id: command.id.clone(),
            reason: "output_schema must declare `properties` (or a compositional \
                     keyword: oneOf/anyOf/allOf/$defs) for static install-ID \
                     leak analysis"
                .to_string(),
        });
    }

    walk_schema_node(&command.id, "", schema)
}

/// Returns true if `node` declares any of the JSON-Schema compositional
/// keywords the static walker is willing to recurse into. Used as the
/// "we still have something to scan" signal when `properties` is absent.
fn contains_compositional_keyword(node: &serde_json::Map<String, serde_json::Value>) -> bool {
    node.contains_key("oneOf")
        || node.contains_key("anyOf")
        || node.contains_key("allOf")
        || node.contains_key("$defs")
        || node.contains_key("items")
}

/// Recursively scans a JSON-Schema node for an install-ID-leak property name.
///
/// Coverage (Hawkeye Gate 5+6 BLOCK closure):
/// - `properties` map — every name is heuristic-checked AND each value is
///   walked as a sub-schema (so leaks under `properties.foo.properties.install_id`
///   are caught).
/// - `items` — array element schema (object form OR array form for
///   tuple-typed schemas).
/// - `oneOf` / `anyOf` / `allOf` — every branch is walked. A leak in ANY
///   branch fails the whole schema (defense-in-depth: a future runtime
///   that picks the "wrong" branch must not be able to surface a leak the
///   author hid in an alternative).
/// - `$defs` — every named definition is walked (definitions can be `$ref`d
///   from the live schema; we don't follow refs, but we DO scan the
///   definition body so a malicious author can't hide a leak in `$defs`).
///
/// Path notation in errors:
/// - `parent.field` for `properties.field`
/// - `parent[]` for `items` (tuple variants use `[0]`, `[1]`, …)
/// - `parent|oneOf[i]` / `|anyOf[i]` / `|allOf[i]` for branches
/// - `$defs.name` for definitions
fn walk_schema_node(
    command_id: &str,
    parent_path: &str,
    node: &serde_json::Value,
) -> Result<(), PluginManifestError> {
    let Some(obj) = node.as_object() else {
        return Ok(());
    };

    // 1. `properties` — name check + recurse on each value.
    if let Some(properties) = obj.get("properties").and_then(|v| v.as_object()) {
        for (name, value) in properties {
            let path = join_path(parent_path, name);
            if name_matches_install_id_leak(name) {
                return Err(PluginManifestError::InstallIdLeakInAiSurface {
                    command_id: command_id.to_string(),
                    field_path: path,
                });
            }
            walk_schema_node(command_id, &path, value)?;
        }
    }

    // 2. `items` — array element. Object form OR tuple-array form.
    if let Some(items) = obj.get("items") {
        let items_path = format!("{parent_path}[]");
        if let Some(tuple) = items.as_array() {
            for (i, item) in tuple.iter().enumerate() {
                let tuple_path = format!("{parent_path}[{i}]");
                walk_schema_node(command_id, &tuple_path, item)?;
            }
        } else {
            walk_schema_node(command_id, &items_path, items)?;
        }
    }

    // 3. `oneOf` / `anyOf` / `allOf` — array of branch schemas.
    for keyword in ["oneOf", "anyOf", "allOf"] {
        if let Some(branches) = obj.get(keyword).and_then(|v| v.as_array()) {
            for (i, branch) in branches.iter().enumerate() {
                let branch_path = format!("{parent_path}|{keyword}[{i}]");
                walk_schema_node(command_id, &branch_path, branch)?;
            }
        }
    }

    // 4. `$defs` — object map of name → schema.
    if let Some(defs) = obj.get("$defs").and_then(|v| v.as_object()) {
        for (name, def) in defs {
            let def_path = if parent_path.is_empty() {
                format!("$defs.{name}")
            } else {
                format!("{parent_path}.$defs.{name}")
            };
            // Definitions also gate on the property NAME directly — a $def
            // named `install_id` fails too, even if it's never $ref'd.
            if name_matches_install_id_leak(name) {
                return Err(PluginManifestError::InstallIdLeakInAiSurface {
                    command_id: command_id.to_string(),
                    field_path: def_path,
                });
            }
            walk_schema_node(command_id, &def_path, def)?;
        }
    }

    Ok(())
}

fn join_path(parent: &str, child: &str) -> String {
    if parent.is_empty() {
        child.to_string()
    } else {
        format!("{parent}.{child}")
    }
}

/// Heuristic for "this field name probably exposes a per-install identifier
/// the agent could weaponize as a forged actor hint."
///
/// **Algorithm (Hawkeye Gate 5+6 BLOCK closure):** strip the name down to
/// its lowercase alphanumerics so casing and separators (`_`, `-`, camelCase
/// boundaries) cannot hide a match. Then check for either anchor substring:
///
/// - `installid` — catches `install_id`, `installId`, `INSTALL_ID`, `installid`,
///   `plugin_install_id`, `pluginInstallId`, `plugin-install-id`, etc.
/// - `installationid` — catches `installation_id`, `installationId`,
///   `INSTALLATION_ID`, `installationid`, `pluginInstallationId`, etc.
///
/// **Why two anchors not one:** `installid` is NOT a substring of
/// `installationid` (the literal `installation` sequence breaks the match),
/// and a one-anchor algorithm misses one or the other family. Both anchors
/// are needed to cover the full space.
///
/// **Acceptable false-positive surface:** any field whose normalized name
/// contains either anchor — e.g. `preinstall_id`, `reinstallation_id`,
/// `would_install_identifier`. These are all legitimately suspicious and
/// rejection here is preferable to a leak: the plugin author can rename
/// the field to break the heuristic. **Not** matched: `installer`,
/// `installation_date`, `installs`, `uninstall` — none contain either anchor.
fn name_matches_install_id_leak(name: &str) -> bool {
    let normalized: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect();
    normalized.contains("installid") || normalized.contains("installationid")
}

/// **Single entry point** the plugin install pipeline calls before
/// persisting a manifest. Composes N1 (capability key shape) + F2
/// (install-ID leak in AI surface). Future Phase 5 work will compose
/// additional validators here without changing the call surface.
pub fn validate_plugin_manifest(
    manifest: &LocalPluginManifestDto,
) -> Result<(), PluginManifestError> {
    // N1: every capability the plugin DECLARES must follow the strict shape.
    // (Caps the plugin USES via `command.required_capabilities` may include
    // builtins like `chat.write` and are NOT validated here — they're gated
    // at `CapabilityService::check` runtime.)
    for cap in &manifest.capabilities {
        validate_plugin_capability(&manifest.plugin_id, cap)?;
    }

    // F2: AI-callable commands must not leak install IDs.
    walk_output_schema_for_install_id_leak(manifest)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin_host::manifest::{LocalPluginManifestDto, LocalPluginRuntimeKindDto};

    fn manifest_with_commands(
        plugin_id: &str,
        capabilities: Vec<String>,
        commands: Vec<LocalPluginManifestCommandDto>,
    ) -> LocalPluginManifestDto {
        LocalPluginManifestDto {
            manifest_version: 1,
            plugin_id: plugin_id.to_string(),
            display_name: "Test".to_string(),
            version: "0.1.0".to_string(),
            description: "test".to_string(),
            author: None,
            runtime_kind: LocalPluginRuntimeKindDto::DesktopLocalProcess,
            entrypoint: "./bin".to_string(),
            capabilities,
            commands,
            keybind_suggestions: vec![],
            context_actions: vec![],
            panels: vec![],
            settings_schema: vec![],
        }
    }

    fn ai_command(
        id: &str,
        ai_callable: bool,
        output_schema: Option<serde_json::Value>,
    ) -> LocalPluginManifestCommandDto {
        LocalPluginManifestCommandDto {
            id: id.to_string(),
            title: id.to_string(),
            description: "test".to_string(),
            keybind_capable: false,
            dangerous: false,
            required_context: vec![],
            required_capabilities: vec![],
            arguments: vec![],
            ai_callable,
            output_schema,
        }
    }

    // ── N1: validate_plugin_capability ──

    /// The dispatch-plan-mandated set: every adversarial input must reject.
    #[test]
    fn n1_rejects_dispatch_plan_garbage_set() {
        let plugin_id = "self";
        for bad in [
            "*",                  // not lowercase, charset reject
            "a",                  // too short
            "abc",                // too short
            "foo bar",            // contains space, charset reject
            "system.evil",        // reserved prefix
            "plugin.other.foo",   // wrong plugin_id prefix
            "PLUGIN.self.foo",    // uppercase, charset reject
            "plugin.self.",       // empty verb
            "plugin.self..bad",   // empty segment
            ".plugin.self.foo",   // leading dot
            "plugin.self.foo.",   // trailing dot
            "plugin.self.audit",  // reserved segment as verb
            "plugin.self.system", // reserved segment as verb
        ] {
            let result = validate_plugin_capability(plugin_id, bad);
            assert!(
                result.is_err(),
                "N1 must reject `{bad}` but it passed: {result:?}"
            );
        }
    }

    #[test]
    fn n1_rejects_overlong_key() {
        let plugin_id = "self";
        let key = format!("plugin.{plugin_id}.{}", "x".repeat(200));
        assert!(validate_plugin_capability(plugin_id, &key).is_err());
    }

    #[test]
    fn n1_rejects_max_len_plus_one() {
        let plugin_id = "self";
        let prefix = format!("plugin.{plugin_id}.");
        // Pad so the total key is exactly 129 bytes.
        let verb_len = MAX_KEY_LEN + 1 - prefix.len();
        let key = format!("{prefix}{}", "a".repeat(verb_len));
        assert_eq!(key.len(), MAX_KEY_LEN + 1);
        assert!(validate_plugin_capability(plugin_id, &key).is_err());
    }

    #[test]
    fn n1_accepts_max_len_exactly() {
        let plugin_id = "self";
        let prefix = format!("plugin.{plugin_id}.");
        let verb_len = MAX_KEY_LEN - prefix.len();
        let key = format!("{prefix}{}", "a".repeat(verb_len));
        assert_eq!(key.len(), MAX_KEY_LEN);
        assert!(validate_plugin_capability(plugin_id, &key).is_ok());
    }

    #[test]
    fn n1_accepts_well_formed_keys() {
        let plugin_id = "dev.example.k8s_tools";
        for good in [
            "plugin.dev.example.k8s_tools.restart_pod",
            "plugin.dev.example.k8s_tools.list_pods",
            "plugin.dev.example.k8s_tools.fetch_logs_2",
        ] {
            let result = validate_plugin_capability(plugin_id, good);
            assert!(
                result.is_ok(),
                "N1 should accept `{good}` but rejected: {result:?}"
            );
        }
    }

    #[test]
    fn n1_rejects_capability_for_other_plugin() {
        // A plugin claiming a cap key under another plugin's namespace is
        // the canonical privilege-escalation attempt N1 closes.
        let result = validate_plugin_capability("attacker_plugin", "plugin.victim_plugin.do_thing");
        assert!(matches!(
            result,
            Err(PluginManifestError::InvalidCapabilityKey { .. })
        ));
    }

    #[test]
    fn n1_error_includes_offending_key_for_diagnostics() {
        let err = validate_plugin_capability("self", "*").unwrap_err();
        match err {
            PluginManifestError::InvalidCapabilityKey { key, .. } => assert_eq!(key, "*"),
            other => panic!("expected InvalidCapabilityKey, got {other:?}"),
        }
    }

    // ── F2: walk_output_schema_for_install_id_leak ──

    #[test]
    fn f2_no_ai_callable_commands_passes() {
        let m = manifest_with_commands("self", vec![], vec![ai_command("safe.cmd", false, None)]);
        assert!(walk_output_schema_for_install_id_leak(&m).is_ok());
    }

    #[test]
    fn f2_ai_callable_without_schema_requires_manual_review() {
        let m = manifest_with_commands("self", vec![], vec![ai_command("opaque.cmd", true, None)]);
        let err = walk_output_schema_for_install_id_leak(&m).unwrap_err();
        assert!(matches!(
            err,
            PluginManifestError::RequiresManualReview { .. }
        ));
    }

    #[test]
    fn f2_ai_callable_with_typed_clean_schema_passes() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "summary": { "type": "string" },
                "count": { "type": "integer" }
            }
        });
        let m = manifest_with_commands(
            "self",
            vec![],
            vec![ai_command("safe.cmd", true, Some(schema))],
        );
        assert!(walk_output_schema_for_install_id_leak(&m).is_ok());
    }

    #[test]
    fn f2_ai_callable_with_install_id_leak_rejects() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "summary": { "type": "string" },
                "plugin_install_id": { "type": "string" }
            }
        });
        let m = manifest_with_commands(
            "self",
            vec![],
            vec![ai_command("leaky.cmd", true, Some(schema))],
        );
        let err = walk_output_schema_for_install_id_leak(&m).unwrap_err();
        match err {
            PluginManifestError::InstallIdLeakInAiSurface {
                command_id,
                field_path,
            } => {
                assert_eq!(command_id, "leaky.cmd");
                assert_eq!(field_path, "plugin_install_id");
            }
            other => panic!("expected InstallIdLeakInAiSurface, got {other:?}"),
        }
    }

    #[test]
    fn f2_detects_nested_install_id_leak() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "envelope": {
                    "type": "object",
                    "properties": {
                        "metadata": {
                            "type": "object",
                            "properties": {
                                "install_id": { "type": "string" }
                            }
                        }
                    }
                }
            }
        });
        let m = manifest_with_commands(
            "self",
            vec![],
            vec![ai_command("nested.cmd", true, Some(schema))],
        );
        let err = walk_output_schema_for_install_id_leak(&m).unwrap_err();
        match err {
            PluginManifestError::InstallIdLeakInAiSurface { field_path, .. } => {
                assert_eq!(field_path, "envelope.metadata.install_id");
            }
            other => panic!("expected nested InstallIdLeakInAiSurface, got {other:?}"),
        }
    }

    #[test]
    fn f2_detects_case_insensitive_install_id_variants() {
        for leak in ["INSTALL_ID", "InstallId", "Plugin_Install_Id", "installid"] {
            let schema = serde_json::json!({
                "type": "object",
                "properties": { leak: { "type": "string" } }
            });
            let m =
                manifest_with_commands("self", vec![], vec![ai_command("c", true, Some(schema))]);
            assert!(
                matches!(
                    walk_output_schema_for_install_id_leak(&m),
                    Err(PluginManifestError::InstallIdLeakInAiSurface { .. })
                ),
                "should reject leak field `{leak}`"
            );
        }
    }

    /// **Hawkeye Gate 5+6 BLOCK closure** — camelCase / kebab-case / mixed
    /// separator forms must all match. The pre-fix heuristic was anchored on
    /// snake_case substrings and missed every camelCase variant.
    #[test]
    fn f2_detects_camel_and_kebab_case_install_id_variants() {
        for leak in [
            "installationId",
            "pluginInstallationId",
            "plugin-install-id",
            "PluginInstallationID",
            "InstallationID",
            "installationid", // already lowercase, no separators
            "PLUGIN-INSTALLATION-ID",
        ] {
            let schema = serde_json::json!({
                "type": "object",
                "properties": { leak: { "type": "string" } }
            });
            let m =
                manifest_with_commands("self", vec![], vec![ai_command("c", true, Some(schema))]);
            assert!(
                matches!(
                    walk_output_schema_for_install_id_leak(&m),
                    Err(PluginManifestError::InstallIdLeakInAiSurface { .. })
                ),
                "should reject leak field `{leak}`"
            );
        }
    }

    /// Negative case — the heuristic must NOT trip on legitimate names that
    /// happen to share substrings. A regression that broadens the anchors
    /// (e.g. matching just `install`) would make plugin manifests un-installable.
    #[test]
    fn f2_does_not_match_legitimate_installer_fields() {
        for legit in [
            "installer",
            "installation_date",
            "installs",
            "uninstall",
            "installer_version",
            "reinstall_count",
            // Hawkeye Gate 5+6 NIT: pin the "two anchors, not the verb
            // `install`" promise. A future regression that broadens an
            // anchor to `install` would silently break these fields —
            // this list catches it loudly.
            "install_path",
            "install_dir",
            "install_size",
            "install_log",
            "installable",
            "installation_status",
            "installation_progress",
        ] {
            let schema = serde_json::json!({
                "type": "object",
                "properties": { legit: { "type": "string" } }
            });
            let m =
                manifest_with_commands("self", vec![], vec![ai_command("c", true, Some(schema))]);
            assert!(
                walk_output_schema_for_install_id_leak(&m).is_ok(),
                "must NOT reject legitimate field `{legit}`"
            );
        }
    }

    /// **Hawkeye Gate 5+6 BLOCK closure** — `items` recursion. Pre-fix the
    /// walker only descended into `properties` and missed array element
    /// schemas entirely.
    #[test]
    fn f2_detects_install_id_under_array_items_object_form() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "results": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "install_id": { "type": "string" }
                        }
                    }
                }
            }
        });
        let m = manifest_with_commands("self", vec![], vec![ai_command("c", true, Some(schema))]);
        let err = walk_output_schema_for_install_id_leak(&m).unwrap_err();
        match err {
            PluginManifestError::InstallIdLeakInAiSurface { field_path, .. } => {
                assert_eq!(field_path, "results[].install_id");
            }
            other => panic!("expected InstallIdLeakInAiSurface, got {other:?}"),
        }
    }

    /// `items` tuple-array form (JSON Schema allows `items: [schemaA, schemaB]`).
    #[test]
    fn f2_detects_install_id_under_array_items_tuple_form() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "row": {
                    "type": "array",
                    "items": [
                        { "type": "string" },
                        { "type": "object", "properties": { "installId": { "type": "string" } } }
                    ]
                }
            }
        });
        let m = manifest_with_commands("self", vec![], vec![ai_command("c", true, Some(schema))]);
        assert!(matches!(
            walk_output_schema_for_install_id_leak(&m),
            Err(PluginManifestError::InstallIdLeakInAiSurface { .. })
        ));
    }

    /// **Hawkeye Gate 5+6 BLOCK closure** — `oneOf` branch recursion. A leak
    /// in any branch fails the whole schema.
    #[test]
    fn f2_detects_install_id_under_one_of_branch() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "outcome": {
                    "oneOf": [
                        { "type": "object", "properties": { "ok": { "type": "boolean" } } },
                        { "type": "object", "properties": { "plugin_install_id": { "type": "string" } } }
                    ]
                }
            }
        });
        let m = manifest_with_commands("self", vec![], vec![ai_command("c", true, Some(schema))]);
        assert!(matches!(
            walk_output_schema_for_install_id_leak(&m),
            Err(PluginManifestError::InstallIdLeakInAiSurface { .. })
        ));
    }

    /// Same defense for `anyOf`.
    #[test]
    fn f2_detects_install_id_under_any_of_branch() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "v": {
                    "anyOf": [
                        { "type": "string" },
                        { "type": "object", "properties": { "installationId": { "type": "string" } } }
                    ]
                }
            }
        });
        let m = manifest_with_commands("self", vec![], vec![ai_command("c", true, Some(schema))]);
        assert!(matches!(
            walk_output_schema_for_install_id_leak(&m),
            Err(PluginManifestError::InstallIdLeakInAiSurface { .. })
        ));
    }

    /// Same defense for `allOf`.
    #[test]
    fn f2_detects_install_id_under_all_of_branch() {
        let schema = serde_json::json!({
            "type": "object",
            "allOf": [
                { "type": "object", "properties": { "summary": { "type": "string" } } },
                { "type": "object", "properties": { "install_id": { "type": "string" } } }
            ]
        });
        let m = manifest_with_commands("self", vec![], vec![ai_command("c", true, Some(schema))]);
        assert!(matches!(
            walk_output_schema_for_install_id_leak(&m),
            Err(PluginManifestError::InstallIdLeakInAiSurface { .. })
        ));
    }

    /// **Hawkeye Gate 5+6 BLOCK closure** — `$defs` recursion. A malicious
    /// author could hide the leak under a definition that's never `$ref`d at
    /// the live schema level; we still scan the body.
    #[test]
    fn f2_detects_install_id_under_dollar_defs() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": { "summary": { "type": "string" } },
            "$defs": {
                "leakBody": {
                    "type": "object",
                    "properties": { "install_id": { "type": "string" } }
                }
            }
        });
        let m = manifest_with_commands("self", vec![], vec![ai_command("c", true, Some(schema))]);
        let err = walk_output_schema_for_install_id_leak(&m).unwrap_err();
        match err {
            PluginManifestError::InstallIdLeakInAiSurface { field_path, .. } => {
                assert!(
                    field_path.contains("$defs.leakBody"),
                    "expected $defs path, got `{field_path}`"
                );
            }
            other => panic!("expected leak under $defs, got {other:?}"),
        }
    }

    /// `$defs` whose KEY itself is the leak name — must reject even with a
    /// benign body, because a `$ref` to it would re-expose the name.
    #[test]
    fn f2_detects_install_id_named_dollar_def_key() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": { "summary": { "type": "string" } },
            "$defs": {
                "install_id": { "type": "string" }
            }
        });
        let m = manifest_with_commands("self", vec![], vec![ai_command("c", true, Some(schema))]);
        assert!(matches!(
            walk_output_schema_for_install_id_leak(&m),
            Err(PluginManifestError::InstallIdLeakInAiSurface { .. })
        ));
    }

    /// A schema that has only `oneOf` at the root (no `properties`) is still
    /// scannable — must NOT escalate to manual review.
    #[test]
    fn f2_root_oneof_without_properties_is_walked_not_escalated() {
        let schema = serde_json::json!({
            "type": "object",
            "oneOf": [
                { "type": "object", "properties": { "a": { "type": "string" } } },
                { "type": "object", "properties": { "b": { "type": "integer" } } }
            ]
        });
        let m = manifest_with_commands("self", vec![], vec![ai_command("c", true, Some(schema))]);
        assert!(walk_output_schema_for_install_id_leak(&m).is_ok());
    }

    #[test]
    fn f2_open_additional_properties_requires_manual_review() {
        let schema = serde_json::json!({
            "type": "object",
            "additionalProperties": true,
            "properties": { "summary": { "type": "string" } }
        });
        let m = manifest_with_commands(
            "self",
            vec![],
            vec![ai_command("opaque.cmd", true, Some(schema))],
        );
        assert!(matches!(
            walk_output_schema_for_install_id_leak(&m),
            Err(PluginManifestError::RequiresManualReview { .. })
        ));
    }

    #[test]
    fn f2_non_object_root_requires_manual_review() {
        let schema = serde_json::json!({ "type": "string" });
        let m = manifest_with_commands(
            "self",
            vec![],
            vec![ai_command("scalar.cmd", true, Some(schema))],
        );
        assert!(matches!(
            walk_output_schema_for_install_id_leak(&m),
            Err(PluginManifestError::RequiresManualReview { .. })
        ));
    }

    #[test]
    fn f2_non_ai_command_with_install_id_field_passes() {
        // Critical contract: the F2 walker only fires on ai_callable: true
        // commands. A non-AI palette command MAY legitimately surface a
        // plugin_install_id to the human user (it never reaches the LLM
        // sandbox).
        let schema = serde_json::json!({
            "type": "object",
            "properties": { "plugin_install_id": { "type": "string" } }
        });
        let m = manifest_with_commands(
            "self",
            vec![],
            vec![ai_command("human_only.cmd", false, Some(schema))],
        );
        assert!(walk_output_schema_for_install_id_leak(&m).is_ok());
    }

    // ── validate_plugin_manifest orchestrator ──

    #[test]
    fn orchestrator_rejects_bad_capability_first() {
        // Even when the manifest also has an F2 problem, N1 fires first
        // because the orchestrator runs validators in order.
        let leaky_schema = serde_json::json!({
            "type": "object",
            "properties": { "install_id": { "type": "string" } }
        });
        let m = manifest_with_commands(
            "self",
            vec!["plugin.other.foo".to_string()],
            vec![ai_command("c", true, Some(leaky_schema))],
        );
        assert!(matches!(
            validate_plugin_manifest(&m),
            Err(PluginManifestError::InvalidCapabilityKey { .. })
        ));
    }

    #[test]
    fn orchestrator_passes_clean_manifest() {
        let clean_schema = serde_json::json!({
            "type": "object",
            "properties": { "summary": { "type": "string" } }
        });
        let m = manifest_with_commands(
            "self",
            vec!["plugin.self.do_thing".to_string()],
            vec![ai_command("c", true, Some(clean_schema))],
        );
        assert!(validate_plugin_manifest(&m).is_ok());
    }

    #[test]
    fn orchestrator_rejects_f2_leak_when_caps_are_clean() {
        let leaky_schema = serde_json::json!({
            "type": "object",
            "properties": { "plugin_install_id": { "type": "string" } }
        });
        let m = manifest_with_commands(
            "self",
            vec!["plugin.self.do_thing".to_string()],
            vec![ai_command("leaky.cmd", true, Some(leaky_schema))],
        );
        assert!(matches!(
            validate_plugin_manifest(&m),
            Err(PluginManifestError::InstallIdLeakInAiSurface { .. })
        ));
    }
}
