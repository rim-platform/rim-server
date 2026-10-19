//! The GraphQL WebSocket transport: async-graphql's protocol engine on an actix actor that also
//! ends the connection when its session ends.
//!
//! A connection closes with code [`SESSION_ENDED`] when its session is revoked, as its
//! [`SessionWatch`] resolves, or when the session reaches its expiry. Closing cancels the
//! protocol engine first, which ends every active stream and drops the connection's data, and
//! with it the connection slot; nothing more is sent after the close frame.
//!
//! A client message longer than [`MAX_MESSAGE_BYTES`], whole or reassembled from fragments,
//! closes the connection the same way with code `1009`. A frame announcing a longer payload is
//! refused at its header, before its payload is read.

use std::cell::Cell;
use std::pin::Pin;
use std::rc::Rc;
use std::str::FromStr;
use std::task::{Context, Poll};
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
use metrics::{Unit, describe_gauge, gauge};

use rim_api::AppSchema;
use rim_services::session_revocation::SessionWatch;

const CONNECTIONS: &str = "rim_websocket_connections";

/// Register and initialize the authenticated WebSocket connection gauge.
pub(crate) fn describe_metrics() {
    describe_gauge!(
        CONNECTIONS,
        Unit::Count,
        "Open authenticated GraphQL WebSocket connections."
    );
    gauge!(CONNECTIONS).increment(0.0);
}

struct ConnectionGuard;

impl ConnectionGuard {
    fn new() -> Self {
        gauge!(CONNECTIONS).increment(1.0);
        Self
    }
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        gauge!(CONNECTIONS).decrement(1.0);
    }
}

/// The close code of a connection whose session ended.
pub(crate) const SESSION_ENDED: u16 = 4401;
/// The close reason sent with [`SESSION_ENDED`].
const SESSION_ENDED_REASON: &str = "session ended";
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);
const CLIENT_TIMEOUT: Duration = Duration::from_secs(10);
/// Longest single wait for a session's expiry; a later expiry is waited for in steps.
const MAX_EXPIRY_WAIT: Duration = Duration::from_secs(24 * 60 * 60);
/// Largest client message, whole or reassembled; the socket carries GraphQL operations only.
const MAX_MESSAGE_BYTES: usize = 256 * 1024;
/// The close reason of a connection whose client sent a message over [`MAX_MESSAGE_BYTES`].
const TOO_LARGE_REASON: &str = "message too large";
/// Client messages waiting for the engine; a client that fills the queue is disconnected.
const MESSAGE_QUEUE: usize = 32;
/// The close reason of a connection whose client filled [`MESSAGE_QUEUE`].
const QUEUE_FULL_REASON: &str = "too many messages";

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
    let payload = FrameLimit::new(payload);
    let connection = Connection {
        schema,
        data: Some(data),
        protocol,
        session: Some(session),
        last_heartbeat: Instant::now(),
        messages: None,
        engine: None,
        continuation: Vec::new(),
        frame_too_large: Rc::clone(&payload.too_large),
        ended: false,
        metric_guard: None,
    };
    WsResponseBuilder::new(connection, request, payload)
        .protocols(&ALL_WEBSOCKET_PROTOCOLS)
        .frame_size(MAX_MESSAGE_BYTES)
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
    /// The fragmented message being reassembled, at most [`MAX_MESSAGE_BYTES`].
    continuation: Vec<u8>,
    /// Set when the client's bytes were cut off at a frame over [`MAX_MESSAGE_BYTES`].
    frame_too_large: Rc<Cell<bool>>,
    /// Set once the server ended the connection: it sends nothing more and reads nothing more.
    ended: bool,
    metric_guard: Option<ConnectionGuard>,
}

impl Connection {
    /// Drop the engine, send the session-ended close and stop.
    fn end_session(&mut self, ctx: &mut WebsocketContext<Self>) {
        self.end(CloseCode::Other(SESSION_ENDED), SESSION_ENDED_REASON, ctx);
    }

    /// Drop the engine and the message being reassembled, send a close with `code` and
    /// `reason`, and stop.
    fn end(&mut self, code: CloseCode, reason: &str, ctx: &mut WebsocketContext<Self>) {
        if self.ended {
            return;
        }
        self.ended = true;
        if let Some(engine) = self.engine.take() {
            ctx.cancel_future(engine);
        }
        self.messages = None;
        self.continuation = Vec::new();
        ctx.close(Some(CloseReason {
            code,
            description: Some(reason.to_string()),
        }));
        ctx.stop();
    }

    fn refuse_too_large(&mut self, ctx: &mut WebsocketContext<Self>) {
        self.end(CloseCode::Size, TOO_LARGE_REASON, ctx);
    }

