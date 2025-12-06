use async_trait::async_trait;
use crate::repositories::repository::Repository;

#[async_trait]
pub trait MessageThreadRepository<M, Id>: Repository<M, Id>
where
    M: Send + Sync,
    Id: Send + Sync,
{
    async fn create_thread(
        &self,
        conversation_id: Id,
        first_message_id: Option<Id>,
    ) -> anyhow::Result<M>;
    
    async fn set_first_message(
        &self,
        thread_id: Id,
        message_id: Id,
    ) -> anyhow::Result<()>;

    async fn get_threads_for_conversation(
        &self,
        conversation_id: Id,
    ) -> anyhow::Result<Vec<M>>;
}