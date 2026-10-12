//! Outgoing HTTP shared by link previews and server-to-server traffic: which addresses may be
//! dialled, a resolver that enforces it, and a body reader that stops at a size cap.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use url::Url;

/// Whether `ip` is reachable on the public internet. IPv4 refused: "this network" (0.0.0.0/8,
/// unspecified included), private, loopback, link-local, shared (100.64.0.0/10), IETF protocol
/// assignments (192.0.0.0/24), benchmarking, documentation, multicast and reserved
/// (240.0.0.0/4, broadcast included). IPv6 is public only inside global unicast (2000::/3),
/// which leaves out unspecified, loopback, discard-only, local-use NAT64 (64:ff9b:1::/48),
/// unique-local, link-local, site-local and multicast; inside it, the IETF protocol assignments
/// (2001::/23: Teredo, benchmarking, ORCHID) and documentation are refused. IPv4-mapped,
/// IPv4-compatible, NAT64 (64:ff9b::/96) and 6to4 (2002::/16) addresses are judged by the IPv4
/// address they embed.
pub fn is_public_address(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_public_v4(v4),
        IpAddr::V6(v6) => is_public_v6(v6),
    }
}

fn is_public_v4(v4: Ipv4Addr) -> bool {
    let [a, b, c, _] = v4.octets();
    !(a == 0
        || a == 10
        || a == 127
        || (a == 100 && (b & 0xc0) == 64)
        || (a == 169 && b == 254)
        || (a == 172 && (b & 0xf0) == 16)
        || (a == 192 && b == 0 && c == 0)
        || (a == 192 && b == 0 && c == 2)
        || (a == 192 && b == 168)
        || (a == 198 && (b & 0xfe) == 18)
        || (a == 198 && b == 51 && c == 100)
        || (a == 203 && b == 0 && c == 113)
        || a >= 224)
}

fn is_public_v6(v6: Ipv6Addr) -> bool {
    let s = v6.segments();
    let o = v6.octets();
    let embedded = |i: usize| Ipv4Addr::new(o[i], o[i + 1], o[i + 2], o[i + 3]);
    // IPv4-mapped (::ffff:0:0/96) and IPv4-compatible (::/96, `::` and `::1` included).
    if s[..5] == [0; 5] && (s[5] == 0 || s[5] == 0xffff) {
        return is_public_v4(embedded(12));
    }
    // NAT64 well-known prefix (64:ff9b::/96).
    if s[..6] == [0x64, 0xff9b, 0, 0, 0, 0] {
        return is_public_v4(embedded(12));
    }
    // 6to4 (2002::/16): the IPv4 address follows the prefix.
    if s[0] == 0x2002 {
        return is_public_v4(embedded(2));
    }
    let global_unicast = (s[0] & 0xe000) == 0x2000;
    let protocol_or_documentation = s[0] == 0x2001 && (s[1] < 0x0200 || s[1] == 0x0db8);
    let new_documentation = s[0] == 0x3fff && s[1] < 0x1000;
    global_unicast && !protocol_or_documentation && !new_documentation
}

/// The URL's host when it is an IP literal; such a host is dialled without a resolver.
pub fn url_ip(url: &Url) -> Option<IpAddr> {
    match url.host()? {
        url::Host::Ipv4(ip) => Some(IpAddr::V4(ip)),
        url::Host::Ipv6(ip) => Some(IpAddr::V6(ip)),
        url::Host::Domain(_) => None,
    }
}

/// A name that resolved to an address [`is_public_address`] refuses.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0} resolves to a non-public address")]
pub struct NonPublicAddress(pub String);

/// A resolver that fails when a name resolves to any non-public address. The client connects
/// to the addresses checked here, so a DNS answer cannot change between check and connect.
#[derive(Debug, Default, Clone, Copy)]
pub struct PublicOnlyResolver;

impl Resolve for PublicOnlyResolver {
    fn resolve(&self, name: Name) -> Resolving {
        Box::pin(async move {
            let host = name.as_str().to_string();
            let addrs: Vec<SocketAddr> =
                tokio::net::lookup_host((host.as_str(), 0)).await?.collect();
            if addrs.is_empty() || addrs.iter().any(|a| !is_public_address(a.ip())) {
                return Err(Box::new(NonPublicAddress(host)) as _);
            }
            Ok(Box::new(addrs.into_iter()) as Addrs)
        })
    }
}

/// Why a body read stopped.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BodyError {
    #[error("response exceeds {0} bytes")]
    TooLarge(usize),
    #[error("{0}")]
    Transport(String),
}

