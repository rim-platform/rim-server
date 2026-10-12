//! The GraphQL WebSocket transport: async-graphql's protocol engine on an actix actor that also
//! ends the connection when its session ends.
//!
//! A connection closes with code [`SESSION_ENDED`] when its session is revoked, as its
//! [`SessionWatch`] resolves, or when the session reaches its expiry. Closing cancels the
//! protocol engine first, which ends every active stream and drops the connection's data, and
//! with it the connection slot; nothing more is sent after the close frame.

use std::str::FromStr;
use std::time::{Duration, Instant};

use actix::{
    Actor, ActorContext, ActorFutureExt, ActorStreamExt, AsyncContext, SpawnHandle, StreamHandler,
    WrapFuture, WrapStream,
};
use actix_http::ws::Item;
use actix_web::error::PayloadError;
use actix_web::web::Bytes;
use actix_web::{HttpRequest, HttpResponse};
use actix_web_actors::ws::{
    CloseCode, CloseReason, Message, ProtocolError, WebsocketContext, WsResponseBuilder,
};
use async_graphql::Data;
use async_graphql::http::{ALL_WEBSOCKET_PROTOCOLS, WebSocket, WebSocketProtocols, WsMessage};
use chrono::{DateTime, Utc};
use futures_util::Stream;

use rim_api::AppSchema;
use rim_services::session_revocation::SessionWatch;

/// The close code of a connection whose session ended.
pub(crate) const SESSION_ENDED: u16 = 4401;
/// The close reason sent with [`SESSION_ENDED`].
const SESSION_ENDED_REASON: &str = "session ended";
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);
const CLIENT_TIMEOUT: Duration = Duration::from_secs(10);
/// Longest single wait for a session's expiry; a later expiry is waited for in steps.
const MAX_EXPIRY_WAIT: Duration = Duration::from_secs(24 * 60 * 60);

/// What ends a connection besides the client: its session's revocation and expiry.
pub(crate) struct SessionBound {
    /// Resolves when the session is revoked; `None` when no registry is wired.
    pub(crate) watch: Option<SessionWatch>,
    pub(crate) expires_at: Option<DateTime<Utc>>,
}

/// Upgrade `request` to a GraphQL WebSocket connection running `schema` with `data`, ended by
/// `session` as well as by the client.
pub(crate) fn start<S>(
    schema: AppSchema,
    data: Data,
    session: SessionBound,
    request: &HttpRequest,
    payload: S,
) -> Result<HttpResponse, actix_web::Error>
where
    S: Stream<Item = Result<Bytes, PayloadError>> + 'static,
{
    let protocol = request
        .headers()
        .get("sec-websocket-protocol")
        .and_then(|value| value.to_str().ok())
        .and_then(|protocols| {
            protocols
                .split(',')
                .find_map(|p| WebSocketProtocols::from_str(p.trim()).ok())
        })
        .ok_or_else(|| actix_web::error::ErrorBadRequest("failed to parse graphql protocol"))?;
    let connection = Connection {
        schema,
        data: Some(data),
        protocol,
        session: Some(session),
        last_heartbeat: Instant::now(),
        messages: None,
        engine: None,
        continuation: Vec::new(),
        ended: false,
    };
    WsResponseBuilder::new(connection, request, payload)
        .protocols(&ALL_WEBSOCKET_PROTOCOLS)
        .start()
}

struct Connection {
    schema: AppSchema,
    /// The connection data, handed to the engine when it starts.
    data: Option<Data>,
    protocol: WebSocketProtocols,
    /// Taken when the connection starts.
    session: Option<SessionBound>,
    last_heartbeat: Instant,
    /// Client messages on their way to the engine.
    messages: Option<async_channel::Sender<Vec<u8>>>,
    /// The protocol engine, with every stream of the connection.
    engine: Option<SpawnHandle>,
    continuation: Vec<u8>,
    /// Set once the session ended: the connection sends nothing more and reads nothing more.
    ended: bool,
}

