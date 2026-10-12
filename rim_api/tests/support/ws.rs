//! A `graphql-transport-ws` client on a test server's `/api/ws`.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

/// How long a test waits for one message.
const RECEIVE_TIMEOUT: Duration = Duration::from_secs(10);

/// An acknowledged `graphql-transport-ws` connection.
pub struct Socket(WebSocketStream<MaybeTlsStream<TcpStream>>);

impl Socket {
    /// Upgrades on `origin`'s `/api/ws` with `session_token` as the session cookie, then sends
    /// `connection_init` and waits for `connection_ack`.
    pub async fn connect(origin: &str, session_token: &str) -> Self {
        let url = format!("ws://{}/api/ws", origin.trim_start_matches("http://"));
        let mut request = url.into_client_request().expect("ws request");
        let headers = request.headers_mut();
        headers.insert(
            "cookie",
            format!("rim_session={session_token}")
                .parse()
                .expect("cookie header"),
        );
        headers.insert(
            "sec-websocket-protocol",
            "graphql-transport-ws".parse().expect("protocol header"),
        );
        let (stream, _) = tokio_tungstenite::connect_async(request)
            .await
            .expect("ws upgrade");
        let mut socket = Self(stream);
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
            let message = tokio::time::timeout(wait, self.0.next())
                .await
                .ok()?
                .expect("the socket is open")
                .expect("a frame");
            let Message::Text(text) = message else {
                continue;
            };
            let value: Value = serde_json::from_str(&text).expect("json");
            if value["type"] == "ping" {
                self.send(json!({ "type": "pong" })).await;
                continue;
            }
            return Some(value);
        }
    }

    async fn send(&mut self, message: Value) {
        self.0
            .send(Message::Text(message.to_string().into()))
            .await
            .expect("send");
    }
}
