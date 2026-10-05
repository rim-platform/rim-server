use async_graphql::*;
use rim_services::Actor;
use rim_services::keybind::request::{CreateKeybindCmd, DeleteKeybindCmd, UpdateKeybindCmd};
use rim_services::keybind::service::KeybindService;

use crate::auth_context::require_actor;
use crate::errors::map_service_error;
use crate::ids::parse_uuid_id;
use crate::types::{Keybind, KeybindContext, KeybindScope};

#[derive(Default)]
pub(crate) struct KeybindMutations;

#[Object]
impl KeybindMutations {
    async fn create_keybind(
        &self,
        ctx: &Context<'_>,
        input: CreateKeybindInput,
    ) -> Result<Keybind> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn KeybindService>>()?;
        let keybind = svc
            .create_keybind(CreateKeybindCmd {
                actor: Actor::new(actor),
                scope: input.scope.into(),
                workspace_id: input
                    .workspace_id
                    .as_ref()
                    .map(parse_uuid_id::<rim_core::server::model::WorkspaceId>)
                    .transpose()?,
                keystroke: input.keystroke,
                command_id: input.command_id,
                context: input.context.into(),
                preset_args: input.preset_args.map(|json| json.0),
            })
            .await
            .map_err(map_service_error)?;

        Ok(keybind.into())
    }

    async fn update_keybind(
        &self,
        ctx: &Context<'_>,
        input: UpdateKeybindInput,
    ) -> Result<Keybind> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn KeybindService>>()?;
        let keybind = svc
            .update_keybind(UpdateKeybindCmd {
                actor: Actor::new(actor),
                keybind_id: parse_uuid_id::<rim_core::keybind::model::KeybindId>(
                    &input.keybind_id,
                )?,
                keystroke: input.keystroke,
                command_id: input.command_id,
                context: input.context.into(),
                preset_args: input.preset_args.map(|json| json.0),
                is_enabled: input.is_enabled,
            })
            .await
            .map_err(map_service_error)?;

        Ok(keybind.into())
    }
}

#[derive(Default)]
pub(crate) struct KeybindDeleteMutations;

#[Object]
impl KeybindDeleteMutations {
    async fn delete_keybind(&self, ctx: &Context<'_>, input: DeleteKeybindInput) -> Result<bool> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn KeybindService>>()?;
        svc.delete_keybind(DeleteKeybindCmd {
            actor: Actor::new(actor),
            keybind_id: parse_uuid_id::<rim_core::keybind::model::KeybindId>(&input.keybind_id)?,
        })
        .await
        .map_err(map_service_error)?;

        Ok(true)
    }
}

#[derive(InputObject)]
pub struct CreateKeybindInput {
    pub scope: KeybindScope,
    pub workspace_id: Option<ID>,
    pub keystroke: String,
    pub command_id: String,
    pub context: KeybindContext,
    pub preset_args: Option<async_graphql::Json<serde_json::Value>>,
}

#[derive(InputObject)]
pub struct UpdateKeybindInput {
    pub keybind_id: ID,
    pub keystroke: String,
    pub command_id: String,
    pub context: KeybindContext,
    pub preset_args: Option<async_graphql::Json<serde_json::Value>>,
    pub is_enabled: bool,
}

#[derive(InputObject)]
pub struct DeleteKeybindInput {
    pub keybind_id: ID,
}
