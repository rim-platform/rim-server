use async_graphql::{Enum, InputObject, SimpleObject};

// ── Channel Lens ────────────────────────────────────────────────────────

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum FilterField {
    HasLinks,
    AuthorId,
    HasAttachments,
    TextContains,
    IsPinned,
}

impl From<rim_services::channel_lens::dto::FilterFieldDto> for FilterField {
    fn from(value: rim_services::channel_lens::dto::FilterFieldDto) -> Self {
        use rim_services::channel_lens::dto::FilterFieldDto as D;
        match value {
            D::HasLinks => Self::HasLinks,
            D::AuthorId => Self::AuthorId,
            D::HasAttachments => Self::HasAttachments,
            D::TextContains => Self::TextContains,
            D::IsPinned => Self::IsPinned,
        }
    }
}

impl From<FilterField> for rim_services::channel_lens::dto::FilterFieldDto {
    fn from(value: FilterField) -> Self {
        match value {
            FilterField::HasLinks => Self::HasLinks,
            FilterField::AuthorId => Self::AuthorId,
            FilterField::HasAttachments => Self::HasAttachments,
            FilterField::TextContains => Self::TextContains,
            FilterField::IsPinned => Self::IsPinned,
        }
    }
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum FilterOp {
    Eq,
    In,
    Contains,
}

impl From<rim_services::channel_lens::dto::FilterOpDto> for FilterOp {
    fn from(value: rim_services::channel_lens::dto::FilterOpDto) -> Self {
        use rim_services::channel_lens::dto::FilterOpDto as D;
        match value {
            D::Eq => Self::Eq,
            D::In => Self::In,
            D::Contains => Self::Contains,
        }
    }
}

impl From<FilterOp> for rim_services::channel_lens::dto::FilterOpDto {
    fn from(value: FilterOp) -> Self {
        match value {
            FilterOp::Eq => Self::Eq,
            FilterOp::In => Self::In,
            FilterOp::Contains => Self::Contains,
        }
    }
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum FilterLogic {
    And,
    Or,
}

impl From<rim_services::channel_lens::dto::FilterLogicDto> for FilterLogic {
    fn from(value: rim_services::channel_lens::dto::FilterLogicDto) -> Self {
        use rim_services::channel_lens::dto::FilterLogicDto as D;
        match value {
            D::And => Self::And,
            D::Or => Self::Or,
        }
    }
}

impl From<FilterLogic> for rim_services::channel_lens::dto::FilterLogicDto {
    fn from(value: FilterLogic) -> Self {
        match value {
            FilterLogic::And => Self::And,
            FilterLogic::Or => Self::Or,
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct FilterConditionGql {
    pub field: FilterField,
    pub op: FilterOp,
    pub value: async_graphql::Json<serde_json::Value>,
}

#[derive(SimpleObject, Clone, Debug)]
pub struct FilterSpecGql {
    pub conditions: Vec<FilterConditionGql>,
    pub logic: FilterLogic,
}

#[derive(SimpleObject, Clone, Debug)]
pub struct ChannelLens {
    pub id: async_graphql::ID,
    pub server_id: async_graphql::ID,
    pub conversation_id: async_graphql::ID,
    pub created_by: async_graphql::ID,
    pub name: String,
    pub filter_spec: FilterSpecGql,
    pub is_shared: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl From<rim_services::channel_lens::dto::ChannelLensDto> for ChannelLens {
    fn from(value: rim_services::channel_lens::dto::ChannelLensDto) -> Self {
        Self {
            id: async_graphql::ID(value.id.to_string()),
            server_id: async_graphql::ID(value.server_id.to_string()),
            conversation_id: async_graphql::ID(value.conversation_id.to_string()),
            created_by: async_graphql::ID(value.created_by.to_string()),
            name: value.name,
            filter_spec: FilterSpecGql {
                conditions: value
                    .filter_spec
                    .conditions
                    .into_iter()
                    .map(|c| FilterConditionGql {
                        field: c.field.into(),
                        op: c.op.into(),
                        value: async_graphql::Json(c.value),
                    })
                    .collect(),
                logic: value.filter_spec.logic.into(),
            },
            is_shared: value.is_shared,
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

#[derive(InputObject, Clone, Debug)]
pub struct FilterConditionInput {
    pub field: FilterField,
    pub op: FilterOp,
    pub value: async_graphql::Json<serde_json::Value>,
}

#[derive(InputObject, Clone, Debug)]
pub struct FilterSpecInput {
    pub conditions: Vec<FilterConditionInput>,
    pub logic: FilterLogic,
}

impl From<FilterSpecInput> for rim_services::channel_lens::dto::FilterSpecDto {
    fn from(value: FilterSpecInput) -> Self {
        Self {
            conditions: value
                .conditions
                .into_iter()
                .map(|c| rim_services::channel_lens::dto::FilterConditionDto {
                    field: c.field.into(),
                    op: c.op.into(),
                    value: c.value.0,
                })
                .collect(),
            logic: value.logic.into(),
        }
    }
}
