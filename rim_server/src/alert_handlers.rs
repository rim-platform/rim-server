//! Alert-ingest webhook endpoints (Track B2, FL2 non-GraphQL transport).
//!
//! Machine-to-machine only: no session cookie, no CSRF. Auth is a per-workspace ingest token
//! presented in a header (never a cookie, so CSRF doesn't apply — see `docs/alerts.md`).
//! Depends on `rim_services` traits only (`IngestTokenService`, `IncidentService`), never on
//! `rim_infra_services` impls, per CLAUDE.md's FL2 pattern.

use std::sync::Arc;

use actix_web::{HttpRequest, HttpResponse, web};
use rim_infra_services::AlertRateLimiter;
use rim_services::alert::{self, AlertEvent, AlertSource};
use rim_services::incident::command::{IngestAlertCmd, IngestAlertOutcome};
use rim_services::incident::service::IncidentService;
use rim_services::ingest_token::service::IngestTokenService;
use rim_services::{Actor, ServiceError};

use crate::file_handlers::map_service_error_to_response;

/// Wraps the configured alert-ingest body size cap. A named type (rather than bare `web::Data<usize>`)
/// avoids colliding with any other `usize`-typed `app_data` — actix keys `app_data` by type, not name.
#[derive(Clone, Copy)]
pub struct AlertMaxBodyBytes(pub usize);

/// Extract the raw ingest token from the request.
///
/// Accepts `Authorization: Bearer <token>` (preferred) and, since some monitoring systems
/// can't easily set a custom `Authorization` value, `X-RIM-Ingest-Token: <token>` as a
/// fallback. Returns `None` if neither is present or well-formed.
fn extract_ingest_token(req: &HttpRequest) -> Option<String> {
    if let Some(value) = req
        .headers()
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        && let Some(token) = value.strip_prefix("Bearer ")
        && !token.is_empty()
    {
        return Some(token.to_string());
    }

    req.headers()
        .get("X-RIM-Ingest-Token")
        .and_then(|v| v.to_str().ok())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Per-alert outcome counts returned to the caller in the 202 response body.
#[derive(Default)]
struct OutcomeCounts {
    received: usize,
    created: usize,
    updated: usize,
    resolved: usize,
    ignored: usize,
}

impl OutcomeCounts {
    fn record(&mut self, outcome: &IngestAlertOutcome) {
        self.received += 1;
        match outcome {
            IngestAlertOutcome::Created(_) => self.created += 1,
            IngestAlertOutcome::Updated(_) => self.updated += 1,
            IngestAlertOutcome::Resolved(_) => self.resolved += 1,
            IngestAlertOutcome::Ignored => self.ignored += 1,
        }
    }

    fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "received": self.received,
            "created": self.created,
            "updated": self.updated,
            "resolved": self.resolved,
            "ignored": self.ignored,
        })
    }
}

/// Bundles the app-data dependencies shared by all three ingest endpoints, so the handler
/// functions stay under clippy's argument-count lint without losing per-parameter clarity.
struct IngestDeps<'a> {
    tokens: &'a dyn IngestTokenService,
    incidents: &'a dyn IncidentService,
    limiter: &'a AlertRateLimiter,
    max_body_bytes: usize,
}

