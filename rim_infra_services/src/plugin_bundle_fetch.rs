use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use reqwest::{Client, Url};
use sha2::{Digest, Sha256};
use tokio::sync::Semaphore;

use rim_services::command::rate_limit::RateLimitDecision;
use rim_services::workspace_plugin::MAX_PLUGIN_BUNDLE_BYTES;
use rim_services::workspace_plugin::command::FetchPluginBundleCmd;
use rim_services::{ServiceError, ServiceResult};

use crate::outbound::{BodyError, PublicOnlyResolver, is_public_address, read_capped, url_ip};
use crate::registration_rate_limiter::RegistrationRateLimiter;

const FETCH_TIMEOUT: Duration = Duration::from_secs(120);

fn checked_url(raw: &str) -> ServiceResult<Url> {
    let url =
        Url::parse(raw).map_err(|_| ServiceError::Validation("invalid bundle URL".to_string()))?;
    let host = url.host_str().unwrap_or_default();
    if raw.len() > 4096
        || url.scheme() != "https"
        || host.is_empty()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || host == "localhost"
        || host.ends_with(".local")
        || url_ip(&url).is_some_and(|ip| !is_public_address(ip))
    {
        return Err(ServiceError::Validation(
            "a bundle URL must use HTTPS and public addresses, without credentials or a fragment"
                .to_string(),
        ));
    }
    Ok(url)
}

fn redirect_policy() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(|attempt| {
        if attempt.previous().len() > 10 || checked_url(attempt.url().as_str()).is_err() {
            attempt.stop()
        } else {
            attempt.follow()
        }
    })
}

fn check_bytes(bytes: &[u8], pin: &str) -> ServiceResult<()> {
    let actual: String = Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    if actual != pin {
        return Err(ServiceError::Validation(
            "bundle SHA-256 mismatch".to_string(),
        ));
    }
    crate::plugin_bundle::check_bundle(bytes).map_err(ServiceError::Validation)
}

async fn download(client: &Client, url: Url, pin: &str) -> ServiceResult<Vec<u8>> {
    let response = client.get(url).send().await.map_err(|error| {
        if error.is_timeout() {
            ServiceError::Timeout
        } else {
            ServiceError::Unavailable
        }
    })?;
    if !response.status().is_success() {
        return Err(ServiceError::Unavailable);
    }
    let bytes = read_capped(response, MAX_PLUGIN_BUNDLE_BYTES)
        .await
        .map_err(|error| match error {
            BodyError::TooLarge(_) => {
                ServiceError::Validation("a bundle is at most 8 MiB".to_string())
            }
            BodyError::Transport(_) => ServiceError::Unavailable,
        })?;
    check_bytes(&bytes, pin)?;
    Ok(bytes)
}

pub(crate) struct BundleFetcher {
    client: Option<Client>,
    limiter: RegistrationRateLimiter,
    slots: Semaphore,
}

impl BundleFetcher {
    pub(crate) fn new() -> Self {
        let client = Client::builder()
            .https_only(true)
            .no_proxy()
            .referer(false)
            .dns_resolver(Arc::new(PublicOnlyResolver))
            .redirect(redirect_policy())
            .timeout(FETCH_TIMEOUT)
            .connect_timeout(Duration::from_secs(10))
            .user_agent("RIM-PluginBundle/1.0")
            .build()
            .inspect_err(|_| tracing::error!("could not initialize plugin bundle HTTP client"))
            .ok();
        Self {
            client,
            limiter: RegistrationRateLimiter::new(3600, 20),
            slots: Semaphore::new(4),
        }
    }

