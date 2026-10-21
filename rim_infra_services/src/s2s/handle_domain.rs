//! Handle domains: resolving a domain to the server that holds its handles, following one
//! delegation, and checking that the domain a peer declares points back at that peer.

use rim_s2s::is_handle_domain;
use serde::Deserialize;

use crate::s2s::net::normalize_origin;
use crate::s2s::peers::{DiscoveryFetcher, VerifiedDiscovery, verify_discovery};

/// A discovery document that hands a domain's handles to the server at another origin.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Delegation {
    delegate: String,
}

/// The server holding a domain's handles.
pub struct ResolvedDomain {
    /// The origin its discovery document was fetched from: the domain's own, or the one the
    /// domain delegates to.
    pub origin: String,
    pub discovery: VerifiedDiscovery,
}

/// Why a domain did not resolve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveError {
    /// A document could not be fetched; a later attempt may succeed.
    Unavailable(String),
    /// What was served does not hand the domain to a server.
    Refused(String),
}

/// Resolve the handle domain `domain` to the server that holds its handles. The discovery
/// document at `https://<domain>` is either the server's own, or a delegation
/// `{"delegate": "<origin>"}`, which is followed once: the document at that origin must be a
/// server's, not another delegation. Either way the server's descriptor must declare
/// `handleDomain` equal to `domain`, so neither a site nor a server can claim the other's
/// people. Fetches go through `fetcher` and its rules (time, size, no redirects, public
/// addresses); `allow_http` accepts an `http` delegate origin.
///
/// # Errors
/// [`ResolveError::Unavailable`] when a document cannot be fetched;
/// [`ResolveError::Refused`] when `domain` is not a handle domain, a document is neither a
/// discovery document nor a delegation, the delegate is not an origin, a delegation names
/// another delegation, or the server declares another handle domain.
pub async fn resolve_handle_domain(
    fetcher: &dyn DiscoveryFetcher,
    domain: &str,
    allow_http: bool,
) -> Result<ResolvedDomain, ResolveError> {
    if !is_handle_domain(domain) {
        return Err(ResolveError::Refused(format!(
            "{domain} is not a handle domain"
        )));
    }
    let apex = format!("https://{domain}");
    let bytes = fetch(fetcher, &apex).await?;
    let (origin, bytes) = match serde_json::from_slice::<Delegation>(&bytes) {
        Ok(delegation) => {
            let origin = normalize_origin(&delegation.delegate, allow_http)
                .map_err(|e| ResolveError::Refused(format!("delegate: {e}")))?;
            let bytes = fetch(fetcher, &origin).await?;
            if serde_json::from_slice::<Delegation>(&bytes).is_ok() {
                return Err(ResolveError::Refused(
                    "a delegation names another delegation".to_string(),
                ));
            }
            (origin, bytes)
        }
        Err(_) => (apex, bytes),
    };
    let discovery = verify_discovery(&bytes, &origin, None).map_err(ResolveError::Refused)?;
    if discovery.content.handle_domain.as_deref() != Some(domain) {
        return Err(ResolveError::Refused(format!(
            "the server at {origin} does not declare the handle domain {domain}"
        )));
    }
    Ok(ResolvedDomain { origin, discovery })
}

async fn fetch(fetcher: &dyn DiscoveryFetcher, origin: &str) -> Result<Vec<u8>, ResolveError> {
    fetcher
        .fetch(origin)
        .await
        .map_err(|e| ResolveError::Unavailable(format!("{origin}: {e}")))
}

/// What checking a peer's declared handle domain found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DomainCheck {
    /// The domain points back at the peer.
    Verified,
    /// The domain points elsewhere or nowhere.
    Refused(String),
    /// The domain could not be checked now.
    Unavailable(String),
}

/// Check that `domain`, which the peer `server_id` pinned at `origin` declares, points back at
/// it: the host of `origin` is `domain`, or `domain` resolves (see [`resolve_handle_domain`])
/// to `server_id`.
pub async fn check_handle_domain(
    fetcher: &dyn DiscoveryFetcher,
    origin: &str,
    server_id: &str,
    domain: &str,
    allow_http: bool,
) -> DomainCheck {
    if !is_handle_domain(domain) {
        return DomainCheck::Refused(format!("{domain} is not a handle domain"));
    }
    let host = url::Url::parse(origin)
        .ok()
        .and_then(|u| u.host_str().map(str::to_ascii_lowercase));
    if host.as_deref() == Some(domain) {
        return DomainCheck::Verified;
    }
    match resolve_handle_domain(fetcher, domain, allow_http).await {
        Ok(resolved) if resolved.discovery.state.server_id == server_id => DomainCheck::Verified,
        Ok(resolved) => DomainCheck::Refused(format!(
            "{domain} points at the server at {}",
            resolved.origin
        )),
        Err(ResolveError::Refused(reason)) => DomainCheck::Refused(reason),
        Err(ResolveError::Unavailable(reason)) => DomainCheck::Unavailable(reason),
    }
}

#[cfg(test)]
#[path = "handle_domain_tests.rs"]
mod tests;