/// Shared body for all three ingest endpoints: authenticate, rate-limit, parse, ingest.
///
/// `parse` turns the raw body into normalized [`AlertEvent`]s for the given `source`.
///
/// Body-size is enforced twice: in production, `actix_web::web::PayloadConfig` is attached to
/// each of the three routes (see `server.rs`) with the same `max_body_bytes` limit, so oversized
/// bodies are rejected by actix itself (also a 413) before `body: web::Bytes` finishes buffering
/// them — avoiding wasted memory/bandwidth on a request that's going to be rejected anyway. The
/// inline check below is the backstop: it's what actually fires in this module's own unit tests
/// (which build a bare `App` without the production `PayloadConfig` wiring) and it stays
/// authoritative for the exact response shape (`{"error": "..."}`) regardless of which layer
/// catches the oversized body.
async fn ingest_common(
    req: HttpRequest,
    body: web::Bytes,
    deps: IngestDeps<'_>,
    source: AlertSource,
    parse: impl Fn(&[u8]) -> rim_services::ServiceResult<Vec<AlertEvent>>,
) -> HttpResponse {
    let IngestDeps {
        tokens,
        incidents,
        limiter,
        max_body_bytes,
    } = deps;

    if body.len() > max_body_bytes {
        return HttpResponse::PayloadTooLarge().json(serde_json::json!({
            "error": format!("body exceeds {max_body_bytes} byte limit")
        }));
    }

    let Some(raw_token) = extract_ingest_token(&req) else {
        return HttpResponse::Unauthorized()
            .json(serde_json::json!({"error": "missing ingest token"}));
    };

    let token = match tokens.authenticate_token(&raw_token).await {
        Ok(t) => t,
        Err(e) => return map_service_error_to_response(e),
    };

    match limiter.check_and_record(&token.id.to_string(), chrono::Utc::now()) {
        rim_services::command::rate_limit::RateLimitDecision::Allowed => {}
        rim_services::command::rate_limit::RateLimitDecision::Limited {
            retry_after_seconds,
        } => {
            return map_service_error_to_response(ServiceError::RateLimited {
                retry_after_seconds,
            });
        }
    }

    let mut events = match parse(&body) {
        Ok(events) => events,
        Err(e) => return map_service_error_to_response(e),
    };

    // Unknown/omitted severity → the token's configured default, else Sev3 (final fallback
    // lives in `IncidentService::ingest_alert` itself).
    for event in &mut events {
        if event.severity.is_none() {
            event.severity = token.severity_default;
        }
    }

    let mut counts = OutcomeCounts::default();
    for event in events {
        let cmd = IngestAlertCmd {
            actor: Actor::new(token.created_by),
            server_id: token.server_id,
            conversation_id: token.conversation_id,
            source,
            event,
        };
        match incidents.ingest_alert(cmd).await {
            Ok(outcome) => counts.record(&outcome),
            // All alerts in one request share the same token → same server/conversation
            // context, so a failure here (stale membership, deleted conversation, ...) will
            // fail identically for every remaining event. Short-circuit rather than return a
            // misleading partial 202.
            Err(e) => return map_service_error_to_response(e),
        }
    }

    HttpResponse::Accepted().json(counts.to_json())
}

/// POST /api/hooks/alerts/generic — RIM's own documented JSON format (see `docs/alerts.md`).
pub async fn ingest_generic(
    req: HttpRequest,
    body: web::Bytes,
    tokens: web::Data<Arc<dyn IngestTokenService>>,
    incidents: web::Data<Arc<dyn IncidentService>>,
    limiter: web::Data<Arc<AlertRateLimiter>>,
    max_body_bytes: web::Data<AlertMaxBodyBytes>,
) -> HttpResponse {
    ingest_common(
        req,
        body,
        IngestDeps {
            tokens: tokens.as_ref().as_ref(),
            incidents: incidents.as_ref().as_ref(),
            limiter: limiter.as_ref().as_ref(),
            max_body_bytes: max_body_bytes.as_ref().0,
        },
        AlertSource::Generic,
        alert::parse_generic,
    )
    .await
}

/// POST /api/hooks/alerts/alertmanager — standard Prometheus Alertmanager webhook payload.
pub async fn ingest_alertmanager(
    req: HttpRequest,
    body: web::Bytes,
    tokens: web::Data<Arc<dyn IngestTokenService>>,
    incidents: web::Data<Arc<dyn IncidentService>>,
    limiter: web::Data<Arc<AlertRateLimiter>>,
    max_body_bytes: web::Data<AlertMaxBodyBytes>,
) -> HttpResponse {
    ingest_common(
        req,
        body,
        IngestDeps {
            tokens: tokens.as_ref().as_ref(),
            incidents: incidents.as_ref().as_ref(),
            limiter: limiter.as_ref().as_ref(),
            max_body_bytes: max_body_bytes.as_ref().0,
        },
        AlertSource::Alertmanager,
        alert::parse_alertmanager,
    )
    .await
}

