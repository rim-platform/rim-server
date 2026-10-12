//! Limits applied at the HTTP edge: the client address of a request, the per-address request
//! limit on `/api` routes, the per-session mutation budget, the GraphQL batch cap and the
//! per-user WebSocket connection cap.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv6Addr, SocketAddr};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use actix_web::body::{EitherBody, MessageBody};
use actix_web::dev::{ServiceRequest, ServiceResponse};
use actix_web::middleware::Next;
use actix_web::{HttpRequest, HttpResponse, web};
use rim_core::auth::model::SessionId;
use rim_core::user::model::UserId;
use rim_infra_services::token_bucket::TokenBucketLimiter;
use rim_services::command::rate_limit::RateLimitDecision;

use crate::config::ApiLimits;

/// The address a request comes from: with `trust_forwarded_headers`, the client named by
/// `Forwarded` or `X-Forwarded-For` when it parses as an address, else the TCP peer. An
/// IPv4-mapped IPv6 address is its IPv4 address.
pub(crate) fn client_ip(req: &HttpRequest, trust_forwarded_headers: bool) -> Option<IpAddr> {
    let forwarded = if trust_forwarded_headers {
        req.connection_info()
            .realip_remote_addr()
            .and_then(parse_address)
    } else {
        None
    };
    forwarded
        .or_else(|| req.peer_addr().map(|addr| addr.ip()))
        .map(|ip| ip.to_canonical())
}

/// An address as a proxy header writes it: bare, with a port, or bracketed IPv6.
fn parse_address(value: &str) -> Option<IpAddr> {
    let value = value.trim();
    value
        .parse::<IpAddr>()
        .ok()
        .or_else(|| value.parse::<SocketAddr>().ok().map(|addr| addr.ip()))
        .or_else(|| {
            value
                .strip_prefix('[')
                .and_then(|v| v.strip_suffix(']'))
                .and_then(|v| v.parse::<Ipv6Addr>().ok())
                .map(IpAddr::V6)
        })
}

/// The key an address is rate-limited under: IPv4 as is, IPv6 by its /64, so one host cannot
/// rotate through its interface identifiers.
fn rate_key(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V4(_) => ip,
        IpAddr::V6(v6) => IpAddr::V6(Ipv6Addr::from(
            u128::from(v6) & 0xffff_ffff_ffff_ffff_0000_0000_0000_0000,
        )),
    }
}

/// Whether `path` is `/api` or under it.
fn is_api_path(path: &str) -> bool {
    path.strip_prefix("/api")
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

/// `429` with `Retry-After` and `{"error":"rate_limited","retryAfterSeconds":n}`.
pub(crate) fn rate_limited_response(retry_after_seconds: u64) -> HttpResponse {
    HttpResponse::TooManyRequests()
        .insert_header(("Retry-After", retry_after_seconds.to_string()))
        .json(serde_json::json!({
            "error": "rate_limited",
            "retryAfterSeconds": retry_after_seconds,
        }))
}

/// `400` with `{"error":"batch_too_large"}`.
pub(crate) fn batch_too_large_response() -> HttpResponse {
    HttpResponse::BadRequest().json(serde_json::json!({ "error": "batch_too_large" }))
}

/// Who a mutation budget belongs to: the session, or the address of a caller without one.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum MutationCaller {
    Session(SessionId),
    Address(IpAddr),
}

/// The edge limits of one server, shared by its workers.
pub(crate) struct EdgeLimits {
    trust_forwarded_headers: bool,
    per_ip: Option<TokenBucketLimiter<IpAddr>>,
    mutations: Option<TokenBucketLimiter<MutationCaller>>,
    ws_connections: Option<Arc<WsConnections>>,
    subscriptions_per_connection: Option<u32>,
    max_batch: usize,
}

