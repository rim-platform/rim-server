//! Alert-ingestion normalization: turns third-party monitoring webhook payloads (RIM's own
//! generic format, Prometheus Alertmanager, Grafana unified alerting) into a single
//! [`AlertEvent`] shape that [`crate::incident::service::IncidentService::ingest_alert`]
//! consumes. Kept transport-agnostic (no actix/HTTP types) so it's unit-testable without a
//! server and reusable if another transport (CLI, other webhook) needs it later.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};

use rim_core::incident::model::IncidentSeverity;

use crate::{ServiceError, ServiceResult};

/// Whether an ingested alert is newly/still firing, or has resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlertStatus {
    Firing,
    Resolved,
}

/// Which webhook format produced this event. Persisted on the incident as `source`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlertSource {
    Generic,
    Alertmanager,
    Grafana,
}

impl AlertSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Generic => "generic",
            Self::Alertmanager => "alertmanager",
            Self::Grafana => "grafana",
        }
    }
}

/// A single normalized alert, produced by one of the `parse_*` functions in this module.
#[derive(Debug, Clone, PartialEq)]
pub struct AlertEvent {
    /// Stable dedup key. Caller-supplied when the source format provides one (Alertmanager,
    /// Grafana); otherwise derived — see [`derive_fingerprint`].
    pub fingerprint: String,
    pub status: AlertStatus,
    /// Incident title / alertname.
    pub title: String,
    pub severity: Option<IncidentSeverity>,
    pub source: AlertSource,
    /// Annotation/description text, surfaced as incident detail / phase note.
    pub summary: Option<String>,
    pub started_at: Option<DateTime<Utc>>,
    pub labels: BTreeMap<String, String>,
}

/// Map a free-text severity label (case-insensitive) to an [`IncidentSeverity`] tier.
/// Unknown labels return `None` — callers should fall back to the ingest token's configured
/// default severity, then [`IncidentSeverity::Sev3`]. See `docs/alerts.md` for the full table.
pub fn map_severity(label: &str) -> Option<IncidentSeverity> {
    match label.to_ascii_lowercase().as_str() {
        "critical" | "crit" | "p1" | "sev1" | "page" => Some(IncidentSeverity::Sev1),
        "high" | "error" | "p2" | "sev2" => Some(IncidentSeverity::Sev2),
        "warning" | "warn" | "p3" | "sev3" => Some(IncidentSeverity::Sev3),
        "info" | "low" | "p4" | "sev4" => Some(IncidentSeverity::Sev4),
        _ => None,
    }
}

/// Derive a dedup fingerprint from `source`, `title`, and `labels` when the source webhook
/// didn't supply one (RIM's generic format allows an omitted `fingerprint`). Deterministic:
/// the same inputs always produce the same output, since `labels` is a `BTreeMap` (sorted
/// iteration). Truncated to 32 hex characters (128 bits) — plenty of collision resistance for
/// a per-server dedup key, shorter than a full SHA-256 hex digest for readability in logs/UI.
pub fn derive_fingerprint(
    source: AlertSource,
    title: &str,
    labels: &BTreeMap<String, String>,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(source.as_str().as_bytes());
    hasher.update(b"|");
    hasher.update(title.as_bytes());
    hasher.update(b"|");
    for (k, v) in labels {
        hasher.update(k.as_bytes());
        hasher.update(b"=");
        hasher.update(v.as_bytes());
        hasher.update(b",");
    }
    let digest = hasher.finalize();
    hex_encode(&digest)[..32].to_string()
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut res = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        res.push(HEX[(b >> 4) as usize] as char);
        res.push(HEX[(b & 0x0f) as usize] as char);
    }
    res
}

fn status_from_str(s: &str) -> AlertStatus {
    if s.eq_ignore_ascii_case("resolved") {
        AlertStatus::Resolved
    } else {
        AlertStatus::Firing
    }
}

// ── Generic format (RIM's own, documented in docs/alerts.md) ────────────────────────────────

#[derive(Debug, Clone, serde::Deserialize)]
struct GenericAlertItem {
    fingerprint: Option<String>,
    #[serde(default = "default_status_firing")]
    status: String,
    title: String,
    severity: Option<String>,
    summary: Option<String>,
    #[serde(default)]
    labels: BTreeMap<String, String>,
}

fn default_status_firing() -> String {
    "firing".to_string()
}

