use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::repository::error::RepoResult;
use crate::setting::model::Setting;

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait SettingRepository: Send + Sync {
    async fn list_settings(&self) -> RepoResult<Vec<Setting>>;
    async fn get_setting(&self, id: &str) -> RepoResult<Option<Setting>>;
}
