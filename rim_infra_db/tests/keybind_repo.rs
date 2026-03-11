#![cfg(feature = "db-tests")]

mod support;

use rim_core::keybind::model::{
    KeybindContext, KeybindScope, KeybindSubject, NewKeybind, UpdateKeybind,
};
use rim_core::keybind::repository::KeybindRepository;
use rim_core::user::model::NewUser;
use rim_core::user::repository::UserRepository;
use rim_infra_db::sea_orm_impl::keybind::SeaOrmKeybindRepository;
use rim_infra_db::sea_orm_impl::user::SeaOrmUserRepository;

#[tokio::test]
async fn create_update_and_delete_keybind() {
    let Some(conn) = support::db_conn_or_skip().await else {
        return;
    };
    support::reset_db(&conn).await;

    let user_repo = SeaOrmUserRepository::new(conn.clone());
    let repo = SeaOrmKeybindRepository::new(conn.clone());

    let user = user_repo
        .create_user(NewUser {
            username: "keybind-user".to_string(),
            password: "password".to_string(),
        })
        .await
        .unwrap();

    let created = repo
        .create_keybind(NewKeybind {
            scope: KeybindScope::User,
            subject: KeybindSubject::User(user.id),
            keystroke: "Ctrl+R".to_string(),
            command_id: "workspace.cycle_recent_activity".to_string(),
            context: KeybindContext::Global,
            preset_args: None,
            is_enabled: true,
        })
        .await
        .unwrap();

    let fetched = repo
        .get_by_binding(
            KeybindScope::User,
            KeybindSubject::User(user.id),
            KeybindContext::Global,
            "Ctrl+R",
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fetched.id, created.id);

    let updated = repo
        .update_keybind(
            created.id,
            UpdateKeybind {
                keystroke: "Ctrl+Shift+R".to_string(),
                command_id: "workspace.cycle_recent_activity".to_string(),
                context: KeybindContext::Global,
                preset_args: Some(serde_json::json!({ "direction": "next" })),
                is_enabled: false,
            },
        )
        .await
        .unwrap();
    assert_eq!(updated.keystroke, "Ctrl+Shift+R");
    assert!(!updated.is_enabled);

    repo.delete_keybind(created.id).await.unwrap();
    assert!(repo.get_by_id(created.id).await.unwrap().is_none());
}
