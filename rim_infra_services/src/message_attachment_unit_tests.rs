#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use rim_core::message_attachment::model::{MessageAttachment, MessageAttachmentId};
    use rim_core::server::model::ServerId;
    use rim_core::user::model::UserId;
    use rim_services::message_attachment::command::GetAttachmentQuery;
    use rim_services::message_attachment::service::MessageAttachmentService;
    use rim_services::{Actor, ServiceError};

    use crate::message_attachment::DefaultMessageAttachmentService;
    use crate::test_support::attachment::{
        AttachmentReposPorts, MockMessageAttachmentRepo, MockServerMemberRepo,
    };

    fn attachment(id: MessageAttachmentId) -> MessageAttachment {
        MessageAttachment {
            id,
            message_id: rim_core::message::model::MessageId::new(),
            storage_url: "u".to_string(),
            file_name: "f.txt".to_string(),
            file_size: 10,
            mime_type: "text/plain".to_string(),
            metadata: None,
        }
    }

    #[tokio::test]
    async fn get_attachment_denies_non_member() {
        let server_id = ServerId::new();
        let actor_id = UserId::new();

        let message_attachment_repo = MockMessageAttachmentRepo::new();
        let mut server_member_repo = MockServerMemberRepo::new();
        server_member_repo
            .expect_is_member()
            .returning(|_sid, _uid| Ok(false));

        let svc = DefaultMessageAttachmentService::new_for_tests(AttachmentReposPorts {
            message_attachment_repo: Arc::new(message_attachment_repo),
            server_member_repo: Arc::new(server_member_repo),
        });

        let err = svc
            .get_attachment(GetAttachmentQuery {
                actor: Actor::new(actor_id),
                server_id,
                attachment_id: MessageAttachmentId::new(),
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::PermissionDenied));
    }

    #[tokio::test]
    async fn get_attachment_returns_not_found_when_missing() {
        let server_id = ServerId::new();
        let actor_id = UserId::new();

        let mut message_attachment_repo = MockMessageAttachmentRepo::new();
        message_attachment_repo
            .expect_get_by_id()
            .returning(|_id| Ok(None));

        let mut server_member_repo = MockServerMemberRepo::new();
        server_member_repo
            .expect_is_member()
            .returning(|_sid, _uid| Ok(true));

        let svc = DefaultMessageAttachmentService::new_for_tests(AttachmentReposPorts {
            message_attachment_repo: Arc::new(message_attachment_repo),
            server_member_repo: Arc::new(server_member_repo),
        });

        let err = svc
            .get_attachment(GetAttachmentQuery {
                actor: Actor::new(actor_id),
                server_id,
                attachment_id: MessageAttachmentId::new(),
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::NotFound));
    }

    #[tokio::test]
    async fn get_attachment_maps_fields() {
        let server_id = ServerId::new();
        let actor_id = UserId::new();
        let id = MessageAttachmentId::new();
        let att = attachment(id);

        let mut message_attachment_repo = MockMessageAttachmentRepo::new();
        message_attachment_repo
            .expect_get_by_id()
            .returning(move |_id| Ok(Some(att.clone())));

        let mut server_member_repo = MockServerMemberRepo::new();
        server_member_repo
            .expect_is_member()
            .returning(|_sid, _uid| Ok(true));

        let svc = DefaultMessageAttachmentService::new_for_tests(AttachmentReposPorts {
            message_attachment_repo: Arc::new(message_attachment_repo),
            server_member_repo: Arc::new(server_member_repo),
        });

        let dto = svc
            .get_attachment(GetAttachmentQuery {
                actor: Actor::new(actor_id),
                server_id,
                attachment_id: id,
            })
            .await
            .unwrap();
        assert_eq!(dto.id, id);
        assert_eq!(dto.filename, "f.txt");
        assert_eq!(dto.size_bytes, 10);
    }
}
