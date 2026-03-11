use async_trait::async_trait;

use rim_services::integration::dto::{IntegrationDto, IntegrationKindDto};
use rim_services::integration::service::IntegrationService;
use rim_services::{Actor, ServiceResult};

#[derive(Clone, Default)]
pub struct DefaultIntegrationService;

#[async_trait]
impl IntegrationService for DefaultIntegrationService {
    async fn list_integrations(&self, actor: Actor) -> ServiceResult<Vec<IntegrationDto>> {
        Ok(vec![IntegrationDto {
            id: "core.messaging".to_string(),
            kind: IntegrationKindDto::BuiltIn,
            title: "Core Messaging".to_string(),
            enabled: true,
            principal_id: Some(actor.principal.id),
        }])
    }
}