/// Parse RIM's own generic JSON alert format: either a single alert object, or
/// `{ "alerts": [...] }`. Each item may omit `fingerprint` (derived via
/// [`derive_fingerprint`]) and `status` (defaults to `"firing"`).
///
/// # Errors
/// Returns [`ServiceError::Validation`] if `body` is not valid JSON or doesn't match the
/// expected shape (missing `title`, wrong field types, etc.).
pub fn parse_generic(body: &[u8]) -> ServiceResult<Vec<AlertEvent>> {
    let value: serde_json::Value = serde_json::from_slice(body)
        .map_err(|e| ServiceError::Validation(format!("invalid JSON body: {e}")))?;

    let items: Vec<GenericAlertItem> = if let Some(alerts) = value.get("alerts") {
        serde_json::from_value(alerts.clone())
            .map_err(|e| ServiceError::Validation(format!("invalid `alerts` array: {e}")))?
    } else {
        let single: GenericAlertItem = serde_json::from_value(value)
            .map_err(|e| ServiceError::Validation(format!("invalid alert payload: {e}")))?;
        vec![single]
    };

    Ok(items
        .into_iter()
        .map(|item| {
            let status = status_from_str(&item.status);
            let severity = item.severity.as_deref().and_then(map_severity);
            let fingerprint = item.fingerprint.unwrap_or_else(|| {
                derive_fingerprint(AlertSource::Generic, &item.title, &item.labels)
            });
            AlertEvent {
                fingerprint,
                status,
                title: item.title,
                severity,
                source: AlertSource::Generic,
                summary: item.summary,
                started_at: None,
                labels: item.labels,
            }
        })
        .collect())
}

// ── Alertmanager / Grafana (near-identical contact-point payload shape) ─────────────────────

#[derive(Debug, Clone, serde::Deserialize)]
struct AmShapedPayload {
    #[serde(default)]
    alerts: Vec<AmShapedAlert>,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct AmShapedAlert {
    #[serde(default = "default_status_firing")]
    status: String,
    #[serde(default)]
    labels: BTreeMap<String, String>,
    #[serde(default)]
    annotations: BTreeMap<String, String>,
    #[serde(rename = "startsAt", default)]
    starts_at: Option<String>,
    #[serde(default)]
    fingerprint: Option<String>,
}

/// Shared parser for the Alertmanager-shaped webhook payload
/// (`{ status, alerts: [{ status, labels, annotations, startsAt, fingerprint }] }`), which
/// Grafana's unified alerting reuses. `source` tags the resulting events so the caller and the
/// persisted incident can tell the two apart even though the wire shape is the same.
fn parse_am_shaped(body: &[u8], source: AlertSource) -> ServiceResult<Vec<AlertEvent>> {
    let payload: AmShapedPayload = serde_json::from_slice(body).map_err(|e| {
        ServiceError::Validation(format!("invalid {} payload: {e}", source.as_str()))
    })?;

    Ok(payload
        .alerts
        .into_iter()
        .map(|a| {
            let status = status_from_str(&a.status);
            let title = a
                .labels
                .get("alertname")
                .cloned()
                .unwrap_or_else(|| "unknown alert".to_string());
            let severity = a.labels.get("severity").and_then(|s| map_severity(s));
            let summary = a
                .annotations
                .get("summary")
                .or_else(|| a.annotations.get("description"))
                .cloned();
            let started_at = a
                .starts_at
                .as_deref()
                .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| dt.with_timezone(&Utc));
            let fingerprint = a
                .fingerprint
                .unwrap_or_else(|| derive_fingerprint(source, &title, &a.labels));
            AlertEvent {
                fingerprint,
                status,
                title,
                severity,
                source,
                summary,
                started_at,
                labels: a.labels,
            }
        })
        .collect())
}

/// Parse the standard Prometheus Alertmanager webhook payload:
/// `{ status, alerts: [{ status, labels{alertname, severity, ...}, annotations{summary,
/// description}, startsAt, fingerprint }] }`. One [`AlertEvent`] per element of `alerts`.
///
/// # Errors
/// Returns [`ServiceError::Validation`] if `body` is not valid JSON matching this shape.
pub fn parse_alertmanager(body: &[u8]) -> ServiceResult<Vec<AlertEvent>> {
    parse_am_shaped(body, AlertSource::Alertmanager)
}

