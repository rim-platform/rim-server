//! Which browser origins the API accepts: the server's own, the packaged desktop app's and one
//! configured extra.

use actix_web::dev::RequestHead;
use actix_web::http::header::HeaderValue;

use crate::config::RIMConfig;

/// The origins the packaged desktop app runs under: the Tauri scheme on macOS and Linux, the
/// `tauri.localhost` host on Windows. A web page cannot carry any of them.
const DESKTOP_ORIGINS: [&str; 3] = [
    "tauri://localhost",
    "http://tauri.localhost",
    "https://tauri.localhost",
];

/// The origins a request carrying an `Origin` header may come from.
#[derive(Clone, Debug)]
pub(crate) struct AllowedOrigins {
    /// The origin of `RIM_PUBLIC_BASE_URL`; `None` derives the server's origin from each request.
    own_origin: Option<String>,
    /// `RIM_CORS_ALLOWED_ORIGIN`, such as the desktop development app.
    extra_origin: Option<String>,
    /// Read the scheme and host from `X-Forwarded-Proto` and `X-Forwarded-Host`.
    trust_forwarded_headers: bool,
}

impl AllowedOrigins {
    pub(crate) fn from_config(config: &RIMConfig) -> Self {
        Self {
            own_origin: config.public_base_url().as_deref().and_then(normalize),
            extra_origin: config.cors_allowed_origin.as_deref().and_then(normalize),
            trust_forwarded_headers: config.trust_forwarded_headers(),
        }
    }

    /// Whether `origin` is this server's own origin as the client sees it, the desktop app's
    /// or the extra one.
    pub(crate) fn allows(&self, origin: &HeaderValue, head: &RequestHead) -> bool {
        let Some(raw) = origin.to_str().ok() else {
            return false;
        };
        if is_desktop_origin(raw) {
            return true;
        }
        let Some(origin) = normalize(raw) else {
            return false;
        };
        if self.extra_origin.as_deref() == Some(origin.as_str()) {
            return true;
        }
        match &self.own_origin {
            Some(own) => *own == origin,
            None => request_origin(head, self.trust_forwarded_headers).as_deref() == Some(&origin),
        }
    }

    /// The CORS middleware for the API routes: a request whose `Origin` is not allowed is
    /// refused with 400.
    pub(crate) fn cors(self) -> actix_cors::Cors {
        actix_cors::Cors::default()
            .allowed_origin_fn(move |origin, head| self.allows(origin, head))
            .allowed_methods(vec!["GET", "POST", "OPTIONS"])
            .allowed_headers(vec![
                "Content-Type",
                "Authorization",
                "x-csrf-token",
                "x-request-id",
                crate::graphql_support::SESSION_SECRET_HEADER,
            ])
            .expose_headers(vec!["Retry-After"])
            .supports_credentials()
            .block_on_origin_mismatch(true)
            .max_age(3600)
    }
}

/// Whether `origin` is one of the packaged desktop app's origins, compared whole and without
/// case, since the Tauri scheme has no URL origin to normalize.
fn is_desktop_origin(origin: &str) -> bool {
    let origin = origin.trim().to_ascii_lowercase();
    DESKTOP_ORIGINS.contains(&origin.as_str())
}

/// The ASCII serialization of a URL's origin: lowercase, without a default port. `None` for
/// anything without a tuple origin: the `null` origin, a URL without a host, a scheme that is
/// not http or https.
fn normalize(url: &str) -> Option<String> {
    let url = url::Url::parse(url.trim()).ok()?;
    url.host()?;
    match url.origin() {
        url::Origin::Tuple(..) => Some(url.origin().ascii_serialization()),
        url::Origin::Opaque(_) => None,
    }
}

/// The origin the client addressed: the request's scheme and `Host`, or the forwarded ones when
/// the proxy in front is trusted. The server itself listens on plain http.
fn request_origin(head: &RequestHead, trust_forwarded_headers: bool) -> Option<String> {
    let header = |name: &str| {
        head.headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(',').next())
            .map(str::trim)
            .filter(|value| !value.is_empty())
    };
    let (forwarded_scheme, forwarded_host) = if trust_forwarded_headers {
        (header("x-forwarded-proto"), header("x-forwarded-host"))
    } else {
        (None, None)
    };
    let scheme = forwarded_scheme.or(head.uri.scheme_str()).unwrap_or("http");
    let host = forwarded_host
        .or_else(|| header("host"))
        .or_else(|| head.uri.authority().map(|authority| authority.as_str()))?;
    normalize(&format!("{scheme}://{host}"))
}

#[cfg(test)]
mod tests {
    use actix_web::http::StatusCode;
    use actix_web::{App, HttpResponse, test as actix_test, web};

