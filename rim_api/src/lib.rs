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
// pub: rim_server injects the FederationContext and runs forwarded GraphQL with its checks.
pub mod federation;
pub mod graphql;
pub mod ids;
pub mod loaders;
pub mod mutation;
pub mod observers;
pub mod pagination;
pub mod query;
pub mod resolve;
pub mod subscription;
pub mod types;

#[cfg(test)]
mod tests;

/// Extra per-deployment context injected into the GraphQL schema at boot.
pub struct RegistrationContext {
    /// The registration mode (invite-only vs open).
    pub registration_mode: Arc<rim_services::auth::dto::RegistrationMode>,
    /// Typed marker for the `previewInviteToken` rate limiter.
    ///
    /// The real limiter lives inside `DefaultAuthService`; this marker is
    /// carried here for future-extensibility. See spec §4.7 R2.
    pub preview_rate_limiter: Arc<rim_services::auth::dto::PreviewInviteRateLimiterToken>,
    /// Typed marker for the open-mode signup rate limiter.
    ///
    /// The real limiter lives inside `DefaultAuthService`; this marker is
    /// carried here for future-extensibility. See spec §6.3 R2.
    pub signup_rate_limiter: Arc<rim_services::auth::dto::SignupRateLimiterToken>,
}

/// Wire the loaders and all shared `.data(...)` context onto a fresh schema
/// builder. Both [`build_schema`] and [`export_sdl`] call this; they differ only
/// in registration-context data and depth/complexity limits, which each applies
/// after this returns. Kept private — the two public entry points are the API.
fn wire_common_schema_data(
    builder: async_graphql::SchemaBuilder<Query, Mutation, Subscription>,
    service_provider: Arc<dyn AppServiceProvider>,
    broadcaster: InMemoryBroadcaster,
    presence_manager: Arc<dyn rim_services::presence::PresenceService>,
    typing_tracker: Arc<dyn rim_services::typing::TypingService>,
    observers: Arc<rim_services::events::Observers>,
) -> async_graphql::SchemaBuilder<Query, Mutation, Subscription> {
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

    builder
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
        .data(presence_manager)
        .data(typing_tracker)
        .data(observers)
        .data(user_loader)
        .data(reaction_loader)
        .data(mention_loader)
        .data(pin_loader)
        .data(link_preview_loader)
}

pub fn build_schema(
    service_provider: Arc<dyn AppServiceProvider>,
    broadcaster: InMemoryBroadcaster,
    presence_manager: Arc<dyn rim_services::presence::PresenceService>,
    typing_tracker: Arc<dyn rim_services::typing::TypingService>,
    observers: Arc<rim_services::events::Observers>,
    registration: RegistrationContext,
    federation: Option<federation::FederationContext>,
) -> AppSchema {
    let builder = wire_common_schema_data(
        Schema::build(Query::default(), Mutation::default(), Subscription),
        service_provider,
        broadcaster,
        presence_manager,
        typing_tracker,
        observers,
    )
    // Registration context — §6.3
    .data(registration.registration_mode)
    .data(registration.preview_rate_limiter)
    .data(registration.signup_rate_limiter)
    .limit_depth(10)
    .limit_complexity(200);
    match federation {
        Some(federation) => builder.data(federation).finish(),
        None => builder.finish(),
    }
}

/// Build the schema SDL string for codegen tooling.
/// Uses the same schema as `build_schema` but without depth/complexity limits
/// so introspection queries succeed. Shares its loader/service wiring with
/// [`build_schema`] via [`wire_common_schema_data`].
pub fn export_sdl(
    service_provider: Arc<dyn AppServiceProvider>,
    broadcaster: InMemoryBroadcaster,
    presence_manager: Arc<dyn rim_services::presence::PresenceService>,
    typing_tracker: Arc<dyn rim_services::typing::TypingService>,
    observers: Arc<rim_services::events::Observers>,
) -> String {
    let schema = wire_common_schema_data(
        Schema::build(Query::default(), Mutation::default(), Subscription),
        service_provider,
        broadcaster,
        presence_manager,
        typing_tracker,
        observers,
    )
    // Registration context defaults for SDL export (unused at runtime).
    .data(Arc::new(
        rim_services::auth::dto::RegistrationMode::InviteOnly,
    ))
    .data(Arc::new(
        rim_services::auth::dto::PreviewInviteRateLimiterToken,
    ))
    .data(Arc::new(rim_services::auth::dto::SignupRateLimiterToken))
    .finish();

    schema.sdl()
}