/// POST /api/hooks/alerts/grafana — Grafana unified-alerting contact-point webhook payload.
pub async fn ingest_grafana(
    req: HttpRequest,
    body: web::Bytes,
    tokens: web::Data<Arc<dyn IngestTokenService>>,
    incidents: web::Data<Arc<dyn IncidentService>>,
    limiter: web::Data<Arc<AlertRateLimiter>>,
    max_body_bytes: web::Data<AlertMaxBodyBytes>,
) -> HttpResponse {
    ingest_common(
        req,
        body,
        IngestDeps {
            tokens: tokens.as_ref().as_ref(),
            incidents: incidents.as_ref().as_ref(),
            limiter: limiter.as_ref().as_ref(),
            max_body_bytes: max_body_bytes.as_ref().0,
        },
        AlertSource::Grafana,
        alert::parse_grafana,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::test::TestRequest;

    #[test]
    fn extract_ingest_token_prefers_authorization_bearer() {
        let req = TestRequest::default()
            .insert_header(("Authorization", "Bearer rim_ingest_abc123"))
            .insert_header(("X-RIM-Ingest-Token", "rim_ingest_other"))
            .to_http_request();
        assert_eq!(
            extract_ingest_token(&req),
            Some("rim_ingest_abc123".to_string())
        );
    }

    #[test]
    fn extract_ingest_token_falls_back_to_custom_header() {
        let req = TestRequest::default()
            .insert_header(("X-RIM-Ingest-Token", "rim_ingest_xyz"))
            .to_http_request();
        assert_eq!(
            extract_ingest_token(&req),
            Some("rim_ingest_xyz".to_string())
        );
    }

    #[test]
    fn extract_ingest_token_none_when_absent() {
        let req = TestRequest::default().to_http_request();
        assert_eq!(extract_ingest_token(&req), None);
    }

    #[test]
    fn extract_ingest_token_rejects_malformed_authorization() {
        // Missing "Bearer " prefix and no fallback header set.
        let req = TestRequest::default()
            .insert_header(("Authorization", "rim_ingest_no_bearer_prefix"))
            .to_http_request();
        assert_eq!(extract_ingest_token(&req), None);
    }

    #[test]
    fn outcome_counts_tally_each_variant() {
        use rim_core::conversation::model::ConversationId;
        use rim_core::incident::model::{Incident, IncidentId, IncidentStatus};
        use rim_core::server::model::ServerId;
        use rim_core::user::model::UserId;
        use rim_services::incident::dto::IncidentDto;

        let dummy = IncidentDto::from(Incident {
            id: IncidentId::new(),
            server_id: ServerId::new(),
            conversation_id: ConversationId::new(),
            started_by: UserId::new(),
            status: IncidentStatus::Active,
            severity: None,
            title: "t".to_string(),
            original_topic: None,
            started_at: chrono::Utc::now(),
            mitigated_at: None,
            resolved_at: None,
            postmortem_draft: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            alert_fingerprint: None,
            source: "generic".to_string(),
            escalation_policy_id: None,
            acknowledged_at: None,
            acknowledged_by: None,
        });

        let mut counts = OutcomeCounts::default();
        counts.record(&IngestAlertOutcome::Created(dummy.clone()));
        counts.record(&IngestAlertOutcome::Updated(dummy.clone()));
        counts.record(&IngestAlertOutcome::Resolved(dummy));
        counts.record(&IngestAlertOutcome::Ignored);

        assert_eq!(counts.received, 4);
        assert_eq!(counts.created, 1);
        assert_eq!(counts.updated, 1);
        assert_eq!(counts.resolved, 1);
        assert_eq!(counts.ignored, 1);
    }

    // ── handler-level tests (actix `test::TestRequest`) ─────────────────────────

    use actix_web::App;
    use actix_web::test as actix_test;
    use rim_core::conversation::model::ConversationId;
    use rim_core::incident::model::{Incident, IncidentId, IncidentStatus};
    use rim_core::ingest_token::model::IngestTokenId;
    use rim_core::server::model::ServerId;
    use rim_core::user::model::UserId;
    use rim_services::incident::dto::IncidentDto;
    use rim_services::incident::service::MockIncidentService;
    use rim_services::ingest_token::dto::IngestTokenDto;
    use rim_services::ingest_token::service::MockIngestTokenService;

    fn dummy_token_dto() -> IngestTokenDto {
        IngestTokenDto {
            id: IngestTokenId::new(),
            server_id: ServerId::new(),
            conversation_id: ConversationId::new(),
            created_by: UserId::new(),
            label: "test".to_string(),
            severity_default: None,
            created_at: chrono::Utc::now(),
            revoked_at: None,
            last_used_at: None,
        }
    }

    fn dummy_incident_dto() -> IncidentDto {
        IncidentDto::from(Incident {
            id: IncidentId::new(),
            server_id: ServerId::new(),
            conversation_id: ConversationId::new(),
            started_by: UserId::new(),
            status: IncidentStatus::Active,
            severity: None,
            title: "t".to_string(),
            original_topic: None,
            started_at: chrono::Utc::now(),
            mitigated_at: None,
            resolved_at: None,
            postmortem_draft: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            alert_fingerprint: None,
            source: "generic".to_string(),
            escalation_policy_id: None,
            acknowledged_at: None,
            acknowledged_by: None,
        })
    }

    macro_rules! test_app {
        ($tokens:expr, $incidents:expr, $limiter:expr, $max_bytes:expr) => {
            actix_test::init_service(
                App::new()
                    .service(
                        web::resource("/api/hooks/alerts/generic")
                            .route(web::post().to(ingest_generic)),
                    )
                    .app_data(web::Data::new(
                        std::sync::Arc::new($tokens) as std::sync::Arc<dyn IngestTokenService>
                    ))
                    .app_data(web::Data::new(
                        std::sync::Arc::new($incidents) as std::sync::Arc<dyn IncidentService>
                    ))
                    .app_data(web::Data::new(std::sync::Arc::new($limiter)))
                    .app_data(web::Data::new(AlertMaxBodyBytes($max_bytes))),
            )
            .await
        };
    }

    #[actix_web::test]
    async fn returns_401_when_token_missing() {
        let app = test_app!(
            MockIngestTokenService::new(),
            MockIncidentService::new(),
            AlertRateLimiter::new(60, 120),
            262_144
        );

        let req = actix_test::TestRequest::post()
            .uri("/api/hooks/alerts/generic")
            .set_json(serde_json::json!({"title": "x"}))
            .to_request();
        let resp = actix_test::call_service(&app, req).await;
        assert_eq!(resp.status(), 401);
    }

    #[actix_web::test]
    async fn returns_401_when_token_unauthenticated() {
        let mut tokens = MockIngestTokenService::new();
        tokens
            .expect_authenticate_token()
            .returning(|_| Box::pin(async { Err(ServiceError::Unauthenticated) }));

        let app = test_app!(
            tokens,
            MockIncidentService::new(),
            AlertRateLimiter::new(60, 120),
            262_144
        );

        let req = actix_test::TestRequest::post()
            .uri("/api/hooks/alerts/generic")
            .insert_header(("Authorization", "Bearer rim_ingest_bogus"))
            .set_json(serde_json::json!({"title": "x"}))
            .to_request();
        let resp = actix_test::call_service(&app, req).await;
        assert_eq!(resp.status(), 401);
    }

    #[actix_web::test]
    async fn returns_429_over_rate_limit_with_retry_after() {
        let mut tokens = MockIngestTokenService::new();
        tokens
            .expect_authenticate_token()
            .returning(|_| Box::pin(async { Ok(dummy_token_dto()) }));

        // Limit of 0 → the very first call is already over budget.
        let app = test_app!(
            tokens,
            MockIncidentService::new(),
            AlertRateLimiter::new(60, 0),
            262_144
        );

        let req = actix_test::TestRequest::post()
            .uri("/api/hooks/alerts/generic")
            .insert_header(("Authorization", "Bearer rim_ingest_ok"))
            .set_json(serde_json::json!({"title": "x"}))
            .to_request();
        let resp = actix_test::call_service(&app, req).await;
        assert_eq!(resp.status(), 429);
        assert!(resp.headers().contains_key("Retry-After"));
    }

    #[actix_web::test]
    async fn returns_400_on_malformed_body() {
        let mut tokens = MockIngestTokenService::new();
        tokens
            .expect_authenticate_token()
            .returning(|_| Box::pin(async { Ok(dummy_token_dto()) }));

        let app = test_app!(
            tokens,
            MockIncidentService::new(),
            AlertRateLimiter::new(60, 120),
            262_144
        );

        let req = actix_test::TestRequest::post()
            .uri("/api/hooks/alerts/generic")
            .insert_header(("Authorization", "Bearer rim_ingest_ok"))
            .insert_header(("Content-Type", "application/json"))
            .set_payload("not json")
            .to_request();
        let resp = actix_test::call_service(&app, req).await;
        assert_eq!(resp.status(), 400);
    }

    #[actix_web::test]
    async fn returns_413_on_oversize_body() {
        let app = test_app!(
            MockIngestTokenService::new(),
            MockIncidentService::new(),
            AlertRateLimiter::new(60, 120),
            8 // tiny cap
        );

        let req = actix_test::TestRequest::post()
            .uri("/api/hooks/alerts/generic")
            .insert_header(("Authorization", "Bearer rim_ingest_ok"))
            .insert_header(("Content-Type", "application/json"))
            .set_payload(
                serde_json::json!({"title": "a much longer payload than 8 bytes"}).to_string(),
            )
            .to_request();
        let resp = actix_test::call_service(&app, req).await;
        assert_eq!(resp.status(), 413);
    }

    #[actix_web::test]
    async fn returns_202_with_outcome_summary_on_success() {
        let mut tokens = MockIngestTokenService::new();
        tokens
            .expect_authenticate_token()
            .returning(|_| Box::pin(async { Ok(dummy_token_dto()) }));

        let mut incidents = MockIncidentService::new();
        incidents.expect_ingest_alert().returning(|_| {
            Box::pin(async { Ok(IngestAlertOutcome::Created(dummy_incident_dto())) })
        });

        let app = test_app!(tokens, incidents, AlertRateLimiter::new(60, 120), 262_144);

        let req = actix_test::TestRequest::post()
            .uri("/api/hooks/alerts/generic")
            .insert_header(("Authorization", "Bearer rim_ingest_ok"))
            .set_json(serde_json::json!({"title": "disk full", "severity": "sev1"}))
            .to_request();
        let resp = actix_test::call_service(&app, req).await;
        assert_eq!(resp.status(), 202);

        let body: serde_json::Value = actix_test::read_body_json(resp).await;
        assert_eq!(body["received"], 1);
        assert_eq!(body["created"], 1);
    }
}
