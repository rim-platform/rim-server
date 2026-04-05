use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

use rim_core::link_preview::model::NewLinkPreview;
use rim_core::link_preview::repository::LinkPreviewRepository;
use rim_core::message::model::MessageId;
use rim_services::link_preview::dto::LinkPreviewDto;
use rim_services::link_preview::service::LinkPreviewService;
use rim_services::{ServiceError, ServiceResult, map_repo_error};

/// Maximum number of link previews per message.
const MAX_PREVIEWS_PER_MESSAGE: usize = 5;

/// Maximum response body size (1 MB).
const MAX_BODY_SIZE: usize = 1_048_576;

/// HTTP request timeout per URL.
const FETCH_TIMEOUT: Duration = Duration::from_secs(5);

pub struct DefaultLinkPreviewService {
    link_preview_repo: Arc<dyn LinkPreviewRepository>,
    http_client: reqwest::Client,
}

impl DefaultLinkPreviewService {
    pub fn new(link_preview_repo: Arc<dyn LinkPreviewRepository>) -> Self {
        let http_client = reqwest::Client::builder()
            .timeout(FETCH_TIMEOUT)
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                // Block redirects to private IPs
                if let Some(addr) = attempt.url().host_str().and_then(|h| h.parse::<IpAddr>().ok())
                {
                    if is_private_ip(&addr) {
                        return attempt.stop();
                    }
                }
                if attempt.previous().len() > 5 {
                    return attempt.stop();
                }
                attempt.follow()
            }))
            .user_agent("RIM-LinkPreview/1.0")
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        Self {
            link_preview_repo,
            http_client,
        }
    }
}

fn is_private_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.octets()[0] == 0
                || v4.is_broadcast()
        }
        IpAddr::V6(v6) => v6.is_loopback(),
    }
}

fn is_url_safe(url: &str) -> bool {
    let lower = url.to_lowercase();
    if !lower.starts_with("http://") && !lower.starts_with("https://") {
        return false;
    }

    // Parse the URL and check for private IPs in the host
    if let Ok(parsed) = reqwest::Url::parse(url) {
        if let Some(host) = parsed.host_str() {
            if let Ok(ip) = host.parse::<IpAddr>() {
                if is_private_ip(&ip) {
                    return false;
                }
            }
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

/// Extract URLs from message text using a simple regex.
pub fn extract_urls(text: &str) -> Vec<String> {
    let re = regex::Regex::new(r"https?://[^\s<>\[\](){}|\\^`]+").unwrap();
    re.find_iter(text)
        .map(|m| m.as_str().to_string())
        .collect()
}

#[async_trait]
impl LinkPreviewService for DefaultLinkPreviewService {
    async fn fetch_and_store(
        &self,
        message_id: MessageId,
        urls: Vec<String>,
    ) -> ServiceResult<Vec<LinkPreviewDto>> {
        let safe_urls: Vec<&String> = urls.iter().filter(|u| is_url_safe(u)).collect();
        let urls_to_fetch = &safe_urls[..safe_urls.len().min(MAX_PREVIEWS_PER_MESSAGE)];

        let mut results = Vec::new();

        for url in urls_to_fetch {
            let og = match self.fetch_og_data(url).await {
                Ok(og) => og,
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

    async fn get_previews_for_message(
        &self,
        message_id: MessageId,
    ) -> ServiceResult<Vec<LinkPreviewDto>> {
        let previews = self
            .link_preview_repo
            .get_for_message(message_id)
            .await
            .map_err(map_repo_error)?;

        Ok(previews.into_iter().map(Into::into).collect())
    }

    async fn get_previews_for_messages(
        &self,
        message_ids: &[MessageId],
    ) -> ServiceResult<HashMap<MessageId, Vec<LinkPreviewDto>>> {
        let map = self
            .link_preview_repo
            .get_for_messages(message_ids)
            .await
            .map_err(map_repo_error)?;

        Ok(map
            .into_iter()
            .map(|(id, previews)| (id, previews.into_iter().map(Into::into).collect()))
            .collect())
    }
}

impl DefaultLinkPreviewService {
    async fn fetch_og_data(&self, url: &str) -> Result<OgData, ServiceError> {
        let response = self
            .http_client
            .get(url)
            .send()
            .await
            .map_err(|e| ServiceError::internal(e))?;

        // Check content length before downloading
        if let Some(len) = response.content_length() {
            if len > MAX_BODY_SIZE as u64 {
                return Err(ServiceError::Validation("response body too large".into()));
            }
        }

        let bytes = response
            .bytes()
            .await
            .map_err(|e| ServiceError::internal(e))?;

        if bytes.len() > MAX_BODY_SIZE {
            return Err(ServiceError::Validation("response body too large".into()));
        }

        let html = String::from_utf8_lossy(&bytes);
        Ok(parse_og_tags(&html))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_urls_from_text() {
        let text = "Check out https://example.com and http://foo.bar/baz?q=1 for more info";
        let urls = extract_urls(text);
        assert_eq!(urls.len(), 2);
        assert_eq!(urls[0], "https://example.com");
        assert_eq!(urls[1], "http://foo.bar/baz?q=1");
    }

    #[test]
    fn extract_urls_empty() {
        assert!(extract_urls("no links here").is_empty());
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
        assert_eq!(og.image_url.as_deref(), Some("https://img.example.com/pic.jpg"));
        assert_eq!(og.site_name.as_deref(), Some("Example"));
    }

    #[test]
    fn parse_og_tags_fallback_to_title() {
        let html = r#"<html><head><title>My Page</title></head><body></body></html>"#;
        let og = parse_og_tags(html);
        assert_eq!(og.title.as_deref(), Some("My Page"));
        assert!(og.description.is_none());
    }
}