impl EdgeLimits {
    pub(crate) fn new(limits: ApiLimits, trust_forwarded_headers: bool) -> Self {
        Self {
            trust_forwarded_headers,
            per_ip: limits.requests_per_ip.map(TokenBucketLimiter::new),
            mutations: limits.mutations_per_session.map(TokenBucketLimiter::new),
            ws_connections: limits.ws_connections_per_user.map(|limit| {
                Arc::new(WsConnections {
                    limit,
                    open: Mutex::new(HashMap::new()),
                })
            }),
            subscriptions_per_connection: limits.subscriptions_per_connection,
            max_batch: limits.max_batch,
        }
    }

    /// The `429` for a request to an `/api` route its address may not send now.
    fn refuse_api_request(&self, req: &HttpRequest, now: Instant) -> Option<HttpResponse> {
        let limiter = self.per_ip.as_ref()?;
        if !is_api_path(req.path()) {
            return None;
        }
        let ip = client_ip(req, self.trust_forwarded_headers)?;
        match limiter.check(&rate_key(ip), now) {
            RateLimitDecision::Allowed => None,
            RateLimitDecision::Limited {
                retry_after_seconds,
            } => {
                rim_infra_services::metrics::rate_limited("api_ip");
                Some(rate_limited_response(retry_after_seconds))
            }
        }
    }

    /// Whether a batch of `len` requests is over the cap.
    pub(crate) fn batch_too_large(&self, len: usize) -> bool {
        len > self.max_batch
    }

    /// Spend `fields` mutation fields from `caller`'s budget, or the seconds until they are
    /// back.
    pub(crate) fn spend_mutations(&self, caller: &MutationCaller, fields: u32) -> Result<(), u64> {
        let Some(limiter) = &self.mutations else {
            return Ok(());
        };
        if fields == 0 {
            return Ok(());
        }
        match limiter.take(caller, fields, Instant::now()) {
            RateLimitDecision::Allowed => Ok(()),
            RateLimitDecision::Limited {
                retry_after_seconds,
            } => {
                rim_infra_services::metrics::rate_limited("api_mutations");
                Err(retry_after_seconds)
            }
        }
    }

    /// A slot for one more WebSocket connection of `user`, `Ok(None)` when connections are not
    /// capped, or `Err` when the user holds as many as allowed.
    pub(crate) fn open_ws(&self, user: UserId) -> Result<Option<WsConnectionSlot>, ()> {
        match &self.ws_connections {
            None => Ok(None),
            Some(connections) => WsConnections::open(connections, user).map(Some),
        }
    }

    /// The subscriptions one WebSocket connection may hold; `None` sets no cap.
    pub(crate) fn subscriptions_per_connection(&self) -> Option<u32> {
        self.subscriptions_per_connection
    }
}

/// Middleware answering `429` to a request to an `/api` route over its address's limit.
pub(crate) async fn limit_api(
    req: ServiceRequest,
    next: Next<impl MessageBody>,
) -> Result<ServiceResponse<EitherBody<impl MessageBody>>, actix_web::Error> {
    let refusal = req
        .app_data::<web::Data<EdgeLimits>>()
        .and_then(|limits| limits.refuse_api_request(req.request(), Instant::now()));
    if let Some(response) = refusal {
        return Ok(req.into_response(response).map_into_right_body());
    }
    next.call(req)
        .await
        .map(ServiceResponse::map_into_left_body)
}

/// Open WebSocket connections per user.
struct WsConnections {
    limit: u32,
    open: Mutex<HashMap<UserId, u32>>,
}

impl WsConnections {
    fn open(this: &Arc<Self>, user: UserId) -> Result<WsConnectionSlot, ()> {
        let mut open = this.open.lock().unwrap_or_else(PoisonError::into_inner);
        let count = open.entry(user).or_insert(0);
        if *count >= this.limit {
            if *count == 0 {
                open.remove(&user);
            }
            rim_infra_services::metrics::rate_limited("ws_connections");
            return Err(());
        }
        *count += 1;
        Ok(WsConnectionSlot {
            connections: Arc::clone(this),
            user,
        })
    }
}

