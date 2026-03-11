use async_trait::async_trait;

use rim_services::capability::dto::CapabilityDto;
use rim_services::capability::service::CapabilityService;
use rim_services::{Actor, ServiceResult};

#[derive(Clone, Default)]
pub struct DefaultCapabilityService;

#[async_trait]
impl CapabilityService for DefaultCapabilityService {
    async fn list_capabilities(&self, _actor: Actor) -> ServiceResult<Vec<CapabilityDto>> {
        Ok(vec![
            CapabilityDto {
                id: "chat.read".to_string(),
                title: "Read Chat".to_string(),
                description: "Read conversations, messages, and threads.".to_string(),
            },
            CapabilityDto {
                id: "chat.write".to_string(),
                title: "Write Chat".to_string(),
                description: "Create messages, threads, and attachment changes.".to_string(),
            },
            CapabilityDto {
                id: "action.invoke".to_string(),
                title: "Invoke Actions".to_string(),
                description: "Invoke registered workspace actions.".to_string(),
            },
            CapabilityDto {
                id: "event.emit".to_string(),
                title: "Emit Events".to_string(),
                description: "Emit structured workspace events.".to_string(),
            },
        ])
    }
}
