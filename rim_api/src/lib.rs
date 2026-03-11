use crate::mutation::Mutation;
use crate::query::Query;
use async_graphql::{EmptySubscription, Schema};

use std::sync::Arc;

use rim_services::provider::AppServiceProvider;

pub type AppSchema = Schema<Query, Mutation, EmptySubscription>;

pub mod auth_context;
pub mod auth_intent;
pub mod errors;
pub mod graphql;
pub mod ids;
pub mod mutation;
pub mod pagination;
pub mod query;
pub mod types;

#[cfg(test)]
mod tests;

pub fn build_schema(service_provider: Arc<dyn AppServiceProvider>) -> AppSchema {
    Schema::build(Query, Mutation, EmptySubscription)
        // Default per-request auth intent container.
        // Actix inserts a fresh one per request; tests that execute schema directly may rely on this.
        .data(std::sync::Arc::new(
            crate::auth_intent::AuthIntentCtx::default(),
        ))
        // Default auth actor context (unauthenticated by default).
        .data(crate::auth_context::AuthActorCtx::default())
        .data(Arc::clone(&service_provider))
        .data(service_provider.action_service())
        .data(service_provider.auth_service())
        .data(service_provider.capability_service())
        .data(service_provider.command_service())
        .data(service_provider.conversation_service())
        .data(service_provider.event_service())
        .data(service_provider.integration_service())
        .data(service_provider.keybind_service())
        .data(service_provider.message_service())
        .data(service_provider.message_attachment_service())
        .data(service_provider.setting_service())
        .data(service_provider.thread_service())
        .finish()
}
