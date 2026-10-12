use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::Semaphore;

use rim_core::link_preview::model::NewLinkPreview;
use rim_core::link_preview::repository::LinkPreviewRepository;
use rim_core::message::model::MessageId;
use rim_core::server_feature::model::ServerFeature;
use rim_services::feature::service::FeatureGate;
use rim_services::link_preview::DEFAULT_FETCH_CONCURRENCY;
use rim_services::link_preview::dto::LinkPreviewDto;
use rim_services::link_preview::service::LinkPreviewService;
use rim_services::{ServiceError, ServiceResult};

use crate::outbound::{BodyError, read_capped};

/// Maximum number of link previews per message.
const MAX_PREVIEWS_PER_MESSAGE: usize = 5;

/// Maximum response body size (1 MB).
const MAX_BODY_SIZE: usize = 1_048_576;

/// HTTP request timeout per URL.
const FETCH_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone)]
pub struct DefaultLinkPreviewService {
    link_preview_repo: Arc<dyn LinkPreviewRepository>,
    /// `None` only if the hardened client failed to build; every fetch is then refused rather
    /// than falling back to an unguarded default client.
    http_client: Option<reqwest::Client>,
    /// `None` keeps previews on; otherwise nothing is fetched or served while
    /// [`ServerFeature::LinkPreviews`] is not enabled.
    features: Option<Arc<dyn FeatureGate>>,
    /// Fetches in flight in this process, shared by every clone of the service.
    fetch_slots: Arc<Semaphore>,
    /// How long a fetch waits for a slot before it is dropped.
    slot_wait: Duration,
}

impl DefaultLinkPreviewService {
    pub fn new(link_preview_repo: Arc<dyn LinkPreviewRepository>) -> Self {
        let http_client = reqwest::Client::builder()
            .timeout(FETCH_TIMEOUT)
            // Every hostname connection (initial request and each redirect hop) resolves
            // through this, so a public name pointing at a private address is refused at
            // connect time — no check-then-connect gap for DNS rebinding.
            .dns_resolver(Arc::new(PublicOnlyResolver))
            // A proxy would make the resolver vet the proxy host instead of the target, and
            // the proxy would then happily fetch internal URLs for us. Connect directly.
            .no_proxy()
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                // Block redirects to private IPs (hostnames are vetted by the resolver)
                if host_ip(attempt.url()).is_some_and(|addr| is_private_ip(&addr)) {
                    return attempt.stop();
                }
                if attempt.previous().len() > 5 {
                    return attempt.stop();
                }
                attempt.follow()
            }))
            .user_agent("RIM-LinkPreview/1.0")
            .build()
            .inspect_err(|e| {
                tracing::error!(error = %e, "failed to build link-preview HTTP client; link previews disabled");
            })
            .ok();

        Self {
            link_preview_repo,
            http_client,
            features: None,
            fetch_slots: Arc::new(Semaphore::new(DEFAULT_FETCH_CONCURRENCY)),
            slot_wait: FETCH_TIMEOUT,
        }
    }

    /// Run at most `fetches` preview fetches at once (at least one); a fetch that finds every
    /// slot busy for longer than the fetch timeout is dropped.
    pub fn with_concurrency(mut self, fetches: usize) -> Self {
        self.fetch_slots = Arc::new(Semaphore::new(fetches.max(1)));
        self
    }

    /// Fetch and serve previews only while the instance has link previews enabled.
    pub fn with_feature_gate(mut self, features: Arc<dyn FeatureGate>) -> Self {
        self.features = Some(features);
        self
    }

    fn enabled(&self) -> bool {
        self.features
            .as_ref()
            .is_none_or(|gate| gate.state(ServerFeature::LinkPreviews).is_enabled())
    }
}

/// DNS resolver for the link-preview client that refuses hosts resolving to a non-public address.
///
/// `is_url_safe` and the redirect policy only see the URL's host *string*, so on their own they
/// block literal private IPs but not a public hostname whose DNS record points at e.g.
/// `169.254.169.254`. Literal-IP hosts never reach a resolver, so both layers are needed.
struct PublicOnlyResolver;

