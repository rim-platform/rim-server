//! Outgoing HTTP for server-to-server traffic: origin checks, a resolver that refuses private
//! addresses, and bounded body reads.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use url::Url;

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

/// Whether `ip` is reachable on the public internet: not loopback, private, link-local,
/// unique-local, shared (CGNAT), unspecified, broadcast, documentation or multicast.
pub fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            !(v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_documentation()
                || v4.is_multicast()
                || o[0] == 0
                || (o[0] == 100 && (o[1] & 0xc0) == 64))
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_public_ip(IpAddr::V4(v4));
            }
            let first = v6.segments()[0];
            !(v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (first & 0xfe00) == 0xfc00
                || (first & 0xffc0) == 0xfe80
                || first == 0x2001 && v6.segments()[1] == 0x0db8)
        }
    }
}

/// Refuse a URL whose host is an IP literal outside the public internet. Names are checked by
/// [`PublicOnlyResolver`] when they are resolved.
pub fn check_url_host(url: &Url, allow_private: bool) -> Result<(), NetError> {
    if allow_private {
        return Ok(());
    }
    let ip = match url.host() {
        Some(url::Host::Ipv4(ip)) => IpAddr::V4(ip),
        Some(url::Host::Ipv6(ip)) => IpAddr::V6(ip),
        _ => return Ok(()),
    };
    if is_public_ip(ip) {
        Ok(())
    } else {
        Err(NetError::PrivateAddress(
            url.host_str().unwrap_or_default().to_string(),
        ))
    }
}

/// A resolver that fails when a name resolves to any non-public address, so a peer's DNS
/// cannot point server-to-server traffic into the private network. The connection uses the
/// addresses checked here.
#[derive(Debug, Default)]
pub struct PublicOnlyResolver;

impl Resolve for PublicOnlyResolver {
    fn resolve(&self, name: Name) -> Resolving {
        Box::pin(async move {
            let host = name.as_str().to_string();
            let addrs: Vec<SocketAddr> =
                tokio::net::lookup_host((host.as_str(), 0)).await?.collect();
            if addrs.is_empty() || addrs.iter().any(|a| !is_public_ip(a.ip())) {
                return Err(Box::new(NetError::PrivateAddress(host)) as _);
            }
            Ok(Box::new(addrs.into_iter()) as Addrs)
        })
    }
}

/// An HTTP client for server-to-server traffic: rustls, no redirects, `timeout` per request,
/// and the public-only resolver unless `allow_private`.
pub fn http_client(allow_private: bool, timeout: Duration) -> Result<reqwest::Client, NetError> {
    let mut builder = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(timeout)
        .connect_timeout(timeout.min(Duration::from_secs(5)))
        .user_agent(concat!("rim-server/", env!("CARGO_PKG_VERSION")));
    if !allow_private {
        builder = builder.dns_resolver(Arc::new(PublicOnlyResolver));
    }
    builder
        .build()
        .map_err(|e| NetError::Transport(e.to_string()))
}

/// Read a response body, failing once it exceeds `max` bytes.
pub async fn read_limited(
    mut response: reqwest::Response,
    max: usize,
) -> Result<Vec<u8>, NetError> {
    if response
        .content_length()
        .is_some_and(|len| len > max as u64)
    {
        return Err(NetError::TooLarge(max));
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| NetError::Transport(e.to_string()))?
    {
        if body.len() + chunk.len() > max {
            return Err(NetError::TooLarge(max));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
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
    fn private_and_special_addresses_are_not_public() {
        for ip in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.1.1",
            "100.64.0.1",
            "0.0.0.0",
            "::1",
            "fd00::1",
            "fe80::1",
            "::ffff:10.0.0.1",
        ] {
            assert!(!is_public_ip(ip.parse().expect("ip")), "{ip}");
        }
        for ip in ["93.184.216.34", "2606:2800:220:1::1"] {
            assert!(is_public_ip(ip.parse().expect("ip")), "{ip}");
        }
    }

    #[test]
    fn ip_literal_hosts_are_checked() {
        let url = Url::parse("http://127.0.0.1:8098/x").expect("url");
        assert!(check_url_host(&url, false).is_err());
        assert!(check_url_host(&url, true).is_ok());
        let url = Url::parse("https://relay.example/x").expect("url");
        assert!(check_url_host(&url, false).is_ok());
    }
}
