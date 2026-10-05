use super::*;

#[tokio::test]
async fn list_audit_log_query_requires_auth() {
    let schema = schema_with_audit_mock(rim_services::audit::service::MockAuditLogService::new());
    let res = schema
        .execute(Request::new(
            "{ listAuditLog { items { id } pageInfo { hasNextPage } } }",
        ))
        .await;
    assert!(
        !res.errors.is_empty(),
        "unauthenticated listAuditLog must return errors"
    );
}

#[tokio::test]
async fn list_audit_log_query_returns_page() {
    use rim_services::audit::dto::{
        AuditLogEntryDto, AuditLogPage, AuditLogPageInfo, AuditOutcome,
    };

    let actor = UserId::new();
    let entry_id = uuid::Uuid::new_v4();
    let server_id = ServerId::new();

    let mut audit_mock = rim_services::audit::service::MockAuditLogService::new();
    audit_mock.expect_list_entries().returning(move |_, _| {
        Box::pin(async move {
            Ok(AuditLogPage {
                items: vec![AuditLogEntryDto {
                    id: entry_id,
                    actor_user_id: entry_id,
                    server_id: server_id.into(),
                    command_id: "/ping".to_string(),
                    initiator: "palette".to_string(),
                    args_redacted: serde_json::Value::Null,
                    sensitivity: "low".to_string(),
                    dangerous: false,
                    confirmation_kind: "none".to_string(),
                    typed_verb: None,
                    outcome: AuditOutcome::Success,
                    error_kind: None,
                    capability_check_outcome: None,
                    confirmation_outcome: None,
                    parent_audit_id: None,
                    created_at: chrono::Utc::now(),
                }],
                next_cursor: None,
                page_info: AuditLogPageInfo {
                    has_next_page: false,
                    has_prev_page: false,
                },
            })
        })
    });

    let schema = schema_with_audit_mock(audit_mock);
    let req = authed_request(
        "{ listAuditLog { items { id commandId outcome } pageInfo { hasNextPage hasPrevPage } } }",
        actor,
    );
    let res = schema.execute(req).await;
    assert_no_errors(&res);
    let data = res.data.into_json().unwrap();
    let items = &data["listAuditLog"]["items"];
    assert_eq!(items.as_array().unwrap().len(), 1);
    assert_eq!(items[0]["commandId"], "/ping");
    assert_eq!(items[0]["outcome"], "SUCCESS");
    assert_eq!(data["listAuditLog"]["pageInfo"]["hasNextPage"], false);
}