/// One open WebSocket connection of a user, counted until dropped with the connection.
pub(crate) struct WsConnectionSlot {
    connections: Arc<WsConnections>,
    user: UserId,
}

impl Drop for WsConnectionSlot {
    fn drop(&mut self) {
        let mut open = self
            .connections
            .open
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(count) = open.get_mut(&self.user) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                open.remove(&self.user);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use actix_web::test::{self, TestRequest};
    use actix_web::{App, http::StatusCode};

    use super::*;

    fn limits(requests_per_ip: u32) -> ApiLimits {
        ApiLimits {
            requests_per_ip: rim_infra_services::token_bucket::BucketRate::per_minute(
                requests_per_ip,
            ),
            mutations_per_session: None,
            messages: None,
            ws_connections_per_user: None,
            subscriptions_per_connection: None,
            max_batch: crate::config::DEFAULT_API_MAX_BATCH,
        }
    }

    async fn ok() -> HttpResponse {
        HttpResponse::Ok().finish()
    }

    /// An app with the per-address limit on every route, as the server mounts it.
    macro_rules! app {
        ($limits:expr) => {
            test::init_service(
                App::new().app_data(web::Data::new($limits)).service(
                    web::scope("")
                        .wrap(actix_web::middleware::from_fn(limit_api))
                        .route("/api", web::post().to(ok))
                        .route("/api/sdl", web::get().to(ok))
                        .route("/healthz", web::get().to(ok)),
                ),
            )
            .await
        };
    }

    fn from(ip: &str) -> TestRequest {
        TestRequest::get()
            .uri("/api/sdl")
            .peer_addr(SocketAddr::new(ip.parse().expect("ip"), 40_000))
    }

    #[actix_web::test]
    async fn past_the_limit_an_address_gets_429_with_retry_after() {
        let app = app!(EdgeLimits::new(limits(2), false));
        for _ in 0..2 {
            let res = test::call_service(&app, from("198.51.100.1").to_request()).await;
            assert_eq!(res.status(), StatusCode::OK);
        }
        let res = test::call_service(&app, from("198.51.100.1").to_request()).await;
        assert_eq!(res.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(
            res.headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok()),
            Some("30")
        );
        let body: serde_json::Value = test::read_body_json(res).await;
        assert_eq!(
            body,
            serde_json::json!({ "error": "rate_limited", "retryAfterSeconds": 30 })
        );

        let other = test::call_service(&app, from("198.51.100.2").to_request()).await;
        assert_eq!(
            other.status(),
            StatusCode::OK,
            "another address has its own"
        );
        let health =
            test::call_service(&app, from("198.51.100.1").uri("/healthz").to_request()).await;
        assert_eq!(health.status(), StatusCode::OK, "only /api routes count");
    }

    #[actix_web::test]
    async fn ipv6_addresses_in_one_slash_64_share_a_budget() {
        let app = app!(EdgeLimits::new(limits(1), false));
        let res = test::call_service(&app, from("2001:db8:1:2::1").to_request()).await;
        assert_eq!(res.status(), StatusCode::OK);
        let res = test::call_service(&app, from("2001:db8:1:2::ffff").to_request()).await;
        assert_eq!(res.status(), StatusCode::TOO_MANY_REQUESTS);
        let res = test::call_service(&app, from("2001:db8:1:3::1").to_request()).await;
        assert_eq!(res.status(), StatusCode::OK);
    }

    #[actix_web::test]
    async fn x_forwarded_for_names_the_client_only_when_trusted() {
        let forwarded = |client: &str| {
            from("10.0.0.1")
                .insert_header(("x-forwarded-for", client.to_string()))
                .to_request()
        };
        // Untrusted: every request is the proxy's, whatever the header says.
        let app = app!(EdgeLimits::new(limits(1), false));
        let res = test::call_service(&app, forwarded("203.0.113.1")).await;
        assert_eq!(res.status(), StatusCode::OK);
        let res = test::call_service(&app, forwarded("203.0.113.2")).await;
        assert_eq!(res.status(), StatusCode::TOO_MANY_REQUESTS);

        // Trusted: each forwarded client has its own budget.
        let app = app!(EdgeLimits::new(limits(1), true));
        let res = test::call_service(&app, forwarded("203.0.113.1")).await;
        assert_eq!(res.status(), StatusCode::OK);
        let res = test::call_service(&app, forwarded("203.0.113.2")).await;
        assert_eq!(res.status(), StatusCode::OK);
        let res = test::call_service(&app, forwarded("203.0.113.1")).await;
        assert_eq!(res.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    #[test]
    fn the_client_address_parses_what_proxies_write() {
        let req = |trust: bool, header: &str| {
            let req = from("10.0.0.1")
                .insert_header(("x-forwarded-for", header.to_string()))
                .to_http_request();
            client_ip(&req, trust)
        };
        let ip = |s: &str| Some(s.parse::<IpAddr>().expect("ip"));
        assert_eq!(req(false, "203.0.113.1"), ip("10.0.0.1"));
        assert_eq!(req(true, "203.0.113.1, 10.0.0.1"), ip("203.0.113.1"));
        assert_eq!(req(true, "203.0.113.1:4711"), ip("203.0.113.1"));
        assert_eq!(req(true, "[2001:db8::1]:4711"), ip("2001:db8::1"));
        assert_eq!(req(true, "::ffff:203.0.113.9"), ip("203.0.113.9"));
        assert_eq!(
            req(true, "unknown"),
            ip("10.0.0.1"),
            "garbage falls back to the peer"
        );
        assert_eq!(parse_address("[2001:db8::2]"), ip("2001:db8::2"));
    }

    #[test]
    fn the_mutation_budget_counts_fields_per_caller() {
        let limits = EdgeLimits::new(
            ApiLimits {
                mutations_per_session: rim_infra_services::token_bucket::BucketRate::per_minute(3),
                ..limits(0)
            },
            false,
        );
        let session = MutationCaller::Session(SessionId::new());
        let anonymous = MutationCaller::Address("198.51.100.1".parse().expect("ip"));
        assert_eq!(limits.spend_mutations(&session, 2), Ok(()));
        assert_eq!(
            limits.spend_mutations(&session, 0),
            Ok(()),
            "queries are free"
        );
        assert_eq!(limits.spend_mutations(&session, 2), Err(20));
        assert_eq!(limits.spend_mutations(&session, 1), Ok(()));
        assert_eq!(limits.spend_mutations(&anonymous, 3), Ok(()));
        let unlimited = EdgeLimits::new(limits_off(), false);
        assert_eq!(unlimited.spend_mutations(&session, 1000), Ok(()));
    }

    fn limits_off() -> ApiLimits {
        limits(0)
    }

    #[test]
    fn the_batch_cap_allows_up_to_its_size() {
        let limits = EdgeLimits::new(
            ApiLimits {
                max_batch: 3,
                ..limits_off()
            },
            false,
        );
        assert!(!limits.batch_too_large(3));
        assert!(limits.batch_too_large(4));
    }

    #[test]
    fn a_user_holds_at_most_the_websocket_connections_allowed() {
        let limits = EdgeLimits::new(
            ApiLimits {
                ws_connections_per_user: Some(2),
                ..limits_off()
            },
            false,
        );
        let user = UserId::new();
        let first = limits.open_ws(user).expect("first").expect("a slot");
        let second = limits.open_ws(user).expect("second").expect("a slot");
        assert!(limits.open_ws(user).is_err());
        assert!(limits.open_ws(UserId::new()).is_ok(), "per user");
        drop(first);
        let third = limits.open_ws(user).expect("a slot is free again");
        drop((second, third));
        let connections = limits.ws_connections.as_ref().expect("capped");
        assert!(
            connections.open.lock().expect("lock").get(&user).is_none(),
            "a user without connections is forgotten"
        );
        assert!(
            EdgeLimits::new(limits_off(), false)
                .open_ws(user)
                .expect("no cap")
                .is_none()
        );
    }
}
