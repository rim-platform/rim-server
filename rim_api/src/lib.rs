use crate::mutation::Mutation;
use crate::query::Query;
use async_graphql::{EmptySubscription, Schema};

use std::sync::Arc;

use rim_services::auth::service::AuthService;
use rim_services::conversation::service::ConversationService;
use rim_services::message::service::MessageService;
use rim_services::message_thread::service::ThreadService;

pub type AppSchema = Schema<Query, Mutation, EmptySubscription>;

pub mod auth_intent;
pub mod graphql;
pub mod ids;
pub mod mutation;
pub mod pagination;
pub mod query;
pub mod types;

#[cfg(test)]
mod tests;

pub fn build_schema(
    auth_service: Arc<dyn AuthService>,
    conversation_service: Arc<dyn ConversationService>,
    message_service: Arc<dyn MessageService>,
    thread_service: Arc<dyn ThreadService>,
) -> AppSchema {
    Schema::build(Query, Mutation, EmptySubscription)
        // Default per-request auth intent container.
        // Actix inserts a fresh one per request; tests that execute schema directly may rely on this.
        .data(std::sync::Arc::new(
            crate::auth_intent::AuthIntentCtx::default(),
        ))
        // Default auth actor context (unauthenticated by default).
        .data(crate::query::AuthActorCtx::default())
        .data(auth_service)
        .data(conversation_service)
        .data(message_service)
        .data(thread_service)
        .finish()
}
