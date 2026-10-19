//! Prometheus metrics: the recorder, HTTP request metrics by route class, the server-to-server
//! and forwarded request outcomes, the database and relay queue samplers, and the `/metrics`
//! listener that serves them on its own address.

use std::net::SocketAddr;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use actix_web::body::MessageBody;
use actix_web::dev::{ServiceRequest, ServiceResponse};
use actix_web::http::StatusCode;
use actix_web::middleware::Next;
use actix_web::{App, HttpResponse, HttpServer, web};
use metrics::{
    Unit, counter, describe_counter, describe_gauge, describe_histogram, gauge, histogram,
};
use metrics_exporter_prometheus::{Matcher, PrometheusBuilder, PrometheusHandle};
use rim_core::monitoring::MonitoringRepository;
use rim_infra_db::sea_orm_impl::relay::SeaOrmRelayRepository;
use sea_orm::{DatabaseConnection, DbBackend};
use tokio::sync::watch;

/// HTTP requests, labelled `route` (route class) and `status` (status class).
pub(crate) const HTTP_REQUESTS: &str = "rim_http_requests_total";
/// HTTP request duration until the response head, labelled like [`HTTP_REQUESTS`].
pub(crate) const HTTP_DURATION: &str = "rim_http_request_duration_seconds";
/// Requests under `/s2s/v1/`, labelled `outcome`.
pub(crate) const S2S_REQUESTS: &str = "rim_s2s_requests_total";
/// Forwarded requests, labelled `direction` (`outgoing` from this relay, `incoming` at this
/// host) and `outcome`.
pub(crate) const FORWARDED_REQUESTS: &str = "rim_forwarded_requests_total";
/// Open connections in the database pool.
pub(crate) const DB_POOL_CONNECTIONS: &str = "rim_db_pool_connections";
/// Pool connections checked out by a query or transaction.
pub(crate) const DB_POOL_IN_USE: &str = "rim_db_pool_connections_in_use";
/// The most connections the pool opens.
pub(crate) const DB_POOL_MAX: &str = "rim_db_pool_max_connections";
/// Migrations the binary knows that the database has not applied.
pub(crate) const PENDING_MIGRATIONS: &str = "rim_pending_migrations";
/// Always 1, labelled `version`.
pub(crate) const BUILD_INFO: &str = "rim_build_info";
/// Requests refused because their address is on the IP deny list.
pub(crate) const IP_BLOCKED: &str = "rim_ip_blocked_requests_total";

/// Histogram buckets of [`HTTP_DURATION`], in seconds.
const DURATION_BUCKETS: &[f64] = &[
    0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0,
];

/// How often the relay queue and the pending migrations are read from the database.
const SAMPLE_INTERVAL: Duration = Duration::from_secs(30);

/// How long one database sample may take before it is skipped.
const SAMPLE_TIMEOUT: Duration = Duration::from_secs(5);

/// The handle of the process-wide recorder, once installed.
static HANDLE: OnceLock<PrometheusHandle> = OnceLock::new();

/// The route class of a request path: a fixed set of values, so the label stays bounded
/// whatever paths clients send.
pub(crate) fn route_class(path: &str) -> &'static str {
    let under = |prefix: &str| {
        path.strip_prefix(prefix)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
    };
    if under("/api/remote") {
        "remote-forward"
    } else if under("/api/files")
        || under("/api/attachments")
        || under("/api/reports")
        || under("/api/plugin-bundles")
    {
        "files"
    } else if under("/api") {
        "api"
    } else if under("/s2s") {
        "s2s"
    } else if under("/.well-known") {
        "wellknown"
    } else if matches!(path, "/healthz" | "/readyz" | "/livez") {
        "health"
    } else {
        "other"
    }
}

/// The status class of an HTTP status: `1xx` to `5xx`.
pub(crate) fn status_class(status: u16) -> &'static str {
    match status {
        100..=199 => "1xx",
        200..=299 => "2xx",
        300..=399 => "3xx",
        400..=499 => "4xx",
        _ => "5xx",
    }
}

/// Count one HTTP request and record how long it took.
pub(crate) fn record_request(route: &'static str, status: u16, elapsed: Duration) {
    let status = status_class(status);
    counter!(HTTP_REQUESTS, "route" => route, "status" => status).increment(1);
    histogram!(HTTP_DURATION, "route" => route, "status" => status).record(elapsed.as_secs_f64());
}

