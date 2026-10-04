use crate::Actor;
use crate::channel_lens::dto::FilterSpecDto;
use rim_core::channel_lens::model::ChannelLensId;
use rim_core::conversation::model::ConversationId;
use rim_core::server::model::ServerId;

#[derive(Debug, Clone)]
pub struct CreateLensCmd {
    pub actor: Actor,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,
    pub name: String,
    pub filter_spec: FilterSpecDto,
    pub is_shared: bool,
}

#[derive(Debug, Clone)]
pub struct UpdateLensCmd {
    pub actor: Actor,
    pub server_id: ServerId,
    pub lens_id: ChannelLensId,
    pub name: Option<String>,
    pub filter_spec: Option<FilterSpecDto>,
    pub is_shared: Option<bool>,
}

#[derive(Debug, Clone)]
pub struct DeleteLensCmd {
    pub actor: Actor,
    pub server_id: ServerId,
    pub lens_id: ChannelLensId,
}

#[derive(Debug, Clone)]
pub struct ListLensesQuery {
    pub actor: Actor,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,
}

#[derive(Debug, Clone)]
pub struct ApplyLensQuery {
    pub actor: Actor,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,
    pub lens_id: ChannelLensId,
    pub limit: u32,
}