    use super::*;

    fn origins(
        public_base_url: Option<&str>,
        cors_allowed_origin: Option<&str>,
        trust_forwarded_headers: bool,
    ) -> AllowedOrigins {
        AllowedOrigins::from_config(&RIMConfig {
            public_base_url: public_base_url.map(str::to_string),
            cors_allowed_origin: cors_allowed_origin.map(str::to_string),
            trust_forwarded_headers: Some(trust_forwarded_headers),
            ..Default::default()
        })
    }

    /// The status of a POST to `/api` through the CORS middleware with the given headers.
    async fn post(origins: AllowedOrigins, headers: &[(&str, &str)]) -> StatusCode {
        let app = actix_test::init_service(
            App::new()
                .wrap(origins.cors())
                .route("/api", web::post().to(HttpResponse::Ok)),
        )
        .await;
        let mut req = actix_test::TestRequest::post().uri("/api");
        for (name, value) in headers {
            req = req.insert_header((*name, *value));
        }
        actix_test::call_service(&app, req.to_request())
            .await
            .status()
    }

    #[actix_web::test]
    async fn same_origin_is_accepted_without_configuration() {
        let status = post(
            origins(None, None, false),
            &[
                ("host", "rim.local:8080"),
                ("origin", "http://rim.local:8080"),
            ],
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    #[actix_web::test]
    async fn a_request_without_origin_passes() {
        let status = post(origins(None, None, false), &[("host", "rim.local:8080")]).await;
        assert_eq!(status, StatusCode::OK);
    }

    #[actix_web::test]
    async fn a_foreign_origin_is_refused() {
        let status = post(
            origins(None, None, false),
            &[
                ("host", "rim.local:8080"),
                ("origin", "http://evil.example"),
            ],
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        let status = post(
            origins(
                Some("https://rim.example.org"),
                Some("http://localhost:1420"),
                true,
            ),
            &[("host", "evil.example"), ("origin", "https://evil.example")],
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[actix_web::test]
    async fn the_public_base_url_is_the_own_origin_when_set() {
        let configured = || origins(Some("https://rim.example.org/"), None, false);
        let status = post(
            configured(),
            &[("host", "rim:8080"), ("origin", "https://rim.example.org")],
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        // With the public URL set, the Host header no longer names the own origin.
        let status = post(
            configured(),
            &[("host", "rim:8080"), ("origin", "http://rim:8080")],
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[actix_web::test]
    async fn the_configured_extra_origin_is_accepted() {
        let status = post(
            origins(
                Some("https://rim.example.org"),
                Some("http://localhost:1420"),
                false,
            ),
            &[
                ("host", "rim.example.org"),
                ("origin", "http://localhost:1420"),
            ],
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    #[actix_web::test]
    async fn forwarded_headers_count_only_when_trusted() {
        let behind_proxy = [
            ("host", "rim:8080"),
            ("x-forwarded-proto", "https"),
            ("x-forwarded-host", "rim.example.org"),
            ("origin", "https://rim.example.org"),
        ];
        assert_eq!(
            post(origins(None, None, true), &behind_proxy).await,
            StatusCode::OK
        );
        assert_eq!(
            post(origins(None, None, false), &behind_proxy).await,
            StatusCode::BAD_REQUEST
        );

        // A spoofed forwarded host cannot make a foreign origin look like the own one.
        let spoofed = [
            ("host", "rim.local:8080"),
            ("x-forwarded-host", "evil.example"),
            ("origin", "http://evil.example"),
        ];
        assert_eq!(
            post(origins(None, None, false), &spoofed).await,
            StatusCode::BAD_REQUEST
        );
    }

    #[actix_web::test]
    async fn the_desktop_app_origins_are_accepted_without_configuration() {
        for origin in [
            "tauri://localhost",
            "http://tauri.localhost",
            "HTTPS://Tauri.localhost",
        ] {
            let status = post(
                origins(Some("https://rim.example.org"), None, false),
                &[("host", "rim.example.org"), ("origin", origin)],
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{origin}");
        }
    }

    #[actix_web::test]
    async fn another_custom_scheme_is_not_the_desktop_app() {
        // Two opaque origins must not compare equal through the serialized `null`.
        let status = post(
            origins(None, Some("tauri://localhost"), false),
            &[("host", "rim.local:8080"), ("origin", "other://localhost")],
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn origins_compare_without_case_or_default_port() {
        assert_eq!(
            normalize("HTTPS://Rim.Example.org:443/app").as_deref(),
            Some("https://rim.example.org")
        );
        assert_eq!(normalize("null"), None);
        assert_eq!(normalize("tauri://localhost"), None);
    }
}
