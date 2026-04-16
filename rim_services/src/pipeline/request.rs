use rim_core::pipeline::model::PipelineId;
use rim_core::server::model::ServerId;

use crate::Actor;
use crate::pipeline::dto::{PipelineActionDto, PipelineFilterDto, PipelineTriggerDto};

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CreatePipelineCmd {
    pub actor: Actor,
    pub server_id: ServerId,
    pub name: String,
    pub description: Option<String>,
    pub trigger: PipelineTriggerDto,
    pub filter: Option<PipelineFilterDto>,
    pub actions: Vec<PipelineActionDto>,
    pub cooldown_secs: Option<i32>,
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct UpdatePipelineCmd {
    pub actor: Actor,
    pub pipeline_id: PipelineId,
    pub name: String,
    pub description: Option<String>,
    pub trigger: PipelineTriggerDto,
    pub filter: Option<PipelineFilterDto>,
    pub actions: Vec<PipelineActionDto>,
    pub cooldown_secs: Option<i32>,
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DeletePipelineCmd {
    pub actor: Actor,
    pub pipeline_id: PipelineId,
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ListPipelinesCmd {
    pub actor: Actor,
    pub server_id: ServerId,
    pub enabled: Option<bool>,
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TogglePipelineCmd {
    pub actor: Actor,
    pub pipeline_id: PipelineId,
    pub enabled: bool,
}