/// Middleware recording [`HTTP_REQUESTS`] and [`HTTP_DURATION`] for every request.
pub(crate) async fn record_http(
    req: ServiceRequest,
    next: Next<impl MessageBody>,
) -> Result<ServiceResponse<impl MessageBody>, actix_web::Error> {
    let route = route_class(req.path());
    let started = Instant::now();
    let result = next.call(req).await;
    let status = match &result {
        Ok(response) => response.status(),
        Err(e) => e.as_response_error().status_code(),
    };
    record_request(route, status.as_u16(), started.elapsed());
    result
}

/// The outcome of a server-to-server request: `verified` says whether every signature, peer,
/// nonce and rate check passed; `status` is the status answered.
pub(crate) fn s2s_outcome(verified: bool, status: StatusCode) -> &'static str {
    match (verified, status.as_u16()) {
        (_, 429) => "rate_limited",
        (true, s) if s >= 500 => "error",
        (true, _) => "ok",
        (false, 401 | 409) => "signature_invalid",
        (false, 403) => "peer_blocked",
        (false, _) => "error",
    }
}

/// Count one server-to-server request under `outcome`.
pub(crate) fn s2s_request(outcome: &'static str) {
    counter!(S2S_REQUESTS, "outcome" => outcome).increment(1);
}

/// The outcome of a forwarded request from the status answered.
pub(crate) fn forward_outcome(status: StatusCode) -> &'static str {
    match status.as_u16() {
        429 => "rate_limited",
        s if s >= 500 => "error",
        s if s >= 400 => "refused",
        _ => "ok",
    }
}

/// Count one forwarded request.
pub(crate) fn forwarded(direction: &'static str, outcome: &'static str) {
    counter!(FORWARDED_REQUESTS, "direction" => direction, "outcome" => outcome).increment(1);
}

/// Count one request refused for its blocked address.
pub(crate) fn ip_blocked() {
    counter!(IP_BLOCKED).increment(1);
}

/// Register the help text and units of every metric with the installed recorder.
fn describe() {
    describe_counter!(
        HTTP_REQUESTS,
        Unit::Count,
        "HTTP requests by route class and status class."
    );
    describe_histogram!(
        HTTP_DURATION,
        Unit::Seconds,
        "HTTP request duration until the response head, by route class and status class."
    );
    describe_counter!(
        S2S_REQUESTS,
        Unit::Count,
        "Server-to-server requests received, by outcome."
    );
    describe_counter!(
        FORWARDED_REQUESTS,
        Unit::Count,
        "Forwarded requests sent as a relay (outgoing) or run as a host (incoming), by outcome."
    );
    describe_gauge!(
        DB_POOL_CONNECTIONS,
        Unit::Count,
        "Open connections in the database pool."
    );
    describe_gauge!(
        DB_POOL_IN_USE,
        Unit::Count,
        "Database pool connections in use."
    );
    describe_gauge!(
        DB_POOL_MAX,
        Unit::Count,
        "The most connections the database pool opens."
    );
    describe_gauge!(
        PENDING_MIGRATIONS,
        Unit::Count,
        "Migrations this binary knows that the database has not applied."
    );
    describe_counter!(
        IP_BLOCKED,
        Unit::Count,
        "Requests refused because their address is on the IP deny list."
    );
    describe_gauge!(
        BUILD_INFO,
        "Always 1; the version label is the server version."
    );
    rim_infra_services::metrics::describe();
    rim_infra_services::usage_metrics::describe();
    rim_api::describe_metrics();
    crate::ws::describe_metrics();
}

/// The exporter configuration: histogram buckets for the request duration.
fn builder() -> Result<PrometheusBuilder, String> {
    PrometheusBuilder::new()
        .set_buckets_for_metric(Matcher::Full(HTTP_DURATION.to_string()), DURATION_BUCKETS)
        .and_then(|builder| {
            builder.set_buckets_for_metric(
                Matcher::Full("rim_graphql_operation_duration_seconds".to_string()),
                DURATION_BUCKETS,
            )
        })
        .and_then(|builder| {
            builder.set_buckets_for_metric(
                Matcher::Full("rim_graphql_subscription_duration_seconds".to_string()),
                &[1.0, 5.0, 15.0, 60.0, 300.0, 900.0, 3600.0, 21600.0, 86400.0],
            )
        })
        .map_err(|e| format!("metrics buckets: {e}"))
}