/// Read a response body chunk by chunk, failing as soon as it would pass `max` bytes. A
/// declared `Content-Length` over `max` fails before anything is read.
pub async fn read_capped(
    mut response: reqwest::Response,
    max: usize,
) -> Result<Vec<u8>, BodyError> {
    let declared = response.content_length();
    if declared.is_some_and(|len| len > max as u64) {
        return Err(BodyError::TooLarge(max));
    }
    let mut body = Vec::with_capacity(declared.map_or(0, |len| len as usize));
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| BodyError::Transport(e.to_string()))?
    {
        if chunk.len() > max - body.len() {
            return Err(BodyError::TooLarge(max));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// A loopback HTTP server for tests of outgoing requests.
#[cfg(test)]
pub(crate) mod fixture {
    use std::net::SocketAddr;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// The server's address and when it accepted each connection.
    pub(crate) struct Server {
        pub(crate) addr: SocketAddr,
        accepted: Arc<Mutex<Vec<Instant>>>,
    }

    impl Server {
        pub(crate) fn accepted(&self) -> Vec<Instant> {
            self.accepted.lock().map(|a| a.clone()).unwrap_or_default()
        }
    }

    /// Answer every connection with `head`, then `body` after `delay`, then keep the
    /// connection open for `linger` before closing it.
    pub(crate) async fn serve(
        head: String,
        body: Vec<u8>,
        delay: Duration,
        linger: Duration,
    ) -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let accepted = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&accepted);
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                if let Ok(mut log) = log.lock() {
                    log.push(Instant::now());
                }
                let (head, body) = (head.clone(), body.clone());
                tokio::spawn(async move {
                    let mut buf = [0u8; 4096];
                    let _ = sock.read(&mut buf).await;
                    let _ = sock.write_all(head.as_bytes()).await;
                    tokio::time::sleep(delay).await;
                    let _ = sock.write_all(&body).await;
                    tokio::time::sleep(linger).await;
                });
            }
        });
        Server { addr, accepted }
    }

    /// A `200` head with `Content-Length: len` that closes the connection afterwards.
    pub(crate) fn sized_head(len: usize) -> String {
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n"
        )
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::fixture::{serve, sized_head};
    use super::*;

    #[test]
    fn non_public_addresses_are_refused() {
        for ip in [
            // IPv4
            "0.0.0.0",
            "0.1.2.3",
            "127.0.0.1",
            "127.255.255.254",
            "10.1.2.3",
            "172.16.0.1",
            "172.31.255.255",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "100.127.255.255",
            "100.100.100.200",
            "192.0.0.170",
            "192.0.2.1",
            "198.51.100.1",
            "203.0.113.1",
            "198.18.0.1",
            "198.19.255.255",
            "224.0.0.1",
            "239.255.255.250",
            "240.0.0.1",
            "255.255.255.255",
            // IPv6
            "::",
            "::1",
            "fc00::1",
            "fd00:ec2::254",
            "fe80::1",
            "fec0::1",
            "ff02::1",
            "100::1",
            "2001::1",
            "2001:0:4136:e378:8000:63bf:3fff:fdd2",
            "2001:2::1",
            "2001:10::1",
            "2001:db8::1",
            "3fff::1",
            "3fff:fff::1",
            "5f00::1",
            "4000::1",
            "::ffff:0:7f00:1",
            // Embedded IPv4, judged by the IPv4 address
            "::ffff:127.0.0.1",
            "::ffff:10.0.0.1",
            "::ffff:169.254.169.254",
            "::7f00:1",
            "::a00:1",
            "64:ff9b::7f00:1",
            "64:ff9b::a9fe:a9fe",
            "64:ff9b::f000:1",
            "2002:7f00:1::1",
            "2002:a9fe:a9fe::",
            "2002:c0a8:101::1",
            // Local-use NAT64, refused whatever it embeds
            "64:ff9b:1::808:808",
            "64:ff9b:1:7f00:1::",
        ] {
            assert!(
                !is_public_address(ip.parse().expect("ip")),
                "{ip} must be refused"
            );
        }
    }

    #[test]
    fn public_addresses_are_accepted() {
        for ip in [
            "8.8.8.8",
            "1.1.1.1",
            "93.184.216.34",
            "100.128.0.1",
            "172.32.0.1",
            "192.0.1.1",
            "198.20.0.1",
            "223.255.255.254",
            "2606:4700::1111",
            "2606:2800:220:1::1",
            "2a00:1450:4001:80b::200e",
            "2001:4860:4860::8888",
            "3ffe:1::1",
            "::ffff:8.8.8.8",
            "64:ff9b::808:808",
            "2002:808:808::1",
        ] {
            assert!(
                is_public_address(ip.parse().expect("ip")),
                "{ip} must be accepted"
            );
        }
    }

    #[test]
    fn url_ip_reads_literals_only() {
        let url = Url::parse("http://[::1]:8080/x").expect("url");
        assert_eq!(url_ip(&url), Some("::1".parse().expect("ip")));
        let url = Url::parse("http://127.0.0.1/").expect("url");
        assert_eq!(url_ip(&url), Some("127.0.0.1".parse().expect("ip")));
        let url = Url::parse("http://example.com/").expect("url");
        assert_eq!(url_ip(&url), None);
    }

    #[tokio::test]
    async fn the_resolver_refuses_a_name_resolving_to_loopback() {
        let name: Name = "localhost".parse().expect("name");
        assert!(PublicOnlyResolver.resolve(name).await.is_err());
    }

    async fn get(addr: SocketAddr) -> reqwest::Response {
        reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(10))
            .build()
            .expect("client")
            .get(format!("http://{addr}/"))
            .send()
            .await
            .expect("response")
    }

    #[tokio::test]
    async fn a_body_under_the_cap_is_read_whole() {
        let body = vec![b'b'; 4000];
        let server = serve(
            sized_head(body.len()),
            body.clone(),
            Duration::ZERO,
            Duration::ZERO,
        )
        .await;
        assert_eq!(read_capped(get(server.addr).await, 4000).await, Ok(body));
    }

    /// The fixture declares a length over the cap and sends nothing: the refusal comes from the
    /// header alone.
    #[tokio::test]
    async fn a_declared_length_over_the_cap_is_refused_before_reading() {
        let linger = Duration::from_secs(30);
        let server = serve(
            sized_head(2 * 1024 * 1024),
            Vec::new(),
            Duration::ZERO,
            linger,
        )
        .await;
        let started = Instant::now();
        let result = read_capped(get(server.addr).await, 1024 * 1024).await;
        assert_eq!(result, Err(BodyError::TooLarge(1024 * 1024)));
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}
