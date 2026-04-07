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

pub fn build_schema(
    service_provider: Arc<dyn AppServiceProvider>,
    broadcaster: InMemoryBroadcaster,
    presence_manager: Arc<rim_infra_services::presence::PresenceManager>,
    typing_tracker: Arc<rim_infra_services::typing::TypingTracker>,
    observers: Arc<rim_services::events::Observers>,
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

    Schema::build(Query, Mutation, Subscription)
        // Default per-request auth intent container.
        // Actix inserts a fresh one per request; tests that execute schema directly may rely on this.
        .data(std::sync::Arc::new(
            crate::auth_intent::AuthIntentCtx::default(),
        ))
        // Default auth actor context (unauthenticated by default).
        .data(crate::auth_context::AuthActorCtx::default())
        .data(broadcaster)
        .data(Arc::clone(&service_provider))
        .data(service_provider.action_service())
        .data(service_provider.auth_service())
        .data(service_provider.capability_service())
        .data(service_provider.command_service())
        .data(service_provider.conversation_service())
        .data(service_provider.event_service())
        .data(service_provider.integration_service())
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
        .data(service_provider.setting_service())
        .data(service_provider.thread_service())
        .data(service_provider.user_service())
        .data(service_provider.server_service())
        .data(service_provider.server_member_service())
        .data(service_provider.link_preview_service())
        .data(service_provider.custom_emoji_service())
        .data(service_provider.incident_service())
        .data(service_provider.channel_lens_service())
        .data(service_provider.skill_profile_service())
        .data(presence_manager)
        .data(typing_tracker)
        .data(observers)
        .data(user_loader)
        .data(reaction_loader)
        .data(mention_loader)
        .data(pin_loader)
        .data(link_preview_loader)
        .limit_depth(10)
        .limit_complexity(200)
        .finish()
}

/// Build the schema SDL string for codegen tooling.
/// Uses the same schema as `build_schema` but without depth/complexity limits
/// so introspection queries succeed.
pub fn export_sdl(
    service_provider: Arc<dyn AppServiceProvider>,
    broadcaster: InMemoryBroadcaster,
    presence_manager: Arc<rim_infra_services::presence::PresenceManager>,
    typing_tracker: Arc<rim_infra_services::typing::TypingTracker>,
    observers: Arc<rim_services::events::Observers>,
) -> String {
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

    let schema = Schema::build(Query, Mutation, Subscription)
        .data(std::sync::Arc::new(
            crate::auth_intent::AuthIntentCtx::default(),
        ))
        .data(crate::auth_context::AuthActorCtx::default())
        .data(broadcaster)
        .data(Arc::clone(&service_provider))
        .data(service_provider.action_service())
        .data(service_provider.auth_service())
        .data(service_provider.capability_service())
        .data(service_provider.command_service())
        .data(service_provider.conversation_service())
        .data(service_provider.event_service())
        .data(service_provider.integration_service())
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
        .data(service_provider.setting_service())
        .data(service_provider.thread_service())
        .data(service_provider.user_service())
        .data(service_provider.server_service())
        .data(service_provider.server_member_service())
        .data(service_provider.link_preview_service())
        .data(service_provider.custom_emoji_service())
        .data(service_provider.incident_service())
        .data(service_provider.channel_lens_service())
        .data(service_provider.skill_profile_service())
        .data(presence_manager)
        .data(typing_tracker)
        .data(observers)
        .data(user_loader)
        .data(reaction_loader)
        .data(mention_loader)
        .data(pin_loader)
        .data(link_preview_loader)
        .finish();

    schema.sdl()
}