/// Install the Prometheus recorder for the process, once, and set `rim_build_info`. A second
/// call returns the same handle.
///
/// # Errors
/// A message when another recorder was installed first.
pub(crate) fn install() -> Result<PrometheusHandle, String> {
    if let Some(handle) = HANDLE.get() {
        return Ok(handle.clone());
    }
    match builder()?.install_recorder() {
        Ok(handle) => {
            let handle = HANDLE.get_or_init(|| handle).clone();
            describe();
            gauge!(BUILD_INFO, "version" => env!("CARGO_PKG_VERSION")).set(1.0);
            Ok(handle)
        }
        Err(e) => HANDLE
            .get()
            .cloned()
            .ok_or_else(|| format!("could not install the metrics recorder: {e}")),
    }
}

/// Set the pool gauges from the pool's current state.
fn sample_pool(db: &DatabaseConnection) {
    if db.get_database_backend() != DbBackend::Postgres {
        return;
    }
    let pool = db.get_postgres_connection_pool();
    let size = pool.size();
    let idle = u32::try_from(pool.num_idle()).unwrap_or(u32::MAX);
    gauge!(DB_POOL_CONNECTIONS).set(f64::from(size));
    gauge!(DB_POOL_IN_USE).set(f64::from(size.saturating_sub(idle)));
    gauge!(DB_POOL_MAX).set(f64::from(pool.options().get_max_connections()));
}

/// Read the relay queue and the pending migrations and set their gauges; a read that fails or
/// takes too long leaves its gauge as it was.
async fn sample_database(db: &DatabaseConnection, monitoring_repo: &dyn MonitoringRepository) {
    let relay_repo = SeaOrmRelayRepository::new(db.clone().into());
    match tokio::time::timeout(
        SAMPLE_TIMEOUT,
        rim_infra_services::metrics::sample_relay_queue(&relay_repo),
    )
    .await
    {
        Ok(Ok(())) => rim_infra_services::usage_metrics::sample_outcome("relay", true),
        result => {
            rim_infra_services::usage_metrics::sample_outcome("relay", false);
            tracing::warn!(?result, "could not sample the relay queue");
        }
    }
    match tokio::time::timeout(SAMPLE_TIMEOUT, crate::health::count_pending_migrations(db)).await {
        Ok(Ok(pending)) => {
            gauge!(PENDING_MIGRATIONS).set(pending as f64);
            rim_infra_services::usage_metrics::sample_outcome("migrations", true);
        }
        result => {
            rim_infra_services::usage_metrics::sample_outcome("migrations", false);
            tracing::warn!(?result, "could not sample the pending migrations");
        }
    }
    for sample in ["usage", "delivery"] {
        let result = tokio::time::timeout(SAMPLE_TIMEOUT, async {
            match sample {
                "usage" => rim_infra_services::usage_metrics::sample_usage(monitoring_repo).await,
                _ => rim_infra_services::usage_metrics::sample_delivery(monitoring_repo).await,
            }
        })
        .await;
        rim_infra_services::usage_metrics::sample_outcome(sample, matches!(result, Ok(Ok(()))));
        if !matches!(result, Ok(Ok(()))) {
            tracing::warn!(sample, ?result, "could not sample database aggregates");
        }
    }
}

/// What the `/metrics` route needs.
#[derive(Clone)]
pub(crate) struct MetricsState {
    pub(crate) handle: PrometheusHandle,
    /// The database whose pool is sampled on each scrape; `None` samples nothing.
    pub(crate) db: Option<DatabaseConnection>,
}

/// `GET /metrics`: the Prometheus text exposition.
async fn scrape(state: web::Data<MetricsState>) -> HttpResponse {
    if let Some(db) = &state.db {
        sample_pool(db);
    }
    state.handle.run_upkeep();
    HttpResponse::Ok()
        .content_type("text/plain; version=0.0.4; charset=utf-8")
        .body(state.handle.render())
}

/// Add the `/metrics` route and its state to an app.
pub(crate) fn configure(state: MetricsState) -> impl FnOnce(&mut web::ServiceConfig) {
    move |cfg| {
        cfg.app_data(web::Data::new(state))
            .service(web::resource("/metrics").route(web::get().to(scrape)));
    }
}

