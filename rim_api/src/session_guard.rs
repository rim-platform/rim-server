//! A WebSocket connection's session, revalidated before each operation on the connection.
//!
//! The transport puts a [`ConnectionSession`] into the connection's data; the [`SessionGuard`]
//! extension checks that session against the repository before any operation on the connection
//! runs. When the session is no longer live it is reported to the revocation registry, which
//! closes the connection, and the operation gets no answer. A connection without a
//! [`ConnectionSession`], and every HTTP request, is not checked here.

use std::sync::Arc;

use async_graphql::extensions::{Extension, ExtensionContext, ExtensionFactory, NextSubscribe};
use async_graphql::futures_util::stream::BoxStream;
use async_graphql::{ErrorExtensions, Pos, Response};
use futures_util::StreamExt;

use rim_core::auth::model::SessionId;
use rim_services::auth::service::RequestAuthenticator;
use rim_services::session_revocation::SessionRevocations;

/// The session a connection runs under, what checks it, and where an ended one is reported.
pub struct ConnectionSession {
    session_id: SessionId,
    authenticator: Arc<dyn RequestAuthenticator>,
    revocations: Arc<dyn SessionRevocations>,
}

impl ConnectionSession {
    pub fn new(
        session_id: SessionId,
        authenticator: Arc<dyn RequestAuthenticator>,
        revocations: Arc<dyn SessionRevocations>,
    ) -> Arc<Self> {
        Arc::new(Self {
            session_id,
            authenticator,
            revocations,
        })
    }
}

/// The extension revalidating a [`ConnectionSession`] before each operation.
pub struct SessionGuard;

impl ExtensionFactory for SessionGuard {
    fn create(&self) -> Arc<dyn Extension> {
        Arc::new(SessionGuardExtension)
    }
}

struct SessionGuardExtension;

impl Extension for SessionGuardExtension {
    fn subscribe<'s>(
        &self,
        ctx: &ExtensionContext<'_>,
        stream: BoxStream<'s, Response>,
        next: NextSubscribe<'_>,
    ) -> BoxStream<'s, Response> {
        let Some(session) = ctx.data_opt::<Arc<ConnectionSession>>() else {
            return next.run(ctx, stream);
        };
        let session = Arc::clone(session);
        // Lazy: nothing of the operation runs before the stream is polled past the check.
        let operation = next.run(ctx, stream);
        async_stream::stream! {
            let id = session.session_id;
            match session.authenticator.live_sessions(&[id]).await {
                Ok(live) if live.contains(&id) => {
                    let mut operation = operation;
                    while let Some(response) = operation.next().await {
                        yield response;
                    }
                }
                Ok(_) => {
                    session.revocations.revoke(&[id]);
                    // The connection closes; the operation is never answered.
                    futures_util::future::pending::<()>().await;
                }
                Err(e) => {
                    tracing::error!(error = %e, session_id = %id, "could not revalidate a connection's session");
                    let error = async_graphql::Error::new("failed to validate session")
                        .extend_with(|_, e| e.set("code", "INTERNAL"));
                    yield Response::from_errors(vec![error.into_server_error(Pos::default())]);
                }
            }
        }
        .boxed()
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use async_graphql::{Data, EmptyMutation, Object, Schema, Subscription};
    use rim_core::user::model::UserId;
    use rim_infra_services::session_revocation::InProcessSessionRevocations;
    use rim_services::ServiceError;
    use rim_services::auth::service::MockRequestAuthenticator;
    use rim_services::session_revocation::{SessionWatch, WatchedSession};

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
        async fn one(&self) -> impl futures_util::Stream<Item = i32> {
            futures_util::stream::iter([1])
        }
    }

    fn schema() -> Schema<Query, EmptyMutation, Sub> {
        Schema::build(Query, EmptyMutation, Sub)
            .extension(SessionGuard)
            .finish()
    }

    /// A connection of a session that `authenticator` checks, watched in a registry of its own.
    struct Connection {
        data: Arc<Data>,
        watch: SessionWatch,
    }

    fn connection(authenticator: MockRequestAuthenticator) -> Connection {
        let session_id = SessionId::new();
        let registry = Arc::new(InProcessSessionRevocations::default());
        let watch = registry.watch(WatchedSession {
            session_id,
            user_id: UserId::new(),
            device_key_id: None,
        });
        let mut data = Data::default();
        data.insert(ConnectionSession::new(
            session_id,
            Arc::new(authenticator),
            registry,
        ));
        Connection {
            data: Arc::new(data),
            watch,
        }
    }

    /// An authenticator answering `live` for whether the session is live.
    fn checking(live: bool) -> MockRequestAuthenticator {
        let mut authenticator = MockRequestAuthenticator::new();
        authenticator
            .expect_live_sessions()
            .times(1)
            .returning(move |ids| {
                let answer = if live { ids.to_vec() } else { Vec::new() };
                Box::pin(async move { Ok(answer) })
            });
        authenticator
    }

    #[tokio::test]
    async fn a_live_session_runs_each_operation_after_checking_it() {
        for operation in ["{ ok }", "subscription { one }"] {
            let mut c = connection(checking(true));
            let mut answers = schema().execute_stream_with_session_data(operation, c.data);
            let answer = answers.next().await.expect("an answer");
            assert!(answer.errors.is_empty(), "{:?}", answer.errors);
            assert!(!c.watch.has_ended());
        }
    }

    #[tokio::test]
    async fn an_ended_session_closes_the_connection_and_leaves_the_operation_unanswered() {
        for operation in ["{ ok }", "subscription { one }"] {
            let mut c = connection(checking(false));
            let mut answers = schema().execute_stream_with_session_data(operation, c.data);
            let answered = tokio::time::timeout(Duration::from_millis(50), answers.next()).await;
            assert!(answered.is_err(), "the operation got an answer");
            assert!(c.watch.has_ended(), "the connection is told to close");
        }
    }

    #[tokio::test]
    async fn a_failed_check_answers_internal_and_keeps_the_connection() {
        let mut authenticator = MockRequestAuthenticator::new();
        authenticator
            .expect_live_sessions()
            .returning(|_| Box::pin(async { Err(ServiceError::Unavailable) }));
        let mut c = connection(authenticator);
        let mut answers = schema().execute_stream_with_session_data("{ ok }", c.data);
        let answer = answers.next().await.expect("an answer");
        assert_eq!(
            answer.errors[0]
                .extensions
                .as_ref()
                .and_then(|e| e.get("code"))
                .cloned(),
            Some(async_graphql::Value::from("INTERNAL"))
        );
        assert!(answers.next().await.is_none());
        assert!(!c.watch.has_ended());
    }

    #[tokio::test]
    async fn without_a_connection_session_nothing_is_checked() {
        let mut answers = schema().execute_stream("{ ok }");
        let answer = answers.next().await.expect("an answer");
        assert!(answer.errors.is_empty(), "{:?}", answer.errors);
    }
}
