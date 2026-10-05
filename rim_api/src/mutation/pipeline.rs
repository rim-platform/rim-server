use async_graphql::*;
use rim_services::Actor;

use crate::auth_context::require_actor;
use crate::errors::map_service_error;
use crate::ids::parse_uuid_id;

#[derive(Default)]
pub(crate) struct PipelineMutations;

#[Object]
impl PipelineMutations {
    async fn create_pipeline(
        &self,
        ctx: &Context<'_>,
        input: crate::types::CreatePipelineInput,
    ) -> Result<crate::types::PipelineGql> {
        let actor = require_actor(ctx)?;
        let svc =
            ctx.data::<std::sync::Arc<dyn rim_services::pipeline::service::PipelineService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&input.server_id)?;

        let trigger = build_pipeline_trigger_dto(input.trigger)?;
        let filter = input.filter.map(build_pipeline_filter_dto).transpose()?;
        let actions = input
            .actions
            .into_iter()
            .map(
                |a| rim_services::pipeline::dto::PipelineActionDto::ExecuteCommand {
                    command_id: a.command_id,
                    args: a.args.map(|j| j.0),
                },
            )
            .collect();

        let pipeline = svc
            .create(rim_services::pipeline::request::CreatePipelineCmd {
                actor: Actor::new(actor),
                server_id,
                name: input.name,
                description: input.description,
                trigger,
                filter,
                actions,
                cooldown_secs: input.cooldown_secs,
            })
            .await
            .map_err(map_service_error)?;

        Ok(pipeline.into())
    }

    async fn update_pipeline(
        &self,
        ctx: &Context<'_>,
        input: crate::types::UpdatePipelineInput,
    ) -> Result<crate::types::PipelineGql> {
        let actor = require_actor(ctx)?;
        let svc =
            ctx.data::<std::sync::Arc<dyn rim_services::pipeline::service::PipelineService>>()?;
        let pipeline_id =
            parse_uuid_id::<rim_core::pipeline::model::PipelineId>(&input.pipeline_id)?;

        let trigger = build_pipeline_trigger_dto(input.trigger)?;
        let filter = input.filter.map(build_pipeline_filter_dto).transpose()?;
        let actions = input
            .actions
            .into_iter()
            .map(
                |a| rim_services::pipeline::dto::PipelineActionDto::ExecuteCommand {
                    command_id: a.command_id,
                    args: a.args.map(|j| j.0),
                },
            )
            .collect();

        let pipeline = svc
            .update(rim_services::pipeline::request::UpdatePipelineCmd {
                actor: Actor::new(actor),
                pipeline_id,
                name: input.name,
                description: input.description,
                trigger,
                filter,
                actions,
                cooldown_secs: input.cooldown_secs,
            })
            .await
            .map_err(map_service_error)?;

        Ok(pipeline.into())
    }

    async fn delete_pipeline(
        &self,
        ctx: &Context<'_>,
        pipeline_id: async_graphql::ID,
    ) -> Result<bool> {
        let actor = require_actor(ctx)?;
        let svc =
            ctx.data::<std::sync::Arc<dyn rim_services::pipeline::service::PipelineService>>()?;
        let pipeline_id = parse_uuid_id::<rim_core::pipeline::model::PipelineId>(&pipeline_id)?;

        svc.delete(rim_services::pipeline::request::DeletePipelineCmd {
            actor: Actor::new(actor),
            pipeline_id,
        })
        .await
        .map_err(map_service_error)?;

        Ok(true)
    }

    async fn toggle_pipeline(
        &self,
        ctx: &Context<'_>,
        pipeline_id: async_graphql::ID,
        enabled: bool,
    ) -> Result<crate::types::PipelineGql> {
        let actor = require_actor(ctx)?;
        let svc =
            ctx.data::<std::sync::Arc<dyn rim_services::pipeline::service::PipelineService>>()?;
        let pipeline_id = parse_uuid_id::<rim_core::pipeline::model::PipelineId>(&pipeline_id)?;

        let pipeline = svc
            .toggle(rim_services::pipeline::request::TogglePipelineCmd {
                actor: Actor::new(actor),
                pipeline_id,
                enabled,
            })
            .await
            .map_err(map_service_error)?;

        Ok(pipeline.into())
    }
}

fn build_pipeline_trigger_dto(
    input: crate::types::PipelineTriggerInput,
) -> async_graphql::Result<rim_services::pipeline::dto::PipelineTriggerDto> {
    let conversation_id = input
        .conversation_id
        .as_ref()
        .map(parse_uuid_id::<rim_core::conversation::model::ConversationId>)
        .transpose()?;
    Ok(rim_services::pipeline::dto::PipelineTriggerDto {
        kind: input.kind.into(),
        conversation_id,
    })
}

fn build_pipeline_filter_dto(
    input: crate::types::PipelineFilterInput,
) -> async_graphql::Result<rim_services::pipeline::dto::PipelineFilterDto> {
    let mut conditions = Vec::new();
    for c in input.conditions {
        let cond = match c.condition_type.as_str() {
            "MessageContains" => {
                let pattern = c.pattern.ok_or_else(|| {
                    async_graphql::Error::new("MessageContains requires a pattern")
                })?;
                rim_services::pipeline::dto::FilterConditionDto::MessageContains { pattern }
            }
            "MessageFromUser" => {
                let uid_str = c.user_id.ok_or_else(|| {
                    async_graphql::Error::new("MessageFromUser requires a user_id")
                })?;
                let user_id = parse_uuid_id::<rim_core::user::model::UserId>(&uid_str)?;
                rim_services::pipeline::dto::FilterConditionDto::MessageFromUser { user_id }
            }
            "ConversationIs" => {
                let conv_str = c.conversation_id.ok_or_else(|| {
                    async_graphql::Error::new("ConversationIs requires a conversation_id")
                })?;
                let conversation_id =
                    parse_uuid_id::<rim_core::conversation::model::ConversationId>(&conv_str)?;
                rim_services::pipeline::dto::FilterConditionDto::ConversationIs { conversation_id }
            }
            other => {
                return Err(async_graphql::Error::new(format!(
                    "unknown filter condition type: {other}"
                )));
            }
        };
        conditions.push(cond);
    }
    Ok(rim_services::pipeline::dto::PipelineFilterDto { conditions })
}
