#[derive(Debug, Clone)]
pub struct ActionDto {
    pub id: String,
    pub title: String,
    pub description: String,
    pub capability_ids: Vec<String>,
}

impl From<rim_core::action::model::Action> for ActionDto {
    fn from(value: rim_core::action::model::Action) -> Self {
        Self {
            id: value.id,
            title: value.title,
            description: value.description,
            capability_ids: value.capability_ids,
        }
    }
}
