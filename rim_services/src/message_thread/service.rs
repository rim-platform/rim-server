use async_trait::async_trait;

use crate::ServiceResult;
use crate::message_thread::command::{CreateThreadCmd, GetThreadQuery, ListThreadsQuery};
use crate::message_thread::dto::{ThreadDto};

#[async_trait]
pub trait ThreadService: Send + Sync {
    async fn create_thread(&self, cmd: CreateThreadCmd) -> ServiceResult<ThreadDto>;

    async fn get_thread(&self, query: GetThreadQuery) -> ServiceResult<ThreadDto>;

    async fn list_threads(&self, query: ListThreadsQuery) -> ServiceResult<Vec<ThreadDto>>;
}
