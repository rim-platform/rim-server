//! Outgoing HTTP for server-to-server traffic: origin checks, the client, and bounded body
//! reads, under the address policy of [`crate::outbound`].

use std::sync::Arc;
use std::time::Duration;

use url::Url;

use crate::outbound::{BodyError, PublicOnlyResolver, is_public_address, read_capped, url_ip};

/// Why an origin or an address was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NetError {
    #[error("{0}")]
    BadOrigin(String),
    #[error("{0} resolves to a private address")]
    PrivateAddress(String),
    #[error("{0}")]
    Transport(String),
    #[error("response exceeds {0} bytes")]
    TooLarge(usize),
}

/// The canonical origin (`scheme://host[:port]`, lowercase, default port omitted) of a URL that
/// is only an origin: `https`, or `http` when `allow_http`; no credentials, path, query or
/// fragment.
pub fn normalize_origin(origin: &str, allow_http: bool) -> Result<String, NetError> {
    let bad = |why: &str| NetError::BadOrigin(format!("{why}: {origin}"));
    let url = Url::parse(origin.trim()).map_err(|_| bad("not a URL"))?;
    match url.scheme() {
        "https" => {}
        "http" if allow_http => {}
        _ => return Err(bad("origin must be https")),
    }
    if url.host().is_none() {
        return Err(bad("origin has no host"));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(bad("origin must not carry credentials"));
    }
    if url.path() != "/" || url.query().is_some() || url.fragment().is_some() {
        return Err(bad("origin must not have a path, query or fragment"));
    }
    Ok(url.origin().ascii_serialization())
}

/// Refuse a URL whose host is an IP literal outside the public internet. Names are checked by
/// the public-only resolver when they are resolved.
pub fn check_url_host(url: &Url, allow_private: bool) -> Result<(), NetError> {
    if allow_private {
        return Ok(());
    }
    match url_ip(url) {
        Some(ip) if !is_public_address(ip) => Err(NetError::PrivateAddress(
            url.host_str().unwrap_or_default().to_string(),
        )),
        _ => Ok(()),
    }
}

/// An HTTP client for server-to-server traffic: rustls, no redirects, no proxy, `timeout` per
/// request, and the public-only resolver unless `allow_private`.
pub fn http_client(allow_private: bool, timeout: Duration) -> Result<reqwest::Client, NetError> {
    let mut builder = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(timeout)
        // A proxy would resolve the target itself, past the resolver's address check.
        .no_proxy()
        .connect_timeout(timeout.min(Duration::from_secs(5)))
        .user_agent(concat!("rim-server/", env!("CARGO_PKG_VERSION")));
    if !allow_private {
        builder = builder.dns_resolver(Arc::new(PublicOnlyResolver));
    }
    builder
        .build()
        .map_err(|e| NetError::Transport(e.to_string()))
}

/// Read a response body, failing once it would pass `max` bytes.
pub async fn read_limited(response: reqwest::Response, max: usize) -> Result<Vec<u8>, NetError> {
    read_capped(response, max).await.map_err(|e| match e {
        BodyError::TooLarge(max) => NetError::TooLarge(max),
        BodyError::Transport(why) => NetError::Transport(why),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origins_are_normalized_and_restricted() {
        assert_eq!(
            normalize_origin("https://Relay.Example:443/", false),
            Ok("https://relay.example".to_string())
        );
        assert_eq!(
            normalize_origin("https://relay.example:8443", false),
            Ok("https://relay.example:8443".to_string())
        );
        assert_eq!(
            normalize_origin("http://127.0.0.1:8098", true),
            Ok("http://127.0.0.1:8098".to_string())
        );
        for bad in [
            "http://relay.example",
            "ftp://relay.example",
            "https://user:pw@relay.example",
            "https://relay.example/path",
            "https://relay.example/?q=1",
            "relay.example",
        ] {
            assert!(normalize_origin(bad, false).is_err(), "{bad}");
        }
    }

    #[test]
    fn ip_literal_hosts_are_checked() {
        for refused in [
            "http://127.0.0.1:8098/x",
            "http://[::1]/x",
            "http://[64:ff9b::7f00:1]/x",
            "http://240.0.0.1/x",
        ] {
            let url = Url::parse(refused).expect("url");
            assert!(check_url_host(&url, false).is_err(), "{refused}");
            assert!(check_url_host(&url, true).is_ok(), "{refused}");
        }
        let url = Url::parse("https://relay.example/x").expect("url");
        assert!(check_url_host(&url, false).is_ok());
    }
}
