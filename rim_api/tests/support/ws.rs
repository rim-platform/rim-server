//! A `graphql-transport-ws` client on a test server's `/api/ws`, framed with actix-http's
//! WebSocket codec in client mode.

use std::time::Duration;

use actix_codec::Framed;
use actix_http::ws::{Codec, Frame, Message};
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
        assert!(head.starts_with("HTTP/1.1 101"), "{head}");
        let mut socket = Self(Framed::new(stream, Codec::new().client_mode()));
        socket.send(json!({ "type": "connection_init" })).await;
        assert_eq!(socket.receive().await["type"], "connection_ack");
        socket
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

    async fn send(&mut self, message: Value) {
        self.0
            .send(Message::Text(message.to_string().into()))
            .await
            .expect("send");
    }
}
