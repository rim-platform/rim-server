#[derive(Debug, Clone)]
pub struct EventDto {
    pub id: String,
    pub title: String,
    pub description: String,
}

impl From<rim_core::event::model::Event> for EventDto {
    fn from(value: rim_core::event::model::Event) -> Self {
        Self {
            id: value.id,
            title: value.title,
            description: value.description,
        }
    }
}
