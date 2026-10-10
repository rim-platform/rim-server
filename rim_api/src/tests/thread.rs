use super::*;

#[tokio::test]
async fn thread_query_calls_service_when_authed() {
    let actor = UserId::new();
    let server_id = ServerId::new();
    let conversation_id = ConversationId::new();
    let thread_id = MessageThreadId::new();

    let mut thread = MockThreadService::new();
    thread.expect_get_thread().returning(move |_q| {
        Box::pin(async move {
            let mut dto = test_thread_dto();
            dto.id = thread_id;
            dto.server_id = server_id;
            dto.conversation_id = conversation_id;
            Ok(dto)
        })
    });

    let schema = schema_with_mocks(
        MockAuthService::new(),
        MockConversationService::new(),
        MockMessageService::new(),
        MockMessageAttachmentService::new(),
        thread,
    );

    let req = authed_request(
        r#"query($sid: ID!, $cid: ID!, $tid: ID!) { thread(serverId: $sid, conversationId: $cid, threadId: $tid) { id } }"#,
        actor,
    )
    .variables(Variables::from_json(serde_json::json!({
        "sid": server_id.to_string(),
        "cid": conversation_id.to_string(),
        "tid": thread_id.to_string(),
    })));

    let res = schema.execute(req).await;
    assert!(res.errors.is_empty());
    assert!(res.data != Value::Null);
}

#[tokio::test]
async fn thread_by_root_message_query_calls_service_when_authed() {
    let actor = UserId::new();
    let server_id = ServerId::new();
    let conversation_id = ConversationId::new();
    let root_message_id = MessageId::new();
    let thread_id = MessageThreadId::new();

    let mut thread = MockThreadService::new();
    thread
        .expect_get_thread_by_root_message()
        .returning(move |_q| {
            Box::pin(async move {
                let mut dto = test_thread_dto();
                dto.id = thread_id;
                dto.server_id = server_id;
                dto.conversation_id = conversation_id;
                dto.root_message_id = Some(root_message_id);
                dto.root_author_id = Some(actor);
                dto.root_text = Some("root".to_string());
                Ok(Some(dto))
            })
        });

    let schema = schema_with_mocks(
        MockAuthService::new(),
        MockConversationService::new(),
        MockMessageService::new(),
        MockMessageAttachmentService::new(),
        thread,
    );

    let req = authed_request(
        r#"query($sid: ID!, $cid: ID!, $mid: ID!) { threadByRootMessage(serverId: $sid, conversationId: $cid, rootMessageId: $mid) { id } }"#,
        actor,
    )
    .variables(Variables::from_json(serde_json::json!({
        "sid": server_id.to_string(),
        "cid": conversation_id.to_string(),
        "mid": root_message_id.to_string(),
    })));

    let res = schema.execute(req).await;
    assert!(res.errors.is_empty());
    assert!(res.data != Value::Null);
}

#[tokio::test]
async fn thread_messages_page_exposes_both_directions() {
    let actor = UserId::new();
    let server_id = ServerId::new();
    let conversation_id = ConversationId::new();
    let thread_id = MessageThreadId::new();
    let message_id = MessageId::new();
    let next_message_id = MessageId::new();
    let prev_message_id = MessageId::new();
    let sent_at = chrono::Utc::now();

    let mut msg = MockMessageService::new();
    msg.expect_list_thread_messages().returning(move |_q| {
        Box::pin(async move {
            let mut dto = test_message_dto();
            dto.id = message_id;
            dto.server_id = server_id;
            dto.conversation_id = conversation_id;
            dto.thread_id = Some(thread_id);
            dto.author_id = actor;
            dto.text = Some("reply".to_string());
            dto.sent_at = sent_at;
            Ok(Page::new(
                vec![dto],
                Some(rim_services::message::dto::MessagePageCursor {
                    sent_at,
                    message_id: next_message_id,
                }),
                Some(rim_services::message::dto::MessagePageCursor {
                    sent_at,
                    message_id: prev_message_id,
                }),
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
        r#"query($sid: ID!, $cid: ID!, $tid: ID!) { threadMessages(serverId: $sid, conversationId: $cid, threadId: $tid, dir: AFTER, limit: 20, includeDeleted: false) { pageInfo { hasNext hasPrev } next { messageId } prev { messageId } } }"#,
        actor,
    )
    .variables(Variables::from_json(serde_json::json!({
        "sid": server_id.to_string(),
        "cid": conversation_id.to_string(),
        "tid": thread_id.to_string(),
    })));

    let res = schema.execute(req).await;
    assert_no_errors(&res);

    let json = res.data.into_json().unwrap();
    assert_eq!(
        json["threadMessages"]["pageInfo"]["hasNext"],
        serde_json::json!(true)
    );
    assert_eq!(
        json["threadMessages"]["pageInfo"]["hasPrev"],
        serde_json::json!(true)
    );
    assert_eq!(
        json["threadMessages"]["next"]["messageId"],
        serde_json::json!(next_message_id.to_string())
    );
    assert_eq!(
        json["threadMessages"]["prev"]["messageId"],
        serde_json::json!(prev_message_id.to_string())
    );
}
