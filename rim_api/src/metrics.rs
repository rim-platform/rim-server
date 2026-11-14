//! Bounded GraphQL request metrics and subscription lifecycle measurements.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use async_graphql::extensions::{
    Extension, ExtensionContext, ExtensionFactory, NextExecute, NextParseQuery, NextPrepareRequest,
    NextRequest, NextSubscribe,
};
use async_graphql::futures_util::stream::BoxStream;
use async_graphql::parser::types::{ExecutableDocument, OperationType};
use async_graphql::{Request, Response, ServerResult, Variables};
use futures_util::StreamExt;
use metrics::{
    Unit, counter, describe_counter, describe_gauge, describe_histogram, gauge, histogram,
};

const FAILED_OPERATIONS: &str = "rim_graphql_failed_operations_total";
const OPERATIONS: &str = "rim_graphql_operations_total";
const DURATION: &str = "rim_graphql_operation_duration_seconds";
const SUBSCRIPTIONS: &str = "rim_graphql_subscriptions";
const SUBSCRIPTION_DURATION: &str = "rim_graphql_subscription_duration_seconds";
const SUBSCRIPTION_RESPONSES: &str = "rim_graphql_subscription_responses_total";
const FIELDS: &[&str] = &[
    "sendMessage",
    "replyToMessage",
    "forwardMessage",
    "sendEncryptedMessage",
    "messages",
    "threadMessages",
    "messageChanges",
    "other",
    "multiple",
    "unknown",
];

pub(crate) fn describe() {
    describe_counter!(
        OPERATIONS,
        Unit::Count,
        "GraphQL requests by bounded operation type, root field and returned outcome."
    );
    describe_histogram!(
        DURATION,
        Unit::Seconds,
        "GraphQL request duration, excluding subscription lifetimes."
    );
    describe_gauge!(
        SUBSCRIPTIONS,
        Unit::Count,
        "Active GraphQL subscription streams."
    );
    describe_histogram!(
        SUBSCRIPTION_DURATION,
        Unit::Seconds,
        "GraphQL subscription stream lifetimes, including cancellation."
    );
    describe_counter!(
        SUBSCRIPTION_RESPONSES,
        Unit::Count,
        "GraphQL subscription stream responses by returned outcome."
    );
    describe_counter!(
        FAILED_OPERATIONS,
        Unit::Count,
        "Failed GraphQL operations by bounded failure kind; internal takes precedence over unknown, then client."
    );
    for kind in ["internal", "client", "unknown"] {
        counter!(FAILED_OPERATIONS, "kind" => kind).absolute(0);
    }
    gauge!(SUBSCRIPTIONS).increment(0.0);
    for outcome in ["success", "error"] {
        counter!(SUBSCRIPTION_RESPONSES, "outcome" => outcome).absolute(0);
        for operation_type in ["query", "mutation", "subscription", "unknown"] {
            for &field in FIELDS {
                counter!(OPERATIONS, "operation_type" => operation_type, "field" => field, "outcome" => outcome).absolute(0);
            }
        }
    }
}

pub(crate) struct OperationMetrics;

impl ExtensionFactory for OperationMetrics {
    fn create(&self) -> Arc<dyn Extension> {
        Arc::new(OperationMetricsExtension::default())
    }
}

#[derive(Clone, Copy)]
struct Labels {
    operation_type: &'static str,
    field: &'static str,
}

impl Default for Labels {
    fn default() -> Self {
        Self {
            operation_type: "unknown",
            field: "unknown",
        }
    }
}

#[derive(Default)]
struct OperationMetricsExtension {
    operation_name: Mutex<Option<String>>,
    labels: Mutex<Labels>,
    executed: AtomicBool,
}

fn outcome(response: &Response) -> &'static str {
    if response.errors.is_empty() {
        "success"
    } else {
        "error"
    }
}