/// Install the recorder, bind `addr` and serve `/metrics` there, and read the relay queue and
/// pending migrations every [`SAMPLE_INTERVAL`], until `shutdown` changes or its sender is
/// dropped.
///
/// # Errors
/// When the recorder cannot be installed or the address cannot be bound.
pub(crate) fn start(
    addr: SocketAddr,
    db: DatabaseConnection,
    monitoring_repo: Arc<dyn MonitoringRepository>,
    shutdown: watch::Receiver<bool>,
) -> std::io::Result<()> {
    let handle = install().map_err(std::io::Error::other)?;
    let state = MetricsState {
        handle,
        db: Some(db.clone()),
    };
    let server = HttpServer::new(move || App::new().configure(configure(state.clone())))
        .workers(1)
        .disable_signals()
        .bind(addr)?
        .run();
    let server_handle = server.handle();
    tracing::info!(%addr, "serving Prometheus metrics at /metrics");
    tokio::spawn(async move {
        if let Err(e) = server.await {
            tracing::error!(error = %e, "the metrics listener stopped");
        }
    });
    let mut shutdown_for_sampler = shutdown.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(SAMPLE_INTERVAL);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = tick.tick() => sample_database(&db, monitoring_repo.as_ref()).await,
                _ = shutdown_for_sampler.changed() => break,
            }
        }
    });
    let mut shutdown = shutdown;
    tokio::spawn(async move {
        let _ = shutdown.changed().await;
        server_handle.stop(true).await;
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use actix_web::test as actix_test;

    use super::*;

    #[test]
    fn routes_fall_into_a_fixed_set_of_classes() {
        for (path, class) in [
            ("/api", "api"),
            ("/api/ws", "api"),
            ("/api/sdl", "api"),
            ("/api/remote/rims1_x/api", "remote-forward"),
            ("/api/remote", "remote-forward"),
            ("/api/files/upload", "files"),
            ("/api/files/a/b.png", "files"),
            ("/api/attachments/1/download", "files"),
            ("/api/reports/1/attachments/abc", "files"),
            ("/api/plugin-bundles/rims1_x/abc", "files"),
            ("/apifoo", "other"),
            ("/s2s/v1/deliver", "s2s"),
            ("/s2s/v1/forward/api", "s2s"),
            ("/.well-known/rim-server", "wellknown"),
            ("/healthz", "health"),
            ("/readyz", "health"),
            ("/livez", "health"),
            ("/healthz/extra", "other"),
            ("/", "other"),
            ("/metrics", "other"),
            ("/wp-login.php", "other"),
        ] {
            assert_eq!(route_class(path), class, "{path}");
        }
    }

    #[test]
    fn statuses_fall_into_classes() {
        assert_eq!(status_class(101), "1xx");
        assert_eq!(status_class(200), "2xx");
        assert_eq!(status_class(304), "3xx");
        assert_eq!(status_class(429), "4xx");
        assert_eq!(status_class(503), "5xx");
        assert_eq!(status_class(999), "5xx");
    }

    #[test]
    fn s2s_outcomes_separate_signature_peer_and_rate_refusals() {
        let unverified = |s: u16| s2s_outcome(false, StatusCode::from_u16(s).unwrap());
        let verified = |s: u16| s2s_outcome(true, StatusCode::from_u16(s).unwrap());
        assert_eq!(unverified(401), "signature_invalid");
        assert_eq!(unverified(409), "signature_invalid");
        assert_eq!(unverified(403), "peer_blocked");
        assert_eq!(unverified(429), "rate_limited");
        assert_eq!(unverified(400), "error");
        assert_eq!(unverified(413), "error");
        assert_eq!(unverified(500), "error");
        assert_eq!(verified(200), "ok");
        assert_eq!(verified(404), "ok");
        assert_eq!(verified(429), "rate_limited");
        assert_eq!(verified(503), "error");
    }

    #[test]
    fn forward_outcomes_follow_the_status() {
        let outcome = |s: u16| forward_outcome(StatusCode::from_u16(s).unwrap());
        assert_eq!(outcome(200), "ok");
        assert_eq!(outcome(403), "refused");
        assert_eq!(outcome(409), "refused");
        assert_eq!(outcome(429), "rate_limited");
        assert_eq!(outcome(502), "error");
    }

    /// A recorder of the test's own, so tests do not share the process-wide one.
    fn local_recorder() -> metrics_exporter_prometheus::PrometheusRecorder {
        builder().expect("builder").build_recorder()
    }

    async fn scrape_text(handle: PrometheusHandle) -> (StatusCode, String, String) {
        let app = actix_test::init_service(
            App::new().configure(configure(MetricsState { handle, db: None })),
        )
        .await;
        let response = actix_test::call_service(
            &app,
            actix_test::TestRequest::get().uri("/metrics").to_request(),
        )
        .await;
        let status = response.status();
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let body = actix_test::read_body(response).await;
        (
            status,
            content_type,
            String::from_utf8(body.to_vec()).expect("utf-8"),
        )
    }

    #[actix_web::test]
    async fn the_metrics_route_serves_what_was_recorded() {
        let recorder = local_recorder();
        let handle = recorder.handle();
        metrics::with_local_recorder(&recorder, || {
            describe();
            gauge!(BUILD_INFO, "version" => env!("CARGO_PKG_VERSION")).set(1.0);
            record_request("api", 200, Duration::from_millis(30));
            record_request("api", 200, Duration::from_millis(70));
            record_request("other", 404, Duration::from_millis(1));
            s2s_request(s2s_outcome(false, StatusCode::UNAUTHORIZED));
            forwarded("outgoing", "upstream_unavailable");
            rim_infra_services::metrics::rate_limited("s2s_peer");
        });
        let (status, content_type, body) = scrape_text(handle).await;
        assert_eq!(status, StatusCode::OK);
        assert!(content_type.starts_with("text/plain"), "{content_type}");
        for line in [
            &format!(
                "rim_build_info{{version=\"{}\"}} 1",
                env!("CARGO_PKG_VERSION")
            ),
            "rim_http_requests_total{route=\"api\",status=\"2xx\"} 2",
            "rim_http_requests_total{route=\"other\",status=\"4xx\"} 1",
            "rim_http_request_duration_seconds_bucket{route=\"api\",status=\"2xx\",le=\"0.05\"} 1",
            "rim_http_request_duration_seconds_bucket{route=\"api\",status=\"2xx\",le=\"0.1\"} 2",
            "rim_http_request_duration_seconds_count{route=\"api\",status=\"2xx\"} 2",
            "rim_s2s_requests_total{outcome=\"signature_invalid\"} 1",
            "rim_forwarded_requests_total{direction=\"outgoing\",outcome=\"upstream_unavailable\"} 1",
            "rim_rate_limit_rejections_total{limiter=\"s2s_peer\"} 1",
            "rim_registrations_total 0",
            "rim_messages_created_total{encryption=\"plaintext\"} 0",
            "rim_messages_created_total{encryption=\"encrypted\"} 0",
            "rim_websocket_connections 0",
            "rim_graphql_subscriptions 0",
            "rim_graphql_failed_operations_total{kind=\"internal\"} 0",
            "rim_database_sample_success{sample=\"usage\"} 0",
            "# TYPE rim_http_request_duration_seconds histogram",
            "# HELP rim_s2s_requests_total Server-to-server requests received, by outcome.",
        ] {
            assert!(body.contains(line), "missing {line:?} in:\n{body}");
        }
    }

    #[actix_web::test]
    async fn the_middleware_records_each_request_by_route_class() {
        let recorder = local_recorder();
        let handle = recorder.handle();
        // The test runtime runs on this thread, so the middleware records into this recorder.
        let guard = metrics::set_default_local_recorder(&recorder);
        let app = actix_test::init_service(
            App::new()
                .wrap(actix_web::middleware::from_fn(record_http))
                .route("/api", web::post().to(HttpResponse::Ok))
                .route(
                    "/s2s/v1/{tail:.*}",
                    web::post().to(HttpResponse::Unauthorized),
                ),
        )
        .await;
        for (method, uri) in [
            (actix_web::http::Method::POST, "/api"),
            (actix_web::http::Method::POST, "/api"),
            (actix_web::http::Method::POST, "/s2s/v1/deliver"),
            (actix_web::http::Method::GET, "/no/such/path"),
        ] {
            actix_test::call_service(
                &app,
                actix_test::TestRequest::default()
                    .method(method)
                    .uri(uri)
                    .to_request(),
            )
            .await;
        }
        drop(guard);
        let (_, _, body) = scrape_text(handle).await;
        for line in [
            "rim_http_requests_total{route=\"api\",status=\"2xx\"} 2",
            "rim_http_requests_total{route=\"s2s\",status=\"4xx\"} 1",
            "rim_http_requests_total{route=\"other\",status=\"4xx\"} 1",
            "rim_http_request_duration_seconds_count{route=\"api\",status=\"2xx\"} 2",
        ] {
            assert!(body.contains(line), "missing {line:?} in:\n{body}");
        }
        assert!(
            !body.contains("/no/such/path"),
            "raw paths stay out of labels"
        );
    }
}
