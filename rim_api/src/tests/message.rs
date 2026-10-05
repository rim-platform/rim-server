use super::*;

#[tokio::test]
async fn message_query_exposes_root_thread_id() {
    let actor = UserId::new();
    let server_id = ServerId::new();
    let conversation_id = ConversationId::new();
    let message_id = MessageId::new();
    let root_thread_id = MessageThreadId::new();

    let mut msg = MockMessageService::new();
    msg.expect_get_message().returning(move |_q| {
        Box::pin(async move {
            let mut dto = test_message_dto();
            dto.id = message_id;
            dto.server_id = server_id;
            dto.conversation_id = conversation_id;
            dto.root_thread_id = Some(root_thread_id);
            dto.author_id = actor;
            dto.text = Some("root".to_string());
            Ok(dto)
        })
    });

    let schema = schema_with_mocks(
        MockAuthService::new(),
        MockConversationService::new(),
        msg,
        MockMessageAttachmentService::new(),
        MockThreadService::new(),
    );

    let req = authed_request(
        r#"query($sid: ID!, $cid: ID!, $mid: ID!) { message(serverId: $sid, conversationId: $cid, messageId: $mid) { id rootThreadId } }"#,
        actor,
    )
    .variables(Variables::from_json(serde_json::json!({
        "sid": server_id.to_string(),
        "cid": conversation_id.to_string(),
        "mid": message_id.to_string(),
    })));

    let res = schema.execute(req).await;
    assert_no_errors(&res);
}

#[tokio::test]
async fn message_query_exposes_attachments() {
    let actor = UserId::new();
    let server_id = ServerId::new();
    let conversation_id = ConversationId::new();
    let message_id = MessageId::new();
    let attachment_id = rim_core::message_attachment::model::MessageAttachmentId::new();

    let mut msg = MockMessageService::new();
    msg.expect_get_message().returning(move |_q| {
        Box::pin(async move {
            let mut att = test_attachment_dto();
            att.id = attachment_id;

            let mut dto = test_message_dto();
            dto.id = message_id;
            dto.server_id = server_id;
            dto.conversation_id = conversation_id;
            dto.author_id = actor;
            dto.text = Some("root".to_string());
            dto.has_attachments = true;
            dto.attachments = vec![att];
            Ok(dto)
        })
    });

    let schema = schema_with_mocks(
        MockAuthService::new(),
        MockConversationService::new(),
        msg,
        MockMessageAttachmentService::new(),
        MockThreadService::new(),
    );

    let req = authed_request(
        r#"query($sid: ID!, $cid: ID!, $mid: ID!) { message(serverId: $sid, conversationId: $cid, messageId: $mid) { id hasAttachments attachments { id filename sizeBytes } } }"#,
        actor,
    )
    .variables(Variables::from_json(serde_json::json!({
        "sid": server_id.to_string(),
        "cid": conversation_id.to_string(),
        "mid": message_id.to_string(),
    })));

    let res = schema.execute(req).await;
    assert_no_errors(&res);
    let json = res.data.into_json().unwrap();
    assert_eq!(json["message"]["hasAttachments"], serde_json::json!(true));
    assert_eq!(
        json["message"]["attachments"][0]["id"],
        serde_json::json!(attachment_id.to_string())
    );
}

#[tokio::test]
async fn message_reference_query_returns_permalink_data() {
    let actor = UserId::new();
    let server_id = ServerId::new();
    let conversation_id = ConversationId::new();
    let message_id = MessageId::new();

    let mut msg = MockMessageService::new();
    msg.expect_get_message_reference().returning(move |_q| {
        Box::pin(async move {
            Ok(rim_services::message::dto::MessageReferenceDto {
                server_id,
                conversation_id,
                message_id,
                thread_id: None,
                permalink_path: format!(
                    "/workspaces/{}/conversations/{}/messages/{}",
                    server_id, conversation_id, message_id
                ),
                anchor: format!("message-{message_id}"),
            })
        })
    });

    let schema = schema_with_mocks(
        MockAuthService::new(),
        MockConversationService::new(),
        msg,
        MockMessageAttachmentService::new(),
        MockThreadService::new(),
    );

    let req = authed_request(
        r#"query($sid: ID!, $cid: ID!, $mid: ID!) { messageReference(serverId: $sid, conversationId: $cid, messageId: $mid) { messageId permalinkPath anchor } }"#,
        actor,
    )
    .variables(Variables::from_json(serde_json::json!({
        "sid": server_id.to_string(),
        "cid": conversation_id.to_string(),
        "mid": message_id.to_string(),
    })));

    let res = schema.execute(req).await;
    assert_no_errors(&res);
}