    /// Add `fragment` to the message being reassembled; `false`, adding nothing, when the
    /// message would pass [`MAX_MESSAGE_BYTES`].
    fn append(&mut self, fragment: &[u8]) -> bool {
        if self.continuation.len() + fragment.len() > MAX_MESSAGE_BYTES {
            return false;
        }
        self.continuation.extend_from_slice(fragment);
        true
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
        self.metric_guard = Some(ConnectionGuard::new());
        ctx.run_interval(HEARTBEAT_INTERVAL, |act, ctx| {
            if Instant::now().duration_since(act.last_heartbeat) > CLIENT_TIMEOUT {
                ctx.stop();
                return;
            }
            ctx.ping(b"");
        });

        let (sender, receiver) = async_channel::bounded(MESSAGE_QUEUE);
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
        if self.ended {
            return;
        }
        let message = match message {
            Ok(message) => message,
            Err(ProtocolError::Overflow) => return self.refuse_too_large(ctx),
            Err(_) if self.frame_too_large.get() => return self.refuse_too_large(ctx),
            Err(_) => {
                ctx.stop();
                return;
            }
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
            Message::Continuation(item) => {
                let (fragment, last) = match item {
                    Item::FirstText(bytes) | Item::FirstBinary(bytes) => {
                        self.continuation.clear();
                        (bytes, false)
                    }
                    Item::Continue(bytes) => (bytes, false),
                    Item::Last(bytes) => (bytes, true),
                };
                if !self.append(&fragment) {
                    return self.refuse_too_large(ctx);
                }
                last.then(|| std::mem::take(&mut self.continuation))
            }
            Message::Text(s) => Some(s.into_bytes().to_vec()),
            Message::Binary(bytes) => Some(bytes.to_vec()),
            Message::Close(_) => {
                ctx.stop();
                None
            }
            Message::Nop => None,
        };
        let (Some(text), Some(sender)) = (text, self.messages.as_ref()) else {
            return;
        };
        if text.len() > MAX_MESSAGE_BYTES {
            return self.refuse_too_large(ctx);
        }
        match sender.try_send(text) {
            Ok(()) => {}
            Err(async_channel::TrySendError::Full(_)) => {
                self.end(CloseCode::Policy, QUEUE_FULL_REASON, ctx)
            }
            Err(async_channel::TrySendError::Closed(_)) => ctx.stop(),
        }
    }
}

/// The client's bytes, cut off with an error at the header of the first frame whose payload
/// is longer than [`MAX_MESSAGE_BYTES`], so that payload is never buffered.
struct FrameLimit<S> {
    bytes: Pin<Box<S>>,
    /// The header read so far of the next frame.
    header: Vec<u8>,
    /// Payload bytes of the current frame not yet passed.
    payload_left: u64,
    /// Set once the bytes were cut off.
    too_large: Rc<Cell<bool>>,
}

impl<S> FrameLimit<S> {
    fn new(bytes: S) -> Self {
        Self {
            bytes: Box::pin(bytes),
            header: Vec::new(),
            payload_left: 0,
            too_large: Rc::new(Cell::new(false)),
        }
    }

    /// Follow the frames through `chunk`; `false` once a frame's header announces a payload
    /// over [`MAX_MESSAGE_BYTES`].
    fn admit(&mut self, mut chunk: &[u8]) -> bool {
        while let Some((&byte, rest)) = chunk.split_first() {
            if self.payload_left > 0 {
                let skip = usize::try_from(self.payload_left)
                    .unwrap_or(usize::MAX)
                    .min(chunk.len());
                self.payload_left -= skip as u64;
                chunk = &chunk[skip..];
                continue;
            }
            self.header.push(byte);
            chunk = rest;
            if let Some(length) = payload_length(&self.header) {
                if length > MAX_MESSAGE_BYTES as u64 {
                    return false;
                }
                self.header.clear();
                self.payload_left = length;
            }
        }
        true
    }
}

/// The payload length a complete frame header announces; `None` while `header` is shorter
/// than the header it starts.
fn payload_length(header: &[u8]) -> Option<u64> {
    let second = *header.get(1)?;
    let mask_len = if second & 0x80 != 0 { 4 } else { 0 };
    let (length_len, short) = match second & 0x7f {
        126 => (2, None),
        127 => (8, None),
        short => (0, Some(u64::from(short))),
    };
    if header.len() < 2 + length_len + mask_len {
        return None;
    }
    Some(short.unwrap_or_else(|| {
        header[2..2 + length_len]
            .iter()
            .fold(0, |length, &byte| (length << 8) | u64::from(byte))
    }))
}

impl<S> Stream for FrameLimit<S>
where
    S: Stream<Item = Result<Bytes, PayloadError>>,
{
    type Item = Result<Bytes, PayloadError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if self.too_large.get() {
            return Poll::Ready(None);
        }
        let next = self.bytes.as_mut().poll_next(cx);
        if let Poll::Ready(Some(Ok(chunk))) = &next
            && !self.admit(chunk)
        {
            self.too_large.set(true);
            return Poll::Ready(Some(Err(PayloadError::Overflow)));
        }
        next
    }
}

#[cfg(test)]
mod tests {
    use actix_http::ws::{OpCode, Parser};
    use actix_web::web::BytesMut;
    use futures_util::{FutureExt, StreamExt};

    use super::*;