impl reqwest::dns::Resolve for PublicOnlyResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        Box::pin(async move {
            let host = name.as_str();
            let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host, 0)).await?.collect();
            if addrs.is_empty() || addrs.iter().any(|a| is_private_ip(&a.ip())) {
                return Err(format!(
                    "refusing to connect: {host} resolves to a non-public address"
                )
                .into());
            }
            Ok(Box::new(addrs.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

fn is_private_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_private_v4(v4),
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                // Unique local (fc00::/7)
                || (v6.segments()[0] & 0xfe00) == 0xfc00
                // Link-local (fe80::/10)
                || (v6.segments()[0] & 0xffc0) == 0xfe80
                // IPv4-mapped (::ffff:x.x.x.x) — check the embedded IPv4
                || v6.to_ipv4_mapped().is_some_and(|v4| is_private_v4(&v4))
                // IPv4-compatible (::a.b.c.d, deprecated) — check the embedded IPv4
                || (v6.segments()[..6] == [0, 0, 0, 0, 0, 0] && {
                    let o = v6.octets();
                    is_private_v4(&Ipv4Addr::new(o[12], o[13], o[14], o[15]))
                })
                // 6to4 (2002::/16) — check the IPv4 embedded in bits 16..48
                || (v6.segments()[0] == 0x2002 && {
                    let o = v6.octets();
                    is_private_v4(&Ipv4Addr::new(o[2], o[3], o[4], o[5]))
                })
                // NAT64 well-known prefix (64:ff9b::/96) — check the embedded IPv4
                || (v6.segments()[..6] == [0x64, 0xff9b, 0, 0, 0, 0] && {
                    let o = v6.octets();
                    is_private_v4(&Ipv4Addr::new(o[12], o[13], o[14], o[15]))
                })
        }
    }
}

fn is_private_v4(v4: &Ipv4Addr) -> bool {
    let o = v4.octets();
    v4.is_loopback()
        || v4.is_private()
        || v4.is_link_local()
        || v4.is_multicast()
        || o[0] == 0
        // Carrier-grade NAT (100.64.0.0/10) — also where some clouds put metadata services
        || (o[0] == 100 && (o[1] & 0xc0) == 64)
        // Reserved (240.0.0.0/4), which includes broadcast
        || o[0] >= 240
}

/// The URL's host as an IP address, if it is an IP literal.
///
/// `Url::host_str` keeps the brackets around IPv6 literals (`"[::1]"`), which `IpAddr` won't
/// parse — and the HTTP connector connects to literals without consulting the resolver — so
/// strip them before parsing.
fn host_ip(url: &reqwest::Url) -> Option<IpAddr> {
    let host = url.host_str()?;
    host.strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host)
        .parse()
        .ok()
}

fn is_url_safe(url: &str) -> bool {
    let lower = url.to_lowercase();
    if !lower.starts_with("http://") && !lower.starts_with("https://") {
        return false;
    }

    // Parse the URL and check for private IPs in the host
    if let Ok(parsed) = reqwest::Url::parse(url) {
        if host_ip(&parsed).is_some_and(|ip| is_private_ip(&ip)) {
            return false;
        }
        if let Some(host) = parsed.host_str() {
            // Block localhost variants
            if host == "localhost" || host.ends_with(".local") {
                return false;
            }
        }
        true
    } else {
        false
    }
}

