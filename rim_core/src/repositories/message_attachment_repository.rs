use async_trait::async_trait;
use crate::repositories::repository::Repository;

#[async_trait]
pub trait MessageAttachmentRepository<M, Id>:  Repository<M, Id>
where
    M: Send + Sync,
    Id: Send + Sync,
{
    async fn add_attachment(
        &self,
        message_id: Id,
        storage_url: &str,
        file_name: &str,
        file_size: i64,
        mime_type: &str,
        metadata: Option<serde_json::Value>,
    ) -> anyhow::Result<M>;
    
    async fn get_attachments_for_message(
        &self,
        message_id: Id,
    ) -> anyhow::Result<Vec<M>>;
}