    #[test]
    fn connection_guards_track_concurrent_connections_and_release_on_drop() {
        let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();
        metrics::with_local_recorder(&recorder, || {
            describe_metrics();
            assert!(handle.render().contains("rim_websocket_connections 0"));
            let first = ConnectionGuard::new();
            let second = ConnectionGuard::new();
            describe_metrics();
            assert!(handle.render().contains("rim_websocket_connections 2"));
            drop(first);
            assert!(handle.render().contains("rim_websocket_connections 1"));
            drop(second);
            assert!(handle.render().contains("rim_websocket_connections 0"));
        });
    }

    #[test]
    fn an_unstarted_connection_has_no_metric_guard() {
        let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();
        metrics::with_local_recorder(&recorder, || {
            describe_metrics();
            let schema = async_graphql::Schema::new(
                rim_api::query::Query::default(),
                rim_api::mutation::Mutation::default(),
                rim_api::subscription::Subscription,
            );
            let request = actix_web::test::TestRequest::default().to_http_request();
            let result = start(
                schema,
                Data::default(),
                SessionBound {
                    watch: None,
                    expires_at: None,
                },
                &request,
                futures_util::stream::pending(),
            );
            assert!(result.is_err());
            assert!(handle.render().contains("rim_websocket_connections 0"));
        });
    }

    fn connection_for_test(session: SessionBound) -> Connection {
        Connection {
            schema: async_graphql::Schema::new(
                rim_api::query::Query::default(),
                rim_api::mutation::Mutation::default(),
                rim_api::subscription::Subscription,
            ),
            data: Some(Data::default()),
            protocol: WebSocketProtocols::GraphQLWS,
            session: Some(session),
            last_heartbeat: Instant::now(),
            messages: None,
            engine: None,
            continuation: Vec::new(),
            frame_too_large: Rc::new(Cell::new(false)),
            ended: false,
            metric_guard: None,
        }
    }

    #[actix_web::test]
    async fn started_connections_release_the_gauge_on_transport_drop_and_session_revocation() {
        let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();
        let _recorder_guard = metrics::set_default_local_recorder(&recorder);
        describe_metrics();
        for revoked in [false, true] {
            let (sender, receiver) = tokio::sync::oneshot::channel();
            let session = SessionBound {
                watch: Some(SessionWatch::new(receiver, ())),
                expires_at: None,
            };
            let mut stream = Box::pin(WebsocketContext::create(
                connection_for_test(session),
                futures_util::stream::pending::<Result<Bytes, PayloadError>>(),
            ));
            assert!(stream.next().now_or_never().is_none());
            assert!(handle.render().contains("rim_websocket_connections 1"));
            if revoked {
                sender.send(()).expect("watch is live");
                let close = tokio::time::timeout(Duration::from_secs(1), stream.next())
                    .await
                    .expect("revocation closes connection");
                assert!(close.is_some());
            }
            drop(stream);
            assert!(handle.render().contains("rim_websocket_connections 0"));
        }
    }

    /// A client frame of `len` payload bytes, masked as a client sends it.
    fn frame(len: usize) -> BytesMut {
        let mut bytes = BytesMut::new();
        Parser::write_message(&mut bytes, vec![b'x'; len], OpCode::Text, true, true);
        bytes
    }

    /// A limit over `chunks`.
    fn limit(
        chunks: Vec<Bytes>,
    ) -> FrameLimit<impl Stream<Item = Result<Bytes, PayloadError>> + use<>> {
        FrameLimit::new(futures_util::stream::iter(chunks.into_iter().map(Ok)))
    }

    #[test]
    fn a_frame_at_the_cap_is_admitted_and_the_next_frame_is_followed() {
        let mut limit = limit(Vec::new());
        assert!(limit.admit(&frame(MAX_MESSAGE_BYTES)));
        assert!(limit.admit(&frame(3)));
        assert!(!limit.admit(&frame(MAX_MESSAGE_BYTES + 1)));
    }

    #[test]
    fn a_frame_over_the_cap_is_refused_at_the_last_byte_of_its_header() {
        let mut bytes = BytesMut::new();
        for len in [0, 125, 126, 65_535, 65_536] {
            bytes.extend_from_slice(&frame(len));
        }
        let header_end = bytes.len() + 2 + 8 + 4;
        bytes.extend_from_slice(&frame(MAX_MESSAGE_BYTES + 1));

        let mut limit = limit(Vec::new());
        let refused_at = bytes.iter().position(|byte| !limit.admit(&[*byte]));
        assert_eq!(refused_at, Some(header_end - 1));
    }

    #[tokio::test]
    async fn the_bytes_end_with_an_overflow_after_a_frame_over_the_cap() {
        let small = frame(10).freeze();
        let large = frame(MAX_MESSAGE_BYTES + 1).freeze();
        let mut limit = limit(vec![small.clone(), large.slice(..20), large.slice(20..)]);
        let too_large = Rc::clone(&limit.too_large);

        assert_eq!(limit.next().await.and_then(Result::ok), Some(small));
        assert!(matches!(
            limit.next().await,
            Some(Err(PayloadError::Overflow))
        ));
        assert!(too_large.get());
        assert!(limit.next().await.is_none());
    }
}
