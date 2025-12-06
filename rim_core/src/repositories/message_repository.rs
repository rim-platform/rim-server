use async_trait::async_trait;
use crate::repositories::repository::Repository;

#[async_trait]
pub trait MessageRepository<M, Id>: Repository<M, Id>
where
    M: Send + Sync,
    Id: Send + Sync,
{
    async fn create_message(
        &self,
        thread_id: Id,
        parent_message_id: Option<Id>,
        sent_by: Id,
        text: &str,
        has_attachments: bool,
    ) -> anyhow::Result<M>;

    async fn delete_message(&self, message_id: Id) -> anyhow::Result<()>;

    async fn get_messages_for_thread(
        &self,
        thread_id: Id,
    ) -> anyhow::Result<Vec<M>>;
}
