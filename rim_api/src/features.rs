//! The instance feature gate as resolvers use it.

use std::sync::Arc;

use async_graphql::{Context, Result};
use rim_core::server_feature::model::{FeatureState, ServerFeature};
use rim_services::feature::service::FeatureGate;

use crate::errors::map_service_error;

/// `FEATURE_DISABLED` unless `feature` is enabled on this instance.
pub(crate) fn require_feature(ctx: &Context<'_>, feature: ServerFeature) -> Result<()> {
    ctx.data::<Arc<dyn FeatureGate>>()?
        .require(feature)
        .map_err(map_service_error)
}

/// The state of `feature` on this instance.
pub(crate) fn feature_state(ctx: &Context<'_>, feature: ServerFeature) -> Result<FeatureState> {
    Ok(ctx.data::<Arc<dyn FeatureGate>>()?.state(feature))
}

/// `FEATURE_DISABLED` for a command or macro an AI agent or MCP server runs while the AI
/// surface is off; any other initiator passes.
pub(crate) fn require_initiator(
    ctx: &Context<'_>,
    initiator: Option<crate::types::CommandInitiator>,
) -> Result<()> {
    use crate::types::CommandInitiator;
    match initiator {
        Some(CommandInitiator::Agent | CommandInitiator::Mcp) => {
            require_feature(ctx, ServerFeature::AiCommands)
        }
        _ => Ok(()),
    }
}

/// `command` as the instance shows it: without its agent-facing metadata while the AI surface
/// is off.
pub(crate) fn shown_command(
    ai_commands: &FeatureState,
    mut command: crate::types::Command,
) -> crate::types::Command {
    if !ai_commands.is_enabled() {
        command.nl_description = None;
        command.intent_samples = Vec::new();
        command.ai_callable = false;
    }
    command
}