fn parse_og_tags(html: &str) -> OgData {
    use scraper::{Html, Selector};

    let document = Html::parse_document(html);

    let og_title_sel = Selector::parse(r#"meta[property="og:title"]"#).ok();
    let og_desc_sel = Selector::parse(r#"meta[property="og:description"]"#).ok();
    let og_image_sel = Selector::parse(r#"meta[property="og:image"]"#).ok();
    let og_site_sel = Selector::parse(r#"meta[property="og:site_name"]"#).ok();
    let title_sel = Selector::parse("title").ok();

    let og_title = og_title_sel
        .as_ref()
        .and_then(|s| document.select(s).next())
        .and_then(|el| el.value().attr("content").map(String::from));

    let og_description = og_desc_sel
        .as_ref()
        .and_then(|s| document.select(s).next())
        .and_then(|el| el.value().attr("content").map(String::from));

    let og_image = og_image_sel
        .as_ref()
        .and_then(|s| document.select(s).next())
        .and_then(|el| el.value().attr("content").map(String::from));

    let og_site_name = og_site_sel
        .as_ref()
        .and_then(|s| document.select(s).next())
        .and_then(|el| el.value().attr("content").map(String::from));

    // Fallback: use <title> if og:title is missing
    let title = og_title.or_else(|| {
        title_sel
            .as_ref()
            .and_then(|s| document.select(s).next())
            .map(|el| el.text().collect::<String>())
    });

    OgData {
        title,
        description: og_description,
        image_url: og_image,
        site_name: og_site_name,
    }
}

struct OgData {
    title: Option<String>,
    description: Option<String>,
    image_url: Option<String>,
    site_name: Option<String>,
}

#[async_trait]
impl LinkPreviewService for DefaultLinkPreviewService {
    async fn fetch_and_store(
        &self,
        message_id: MessageId,
        urls: Vec<String>,
    ) -> ServiceResult<Vec<LinkPreviewDto>> {
        if !self.enabled() {
            return Ok(Vec::new());
        }
        let safe_urls: Vec<&String> = urls.iter().filter(|u| is_url_safe(u)).collect();
        let urls_to_fetch = &safe_urls[..safe_urls.len().min(MAX_PREVIEWS_PER_MESSAGE)];

        let mut results = Vec::new();

        for url in urls_to_fetch {
            let og = match self.fetch_og_data(url).await {
                Ok(og) => og,
                // Dropped for want of a fetch slot, and logged there.
                Err(ServiceError::Unavailable) => continue,
                Err(e) => {
                    tracing::warn!(url = %url, error = %e, "failed to fetch link preview");
                    continue;
                }
            };

            // Skip if we got nothing useful
            if og.title.is_none() && og.description.is_none() && og.image_url.is_none() {
                continue;
            }

            match self
                .link_preview_repo
                .create(NewLinkPreview {
                    message_id,
                    url: url.to_string(),
                    title: og.title,
                    description: og.description,
                    image_url: og.image_url,
                    site_name: og.site_name,
                })
                .await
            {
                Ok(preview) => results.push(LinkPreviewDto::from(preview)),
                Err(e) => {
                    tracing::warn!(url = %url, error = %e, "failed to store link preview");
                }
            }
        }

        Ok(results)
    }

    async fn get_previews_for_messages(
        &self,
        message_ids: &[MessageId],
    ) -> ServiceResult<HashMap<MessageId, Vec<LinkPreviewDto>>> {
        if !self.enabled() {
            return Ok(HashMap::new());
        }
        let map = self.link_preview_repo.get_for_messages(message_ids).await?;

        Ok(map
            .into_iter()
            .map(|(id, previews)| (id, previews.into_iter().map(Into::into).collect()))
            .collect())
    }
}

impl DefaultLinkPreviewService {
    async fn fetch_og_data(&self, url: &str) -> Result<OgData, ServiceError> {
        let Some(http_client) = &self.http_client else {
            return Err(ServiceError::internal(anyhow::anyhow!(
                "link-preview HTTP client unavailable"
            )));
        };
        let Ok(Ok(_slot)) = tokio::time::timeout(self.slot_wait, self.fetch_slots.acquire()).await
        else {
            tracing::warn!(url = %url, "link preview dropped: every fetch slot stayed busy");
            return Err(ServiceError::Unavailable);
        };
        let response = http_client
            .get(url)
            .send()
            .await
            .map_err(ServiceError::internal)?;
        let bytes = read_capped(response, MAX_BODY_SIZE)
            .await
            .map_err(|e| match e {
                BodyError::TooLarge(_) => {
                    ServiceError::Validation("response body too large".into())
                }
                BodyError::Transport(why) => ServiceError::internal(anyhow::anyhow!(why)),
            })?;

        let html = String::from_utf8_lossy(&bytes);
        Ok(parse_og_tags(&html))
    }
}

#[cfg(test)]
mod tests {
    use crate::outbound::fixture::{serve, sized_head};

    use super::*;

    #[test]
    fn extract_urls_from_text() {
        let text = "Check out https://example.com and http://foo.bar/baz?q=1 for more info";
        let urls = rim_services::link_preview::extract_urls(text);
        assert_eq!(urls.len(), 2);
        assert_eq!(urls[0], "https://example.com");
        assert_eq!(urls[1], "http://foo.bar/baz?q=1");
    }

    #[test]
    fn extract_urls_empty() {
        assert!(rim_services::link_preview::extract_urls("no links here").is_empty());
    }

    #[test]
    fn private_ip_blocking() {
        assert!(!is_url_safe("http://127.0.0.1/admin"));
        assert!(!is_url_safe("http://10.0.0.1/internal"));
        assert!(!is_url_safe("http://192.168.1.1/router"));
        assert!(!is_url_safe("http://172.16.0.1/private"));
        assert!(!is_url_safe("http://localhost/admin"));
        assert!(!is_url_safe("ftp://example.com/file"));
        assert!(is_url_safe("https://example.com"));
    }

    #[test]
    fn parse_og_tags_basic() {
        let html = r#"
        <html><head>
            <meta property="og:title" content="Test Title">
            <meta property="og:description" content="Test Desc">
            <meta property="og:image" content="https://img.example.com/pic.jpg">
            <meta property="og:site_name" content="Example">
            <title>Fallback Title</title>
        </head><body></body></html>
        "#;
        let og = parse_og_tags(html);
        assert_eq!(og.title.as_deref(), Some("Test Title"));
        assert_eq!(og.description.as_deref(), Some("Test Desc"));
        assert_eq!(
            og.image_url.as_deref(),
            Some("https://img.example.com/pic.jpg")
        );
        assert_eq!(og.site_name.as_deref(), Some("Example"));
    }

    #[test]
    fn parse_og_tags_fallback_to_title() {
        let html = r#"<html><head><title>My Page</title></head><body></body></html>"#;
        let og = parse_og_tags(html);
        assert_eq!(og.title.as_deref(), Some("My Page"));
        assert!(og.description.is_none());
    }

    #[test]
    fn parse_og_tags_empty_html() {
        let og = parse_og_tags("");
        assert!(og.title.is_none());
        assert!(og.description.is_none());
        assert!(og.image_url.is_none());
        assert!(og.site_name.is_none());
    }

    #[test]
    fn parse_og_tags_og_title_overrides_html_title() {
        let html = r#"<html><head>
            <meta property="og:title" content="OG Title">
            <title>HTML Title</title>
        </head><body></body></html>"#;
        let og = parse_og_tags(html);
        assert_eq!(og.title.as_deref(), Some("OG Title"));
    }

    #[test]
    fn extract_urls_ignores_non_http() {
        let text = "ftp://files.example.com and mailto:a@b.com";
        assert!(rim_services::link_preview::extract_urls(text).is_empty());
    }

    #[test]
    fn extract_urls_handles_urls_in_brackets() {
        let text = "see <https://example.com> for details";
        let urls = rim_services::link_preview::extract_urls(text);
        assert_eq!(urls.len(), 1);
        assert_eq!(urls[0], "https://example.com");
    }

    #[test]
    fn is_private_ip_ipv6_loopback() {
        let addr: std::net::IpAddr = "::1".parse().unwrap();
        assert!(is_private_ip(&addr));
    }

    #[test]
    fn is_private_ip_169_254_link_local() {
        assert!(!is_url_safe("http://169.254.1.1/something"));
    }

    #[test]
    fn is_url_safe_rejects_dot_local() {
        assert!(!is_url_safe("http://myserver.local/api"));
    }

    #[test]
    fn is_url_safe_rejects_zero_prefix_ip() {
        assert!(!is_url_safe("http://0.0.0.0/admin"));
    }

    #[test]
    fn extract_urls_multiple_same_line() {
        let text = "https://a.com https://b.com https://c.com";
        assert_eq!(rim_services::link_preview::extract_urls(text).len(), 3);
    }

    #[test]
    fn is_private_ip_covers_non_public_ranges() {
        for ip in [
            "100.64.0.1",      // CGNAT
            "100.100.100.200", // CGNAT-hosted cloud metadata
            "224.0.0.1",       // multicast
            "240.0.0.1",       // reserved
            "255.255.255.255", // broadcast
            "::",              // unspecified
            "ff02::1",         // IPv6 multicast
            "::ffff:169.254.169.254",
            "64:ff9b::a9fe:a9fe", // NAT64-embedded 169.254.169.254
            "::7f00:1",           // IPv4-compatible 127.0.0.1
            "2002:a9fe:a9fe::",   // 6to4-embedded 169.254.169.254
        ] {
            let addr: IpAddr = ip.parse().unwrap();
            assert!(is_private_ip(&addr), "{ip} must be treated as non-public");
        }
    }

    #[test]
    fn is_url_safe_rejects_ipv6_literals() {
        for url in [
            "http://[::1]/",
            "http://[::ffff:127.0.0.1]/",
            "http://[::ffff:a9fe:a9fe]/latest/meta-data/",
            "http://[fd00:ec2::254]/",
            "http://[2002:a9fe:a9fe::1]/",
        ] {
            assert!(!is_url_safe(url), "{url} must be rejected");
        }
        assert!(is_url_safe("http://[2606:4700::1111]/"));
    }

    #[test]
    fn host_ip_strips_ipv6_brackets() {
        let url = reqwest::Url::parse("http://[::1]:8080/x").unwrap();
        assert_eq!(host_ip(&url), Some("::1".parse().unwrap()));
        let url = reqwest::Url::parse("http://example.com/").unwrap();
        assert_eq!(host_ip(&url), None);
    }

    /// Redirect to an IPv6 loopback literal must be stopped by the redirect policy (literals
    /// never reach the resolver).
    #[tokio::test]
    async fn fetch_refuses_redirect_to_ipv6_loopback_literal() {
        use rim_core::link_preview::repository::MockLinkPreviewRepository;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let Ok(target) = tokio::net::TcpListener::bind("[::1]:0").await else {
            eprintln!("skipping: no IPv6 loopback in this environment");
            return;
        };
        let target_port = target.local_addr().unwrap().port();

        // The redirector itself must be reachable, so call the client from a test-only
        // service whose resolver is irrelevant: connect to it by IPv4 literal.
        let redirector = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let redirector_port = redirector.local_addr().unwrap().port();
        tokio::spawn(async move {
            if let Ok((mut sock, _)) = redirector.accept().await {
                let mut buf = [0u8; 1024];
                let _ = sock.read(&mut buf).await;
                let resp = format!(
                    "HTTP/1.1 302 Found\r\nLocation: http://[::1]:{target_port}/\r\nContent-Length: 0\r\n\r\n"
                );
                let _ = sock.write_all(resp.as_bytes()).await;
            }
        });

        let svc = DefaultLinkPreviewService::new(Arc::new(MockLinkPreviewRepository::new()));
        let _ = svc
            .fetch_og_data(&format!("http://127.0.0.1:{redirector_port}/"))
            .await;

        let accepted = tokio::time::timeout(Duration::from_millis(200), target.accept()).await;
        assert!(accepted.is_err(), "redirect to [::1] must not be followed");
    }

    #[test]
    fn is_private_ip_allows_public_addresses() {
        for ip in [
            "8.8.8.8",
            "100.128.0.1",
            "2606:4700::1111",
            "64:ff9b::808:808",
        ] {
            let addr: IpAddr = ip.parse().unwrap();
            assert!(!is_private_ip(&addr), "{ip} must be treated as public");
        }
    }

    #[tokio::test]
    async fn resolver_rejects_hostname_resolving_to_loopback() {
        use reqwest::dns::Resolve;

        let name: reqwest::dns::Name = "localhost".parse().unwrap();
        let result = PublicOnlyResolver.resolve(name).await;
        assert!(
            result.is_err(),
            "localhost resolves to loopback and must be refused"
        );
    }

    /// A *hostname* that resolves to a private address must not be fetched.
    /// `fetch_og_data` is called directly so the string-level `is_url_safe` check (which would
    /// catch the literal `localhost`) is bypassed and only the client's resolver stands guard.
    #[tokio::test]
    async fn fetch_refuses_hostname_resolving_to_private_address() {
        use rim_core::link_preview::repository::MockLinkPreviewRepository;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let svc = DefaultLinkPreviewService::new(Arc::new(MockLinkPreviewRepository::new()));

        let result = svc
            .fetch_og_data(&format!("http://localhost:{port}/"))
            .await;
        assert!(result.is_err(), "fetch must fail before connecting");

        let accepted = tokio::time::timeout(Duration::from_millis(200), listener.accept()).await;
        assert!(
            accepted.is_err(),
            "no connection may reach the loopback listener"
        );
    }

    /// A disabled feature fetches nothing and serves no stored preview: the unprimed repository
    /// mock panics on any call.
    #[tokio::test]
    async fn a_disabled_feature_fetches_and_serves_nothing() {
        use rim_core::link_preview::repository::MockLinkPreviewRepository;

        let gate =
            crate::feature_gate::InstanceFeatureGate::new(&[ServerFeature::LinkPreviews], &[]);
        let svc = DefaultLinkPreviewService::new(Arc::new(MockLinkPreviewRepository::new()))
            .with_feature_gate(Arc::new(gate));
        let fetched = svc
            .fetch_and_store(MessageId::new(), vec!["https://example.com/".to_string()])
            .await
            .expect("fetch");
        assert!(fetched.is_empty());
        let served = svc
            .get_previews_for_messages(&[MessageId::new()])
            .await
            .expect("serve");
        assert!(served.is_empty());
    }

    /// An enabled feature serves the stored previews from the repository, as without a gate.
    #[tokio::test]
    async fn an_enabled_feature_serves_stored_previews() {
        use rim_core::link_preview::model::{LinkPreview, LinkPreviewId};
        use rim_core::link_preview::repository::MockLinkPreviewRepository;

        let message_id = MessageId::new();
        let mut repo = MockLinkPreviewRepository::new();
        repo.expect_get_for_messages().times(1).returning(move |_| {
            let preview = LinkPreview {
                id: LinkPreviewId::new(),
                message_id,
                url: "https://example.com/".to_string(),
                title: Some("Example".to_string()),
                description: None,
                image_url: None,
                site_name: None,
                fetched_at: chrono::Utc::now(),
            };
            Box::pin(async move { Ok(HashMap::from([(message_id, vec![preview])])) })
        });
        let gate = crate::feature_gate::InstanceFeatureGate::new(&[], &[]);
        let svc = DefaultLinkPreviewService::new(Arc::new(repo)).with_feature_gate(Arc::new(gate));
        let served = svc
            .get_previews_for_messages(&[message_id])
            .await
            .expect("serve");
        assert_eq!(served[&message_id][0].url, "https://example.com/");
    }

    /// A service whose fetches share `slots` slots and wait at most `wait` for one.
    fn limited_service(slots: usize, wait: Duration) -> DefaultLinkPreviewService {
        use rim_core::link_preview::repository::MockLinkPreviewRepository;

        let mut svc = DefaultLinkPreviewService::new(Arc::new(MockLinkPreviewRepository::new()))
            .with_concurrency(slots);
        svc.slot_wait = wait;
        svc
    }

    const PAGE: &str = r#"<html><head><meta property="og:title" content="Slow"></head></html>"#;

    /// 2 MiB of chunks with no terminating chunk: the fetch stops at the 1 MiB cap while the
    /// connection is still open, before the fetch timeout.
    #[tokio::test]
    async fn a_chunked_body_over_the_cap_fails_before_eof() {
        let mut body = Vec::new();
        for _ in 0..32 {
            body.extend_from_slice(format!("{:x}\r\n", 64 * 1024).as_bytes());
            body.extend(std::iter::repeat_n(b'a', 64 * 1024));
            body.extend_from_slice(b"\r\n");
        }
        let head =
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nTransfer-Encoding: chunked\r\n\r\n";
        let linger = Duration::from_secs(30);
        let server = serve(head.to_string(), body, Duration::ZERO, linger).await;
        let svc = limited_service(1, FETCH_TIMEOUT);

        let started = std::time::Instant::now();
        let result = svc.fetch_og_data(&format!("http://{}/", server.addr)).await;
        assert!(
            matches!(&result, Err(ServiceError::Validation(why)) if why.contains("too large")),
            "{:?}",
            result.err()
        );
        assert!(started.elapsed() < FETCH_TIMEOUT);
    }

    /// With one slot, the second fetch connects only once the first has finished.
    #[tokio::test]
    async fn a_second_fetch_waits_for_the_only_slot() {
        let delay = Duration::from_millis(500);
        let server = serve(sized_head(PAGE.len()), PAGE.into(), delay, Duration::ZERO).await;
        let svc = limited_service(1, FETCH_TIMEOUT);
        let url = format!("http://{}/", server.addr);

        let (first, second) = tokio::join!(svc.fetch_og_data(&url), svc.fetch_og_data(&url));
        assert_eq!(first.expect("first").title.as_deref(), Some("Slow"));
        assert_eq!(second.expect("second").title.as_deref(), Some("Slow"));
        let accepted = server.accepted();
        assert_eq!(accepted.len(), 2);
        assert!(accepted[1] - accepted[0] >= delay);
    }

    /// With one slot held past the wait, the second fetch is dropped without connecting.
    #[tokio::test]
    async fn a_fetch_waiting_past_the_limit_is_dropped() {
        let delay = Duration::from_secs(2);
        let server = serve(sized_head(PAGE.len()), PAGE.into(), delay, Duration::ZERO).await;
        let svc = limited_service(1, Duration::from_millis(200));
        let url = format!("http://{}/", server.addr);

        let (first, second) = tokio::join!(svc.fetch_og_data(&url), async {
            tokio::time::sleep(Duration::from_millis(50)).await;
            svc.fetch_og_data(&url).await
        });
        assert!(first.is_ok());
        assert!(matches!(second, Err(ServiceError::Unavailable)));
        assert_eq!(server.accepted().len(), 1);
    }
}