#[tokio::test]
async fn search_messages_query_returns_matches() {
    let actor = UserId::new();
    let server_id = ServerId::new();
    let conversation_id = ConversationId::new();
    let message_id = MessageId::new();

    let mut msg = MockMessageService::new();
    msg.expect_search_messages().returning(move |_q| {
        Box::pin(async move {
            let mut dto = test_message_dto();
            dto.id = message_id;
            dto.server_id = server_id;
            dto.conversation_id = conversation_id;
            dto.author_id = actor;
            dto.text = Some("deploy failed".to_string());
            let hit = rim_services::message::dto::MessageSearchHitDto {
                message: dto,
                rank: 0.5,
                headline: Some("<mark>deploy</mark> failed".to_string()),
            };
            Ok(Page::new(vec![hit], None, None))
        })
    });

    let mut conv = MockConversationService::new();
    conv.expect_get_conversations_by_ids()
        .returning(move |_ids| {
            Box::pin(async move {
                Ok(vec![rim_services::conversation::dto::ConversationDto {
                    id: conversation_id,
                    server_id,
                    conversation_type: rim_core::conversation::model::ConversationType::Channel,
                    title: Some("general".to_string()),
                    topic: None,
                    member_ids: vec![],
                }])
            })
        });

    let schema = schema_with_mocks(
        MockAuthService::new(),
        conv,
        msg,
        MockMessageAttachmentService::new(),
        MockThreadService::new(),
    );

    let req = authed_request(
        r#"query($sid: ID!, $cid: ID!) {
                searchMessages(input: { serverId: $sid, conversationId: $cid, query: "deploy", limit: 10 }) {
                    items { id text conversationTitle conversationType rank headline }
                    pageInfo { hasNext hasPrev }
                }
            }"#,
        actor,
    )
    .variables(Variables::from_json(serde_json::json!({
        "sid": server_id.to_string(),
        "cid": conversation_id.to_string(),
    })));

    let res = schema.execute(req).await;
    assert_no_errors(&res);
}

#[tokio::test]
async fn messages_page_exposes_has_next_without_has_prev() {
    let actor = UserId::new();
    let server_id = ServerId::new();
    let conversation_id = ConversationId::new();
    let message_id = MessageId::new();
    let next_message_id = MessageId::new();
    let sent_at = chrono::Utc::now();

    let mut msg = MockMessageService::new();
    msg.expect_list_messages().returning(move |_q| {
        Box::pin(async move {
            let mut dto = test_message_dto();
            dto.id = message_id;
            dto.server_id = server_id;
            dto.conversation_id = conversation_id;
            dto.author_id = actor;
            dto.text = Some("hello".to_string());
            dto.sent_at = sent_at;
            Ok(Page::new(
                vec![dto],
                Some(rim_services::message::dto::MessagePageCursor {
                    sent_at,
                    message_id: next_message_id,
                }),
                None,
            ))
        })
    });

    let schema = schema_with_mocks(
        MockAuthService::new(),
        MockConversationService::new(),
        msg,
        MockMessageAttachmentService::new(),
        MockThreadService::new(),
    );

    let req = authed_request(
        r#"query($sid: ID!, $cid: ID!) { messages(serverId: $sid, conversationId: $cid, dir: AFTER, limit: 20, includeDeleted: false) { pageInfo { hasNext hasPrev } next { messageId } prev { messageId } } }"#,
        actor,
    )
    .variables(Variables::from_json(serde_json::json!({
        "sid": server_id.to_string(),
        "cid": conversation_id.to_string(),
    })));

    let res = schema.execute(req).await;
    assert_no_errors(&res);

    let json = res.data.into_json().unwrap();
    assert_eq!(
        json["messages"]["pageInfo"]["hasNext"],
        serde_json::json!(true)
    );
    assert_eq!(
        json["messages"]["pageInfo"]["hasPrev"],
        serde_json::json!(false)
    );
    assert_eq!(
        json["messages"]["next"]["messageId"],
        serde_json::json!(next_message_id.to_string())
    );
    assert_eq!(json["messages"]["prev"], serde_json::Value::Null);
}

