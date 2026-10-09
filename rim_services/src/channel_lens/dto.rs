use chrono::{DateTime, Utc};
use rim_core::channel_lens::model::{
    ChannelLens, ChannelLensId, FilterCondition, FilterField, FilterLogic, FilterOp, FilterSpec,
};
use rim_core::conversation::model::ConversationId;
use rim_core::server::model::ServerId;
use rim_core::user::model::UserId;

#[derive(Debug, Clone)]
pub struct ChannelLensDto {
    pub id: ChannelLensId,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,
    pub created_by: UserId,
    pub name: String,
    pub filter_spec: FilterSpecDto,
    pub is_shared: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<ChannelLens> for ChannelLensDto {
    fn from(lens: ChannelLens) -> Self {
        Self {
            id: lens.id,
            server_id: lens.server_id,
            conversation_id: lens.conversation_id,
            created_by: lens.created_by,
            name: lens.name,
            filter_spec: lens.filter_spec.into(),
            is_shared: lens.is_shared,
            created_at: lens.created_at,
            updated_at: lens.updated_at,
        }
    }
}

#[derive(Debug, Clone)]
pub struct FilterSpecDto {
    pub conditions: Vec<FilterConditionDto>,
    pub logic: FilterLogicDto,
}

impl From<FilterSpec> for FilterSpecDto {
    fn from(spec: FilterSpec) -> Self {
        Self {
            conditions: spec.conditions.into_iter().map(Into::into).collect(),
            logic: spec.logic.into(),
        }
    }
}

impl From<FilterSpecDto> for FilterSpec {
    fn from(dto: FilterSpecDto) -> Self {
        Self {
            conditions: dto.conditions.into_iter().map(Into::into).collect(),
            logic: dto.logic.into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct FilterConditionDto {
    pub field: FilterFieldDto,
    pub op: FilterOpDto,
    pub value: serde_json::Value,
}

impl From<FilterCondition> for FilterConditionDto {
    fn from(cond: FilterCondition) -> Self {
        Self {
            field: cond.field.into(),
            op: cond.op.into(),
            value: cond.value,
        }
    }
}

impl From<FilterConditionDto> for FilterCondition {
    fn from(dto: FilterConditionDto) -> Self {
        Self {
            field: dto.field.into(),
            op: dto.op.into(),
            value: dto.value,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterFieldDto {
    HasLinks,
    AuthorId,
    HasAttachments,
    TextContains,
    IsPinned,
}

impl From<FilterField> for FilterFieldDto {
    fn from(f: FilterField) -> Self {
        match f {
            FilterField::HasLinks => Self::HasLinks,
            FilterField::AuthorId => Self::AuthorId,
            FilterField::HasAttachments => Self::HasAttachments,
            FilterField::TextContains => Self::TextContains,
            FilterField::IsPinned => Self::IsPinned,
        }
    }
}

impl From<FilterFieldDto> for FilterField {
    fn from(dto: FilterFieldDto) -> Self {
        match dto {
            FilterFieldDto::HasLinks => Self::HasLinks,
            FilterFieldDto::AuthorId => Self::AuthorId,
            FilterFieldDto::HasAttachments => Self::HasAttachments,
            FilterFieldDto::TextContains => Self::TextContains,
            FilterFieldDto::IsPinned => Self::IsPinned,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterOpDto {
    Eq,
    In,
    Contains,
}

impl From<FilterOp> for FilterOpDto {
    fn from(op: FilterOp) -> Self {
        match op {
            FilterOp::Eq => Self::Eq,
            FilterOp::In => Self::In,
            FilterOp::Contains => Self::Contains,
        }
    }
}

impl From<FilterOpDto> for FilterOp {
    fn from(dto: FilterOpDto) -> Self {
        match dto {
            FilterOpDto::Eq => Self::Eq,
            FilterOpDto::In => Self::In,
            FilterOpDto::Contains => Self::Contains,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterLogicDto {
    And,
    Or,
}

impl From<FilterLogic> for FilterLogicDto {
    fn from(logic: FilterLogic) -> Self {
        match logic {
            FilterLogic::And => Self::And,
            FilterLogic::Or => Self::Or,
        }
    }
}

impl From<FilterLogicDto> for FilterLogic {
    fn from(dto: FilterLogicDto) -> Self {
        match dto {
            FilterLogicDto::And => Self::And,
            FilterLogicDto::Or => Self::Or,
        }
    }
}
