//! A `graphql-transport-ws` client on a test server's `/api/ws`, framed with actix-http's
//! WebSocket codec in client mode.

use std::time::Duration;

use actix_codec::Framed;
use actix_http::ws::{Codec, Frame, Item, Message};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// How long a test waits for one message.
const RECEIVE_TIMEOUT: Duration = Duration::from_secs(10);

/// An acknowledged `graphql-transport-ws` connection.
pub struct Socket(Framed<TcpStream, Codec>);

impl Socket {
    /// Upgrades on `origin`'s `/api/ws` with `session_token` as the session cookie, then sends
    /// `connection_init` and waits for `connection_ack`.
    pub async fn connect(origin: &str, session_token: &str) -> Self {
        match Self::open(origin, session_token).await {
            Ok(socket) => socket,
            Err(status) => panic!("the upgrade was refused with {status}"),
        }
    }

    /// [`Socket::connect`], or the HTTP status the upgrade was refused with.
    pub async fn open(origin: &str, session_token: &str) -> Result<Self, u16> {
        let mut socket = Self::upgrade(origin, session_token).await?;
        socket.send(json!({ "type": "connection_init" })).await;
        assert_eq!(socket.receive().await["type"], "connection_ack");
        Ok(socket)
    }

    /// Upgrades on `origin`'s `/api/ws` with `session_token` as the session cookie and sends
    /// nothing, or answers the HTTP status the upgrade was refused with.
    pub async fn upgrade(origin: &str, session_token: &str) -> Result<Self, u16> {
        let host = origin.trim_start_matches("http://");
        let mut stream = TcpStream::connect(host).await.expect("connect");
        let handshake = format!(
            "GET /api/ws HTTP/1.1\r\nhost: {host}\r\nupgrade: websocket\r\nconnection: Upgrade\r\n\
             sec-websocket-version: 13\r\nsec-websocket-key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
             sec-websocket-protocol: graphql-transport-ws\r\ncookie: rim_session={session_token}\r\n\r\n"
        );
        stream
            .write_all(handshake.as_bytes())
            .await
            .expect("handshake");
        let mut head = Vec::new();
        while !head.ends_with(b"\r\n\r\n") {
            head.push(stream.read_u8().await.expect("response head"));
        }
        let head = String::from_utf8_lossy(&head);
        let status: u16 = head
            .split(' ')
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or_else(|| panic!("no status in {head}"));
        if status != 101 {
            return Err(status);
        }
        Ok(Self(Framed::new(stream, Codec::new().client_mode())))
    }

    /// Sends `subscribe` for `query` under `id`.
    pub async fn subscribe(&mut self, id: &str, query: &str, variables: Value) {
        self.send(json!({
            "id": id,
            "type": "subscribe",
            "payload": { "query": query, "variables": variables },
        }))
        .await;
    }

    /// The next protocol message, answering pings on the way.
    pub async fn receive(&mut self) -> Value {
        self.receive_within(RECEIVE_TIMEOUT)
            .await
            .expect("a message in time")
    }

    /// The next protocol message, or `None` when none comes within `wait`.
    pub async fn receive_within(&mut self, wait: Duration) -> Option<Value> {
        loop {
            let frame = tokio::time::timeout(wait, self.0.next())
                .await
                .ok()?
                .expect("the socket is open")
                .expect("a frame");
            match frame {
                Frame::Text(text) => {
                    let value: Value = serde_json::from_slice(&text).expect("json");
                    if value["type"] == "ping" {
                        self.send(json!({ "type": "pong" })).await;
                        continue;
                    }
                    return Some(value);
                }
                Frame::Ping(payload) => {
                    self.0.send(Message::Pong(payload)).await.expect("pong");
                }
                Frame::Close(reason) => panic!("the socket closed: {reason:?}"),
                _ => {}
            }
        }
    }

    /// Within `wait`, the server closes the socket with `code` and `reason` before sending any
    /// other message, and nothing follows the close.
    pub async fn expect_close(&mut self, code: u16, reason: &str, wait: Duration) {
        let close = tokio::time::timeout(wait, async {
            loop {
                match self
                    .0
                    .next()
                    .await
                    .expect("a close frame")
                    .expect("a frame")
                {
                    Frame::Close(close) => return close,
                    Frame::Ping(payload) => {
                        let _ = self.0.send(Message::Pong(payload)).await;
                    }
                    Frame::Pong(_) => {}
                    other => panic!("expected the close, got {other:?}"),
                }
            }
        })
        .await
        .expect("the close in time")
        .expect("a close reason");
        assert_eq!(u16::from(close.code), code);
        assert_eq!(close.description.as_deref(), Some(reason));
        let after = tokio::time::timeout(wait, self.0.next())
            .await
            .expect("the server ends the connection");
        assert!(after.is_none(), "nothing follows the close: {after:?}");
    }

    /// Sends `message` as one text message in fragments of `fragment` bytes, at least two.
    pub async fn send_fragmented(&mut self, message: &[u8], fragment: usize) {
        let fragments: Vec<&[u8]> = message.chunks(fragment).collect();
        assert!(
            fragments.len() > 1,
            "a fragmented message has two fragments"
        );
        let last = fragments.len() - 1;
        for (i, fragment) in fragments.into_iter().enumerate() {
            let bytes = fragment.to_vec().into();
            let item = match i {
                0 => Item::FirstText(bytes),
                i if i == last => Item::Last(bytes),
                _ => Item::Continue(bytes),
            };
            self.0
                .send(Message::Continuation(item))
                .await
                .expect("send a fragment");
        }
    }

    /// Sends the header of a text frame announcing `length` payload bytes, and no payload.
    pub async fn send_frame_header(&mut self, length: u64) {
        let mut header = vec![0x81, 0x80 | 127];
        header.extend_from_slice(&length.to_be_bytes());
        header.extend_from_slice(&[0; 4]);
        self.0
            .io_mut()
            .write_all(&header)
            .await
            .expect("send a frame header");
    }

    /// Sends `message` as one text message.
    pub async fn send(&mut self, message: Value) {
        self.0
            .send(Message::Text(message.to_string().into()))
            .await
            .expect("send");
    }
}
