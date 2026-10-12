use rim_services::account_deletion::dto::AccountDeletionScheduleDto;
use rim_services::account_deletion::service::{AccountDeletionService, MockAccountDeletionService};

use super::*;

const REQUEST: &str =
    r#"mutation { requestAccountDeletion(input: { confirmUsername: "alice" }) { scheduledFor } }"#;

/// `request` answered by `service` in place of the schema's account deletion service.
fn with_service(request: Request, service: MockAccountDeletionService) -> Request {
    let service: Arc<dyn AccountDeletionService> = Arc::new(service);
    request.data(service)
}

fn schema() -> crate::AppSchema {
    schema_with_user_mock(MockUserService::new())
}

#[tokio::test]
async fn request_account_deletion_passes_the_typed_username_and_this_session() {
    let actor = UserId::new();
    let session_id = SessionId::new();
    let at = chrono::DateTime::parse_from_rfc3339("2026-10-26T12:00:00Z")
        .expect("time")
        .with_timezone(&chrono::Utc);
    let mut service = MockAccountDeletionService::new();
    service
        .expect_request_deletion()
        .withf(move |cmd| {
            cmd.actor.user_id == actor
                && cmd.session_id == session_id
                && cmd.confirm_username == "alice"
        })
        .times(1)
        .returning(move |_| {
            Box::pin(async move { Ok(AccountDeletionScheduleDto { scheduled_for: at }) })
        });

    let request = Request::new(REQUEST).data(AuthActorCtx::new(Some(actor), Some(session_id)));
    let res = schema().execute(with_service(request, service)).await;
    assert_no_errors(&res);
    let json = res.data.into_json().expect("json");
    assert_eq!(
        json["requestAccountDeletion"]["scheduledFor"],
        serde_json::json!("2026-10-26T12:00:00+00:00")
    );
}

#[tokio::test]
async fn request_account_deletion_reports_a_refusal_with_its_code() {
    let mut service = MockAccountDeletionService::new();
    service.expect_request_deletion().returning(|_| {
        Box::pin(async {
            Err(ServiceError::Validation(
                "the username does not match this account".to_string(),
            ))
        })
    });
    let res = schema()
        .execute(with_service(
            authed_request(REQUEST, UserId::new()),
            service,
        ))
        .await;
    assert_eq!(res.errors.len(), 1);
    let code = res.errors[0]
        .extensions
        .as_ref()
        .and_then(|e| e.get("code"))
        .cloned();
    assert_eq!(code, Some(Value::from("VALIDATION")));
}

#[tokio::test]
async fn the_account_mutations_require_a_session() {
    for query in [REQUEST, "mutation { cancelAccountDeletion }"] {
        let mut service = MockAccountDeletionService::new();
        service.expect_request_deletion().never();
        service.expect_cancel_deletion().never();
        let res = schema()
            .execute(with_service(Request::new(query), service))
            .await;
        assert!(
            res.errors[0].message.contains("unauthenticated"),
            "{query}: {:?}",
            res.errors
        );
    }
}

#[tokio::test]
async fn cancel_account_deletion_says_whether_one_was_pending() {
    let actor = UserId::new();
    let mut service = MockAccountDeletionService::new();
    service
        .expect_cancel_deletion()
        .withf(move |a| a.user_id == actor)
        .times(1)
        .returning(|_| Box::pin(async { Ok(true) }));
    let res = schema()
        .execute(with_service(
            authed_request("mutation { cancelAccountDeletion }", actor),
            service,
        ))
        .await;
    assert_no_errors(&res);
    assert_eq!(
        res.data.into_json().expect("json")["cancelAccountDeletion"],
        serde_json::json!(true)
    );
}

#[test]
fn people_from_other_servers_delete_their_account_at_their_relay() {
    for (query, name) in [
        (REQUEST, "requestAccountDeletion"),
        (
            "mutation { cancelAccountDeletion }",
            "cancelAccountDeletion",
        ),
    ] {
        assert_eq!(
            crate::federation::refused_account_mutation(query),
            Some(name.to_string())
        );
    }
}

#[tokio::test]
async fn me_shows_the_pending_deletion() {
    let actor = UserId::new();
    let at = chrono::DateTime::parse_from_rfc3339("2026-10-26T12:00:00Z")
        .expect("time")
        .with_timezone(&chrono::Utc);
    let mut user = MockUserService::new();
    user.expect_get_me().returning(move |_| {
        Box::pin(async move {
            Ok(rim_services::user::dto::UserDto {
                deletion_scheduled_for: Some(at),
                ..test_user_dto()
            })
        })
    });
    let res = schema_with_user_mock(user)
        .execute(authed_request(
            "query { me { deletionScheduledFor } }",
            actor,
        ))
        .await;
    assert_no_errors(&res);
    assert_eq!(
        res.data.into_json().expect("json")["me"]["deletionScheduledFor"],
        serde_json::json!("2026-10-26T12:00:00+00:00")
    );
}
