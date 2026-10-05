use async_graphql::SimpleObject;

#[derive(SimpleObject, Clone, Debug)]
pub struct Thread {
    pub id: async_graphql::ID,
    pub server_id: async_graphql::ID,
    pub conversation_id: async_graphql::ID,
    pub root_message_id: Option<async_graphql::ID>,
    pub root_author_id: Option<async_graphql::ID>,
    pub root_text: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub last_message_at: chrono::DateTime<chrono::Utc>,
}

impl From<rim_services::message_thread::dto::ThreadDto> for Thread {
    fn from(value: rim_services::message_thread::dto::ThreadDto) -> Self {
        Self {
            id: async_graphql::ID(value.id.to_string()),
            server_id: async_graphql::ID(value.server_id.to_string()),
            conversation_id: async_graphql::ID(value.conversation_id.to_string()),
            root_message_id: value
                .root_message_id
                .map(|id| async_graphql::ID(id.to_string())),
            root_author_id: value
                .root_author_id
                .map(|id| async_graphql::ID(id.to_string())),
            root_text: value.root_text,
            created_at: value.created_at,
            last_message_at: value.last_message_at,
        }
    }
}
