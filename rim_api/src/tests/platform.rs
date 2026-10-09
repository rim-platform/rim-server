use super::*;

#[tokio::test]
async fn capabilities_query_returns_the_catalogue() {
    let actor = UserId::new();

    let mut capability = MockCapabilityService::new();
    capability.expect_list_capabilities().returning(|_actor| {
        Ok(vec![rim_services::capability::dto::CapabilityDto {
            id: "chat.write".to_string(),
            title: "Write Chat".to_string(),
            description: "desc".to_string(),
        }])
    });

    let schema = schema_with_capability_mock(capability);

    let req = authed_request(r#"query { capabilities { id } }"#, actor);

    let res = schema.execute(req).await;
    assert_no_errors(&res);

    let json = res.data.into_json().unwrap();
    assert_eq!(
        json["capabilities"][0]["id"],
        serde_json::json!("chat.write")
    );
}
