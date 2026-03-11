use async_trait::async_trait;

use rim_services::setting::dto::{SettingDto, SettingScopeDto};
use rim_services::setting::service::SettingService;
use rim_services::{Actor, ServiceResult};

#[derive(Clone, Default)]
pub struct DefaultSettingService;

#[async_trait]
impl SettingService for DefaultSettingService {
    async fn list_settings(&self, _actor: Actor) -> ServiceResult<Vec<SettingDto>> {
        Ok(vec![
            SettingDto {
                id: "user.notifications.enabled".to_string(),
                scope: SettingScopeDto::User,
                title: "Notifications Enabled".to_string(),
                description: "Enable or disable user notifications.".to_string(),
                value_type: "boolean".to_string(),
                is_secret: false,
            },
            SettingDto {
                id: "workspace.default_channel".to_string(),
                scope: SettingScopeDto::Workspace,
                title: "Default Channel".to_string(),
                description: "Default channel used for workspace onboarding flows.".to_string(),
                value_type: "conversation_id".to_string(),
                is_secret: false,
            },
            SettingDto {
                id: "integration.webhook_secret".to_string(),
                scope: SettingScopeDto::Integration,
                title: "Webhook Secret".to_string(),
                description: "Signing secret used by webhook integrations.".to_string(),
                value_type: "string".to_string(),
                is_secret: true,
            },
        ])
    }
}
