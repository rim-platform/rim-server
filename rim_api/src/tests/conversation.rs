use super::*;

#[tokio::test]
async fn mutation_create_conversation_keeps_title_for_channel() {
    let actor = UserId::new();
    let server_id = ServerId::new();
    let conversation_id = ConversationId::new();

    let mut conv = MockConversationService::new();
    conv.expect_create_conversation().returning(move |cmd| {
        assert_eq!(cmd.title.as_deref(), Some("Launches"));
        Box::pin(async move {
            let mut dto = test_conversation_dto();
            dto.id = conversation_id;
            dto.server_id = server_id;
            dto.conversation_type = ConversationType::Channel;
            dto.title = Some("Launches".to_string());
            dto.member_ids = vec![actor];
            Ok(dto)
        })
    });

    let schema = schema_with_mocks(
        MockAuthService::new(),
        conv,
        MockMessageService::new(),
        MockMessageAttachmentService::new(),
        MockThreadService::new(),
    );

    let req = authed_request(
        r#"mutation($sid: ID!) { createConversation(input: { serverId: $sid, isChannel: true, title: "Launches" }) { id title conversationType } }"#,
        actor,
    )
    .variables(Variables::from_json(serde_json::json!({
        "sid": server_id.to_string(),
    })));

    let res = schema.execute(req).await;
    assert_no_errors(&res);
    let json = res.data.into_json().unwrap();
    assert_eq!(
        json["createConversation"]["title"],
        serde_json::json!("Launches")
    );
}

#[tokio::test]
async fn inbox_page_exposes_has_prev_without_has_next() {
    let actor = UserId::new();
    let server_id = ServerId::new();
    let conversation_id = ConversationId::new();
    let prev_conversation_id = ConversationId::new();
    let last_activity_at = chrono::Utc::now();

    let mut conv = MockConversationService::new();
    conv.expect_list_inbox_conversations().returning(move |_q| {
        Box::pin(async move {
            let mut dto = test_conversation_dto();
            dto.id = conversation_id;
            dto.server_id = server_id;
            dto.conversation_type = ConversationType::Direct;
            dto.member_ids = vec![actor];
            Ok(Page::new(
                vec![dto],
                None,
                Some(rim_services::conversation::dto::InboxPageCursor {
                    last_activity_at,
                    conversation_id: prev_conversation_id,
                }),
            ))
        })
    });

    let schema = schema_with_mocks(
        MockAuthService::new(),
        conv,
        MockMessageService::new(),
        MockMessageAttachmentService::new(),
        MockThreadService::new(),
    );

    let req = authed_request(
        r#"query($sid: ID!) { inboxConversations(serverId: $sid, dir: BEFORE, limit: 20) { pageInfo { hasNext hasPrev } next { conversationId } prev { conversationId } } }"#,
        actor,
    )
    .variables(Variables::from_json(serde_json::json!({
        "sid": server_id.to_string(),
    })));

    let res = schema.execute(req).await;
    assert_no_errors(&res);

    let json = res.data.into_json().unwrap();
    assert_eq!(
        json["inboxConversations"]["pageInfo"]["hasNext"],
        serde_json::json!(false)
    );
    assert_eq!(
        json["inboxConversations"]["pageInfo"]["hasPrev"],
        serde_json::json!(true)
    );
    assert_eq!(json["inboxConversations"]["next"], serde_json::Value::Null);
    assert_eq!(
        json["inboxConversations"]["prev"]["conversationId"],
        serde_json::json!(prev_conversation_id.to_string())
    );
}

#[tokio::test]
async fn channels_page_exact_fit_has_no_next_or_prev() {
    let actor = UserId::new();
    let server_id = ServerId::new();
    let conversation_id = ConversationId::new();

    let mut conv = MockConversationService::new();
    conv.expect_list_channels().returning(move |_q| {
        Box::pin(async move {
            let mut dto = test_conversation_dto();
            dto.id = conversation_id;
            dto.server_id = server_id;
            dto.conversation_type = ConversationType::Channel;
            dto.member_ids = vec![actor];
            Ok(Page::new(vec![dto], None, None))
        })
    });

    let schema = schema_with_mocks(
        MockAuthService::new(),
        conv,
        MockMessageService::new(),
        MockMessageAttachmentService::new(),
        MockThreadService::new(),
    );

    let req = authed_request(
        r#"query($sid: ID!) { channels(serverId: $sid, dir: AFTER, limit: 1) { pageInfo { hasNext hasPrev } next { conversationId } prev { conversationId } } }"#,
        actor,
    )
    .variables(Variables::from_json(serde_json::json!({
        "sid": server_id.to_string(),
    })));

    let res = schema.execute(req).await;
    assert_no_errors(&res);

    let json = res.data.into_json().unwrap();
    assert_eq!(
        json["channels"]["pageInfo"]["hasNext"],
        serde_json::json!(false)
    );
    assert_eq!(
        json["channels"]["pageInfo"]["hasPrev"],
        serde_json::json!(false)
    );
    assert_eq!(json["channels"]["next"], serde_json::Value::Null);
    assert_eq!(json["channels"]["prev"], serde_json::Value::Null);
}