impl Connection {
    /// Drop the engine, send the session-ended close and stop.
    fn end_session(&mut self, ctx: &mut WebsocketContext<Self>) {
        if self.ended {
            return;
        }
        self.ended = true;
        if let Some(engine) = self.engine.take() {
            ctx.cancel_future(engine);
        }
        self.messages = None;
        ctx.close(Some(CloseReason {
            code: CloseCode::Other(SESSION_ENDED),
            description: Some(SESSION_ENDED_REASON.to_string()),
        }));
        ctx.stop();
    }

    /// End the session at `expires_at`, waiting at most [`MAX_EXPIRY_WAIT`] at a time.
    fn end_at(&self, expires_at: DateTime<Utc>, ctx: &mut WebsocketContext<Self>) {
        let left = (expires_at - Utc::now()).to_std().unwrap_or(Duration::ZERO);
        if left.is_zero() {
            ctx.run_later(Duration::ZERO, |act, ctx| act.end_session(ctx));
            return;
        }
        ctx.run_later(left.min(MAX_EXPIRY_WAIT), move |act, ctx| {
            act.end_at(expires_at, ctx)
        });
    }
}

impl Actor for Connection {
    type Context = WebsocketContext<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        ctx.run_interval(HEARTBEAT_INTERVAL, |act, ctx| {
            if Instant::now().duration_since(act.last_heartbeat) > CLIENT_TIMEOUT {
                ctx.stop();
                return;
            }
            ctx.ping(b"");
        });

        let (sender, receiver) = async_channel::unbounded();
        let engine = WebSocket::new(self.schema.clone(), receiver, self.protocol)
            .connection_data(self.data.take().unwrap_or_default())
            .into_actor(self)
            .map(|message, act, ctx| {
                if act.ended {
                    return;
                }
                match message {
                    WsMessage::Text(text) => ctx.text(text),
                    WsMessage::Close(code, reason) => ctx.close(Some(CloseReason {
                        code: code.into(),
                        description: Some(reason),
                    })),
                }
            })
            .finish();
        self.engine = Some(ctx.spawn(engine));
        self.messages = Some(sender);

        if let Some(SessionBound { watch, expires_at }) = self.session.take() {
            if let Some(watch) = watch {
                ctx.spawn(
                    watch
                        .into_actor(self)
                        .map(|(), act, ctx| act.end_session(ctx)),
                );
            }
            if let Some(expires_at) = expires_at {
                self.end_at(expires_at, ctx);
            }
        }
    }
}

impl StreamHandler<Result<Message, ProtocolError>> for Connection {
    fn handle(&mut self, message: Result<Message, ProtocolError>, ctx: &mut Self::Context) {
        let Ok(message) = message else {
            ctx.stop();
            return;
        };
        let text = match message {
            Message::Ping(bytes) => {
                self.last_heartbeat = Instant::now();
                ctx.pong(&bytes);
                None
            }
            Message::Pong(_) => {
                self.last_heartbeat = Instant::now();
                None
            }
            Message::Continuation(item) => match item {
                Item::FirstText(bytes) | Item::FirstBinary(bytes) => {
                    self.continuation = bytes.to_vec();
                    None
                }
                Item::Continue(bytes) => {
                    self.continuation.extend_from_slice(&bytes);
                    None
                }
                Item::Last(bytes) => {
                    self.continuation.extend_from_slice(&bytes);
                    Some(std::mem::take(&mut self.continuation))
                }
            },
            Message::Text(s) => Some(s.into_bytes().to_vec()),
            Message::Binary(bytes) => Some(bytes.to_vec()),
            Message::Close(_) => {
                ctx.stop();
                None
            }
            Message::Nop => None,
        };
        let (Some(text), Some(sender)) = (text, self.messages.clone()) else {
            return;
        };
        ctx.spawn(async move { sender.send(text).await }.into_actor(self).map(
            |sent, _act, ctx| {
                if sent.is_err() {
                    ctx.stop();
                }
            },
        ));
    }
}