/// Parse a Grafana (≥10) unified-alerting contact-point webhook payload. Grafana reuses the
/// Alertmanager contact-point shape, so this shares the same tolerant parser as
/// [`parse_alertmanager`] — only the tagged [`AlertSource`] differs. If a future Grafana
/// version diverges from the Alertmanager shape, split this into its own struct/parser.
///
/// # Errors
/// Returns [`ServiceError::Validation`] if `body` is not valid JSON matching this shape.
pub fn parse_grafana(body: &[u8]) -> ServiceResult<Vec<AlertEvent>> {
    parse_am_shaped(body, AlertSource::Grafana)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── map_severity ─────────────────────────────────────────────────────────

    #[test]
    fn map_severity_table() {
        for label in ["critical", "crit", "p1", "sev1", "page", "CRITICAL"] {
            assert_eq!(map_severity(label), Some(IncidentSeverity::Sev1), "{label}");
        }
        for label in ["high", "error", "p2", "sev2"] {
            assert_eq!(map_severity(label), Some(IncidentSeverity::Sev2), "{label}");
        }
        for label in ["warning", "warn", "p3", "sev3"] {
            assert_eq!(map_severity(label), Some(IncidentSeverity::Sev3), "{label}");
        }
        for label in ["info", "low", "p4", "sev4"] {
            assert_eq!(map_severity(label), Some(IncidentSeverity::Sev4), "{label}");
        }
        assert_eq!(map_severity("bogus"), None);
    }

    // ── fingerprint derivation ───────────────────────────────────────────────

    #[test]
    fn derive_fingerprint_is_deterministic() {
        let mut labels = BTreeMap::new();
        labels.insert("host".to_string(), "web-1".to_string());
        let a = derive_fingerprint(AlertSource::Generic, "disk full", &labels);
        let b = derive_fingerprint(AlertSource::Generic, "disk full", &labels);
        assert_eq!(a, b);
        assert_eq!(a.len(), 32);
    }

    #[test]
    fn derive_fingerprint_differs_for_different_inputs() {
        let labels = BTreeMap::new();
        let a = derive_fingerprint(AlertSource::Generic, "disk full", &labels);
        let b = derive_fingerprint(AlertSource::Generic, "cpu high", &labels);
        assert_ne!(a, b);
    }

    // ── parse_generic ────────────────────────────────────────────────────────

    #[test]
    fn parse_generic_single_object() {
        let body =
            br#"{"title": "disk full", "severity": "sev1", "summary": "root partition at 95%"}"#;
        let events = parse_generic(body).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].title, "disk full");
        assert_eq!(
            events[0].status,
            AlertStatus::Firing,
            "status defaults to firing"
        );
        assert_eq!(events[0].severity, Some(IncidentSeverity::Sev1));
        assert_eq!(events[0].source, AlertSource::Generic);
        assert!(
            !events[0].fingerprint.is_empty(),
            "fingerprint must be derived when omitted"
        );
    }

    #[test]
    fn parse_generic_batch_with_explicit_fingerprint_and_status() {
        let body = br#"{"alerts": [
            {"title": "a", "status": "resolved", "fingerprint": "fp-1"},
            {"title": "b", "status": "firing", "fingerprint": "fp-2"}
        ]}"#;
        let events = parse_generic(body).unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].status, AlertStatus::Resolved);
        assert_eq!(events[0].fingerprint, "fp-1");
        assert_eq!(events[1].status, AlertStatus::Firing);
        assert_eq!(events[1].fingerprint, "fp-2");
    }

    #[test]
    fn parse_generic_malformed_json_is_validation_error() {
        let err = parse_generic(b"not json").unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[test]
    fn parse_generic_missing_title_is_validation_error() {
        let err = parse_generic(br#"{"severity": "sev1"}"#).unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }

    // ── parse_alertmanager ───────────────────────────────────────────────────

    fn alertmanager_sample() -> &'static [u8] {
        br#"{
            "version": "4",
            "status": "firing",
            "alerts": [
                {
                    "status": "firing",
                    "labels": {"alertname": "HighCPU", "severity": "critical", "instance": "web-1"},
                    "annotations": {"summary": "CPU above 90%", "description": "long desc"},
                    "startsAt": "2026-06-01T12:00:00Z",
                    "endsAt": "0001-01-01T00:00:00Z",
                    "fingerprint": "abc123"
                }
            ]
        }"#
    }

    #[test]
    fn parse_alertmanager_extracts_fields() {
        let events = parse_alertmanager(alertmanager_sample()).unwrap();
        assert_eq!(events.len(), 1);
        let e = &events[0];
        assert_eq!(e.title, "HighCPU");
        assert_eq!(e.severity, Some(IncidentSeverity::Sev1));
        assert_eq!(e.summary.as_deref(), Some("CPU above 90%"));
        assert_eq!(e.fingerprint, "abc123");
        assert_eq!(e.status, AlertStatus::Firing);
        assert_eq!(e.source, AlertSource::Alertmanager);
        assert!(e.started_at.is_some());
    }

    #[test]
    fn parse_alertmanager_resolved_status() {
        let body = br#"{"alerts": [{"status": "resolved", "labels": {"alertname": "X"}, "fingerprint": "fp"}]}"#;
        let events = parse_alertmanager(body).unwrap();
        assert_eq!(events[0].status, AlertStatus::Resolved);
    }

    #[test]
    fn parse_alertmanager_missing_fingerprint_is_derived() {
        let body = br#"{"alerts": [{"status": "firing", "labels": {"alertname": "X"}}]}"#;
        let events = parse_alertmanager(body).unwrap();
        assert!(!events[0].fingerprint.is_empty());
    }

    #[test]
    fn parse_alertmanager_malformed_is_validation_error() {
        let err = parse_alertmanager(b"{not json").unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }

    // ── parse_grafana ────────────────────────────────────────────────────────

    #[test]
    fn parse_grafana_extracts_fields_and_tags_source() {
        let body = br#"{
            "status": "firing",
            "alerts": [
                {
                    "status": "firing",
                    "labels": {"alertname": "PodCrashLooping", "severity": "warning"},
                    "annotations": {"description": "pod is crash-looping"},
                    "startsAt": "2026-06-01T08:00:00Z",
                    "fingerprint": "grafana-fp-1"
                }
            ]
        }"#;
        let events = parse_grafana(body).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].title, "PodCrashLooping");
        assert_eq!(events[0].severity, Some(IncidentSeverity::Sev3));
        assert_eq!(events[0].summary.as_deref(), Some("pod is crash-looping"));
        assert_eq!(events[0].source, AlertSource::Grafana);
    }
}