fn record_failure(response: &Response, executed: bool) {
    let kind = response
        .errors
        .iter()
        .map(|error| {
            match error
                .extensions
                .as_ref()
                .and_then(|extensions| extensions.get("code"))
            {
                Some(async_graphql::Value::String(code)) => match code.as_str() {
                    "INTERNAL" | "UNAVAILABLE" | "TIMEOUT" | "UPSTREAM_UNAVAILABLE" => 2,
                    "NOT_FOUND"
                    | "VALIDATION"
                    | "CONFLICT"
                    | "FORBIDDEN"
                    | "UNAUTHENTICATED"
                    | "CONFIRMATION_STALE"
                    | "CONFIRMATION_FAILED"
                    | "RATE_LIMITED"
                    | "INVALID_TOKEN"
                    | "EXPIRED_TOKEN"
                    | "EMAIL_TAKEN"
                    | "CONVERSATION_ARCHIVED"
                    | "TIMED_OUT"
                    | "STALE_EPOCH"
                    | "MLS_COMMIT_REQUIRED"
                    | "FEATURE_DISABLED"
                    | "REGISTRATION_FULL"
                    | "TOO_MANY_SUBSCRIPTIONS" => 0,
                    _ => 1,
                },
                _ if !executed => 0,
                _ => 1,
            }
        })
        .max();
    if let Some(kind) = kind {
        let kind = match kind {
            2 => "internal",
            1 => "unknown",
            _ => "client",
        };
        counter!(FAILED_OPERATIONS, "kind" => kind).increment(1);
    }
}

fn classify(document: &ExecutableDocument, operation_name: Option<&str>) -> Labels {
    let roots = crate::operation::root_fields(document, operation_name);
    if !roots.is_complete() {
        return Labels::default();
    }
    let operation_type = match roots.operation {
        Some(OperationType::Query) => "query",
        Some(OperationType::Mutation) => "mutation",
        Some(OperationType::Subscription) => "subscription",
        None => "unknown",
    };
    let field = match roots.fields.as_slice() {
        [name] => FIELDS
            .iter()
            .copied()
            .find(|field| *field == name)
            .unwrap_or("other"),
        [] => "unknown",
        _ => "multiple",
    };
    Labels {
        operation_type,
        field,
    }
}

#[async_trait::async_trait]
impl Extension for OperationMetricsExtension {
    async fn prepare_request(
        &self,
        ctx: &ExtensionContext<'_>,
        request: Request,
        next: NextPrepareRequest<'_>,
    ) -> ServerResult<Request> {
        if let Ok(mut name) = self.operation_name.lock() {
            *name = request.operation_name.clone();
        }
        next.run(ctx, request).await
    }

    async fn parse_query(
        &self,
        ctx: &ExtensionContext<'_>,
        query: &str,
        variables: &Variables,
        next: NextParseQuery<'_>,
    ) -> ServerResult<ExecutableDocument> {
        let document = next.run(ctx, query, variables).await?;
        let name = self
            .operation_name
            .lock()
            .ok()
            .and_then(|name| name.clone());
        if let Ok(mut labels) = self.labels.lock() {
            *labels = classify(&document, name.as_deref());
        }
        Ok(document)
    }

    async fn execute(
        &self,
        ctx: &ExtensionContext<'_>,
        operation_name: Option<&str>,
        next: NextExecute<'_>,
    ) -> Response {
        self.executed.store(true, Ordering::Relaxed);
        next.run(ctx, operation_name).await
    }

    async fn request(&self, ctx: &ExtensionContext<'_>, next: NextRequest<'_>) -> Response {
        let started = Instant::now();
        let response = next.run(ctx).await;
        let labels = self.labels.lock().map(|labels| *labels).unwrap_or_default();
        record_failure(
            &response,
            self.executed.load(Ordering::Relaxed) && labels.operation_type != "subscription",
        );
        let outcome = outcome(&response);
        counter!(OPERATIONS, "operation_type" => labels.operation_type, "field" => labels.field, "outcome" => outcome).increment(1);
        histogram!(DURATION, "operation_type" => labels.operation_type, "field" => labels.field, "outcome" => outcome).record(started.elapsed().as_secs_f64());
        response
    }

    fn subscribe<'s>(
        &self,
        ctx: &ExtensionContext<'_>,
        stream: BoxStream<'s, Response>,
        next: NextSubscribe<'_>,
    ) -> BoxStream<'s, Response> {
        let stream = next.run(ctx, stream);
        async_stream::stream! {
            let _guard = SubscriptionGuard::new();
            futures_util::pin_mut!(stream);
            while let Some(response) = stream.next().await {
                counter!(SUBSCRIPTION_RESPONSES, "outcome" => outcome(&response)).increment(1);
                yield response;
            }
        }
        .boxed()
    }
}

struct SubscriptionGuard(Instant);

impl SubscriptionGuard {
    fn new() -> Self {
        gauge!(SUBSCRIPTIONS).increment(1.0);
        Self(Instant::now())
    }
}

