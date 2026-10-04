use async_trait::async_trait;

use rim_services::action::dto::ActionDto;
use rim_services::action::service::ActionService;
use rim_services::{Actor, ServiceResult};

#[derive(Clone, Default)]
pub struct DefaultActionService;

#[async_trait]
impl ActionService for DefaultActionService {
    async fn list_actions(&self, _actor: Actor) -> ServiceResult<Vec<ActionDto>> {
        Ok(vec![
            ActionDto {
                id: "post_message".to_string(),
                title: "Post Message".to_string(),
                description: "Post a message into a conversation timeline.".to_string(),
                capability_ids: vec!["chat.write".to_string(), "action.invoke".to_string()],
            },
            ActionDto {
                id: "open_thread".to_string(),
                title: "Open Thread".to_string(),
                description: "Create or open a thread for a timeline message.".to_string(),
                capability_ids: vec!["chat.write".to_string(), "action.invoke".to_string()],
            },
        ])
    }
}
