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
            ai_callable: false,
            nl_description: String::new(),
            intent_samples: vec![],
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
                ai_callable: false,
                nl_description: String::new(),
                intent_samples: vec![],
            },
        )
        .await
        .unwrap();
    assert_eq!(updated.title, "Quick Group 2");
    assert!(!updated.is_enabled);

    repo.delete_user_command(created.id).await.unwrap();
    assert!(repo.list_for_user(user.id).await.unwrap().is_empty());
}

/// A row written before macros had agent fields and asked arguments reads with the defaults:
/// not callable by the agent, no agent text and its steps as stored.
#[tokio::test]
async fn a_row_in_the_old_shape_reads_with_the_macro_defaults() {
    use sea_orm::prelude::Uuid;
    use sea_orm::{ConnectionTrait, DbBackend, Statement};

    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmUserCommandRepository::new(ctx.db.clone());
    let user = ctx.create_user("old_shape").await;
    let owner: Uuid = user.id.into();
    ctx.db
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO user_command (id, owner_user_id, command_key, title, description, steps_json) \
             VALUES ($1, $2, 'user.old', 'Old', 'saved before macros', \
             '[{\"command_id\": \"chat.mark_as_read\", \"args\": null}]'::jsonb)",
            vec![Uuid::now_v7().into(), owner.into()],
        ))
        .await
        .expect("insert an old-shape row");

    let stored = repo
        .get_by_key_for_user(user.id, "user.old")
        .await
        .unwrap()
        .expect("the old row is read");

    assert!(stored.is_enabled);
    assert!(!stored.ai_callable);
    assert_eq!(stored.nl_description, "");
    assert!(stored.intent_samples.is_empty());
    assert_eq!(
        stored.steps_json,
        serde_json::json!([{ "command_id": "chat.mark_as_read", "args": null }])
    );
}

/// The agent fields and a step's asked arguments are stored on create and replaced on update.
#[tokio::test]
async fn the_macro_fields_are_stored_and_updated() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmUserCommandRepository::new(ctx.db.clone());
    let user = ctx.create_user("macro_fields").await;
    let steps = serde_json::json!([
        { "command_id": "chat.set_topic", "args": null, "ask": ["topic"] }
    ]);

    let created = repo
        .create_user_command(NewUserCommand {
            owner_user_id: user.id,
            command_key: "user.weekly".to_string(),
            title: "Weekly".to_string(),
            description: String::new(),
            steps_json: steps.clone(),
            is_enabled: true,
            ai_callable: true,
            nl_description: "Start the weekly".to_string(),
            intent_samples: vec!["weekly".to_string(), "start the week".to_string()],
        })
        .await
        .unwrap();
    assert!(created.ai_callable);
    assert_eq!(created.nl_description, "Start the weekly");
    assert_eq!(created.intent_samples, vec!["weekly", "start the week"]);
    assert_eq!(created.steps_json, steps);

    let updated = repo
        .update_user_command(
            created.id,
            rim_core::command::model::UpdateUserCommand {
                title: "Weekly".to_string(),
                description: String::new(),
                steps_json: steps,
                is_enabled: true,
                ai_callable: false,
                nl_description: String::new(),
                intent_samples: vec![],
            },
        )
        .await
        .unwrap();
    let fetched = repo
        .get_by_key_for_user(user.id, "user.weekly")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fetched, updated);
    assert!(!fetched.ai_callable);
    assert!(fetched.intent_samples.is_empty());
}
