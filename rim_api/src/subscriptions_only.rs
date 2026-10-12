//! Subscriptions only over the WebSocket.
//!
//! The transport puts a [`WebSocketConnection`] into the connection's data; the
//! [`SubscriptionsOnly`] extension then answers a query or a mutation sent on it with one
//! `VALIDATION` error and runs nothing. Requests without the marker, over HTTP or forwarded from
//! another server, are untouched.

use std::sync::Arc;

use async_graphql::extensions::{
    Extension, ExtensionContext, ExtensionFactory, NextPrepareRequest,
};
use async_graphql::parser::types::OperationType;
use async_graphql::{ErrorExtensions, Pos, Request, ServerResult};

use crate::operation::selected_operation_type;

/// The message of a query or mutation refused on a WebSocket connection.
pub const SUBSCRIPTIONS_ONLY: &str = "only subscriptions over the WebSocket";

/// Marks the data of a WebSocket connection.
#[derive(Debug, Clone, Copy, Default)]
pub struct WebSocketConnection;

/// The extension refusing queries and mutations on a [`WebSocketConnection`].
pub struct SubscriptionsOnly;

impl ExtensionFactory for SubscriptionsOnly {
    fn create(&self) -> Arc<dyn Extension> {
        Arc::new(SubscriptionsOnlyExtension)
    }
}

struct SubscriptionsOnlyExtension;

#[async_graphql::async_trait::async_trait]
impl Extension for SubscriptionsOnlyExtension {
    /// Refuses a request on a socket whose selected operation is a query or a mutation, before
    /// validation and execution. The parsed document stays in the request, so async-graphql
    /// does not parse it again; a request that does not parse or selects no operation passes on
    /// to the refusal async-graphql gives it.
    async fn prepare_request(
        &self,
        ctx: &ExtensionContext<'_>,
        mut request: Request,
        next: NextPrepareRequest<'_>,
    ) -> ServerResult<Request> {
        if ctx.data_opt::<WebSocketConnection>().is_some() {
            let operation_name = request.operation_name.clone();
            if let Ok(document) = request.parsed_query()
                && let Some(OperationType::Query | OperationType::Mutation) =
                    selected_operation_type(document, operation_name.as_deref())
            {
                return Err(async_graphql::Error::new(SUBSCRIPTIONS_ONLY)
                    .extend_with(|_, e| e.set("code", "VALIDATION"))
                    .into_server_error(Pos::default()));
            }
        }
        next.run(ctx, request).await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_graphql::futures_util::stream;
    use async_graphql::{Data, Object, Response, Schema, Subscription};
    use futures_util::StreamExt;

    use super::*;

    /// Resolver runs, counted to show what was executed.
    #[derive(Default)]
    struct Runs(AtomicUsize);

    struct Query;

    #[Object]
    impl Query {
        async fn ok(&self, ctx: &async_graphql::Context<'_>) -> bool {
            ctx.data_unchecked::<Arc<Runs>>()
                .0
                .fetch_add(1, Ordering::SeqCst);
            true
        }
    }

    struct Mutation;

    #[Object]
    impl Mutation {
        async fn touch(&self, ctx: &async_graphql::Context<'_>) -> bool {
            ctx.data_unchecked::<Arc<Runs>>()
                .0
                .fetch_add(1, Ordering::SeqCst);
            true
        }
    }

    struct Sub;

    #[Subscription]
    impl Sub {
        async fn one(&self) -> impl futures_util::Stream<Item = i32> {
            stream::iter([1])
        }
    }

    fn schema(runs: &Arc<Runs>) -> Schema<Query, Mutation, Sub> {
        Schema::build(Query, Mutation, Sub)
            .data(Arc::clone(runs))
            .extension(SubscriptionsOnly)
            .finish()
    }

    fn socket() -> Arc<Data> {
        let mut data = Data::default();
        data.insert(WebSocketConnection);
        Arc::new(data)
    }

    fn code(response: &Response) -> Option<async_graphql::Value> {
        response.errors[0]
            .extensions
            .as_ref()
            .and_then(|e| e.get("code"))
            .cloned()
    }

    #[tokio::test]
    async fn queries_and_mutations_on_a_socket_are_refused_unrun() {
        let runs = Arc::new(Runs::default());
        let schema = schema(&runs);
        for (query, operation_name) in [
            ("{ ok }", None),
            ("mutation { touch }", None),
            ("query Q { ok } mutation M { touch }", Some("M")),
        ] {
            let mut request = async_graphql::Request::new(query);
            if let Some(name) = operation_name {
                request = request.operation_name(name);
            }
            let mut answers = schema.execute_stream_with_session_data(request, socket());
            let answer = answers.next().await.expect("an answer");
            assert_eq!(answer.errors.len(), 1, "{query}");
            assert_eq!(answer.errors[0].message, SUBSCRIPTIONS_ONLY);
            assert_eq!(
                code(&answer),
                Some(async_graphql::Value::from("VALIDATION"))
            );
            assert_eq!(answer.data, async_graphql::Value::Null);
            assert!(answers.next().await.is_none());
        }
        assert_eq!(runs.0.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn a_subscription_on_a_socket_runs() {
        let schema = schema(&Arc::new(Runs::default()));
        for request in [
            async_graphql::Request::new("subscription { one }"),
            async_graphql::Request::new("mutation M { touch } subscription S { one }")
                .operation_name("S"),
        ] {
            let mut answers = schema.execute_stream_with_session_data(request, socket());
            let answer = answers.next().await.expect("an answer");
            assert!(answer.errors.is_empty(), "{:?}", answer.errors);
            assert_eq!(answer.data.into_json().expect("json")["one"], 1);
        }
    }

    #[tokio::test]
    async fn without_the_marker_queries_and_mutations_run() {
        let runs = Arc::new(Runs::default());
        let schema = schema(&runs);
        for query in ["{ ok }", "mutation { touch }"] {
            let answer = schema.execute(query).await;
            assert!(answer.errors.is_empty(), "{:?}", answer.errors);
            let streamed = schema
                .execute_stream(query)
                .next()
                .await
                .expect("an answer");
            assert!(streamed.errors.is_empty(), "{:?}", streamed.errors);
        }
        assert_eq!(runs.0.load(Ordering::SeqCst), 4);
    }
}