    pub(crate) async fn fetch(&self, cmd: FetchPluginBundleCmd) -> ServiceResult<Vec<u8>> {
        if cmd.sha256.len() != 64
            || !cmd
                .sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(ServiceError::Validation(
                "sha256 must be 64 lowercase hex characters".to_string(),
            ));
        }
        let url = checked_url(&cmd.url)?;
        let host = url.host_str().unwrap_or_default().to_string();
        if let RateLimitDecision::Limited {
            retry_after_seconds,
        } = self.limiter.check_and_record(
            &cmd.actor.user_id.to_string(),
            "plugin_bundle_fetch",
            Utc::now(),
        ) {
            tracing::info!(target: "audit", event = "plugin_bundle_fetch", actor_id = %cmd.actor.user_id, host, outcome = "rate_limited");
            return Err(ServiceError::RateLimited {
                retry_after_seconds,
            });
        }
        let result = async {
            let _permit = tokio::time::timeout(Duration::from_secs(5), self.slots.acquire())
                .await
                .map_err(|_| ServiceError::Unavailable)?
                .map_err(|_| ServiceError::Unavailable)?;
            let client = self.client.as_ref().ok_or(ServiceError::Unavailable)?;
            download(client, url, &cmd.sha256).await
        }
        .await;
        tracing::info!(target: "audit", event = "plugin_bundle_fetch", actor_id = %cmd.actor.user_id, host, success = result.is_ok());
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_refuse_private_literals_other_schemes_credentials_and_fragments() {
        for url in [
            "http://example.com/x",
            "https://127.0.0.1/x",
            "https://[::1]/x",
            "https://10.0.0.1/x",
            "https://169.254.169.254/x",
            "https://user:pass@example.com/x",
            "https://localhost/x",
            "https://service.local/x",
            "https://example.com/x#pin",
        ] {
            assert!(checked_url(url).is_err(), "{url}");
        }
        assert!(checked_url("https://example.com/plugin.rimbundle").is_ok());
    }

    #[test]
    fn fetched_bytes_need_both_the_pin_and_bundle_structure() {
        let bundle = br#"{"format":"rim-plugin-bundle","v":1,"files":[{"path":"rim-plugin.json","text":"{}"}]}"#;
        let hash = |bytes: &[u8]| {
            Sha256::digest(bytes)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        };
        assert!(check_bytes(bundle, &hash(bundle)).is_ok());
        assert!(check_bytes(bundle, &"0".repeat(64)).is_err());
        assert!(check_bytes(b"not a bundle", &hash(b"not a bundle")).is_err());
    }

    #[tokio::test]
    async fn downloads_check_the_body_cap_and_timeout() {
        let server = crate::outbound::fixture::serve(
            format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
                MAX_PLUGIN_BUNDLE_BYTES + 1
            ),
            Vec::new(),
            Duration::ZERO,
            Duration::ZERO,
        )
        .await;
        let client = Client::builder()
            .no_proxy()
            .timeout(Duration::from_millis(100))
            .build()
            .unwrap();
        let result = download(
            &client,
            Url::parse(&format!("http://{}/", server.addr)).unwrap(),
            &"0".repeat(64),
        )
        .await;
        assert!(matches!(result, Err(ServiceError::Validation(_))));
        let server = crate::outbound::fixture::serve(
            "HTTP/1.1 200 OK\r\nContent-Length: 1\r\n\r\n".to_string(),
            vec![b'x'],
            Duration::from_secs(1),
            Duration::ZERO,
        )
        .await;
        let result = download(
            &client,
            Url::parse(&format!("http://{}/", server.addr)).unwrap(),
            &"0".repeat(64),
        )
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn redirects_to_private_addresses_are_not_followed() {
        let server = crate::outbound::fixture::serve(
            "HTTP/1.1 302 Found\r\nLocation: https://127.0.0.1/private\r\nContent-Length: 0\r\n\r\n".to_string(),
            Vec::new(), Duration::ZERO, Duration::ZERO,
        ).await;
        let client = Client::builder()
            .no_proxy()
            .redirect(redirect_policy())
            .build()
            .unwrap();
        let response = client
            .get(format!("http://{}/", server.addr))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 302);
    }

    #[tokio::test]
    async fn fetching_is_rate_limited_before_opening_a_connection() {
        let fetcher = BundleFetcher::new();
        let user = rim_core::user::model::UserId::new();
        for _ in 0..20 {
            assert!(matches!(
                fetcher.limiter.check_and_record(
                    &user.to_string(),
                    "plugin_bundle_fetch",
                    Utc::now()
                ),
                RateLimitDecision::Allowed
            ));
        }
        let result = fetcher
            .fetch(FetchPluginBundleCmd {
                actor: rim_services::Actor::new(user),
                url: "https://example.com/bundle".to_string(),
                sha256: "0".repeat(64),
            })
            .await;
        assert!(matches!(result, Err(ServiceError::RateLimited { .. })));
    }

    #[tokio::test]
    async fn fetching_a_private_address_is_refused_before_a_request() {
        let result = BundleFetcher::new()
            .fetch(FetchPluginBundleCmd {
                actor: rim_services::Actor::new(rim_core::user::model::UserId::new()),
                url: "https://127.0.0.1/".to_string(),
                sha256: "0".repeat(64),
            })
            .await;
        assert!(matches!(result, Err(ServiceError::Validation(_))));
    }
}