// ── sendMessage mutation ─────────────────────────────────────────────

#[tokio::test]
async fn send_message_mutation_creates_message() {
    let actor = UserId::new();
    let server_id = ServerId::new();
    let conversation_id = ConversationId::new();
    let message_id = MessageId::new();

    let mut msg = MockMessageService::new();
    msg.expect_send_message().returning(move |_cmd| {
        Box::pin(async move {
            let mut dto = test_message_dto();
            dto.id = message_id;
            dto.server_id = server_id;
            dto.conversation_id = conversation_id;
            dto.author_id = actor;
            dto.text = Some("hello world".to_string());
            Ok(dto)
        })
    });

    let schema = schema_with_mocks(
        MockAuthService::new(),
        MockConversationService::new(),
        msg,
        MockMessageAttachmentService::new(),
        MockThreadService::new(),
    );

    let req = authed_request(
        r#"mutation($sid: ID!, $cid: ID!) { sendMessage(input: { serverId: $sid, conversationId: $cid, text: "hello world" }) { id text authorId } }"#,
        actor,
    )
    .variables(Variables::from_json(serde_json::json!({
        "sid": server_id.to_string(),
        "cid": conversation_id.to_string(),
    })));

    let res = schema.execute(req).await;
    assert_no_errors(&res);

    let json = res.data.into_json().unwrap();
    assert_eq!(
        json["sendMessage"]["id"],
        serde_json::json!(message_id.to_string())
    );
    assert_eq!(
        json["sendMessage"]["text"],
        serde_json::json!("hello world")
    );
    assert_eq!(
        json["sendMessage"]["authorId"],
        serde_json::json!(actor.to_string())
    );
}

// ── editMessage mutation ─────────────────────────────────────────────

#[tokio::test]
async fn edit_message_mutation_updates_text() {
    let actor = UserId::new();
    let server_id = ServerId::new();
    let conversation_id = ConversationId::new();
    let message_id = MessageId::new();

    let mut msg = MockMessageService::new();
    msg.expect_edit_message().returning(move |_cmd| {
        Box::pin(async move {
            let mut dto = test_message_dto();
            dto.id = message_id;
            dto.server_id = server_id;
            dto.conversation_id = conversation_id;
            dto.author_id = actor;
            dto.text = Some("updated text".to_string());
            Ok(dto)
        })
    });

    let schema = schema_with_mocks(
        MockAuthService::new(),
        MockConversationService::new(),
        msg,
        MockMessageAttachmentService::new(),
        MockThreadService::new(),
    );

    let req = authed_request(
        r#"mutation($sid: ID!, $cid: ID!, $mid: ID!) { editMessage(input: { serverId: $sid, conversationId: $cid, messageId: $mid, text: "updated text" }) { id text } }"#,
        actor,
    )
    .variables(Variables::from_json(serde_json::json!({
        "sid": server_id.to_string(),
        "cid": conversation_id.to_string(),
        "mid": message_id.to_string(),
    })));

    let res = schema.execute(req).await;
    assert_no_errors(&res);

    let json = res.data.into_json().unwrap();
    assert_eq!(
        json["editMessage"]["id"],
        serde_json::json!(message_id.to_string())
    );
    assert_eq!(
        json["editMessage"]["text"],
        serde_json::json!("updated text")
    );
}

// ── deleteMessage mutation ───────────────────────────────────────────

#[tokio::test]
async fn delete_message_mutation_soft_deletes() {
    let actor = UserId::new();
    let server_id = ServerId::new();
    let conversation_id = ConversationId::new();
    let message_id = MessageId::new();

    let mut msg = MockMessageService::new();
    msg.expect_delete_message()
        .returning(|_cmd| Box::pin(async { Ok(()) }));

    let schema = schema_with_mocks(
        MockAuthService::new(),
        MockConversationService::new(),
        msg,
        MockMessageAttachmentService::new(),
        MockThreadService::new(),
    );

    let req = authed_request(
        r#"mutation($sid: ID!, $cid: ID!, $mid: ID!) { deleteMessage(input: { serverId: $sid, conversationId: $cid, messageId: $mid }) }"#,
        actor,
    )
    .variables(Variables::from_json(serde_json::json!({
        "sid": server_id.to_string(),
        "cid": conversation_id.to_string(),
        "mid": message_id.to_string(),
    })));

    let res = schema.execute(req).await;
    assert_no_errors(&res);
}