impl Drop for SubscriptionGuard {
    fn drop(&mut self) {
        gauge!(SUBSCRIPTIONS).decrement(1.0);
        histogram!(SUBSCRIPTION_DURATION).record(self.0.elapsed().as_secs_f64());
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use async_graphql::{EmptyMutation, Object, Schema, Subscription};
    use metrics_util::debugging::{DebugValue, DebuggingRecorder};
    use metrics_util::{CompositeKey, MetricKind};

    use super::*;

    struct Query;

    #[Object]
    impl Query {
        async fn messages(&self) -> async_graphql::Result<bool> {
            Err("refused".into())
        }

        async fn ok(&self) -> bool {
            true
        }
    }

    struct Sub;

    #[Subscription]
    impl Sub {
        async fn events(&self) -> impl futures_util::Stream<Item = i32> {
            futures_util::stream::iter([1]).chain(futures_util::stream::pending())
        }
    }

    fn schema() -> Schema<Query, EmptyMutation, Sub> {
        describe();
        Schema::build(Query, EmptyMutation, Sub)
            .extension(OperationMetrics)
            .finish()
    }

    struct RecorderView {
        snapshotter: metrics_util::debugging::Snapshotter,
        values: RefCell<HashMap<CompositeKey, DebugValue>>,
    }

    fn with_recorder(f: impl FnOnce(&RecorderView)) {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let view = RecorderView {
            snapshotter,
            values: RefCell::new(HashMap::new()),
        };
        metrics::with_local_recorder(&recorder, || f(&view));
    }

    fn value(
        snapshotter: &RecorderView,
        kind: MetricKind,
        name: &str,
        labels: &[(&str, &str)],
    ) -> Option<DebugValue> {
        let key = CompositeKey::new(
            kind,
            metrics::Key::from_parts(
                name.to_string(),
                labels
                    .iter()
                    .map(|(k, v)| metrics::Label::new(k.to_string(), v.to_string()))
                    .collect::<Vec<_>>(),
            ),
        );
        let mut values = snapshotter.values.borrow_mut();
        for (key, _, _, delta) in snapshotter.snapshotter.snapshot().into_vec() {
            values
                .entry(key)
                .and_modify(|current| match (current, &delta) {
                    (DebugValue::Counter(current), DebugValue::Counter(delta)) => *current += delta,
                    (DebugValue::Gauge(current), DebugValue::Gauge(delta)) => {
                        *current = (current.into_inner() + delta.into_inner()).into()
                    }
                    (DebugValue::Histogram(current), DebugValue::Histogram(delta)) => {
                        current.extend_from_slice(delta)
                    }
                    _ => unreachable!("metric kind is stable"),
                })
                .or_insert(delta);
        }
        values.get(&key).map(|value| match value {
            DebugValue::Counter(value) => DebugValue::Counter(*value),
            DebugValue::Gauge(value) => DebugValue::Gauge(*value),
            DebugValue::Histogram(values) => DebugValue::Histogram(values.clone()),
        })
    }

    fn count(snapshotter: &RecorderView, name: &str, labels: &[(&str, &str)]) -> u64 {
        match value(snapshotter, MetricKind::Counter, name, labels) {
            Some(DebugValue::Counter(count)) => count,
            _ => 0,
        }
    }

    fn active(snapshotter: &RecorderView) -> f64 {
        match value(snapshotter, MetricKind::Gauge, SUBSCRIPTIONS, &[]) {
            Some(DebugValue::Gauge(value)) => value.into_inner(),
            _ => panic!("subscription gauge is registered"),
        }
    }

    fn run(f: impl Future<Output = ()>) {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime")
            .block_on(f);
    }

    #[test]
    fn failure_kinds_count_each_response_once_and_prioritize_internal_errors() {
        use async_graphql::ErrorExtensions;
        fn error(code: Option<&str>) -> async_graphql::ServerError {
            let mut error = async_graphql::Error::new("failure");
            if let Some(code) = code {
                error = error.extend_with(|_, extensions| extensions.set("code", code));
            }
            error.into_server_error(async_graphql::Pos::default())
        }
        with_recorder(|snapshotter| {
            describe();
            record_failure(&Response::new(true), true);
            record_failure(
                &Response::from_errors(vec![error(Some("UNAUTHENTICATED"))]),
                true,
            );
            record_failure(
                &Response::from_errors(vec![error(Some("VALIDATION")), error(None)]),
                true,
            );
            record_failure(
                &Response::from_errors(vec![
                    error(None),
                    error(Some("INTERNAL")),
                    error(Some("FORBIDDEN")),
                ]),
                true,
            );
            for kind in ["client", "unknown", "internal"] {
                assert_eq!(count(snapshotter, FAILED_OPERATIONS, &[("kind", kind)]), 1);
            }
        });
    }

    #[test]
    fn aliases_fragments_and_client_operation_names_keep_labels_bounded() {
        let document = async_graphql::parser::parse_query(
            "query ClientSupplied { ...F } fragment F on Query { privateAlias: messages }",
        )
        .expect("document");
        let labels = classify(&document, Some("ClientSupplied"));
        assert_eq!(labels.operation_type, "query");
        assert_eq!(labels.field, "messages");
        let document =
            async_graphql::parser::parse_query("{ a: messages b: messages }").expect("document");
        assert_eq!(classify(&document, None).field, "multiple");
        let document =
            async_graphql::parser::parse_query("query Selected { ok } query Other { messages }")
                .expect("document");
        assert_eq!(classify(&document, Some("Selected")).field, "other");
        assert_eq!(classify(&document, None).field, "unknown");
        for (query, kind, field) in [
            ("mutation { sendMessage }", "mutation", "sendMessage"),
            ("subscription { events }", "subscription", "other"),
        ] {
            let document = async_graphql::parser::parse_query(query).expect("document");
            let labels = classify(&document, None);
            assert_eq!(labels.operation_type, kind);
            assert_eq!(labels.field, field);
        }
        let document =
            async_graphql::parser::parse_query("{ arbitraryAttackerField }").expect("document");
        assert_eq!(classify(&document, None).field, "other");
    }

    #[test]
    fn returned_errors_and_invalid_documents_are_counted_and_timed() {
        with_recorder(|snapshotter| {
            run(async {
                let schema = schema();
                let failed = schema.execute("query ClientName { alias: messages }").await;
                assert!(!failed.errors.is_empty());
                assert!(schema.execute("{ ok }").await.errors.is_empty());
                assert!(!schema.execute("invalid document").await.errors.is_empty());
                assert!(!schema.execute("{ nonexistent }").await.errors.is_empty());
                assert_eq!(
                    count(snapshotter, FAILED_OPERATIONS, &[("kind", "client")]),
                    2
                );
                assert_eq!(
                    count(snapshotter, FAILED_OPERATIONS, &[("kind", "unknown")]),
                    1
                );
                for (operation_type, field, outcome) in [
                    ("query", "messages", "error"),
                    ("query", "other", "success"),
                    ("unknown", "unknown", "error"),
                ] {
                    let labels = [
                        ("operation_type", operation_type),
                        ("field", field),
                        ("outcome", outcome),
                    ];
                    assert_eq!(count(snapshotter, OPERATIONS, &labels), 1);
                    assert!(
                        matches!(value(snapshotter, MetricKind::Histogram, DURATION, &labels), Some(DebugValue::Histogram(values)) if values.len() == 1)
                    );
                }
            })
        });
    }

    #[test]
    fn subscriptions_hold_the_gauge_until_cancelled_without_recording_request_latency() {
        with_recorder(|snapshotter| {
            run(async {
                let schema = schema();
                let mut stream = schema.execute_stream("subscription { events }");
                assert!(stream.next().await.expect("first event").errors.is_empty());
                assert_eq!(active(snapshotter), 1.0);
                let _second_schema = self::schema();
                assert_eq!(active(snapshotter), 1.0);
                assert_eq!(
                    count(
                        snapshotter,
                        SUBSCRIPTION_RESPONSES,
                        &[("outcome", "success")]
                    ),
                    1
                );
                drop(stream);
                assert_eq!(active(snapshotter), 0.0);
                assert!(
                    matches!(value(snapshotter, MetricKind::Histogram, SUBSCRIPTION_DURATION, &[]), Some(DebugValue::Histogram(values)) if values.len() == 1)
                );
                assert_eq!(
                    count(
                        snapshotter,
                        OPERATIONS,
                        &[
                            ("operation_type", "subscription"),
                            ("field", "other"),
                            ("outcome", "success")
                        ]
                    ),
                    0
                );
            })
        });
    }

    #[test]
    fn failed_subscription_startup_records_an_error_and_releases_the_gauge() {
        with_recorder(|snapshotter| {
            run(async {
                let schema = schema();
                let mut stream = schema.execute_stream("subscription { nonexistent }");
                assert!(
                    !stream
                        .next()
                        .await
                        .expect("error response")
                        .errors
                        .is_empty()
                );
                assert!(stream.next().await.is_none());
                assert_eq!(active(snapshotter), 0.0);
                assert_eq!(
                    count(snapshotter, SUBSCRIPTION_RESPONSES, &[("outcome", "error")]),
                    1
                );
            })
        });
    }
}
