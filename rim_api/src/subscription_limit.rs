//! A cap on the subscriptions one WebSocket connection holds at once.
//!
//! The transport puts a [`SubscriptionSlots`] into the connection's data; the
//! [`SubscriptionLimit`] extension takes a slot for each subscription started on it and gives it
//! back when the subscription ends. A connection without slots has no cap.
//!
//! The extension also refuses, on any transport, a subscription operation that selects more
//! than one root field, so one slot holds one stream: aliases, repeated fields and fragment
//! expansions each count as a field.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_graphql::extensions::{
    Extension, ExtensionContext, ExtensionFactory, NextPrepareRequest, NextSubscribe,
};
use async_graphql::futures_util::stream::BoxStream;
use async_graphql::parser::types::OperationType;
use async_graphql::{ErrorExtensions, Pos, Request, Response, ServerResult};
use futures_util::StreamExt;

use crate::operation::root_fields;

/// The error code of a subscription refused for the connection's cap.
pub const TOO_MANY_SUBSCRIPTIONS: &str = "TOO_MANY_SUBSCRIPTIONS";

/// The message of a subscription operation refused for selecting more than one root field.
pub const ONE_ROOT_FIELD: &str = "a subscription selects exactly one root field";

/// The subscriptions one connection may hold and how many it holds.
pub struct SubscriptionSlots {
    limit: usize,
    active: AtomicUsize,
    on_refused: fn(),
}

impl SubscriptionSlots {
    /// Slots for `limit` subscriptions; `on_refused` runs for each subscription refused.
    pub fn new(limit: u32, on_refused: fn()) -> Arc<Self> {
        Arc::new(Self {
            limit: usize::try_from(limit).unwrap_or(usize::MAX),
            active: AtomicUsize::new(0),
            on_refused,
        })
    }

    /// Subscriptions the connection holds now.
    pub fn active(&self) -> usize {
        self.active.load(Ordering::Acquire)
    }

    /// A slot, or `None` when every slot is taken.
    fn acquire(self: &Arc<Self>) -> Option<SlotGuard> {
        self.active
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                (n < self.limit).then_some(n + 1)
            })
            .ok()
            .map(|_| SlotGuard(Arc::clone(self)))
    }
}

/// A taken slot, given back when dropped.
struct SlotGuard(Arc<SubscriptionSlots>);

impl Drop for SlotGuard {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::AcqRel);
    }
}

/// The extension enforcing [`SubscriptionSlots`] on the schema's subscriptions.
pub struct SubscriptionLimit;

impl ExtensionFactory for SubscriptionLimit {
    fn create(&self) -> Arc<dyn Extension> {
        Arc::new(SubscriptionLimitExtension)
    }
}

struct SubscriptionLimitExtension;

#[async_graphql::async_trait::async_trait]
impl Extension for SubscriptionLimitExtension {
    /// Refuses a request whose selected operation is a subscription with more than one root
    /// field, with one `VALIDATION` error, before validation and execution. A request that does
    /// not parse or selects no operation passes on to the refusal async-graphql gives it.
    async fn prepare_request(
        &self,
        ctx: &ExtensionContext<'_>,
        mut request: Request,
        next: NextPrepareRequest<'_>,
    ) -> ServerResult<Request> {
        let operation_name = request.operation_name.clone();
        if let Ok(document) = request.parsed_query() {
            let fields = root_fields(document, operation_name.as_deref());
            if fields.operation == Some(OperationType::Subscription) && fields.fields.len() > 1 {
                return Err(async_graphql::Error::new(ONE_ROOT_FIELD)
                    .extend_with(|_, e| e.set("code", "VALIDATION"))
                    .into_server_error(Pos::default()));
            }
        }
        next.run(ctx, request).await
    }

