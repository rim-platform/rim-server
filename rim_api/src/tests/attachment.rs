use super::*;

#[tokio::test]
async fn attachment_query_calls_service_when_authed() {
    let actor = UserId::new();
    let server_id = ServerId::new();
    let attachment_id = rim_core::message_attachment::model::MessageAttachmentId::new();

    let mut attachment = MockMessageAttachmentService::new();
    attachment.expect_get_attachment().returning(move |_q| {
        Box::pin(async move {
            let mut dto = test_attachment_dto();
            dto.id = attachment_id;
            dto.filename = "f.txt".to_string();
            Ok(dto)
        })
    });

    let schema = schema_with_mocks(
        MockAuthService::new(),
        MockConversationService::new(),
        MockMessageService::new(),
        attachment,
        MockThreadService::new(),
    );

    let req = authed_request(
        r#"query($sid: ID!, $aid: ID!) { attachment(serverId: $sid, attachmentId: $aid) { id status filename sizeBytes } }"#,
        actor,
    )
    .variables(Variables::from_json(serde_json::json!({
        "sid": server_id.to_string(),
        "aid": attachment_id.to_string(),
    })));

    let res = schema.execute(req).await;
    assert_no_errors(&res);
}

#[tokio::test]
async fn mutation_create_attachment_returns_upload_ticket() {
    let actor = UserId::new();
    let server_id = ServerId::new();
    let conversation_id = ConversationId::new();
    let attachment_id = rim_core::message_attachment::model::MessageAttachmentId::new();

    let mut attachment = MockMessageAttachmentService::new();
    attachment
        .expect_create_attachment()
        .returning(move |_cmd| {
            let attachment_id = attachment_id;
            Box::pin(async move {
                Ok(rim_services::message_attachment::dto::UploadTicketDto {
                    attachment_id,
                    upload_token: "upload-token".to_string(),
                    max_size_bytes: Some(42),
                    expires_at: None,
                })
            })
        });

    let schema = schema_with_mocks(
        MockAuthService::new(),
        MockConversationService::new(),
        MockMessageService::new(),
        attachment,
        MockThreadService::new(),
    );

    let req = authed_request(
        r#"mutation($sid: ID!, $cid: ID!, $mid: ID!) { createAttachment(input: { serverId: $sid, conversationId: $cid, messageId: $mid, filename: "f.txt", contentType: "text/plain", sizeBytes: 42 }) { attachmentId uploadToken maxSizeBytes } }"#,
        actor,
    )
    .variables(Variables::from_json(serde_json::json!({
        "sid": server_id.to_string(),
        "cid": conversation_id.to_string(),
        "mid": MessageId::new().to_string(),
    })));

    let res = schema.execute(req).await;
    assert_no_errors(&res);
    let json = res.data.into_json().unwrap();
    assert_eq!(
        json["createAttachment"]["attachmentId"],
        serde_json::json!(attachment_id.to_string())
    );
}

#[tokio::test]
async fn mutation_finalize_attachment_returns_attachment() {
    let actor = UserId::new();
    let server_id = ServerId::new();
    let attachment_id = rim_core::message_attachment::model::MessageAttachmentId::new();

    let mut attachment = MockMessageAttachmentService::new();
    attachment
        .expect_finalize_attachment()
        .returning(move |_cmd| {
            let attachment_id = attachment_id;
            Box::pin(async move {
                let mut dto = test_attachment_dto();
                dto.id = attachment_id;
                dto.filename = "f.txt".to_string();
                Ok(dto)
            })
        });

    let schema = schema_with_mocks(
        MockAuthService::new(),
        MockConversationService::new(),
        MockMessageService::new(),
        attachment,
        MockThreadService::new(),
    );

    let req = authed_request(
        r#"mutation($sid: ID!, $aid: ID!) { finalizeAttachment(input: { serverId: $sid, attachmentId: $aid }) { id } }"#,
        actor,
    )
    .variables(Variables::from_json(serde_json::json!({
        "sid": server_id.to_string(),
        "aid": attachment_id.to_string(),
    })));

    let res = schema.execute(req).await;
    assert_no_errors(&res);
}

#[tokio::test]
async fn mutation_delete_attachment_returns_true() {
    let actor = UserId::new();
    let server_id = ServerId::new();
    let attachment_id = rim_core::message_attachment::model::MessageAttachmentId::new();

    let mut attachment = MockMessageAttachmentService::new();
    attachment
        .expect_delete_attachment()
        .returning(|_cmd| Box::pin(async { Ok(()) }));

    let schema = schema_with_mocks(
        MockAuthService::new(),
        MockConversationService::new(),
        MockMessageService::new(),
        attachment,
        MockThreadService::new(),
    );

    let req = authed_request(
        r#"mutation($sid: ID!, $aid: ID!) { deleteAttachment(input: { serverId: $sid, attachmentId: $aid }) }"#,
        actor,
    )
    .variables(Variables::from_json(serde_json::json!({
        "sid": server_id.to_string(),
        "aid": attachment_id.to_string(),
    })));

    let res = schema.execute(req).await;
    assert_no_errors(&res);
}
