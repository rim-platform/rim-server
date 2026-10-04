use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::ServiceResult;
use crate::server::command::{CreateServerCmd, GetServerQuery, ListMyServersQuery};
use crate::server::dto::ServerDto;

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait ServerService: Send + Sync {
    async fn create_server(&self, cmd: CreateServerCmd) -> ServiceResult<ServerDto>;
    async fn get_server(&self, query: GetServerQuery) -> ServiceResult<ServerDto>;
    async fn list_my_servers(&self, query: ListMyServersQuery) -> ServiceResult<Vec<ServerDto>>;
}
