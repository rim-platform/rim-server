use async_trait::async_trait;

use rim_services::event::dto::EventDto;
use rim_services::event::service::EventService;
use rim_services::{Actor, ServiceResult};

#[derive(Clone, Default)]
pub struct DefaultEventService;

#[async_trait]
impl EventService for DefaultEventService {
    async fn list_events(&self, _actor: Actor) -> ServiceResult<Vec<EventDto>> {
        Ok(vec![
            EventDto {
                id: "message.created".to_string(),
                title: "Message Created".to_string(),
                description: "A message was created in a conversation or thread.".to_string(),
            },
            EventDto {
                id: "thread.created".to_string(),
                title: "Thread Created".to_string(),
                description: "A thread was created from a timeline message.".to_string(),
            },
        ])
    }
}
