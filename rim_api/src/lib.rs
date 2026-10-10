use std::sync::Arc;

use async_graphql::Schema;
use rim_services::provider::AppServiceProvider;

use crate::broadcast::InMemoryBroadcaster;
use crate::mutation::Mutation;
use crate::query::Query;
use crate::subscription::Subscription;

pub type AppSchema = Schema<Query, Mutation, Subscription>;

pub mod auth_context;
pub mod auth_intent;
pub mod broadcast;
pub mod errors;
pub(crate) mod features;
// pub: rim_server injects the FederationContext and runs forwarded GraphQL with its checks.
pub mod federation;
pub mod graphql;
pub mod ids;
pub mod loaders;
pub mod mutation;
pub mod pagination;
pub mod query;
pub mod resolve;
pub mod subscription;
pub mod types;

#[cfg(test)]
mod tests;

/// Extra per-deployment context injected into the GraphQL schema at boot.
pub struct RegistrationContext {
    /// The registration mode (invite-only, open or proof-of-work).
    pub registration_mode: Arc<rim_services::auth::dto::RegistrationMode>,
}

/// The GraphQL schema with its loaders, services and per-deployment context wired in, limited
/// to depth 10 and complexity 200.
pub fn build_schema(
    service_provider: Arc<dyn AppServiceProvider>,
    broadcaster: InMemoryBroadcaster,
    presence_manager: Arc<dyn rim_services::presence::PresenceService>,
    typing_tracker: Arc<dyn rim_services::typing::TypingService>,
    observers: Arc<rim_services::events::Observers>,
    registration: RegistrationContext,
    federation: Option<federation::FederationContext>,
) -> AppSchema {
    use crate::loaders::{LinkPreviewLoader, MentionLoader, PinLoader, ReactionLoader, UserLoader};
    use async_graphql::dataloader::DataLoader;

    let user_loader = DataLoader::new(
        UserLoader::new(service_provider.user_service()),
        tokio::spawn,
    );
    let reaction_loader = DataLoader::new(
        ReactionLoader::new(service_provider.message_reaction_service()),
        tokio::spawn,
    );
    let mention_loader = DataLoader::new(
        MentionLoader::new(service_provider.mention_service()),
        tokio::spawn,
    );
    let pin_loader = DataLoader::new(PinLoader::new(service_provider.pin_service()), tokio::spawn);
    let link_preview_loader = DataLoader::new(
        LinkPreviewLoader::new(service_provider.link_preview_service()),
        tokio::spawn,
    );

    let builder = Schema::build(Query::default(), Mutation::default(), Subscription)
        // Default per-request auth intent container.
        // Actix inserts a fresh one per request; tests that execute schema directly may rely on this.
        .data(std::sync::Arc::new(
            crate::auth_intent::AuthIntentCtx::default(),
        ))
        // Default auth actor context (unauthenticated by default).
        .data(crate::auth_context::AuthActorCtx::default())
        .data(broadcaster)
        .data(Arc::clone(&service_provider))
        .data(service_provider.audit_log_service())
        .data(service_provider.auth_service())
        .data(service_provider.command_service())
        .data(service_provider.conversation_service())
        .data(service_provider.key_auth_service())
        .data(service_provider.keybind_service())
        .data(service_provider.macro_service())
        .data(service_provider.mention_service())
        .data(service_provider.message_service())
        .data(service_provider.message_attachment_service())
        .data(service_provider.message_reaction_service())
        .data(service_provider.notification_preference_service())
        .data(service_provider.password_reset_service())
        .data(service_provider.pin_service())
        .data(service_provider.read_state_service())
        .data(service_provider.thread_service())
        .data(service_provider.user_service())
        .data(service_provider.server_service())
        .data(service_provider.server_member_service())
        .data(service_provider.link_preview_service())
        .data(service_provider.custom_emoji_service())
        .data(service_provider.channel_lens_service())
        .data(service_provider.workspace_state_service())
        .data(service_provider.moderation_service())
        .data(service_provider.notification_service())
        .data(service_provider.pipeline_service())
        .data(service_provider.device_link_service())
        .data(service_provider.e2ee_service())
        .data(service_provider.workspace_plugin_service())
        .data(service_provider.feature_gate())
        .data(presence_manager)
        .data(typing_tracker)
        .data(observers)
        .data(user_loader)
        .data(reaction_loader)
        .data(mention_loader)
        .data(pin_loader)
        .data(link_preview_loader)
        .data(registration.registration_mode)
        .limit_depth(10)
        .limit_complexity(200);
    match federation {
        Some(federation) => builder.data(federation).finish(),
        None => builder.finish(),
    }
}