    fn subscribe<'s>(
        &self,
        ctx: &ExtensionContext<'_>,
        stream: BoxStream<'s, Response>,
        next: NextSubscribe<'_>,
    ) -> BoxStream<'s, Response> {
        let Some(slots) = ctx.data_opt::<Arc<SubscriptionSlots>>() else {
            return next.run(ctx, stream);
        };
        match slots.acquire() {
            Some(slot) => next
                .run(ctx, stream)
                .map(move |response| {
                    let _held = &slot;
                    response
                })
                .boxed(),
            None => {
                (slots.on_refused)();
                let limit = slots.limit;
                let error = async_graphql::Error::new(format!(
                    "this connection holds {limit} subscriptions, the most it may"
                ))
                .extend_with(|_, e| {
                    e.set("code", TOO_MANY_SUBSCRIPTIONS);
                    e.set("limit", limit as u64);
                });
                let response = Response::from_errors(vec![error.into_server_error(Pos::default())]);
                futures_util::stream::once(async move { response }).boxed()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use async_graphql::futures_util::stream;
    use async_graphql::{Data, EmptyMutation, Object, Schema, Subscription};

    use super::*;

    struct Query;

    #[Object]
    impl Query {
        async fn ok(&self) -> bool {
            true
        }
    }

    struct Sub;

    #[Subscription]
    impl Sub {
        async fn forever(&self) -> impl futures_util::Stream<Item = i32> {
            stream::pending()
        }

        async fn one(&self) -> impl futures_util::Stream<Item = i32> {
            stream::iter([1])
        }
    }

    fn schema() -> Schema<Query, EmptyMutation, Sub> {
        Schema::build(Query, EmptyMutation, Sub)
            .extension(SubscriptionLimit)
            .finish()
    }

    fn session(slots: &Arc<SubscriptionSlots>) -> Arc<Data> {
        let mut data = Data::default();
        data.insert(Arc::clone(slots));
        Arc::new(data)
    }

    #[tokio::test]
    async fn a_connection_holds_at_most_its_limit_and_an_ended_subscription_frees_its_slot() {
        let schema = schema();
        let slots = SubscriptionSlots::new(2, || {});
        let first =
            schema.execute_stream_with_session_data("subscription { forever }", session(&slots));
        let second =
            schema.execute_stream_with_session_data("subscription { forever }", session(&slots));
        assert_eq!(slots.active(), 2);

        let mut third =
            schema.execute_stream_with_session_data("subscription { forever }", session(&slots));
        let refused = third.next().await.expect("an answer");
        assert_eq!(
            refused.errors[0]
                .extensions
                .as_ref()
                .and_then(|e| e.get("code"))
                .cloned(),
            Some(async_graphql::Value::from(TOO_MANY_SUBSCRIPTIONS))
        );
        assert!(third.next().await.is_none());
        assert_eq!(slots.active(), 2);

        drop(first);
        assert_eq!(slots.active(), 1);
        let _fourth =
            schema.execute_stream_with_session_data("subscription { forever }", session(&slots));
        assert_eq!(slots.active(), 2);
        drop(second);
    }

    fn refused_for_root_fields(response: &Response) -> bool {
        response.errors.len() == 1
            && response.errors[0].message == ONE_ROOT_FIELD
            && response.errors[0]
                .extensions
                .as_ref()
                .and_then(|e| e.get("code"))
                .cloned()
                == Some(async_graphql::Value::from("VALIDATION"))
    }

    #[tokio::test]
    async fn a_subscription_with_more_than_one_root_field_is_refused_and_frees_its_slot() {
        let schema = schema();
        let slots = SubscriptionSlots::new(10, || {});
        for (query, operation) in [
            ("subscription { a: forever b: forever }", None),
            ("subscription { forever one }", None),
            (
                "subscription { forever ...F } fragment F on Sub { one }",
                None,
            ),
            ("subscription { ... on Sub { forever forever } }", None),
            ("subscription { forever __typename }", None),
            ("query Q { ok } subscription S { forever one }", Some("S")),
        ] {
            let mut request = Request::new(query);
            if let Some(operation) = operation {
                request = request.operation_name(operation);
            }
            let mut stream = schema.execute_stream_with_session_data(request, session(&slots));
            let answer = stream.next().await.expect("an answer");
            assert!(
                refused_for_root_fields(&answer),
                "{query}: {:?}",
                answer.errors
            );
            assert!(stream.next().await.is_none(), "{query}");
            drop(stream);
            assert_eq!(slots.active(), 0, "{query}");
        }
        let answer = schema
            .execute_stream("subscription { one one }")
            .next()
            .await
            .expect("an answer");
        assert!(refused_for_root_fields(&answer), "without slots too");
    }

    #[tokio::test]
    async fn one_root_field_runs_and_other_operations_are_untouched() {
        let schema = schema();
        let held = schema.execute_stream("subscription { forever }");
        let mut one = schema.execute_stream("subscription S { renamed: one }");
        let answer = one.next().await.expect("an answer");
        assert!(answer.errors.is_empty(), "{:?}", answer.errors);
        let query = schema.execute("{ a: ok b: ok }").await;
        assert!(query.errors.is_empty(), "{:?}", query.errors);
        drop(held);
    }

    #[tokio::test]
    async fn without_slots_there_is_no_cap() {
        let schema = schema();
        let held: Vec<_> = (0..3)
            .map(|_| schema.execute_stream("subscription { forever }"))
            .collect();
        let mut one = schema.execute_stream("subscription { one }");
        let answer = one.next().await.expect("an answer");
        assert!(answer.errors.is_empty(), "{:?}", answer.errors);
        drop(held);
    }
}
