//! Outgoing HTTP shared by link previews and server-to-server traffic: a body reader that stops
//! at a size cap.

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

    async fn get(addr: std::net::SocketAddr) -> reqwest::Response {
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
