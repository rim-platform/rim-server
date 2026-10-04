#![cfg(feature = "db-tests")]

mod support;

use rim_core::command::model::NewUserCommand;
use rim_core::command::repository::UserCommandRepository;
use rim_infra_db::sea_orm_impl::user_command::SeaOrmUserCommandRepository;

#[tokio::test]
async fn create_and_list_user_commands() {
    let ctx = support::TestCtx::new().await;

    let repo = SeaOrmUserCommandRepository::new(ctx.db.clone());
    let user = ctx.create_user("tester").await;

    let created = repo
        .create_user_command(NewUserCommand {
            owner_user_id: user.id,
            command_key: "user.quick_group".to_string(),
            title: "Quick Group".to_string(),
            description: "Create a quick group".to_string(),
            steps_json: serde_json::json!([
                { "command_id": "chat.create_group", "args": { "server_id": "x" } }
            ]),
            is_enabled: true,
        })
        .await
        .unwrap();

    let fetched = repo
        .get_by_key_for_user(user.id, "user.quick_group")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fetched.id, created.id);

    let listed = repo.list_for_user(user.id).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].command_key, "user.quick_group");

    let updated = repo
        .update_user_command(
            created.id,
            rim_core::command::model::UpdateUserCommand {
                title: "Quick Group 2".to_string(),
                description: "updated".to_string(),
                steps_json: serde_json::json!([
                    { "command_id": "chat.post_message", "args": { "text": "hello" } }
                ]),
                is_enabled: false,
            },
        )
        .await
        .unwrap();
    assert_eq!(updated.title, "Quick Group 2");
    assert!(!updated.is_enabled);

    repo.delete_user_command(created.id).await.unwrap();
    assert!(repo.get_by_id(created.id).await.unwrap().is_none());
}
