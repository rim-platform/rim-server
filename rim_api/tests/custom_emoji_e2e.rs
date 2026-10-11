//! Through-the-stack e2e tests for the custom emoji GraphQL contract: `createCustomEmoji`,
//! `deleteCustomEmoji` and `customEmojis`.
//!
//! Run with:
//! ```sh
//! cargo test -p rim_api --features e2e-tests -- --test-threads=1
//! ```
#![cfg(feature = "e2e-tests")]

mod support;

use std::path::PathBuf;

use base64::Engine;
use serde_json::{Value, json};

use rim_api::AppSchema;
use rim_core::server_feature::model::ServerFeature;
use rim_core::server_member::model::ServerMemberRole;
use rim_core::user::model::User;
use rim_services::auth::dto::RegistrationMode;

use support::TestCtx;
use support::graphql::{Caller, anonymous_request, exec_err_code, exec_ok};

const CREATE_EMOJI: &str = r#"
mutation Create($input: CreateCustomEmojiInput!) {
  emoji: createCustomEmoji(input: $input) { id serverId name imageUrl uploadedBy createdAt }
}
"#;

const DELETE_EMOJI: &str = r#"
mutation Delete($emojiId: ID!) { deleteCustomEmoji(emojiId: $emojiId) }
"#;

const CUSTOM_EMOJIS: &str = r#"
query Emojis($serverId: ID!) {
  emojis: customEmojis(serverId: $serverId) { id serverId name imageUrl uploadedBy createdAt }
}
"#;

/// Where the test provider's local storage keeps uploaded files.
const STORAGE_ROOT: &str = "/tmp/rim_api_e2e_test_storage";

/// The largest image the server accepts, in bytes.
const MAX_IMAGE_BYTES: usize = 256 * 1024;

struct Fixture {
    ctx: TestCtx,
    schema: AppSchema,
    server_id: String,
    other_server_id: String,
    alice: Caller,
    mona: Caller,
    bob: Caller,
    carol: Caller,
    gina: Caller,
    /// Owner of another workspace, not a member of this one.
    olaf: Caller,
}

async fn caller(ctx: &TestCtx, user: &User) -> Caller {
    Caller {
        user_id: user.id,
        session_id: ctx.create_session(user).await,
    }
}

/// alice (Owner), mona (Moderator), bob and carol (Member) and gina (Guest) in one workspace;
/// olaf owns another.
async fn setup() -> Fixture {
    let ctx = TestCtx::new().await;
    let server = ctx.create_server("emoji-e2e").await;
    let other = ctx.create_server("emoji-e2e-other").await;
    let alice_user = ctx.create_user("emoji-alice").await;
    let mona_user = ctx.create_user("emoji-mona").await;
    let bob_user = ctx.create_user("emoji-bob").await;
    let carol_user = ctx.create_user("emoji-carol").await;
    let gina_user = ctx.create_user("emoji-gina").await;
    let olaf_user = ctx.create_user("emoji-olaf").await;
    ctx.add_owner(&server, &alice_user).await;
    ctx.add_member(&server, &mona_user, ServerMemberRole::Moderator)
        .await;
    ctx.add_member(&server, &bob_user, ServerMemberRole::Member)
        .await;
    ctx.add_member(&server, &carol_user, ServerMemberRole::Member)
        .await;
    ctx.add_member(&server, &gina_user, ServerMemberRole::Guest)
        .await;
    ctx.add_owner(&other, &olaf_user).await;

    Fixture {
        schema: ctx.build_schema(RegistrationMode::InviteOnly).await,
        server_id: server.id.to_string(),
        other_server_id: other.id.to_string(),
        alice: caller(&ctx, &alice_user).await,
        mona: caller(&ctx, &mona_user).await,
        bob: caller(&ctx, &bob_user).await,
        carol: caller(&ctx, &carol_user).await,
        gina: caller(&ctx, &gina_user).await,
        olaf: caller(&ctx, &olaf_user).await,
        ctx,
    }
}

fn image(bytes: usize) -> String {
    base64::engine::general_purpose::STANDARD.encode(vec![0x89u8; bytes])
}

fn create_vars(server_id: &str, name: &str, image_base64: &str) -> Value {
    json!({ "input": { "serverId": server_id, "name": name, "imageBase64": image_base64 } })
}

impl Fixture {
    async fn create(&self, who: &Caller, name: &str) -> Value {
        let data = exec_ok(
            &self.schema,
            who.request(CREATE_EMOJI, create_vars(&self.server_id, name, &image(16))),
        )
        .await;
        data["emoji"].clone()
    }

    async fn create_code(&self, who: &Caller, name: &str, image_base64: &str) -> String {
        exec_err_code(
            &self.schema,
            who.request(
                CREATE_EMOJI,
                create_vars(&self.server_id, name, image_base64),
            ),
        )
        .await
    }

    async fn list(&self, who: &Caller) -> Vec<Value> {
        let data = exec_ok(
            &self.schema,
            who.request(CUSTOM_EMOJIS, json!({ "serverId": self.server_id })),
        )
        .await;
        data["emojis"].as_array().expect("emojis").clone()
    }

    async fn delete_code(&self, who: &Caller, emoji_id: &str) -> String {
        exec_err_code(
            &self.schema,
            who.request(DELETE_EMOJI, json!({ "emojiId": emoji_id })),
        )
        .await
    }

    async fn delete(&self, who: &Caller, emoji_id: &str) {
        let data = exec_ok(
            &self.schema,
            who.request(DELETE_EMOJI, json!({ "emojiId": emoji_id })),
        )
        .await;
        assert_eq!(data["deleteCustomEmoji"], json!(true));
    }
}

fn stored_file(emoji: &Value) -> PathBuf {
    PathBuf::from(STORAGE_ROOT).join(emoji["imageUrl"].as_str().expect("imageUrl"))
}

/// Every custom emoji root answers `UNAUTHENTICATED` without a session.
#[tokio::test]
async fn custom_emoji_roots_refuse_anonymous_callers() {
    let f = setup().await;
    let calls = [
        (CREATE_EMOJI, create_vars(&f.server_id, "party", &image(16))),
        (
            DELETE_EMOJI,
            json!({ "emojiId": uuid::Uuid::now_v7().to_string() }),
        ),
        (CUSTOM_EMOJIS, json!({ "serverId": f.server_id })),
    ];
    for (query, vars) in calls {
        let code = exec_err_code(&f.schema, anonymous_request(query, vars)).await;
        assert_eq!(code, "UNAUTHENTICATED", "{query}");
    }
}

/// A user outside the workspace can neither list, add nor delete its emoji.
#[tokio::test]
async fn custom_emoji_roots_refuse_non_members() {
    let f = setup().await;
    let emoji = f.create(&f.bob, "party").await;

    let code = f.create_code(&f.olaf, "intrude", &image(16)).await;
    assert_eq!(code, "FORBIDDEN");
    let code = exec_err_code(
        &f.schema,
        f.olaf
            .request(CUSTOM_EMOJIS, json!({ "serverId": f.server_id })),
    )
    .await;
    assert_eq!(code, "FORBIDDEN");
    let code = f
        .delete_code(&f.olaf, emoji["id"].as_str().expect("id"))
        .await;
    assert_eq!(code, "FORBIDDEN");
    assert_eq!(f.list(&f.alice).await, vec![emoji]);
}

/// Any member, a guest included, may add an emoji. The answer carries every field, the image is
/// stored under the workspace, and every member lists it.
#[tokio::test]
async fn create_custom_emoji_returns_the_emoji_and_lists_it_for_members() {
    let f = setup().await;
    let before = chrono::Utc::now();
    let emoji = f.create(&f.bob, "party_parrot").await;

    assert!(emoji["id"].is_string());
    assert_eq!(emoji["serverId"], json!(f.server_id));
    assert_eq!(emoji["name"], json!("party_parrot"));
    let image_url = emoji["imageUrl"].as_str().expect("imageUrl");
    assert!(
        image_url.starts_with(&format!("{}/emoji/", f.server_id)) && image_url.ends_with(".png"),
        "{image_url}"
    );
    assert_eq!(emoji["uploadedBy"], json!(f.bob.user_id.to_string()));
    let created_at: chrono::DateTime<chrono::Utc> = emoji["createdAt"]
        .as_str()
        .expect("createdAt")
        .parse()
        .expect("RFC 3339");
    assert!(created_at >= before - chrono::Duration::seconds(1));
    assert_eq!(
        std::fs::read(stored_file(&emoji)).expect("stored image"),
        vec![0x89u8; 16]
    );

    let guest_emoji = f.create(&f.gina, "wave").await;
    assert_eq!(guest_emoji["uploadedBy"], json!(f.gina.user_id.to_string()));

    for who in [&f.alice, &f.bob, &f.gina] {
        let listed = f.list(who).await;
        assert_eq!(listed.len(), 2);
        assert!(listed.contains(&emoji) && listed.contains(&guest_emoji));
    }
}

/// Names are one to sixty-four letters, digits or underscores; anything else answers
/// `VALIDATION` and stores nothing.
#[tokio::test]
async fn create_custom_emoji_validates_the_name() {
    let f = setup().await;
    let too_long = "a".repeat(65);
    for name in ["", too_long.as_str(), "my-emoji", "my emoji", "emoji!"] {
        let code = f.create_code(&f.bob, name, &image(16)).await;
        assert_eq!(code, "VALIDATION", "{name:?}");
    }
    assert!(f.list(&f.bob).await.is_empty());

    f.create(&f.bob, &"a".repeat(64)).await;
    f.create(&f.bob, "_Under_score9").await;
    assert_eq!(f.list(&f.bob).await.len(), 2);
}

/// The image must be valid base64 of one byte up to 256 KiB.
#[tokio::test]
async fn create_custom_emoji_validates_the_image() {
    let f = setup().await;
    let code = f
        .create_code(&f.bob, "big", &image(MAX_IMAGE_BYTES + 1))
        .await;
    assert_eq!(code, "VALIDATION");
    let code = f.create_code(&f.bob, "empty", "").await;
    assert_eq!(code, "VALIDATION");

    let res = f
        .schema
        .execute(f.bob.request(
            CREATE_EMOJI,
            create_vars(&f.server_id, "garbled", "not base64!"),
        ))
        .await;
    let err = res.errors.first().expect("an error");
    assert!(err.message.starts_with("invalid base64"), "{}", err.message);
    assert!(f.list(&f.bob).await.is_empty());

    let data = exec_ok(
        &f.schema,
        f.bob.request(
            CREATE_EMOJI,
            create_vars(&f.server_id, "largest", &image(MAX_IMAGE_BYTES)),
        ),
    )
    .await;
    assert_eq!(data["emoji"]["name"], json!("largest"));
}

/// A name is unique within a workspace (`CONFLICT`) but may repeat in another workspace.
#[tokio::test]
async fn create_custom_emoji_refuses_a_duplicate_name() {
    let f = setup().await;
    f.create(&f.bob, "party").await;
    let code = f.create_code(&f.carol, "party", &image(16)).await;
    assert_eq!(code, "CONFLICT");
    assert_eq!(f.list(&f.bob).await.len(), 1);

    exec_ok(
        &f.schema,
        f.olaf.request(
            CREATE_EMOJI,
            create_vars(&f.other_server_id, "party", &image(16)),
        ),
    )
    .await;
}

/// The uploader may delete their own emoji and the owner may delete anyone's; another member, a
/// moderator included, gets `FORBIDDEN`. Deleting removes the stored image; a deleted or unknown
/// id answers `NOT_FOUND`.
#[tokio::test]
async fn delete_custom_emoji_is_for_the_uploader_and_the_owner() {
    let f = setup().await;
    let bobs = f.create(&f.bob, "bobs").await;
    let bobs_id = bobs["id"].as_str().expect("id");
    let carols = f.create(&f.carol, "carols").await;
    let carols_id = carols["id"].as_str().expect("id");

    for who in [&f.mona, &f.carol, &f.gina] {
        let code = f.delete_code(who, bobs_id).await;
        assert_eq!(code, "FORBIDDEN");
    }
    assert!(stored_file(&bobs).exists());

    f.delete(&f.bob, bobs_id).await;
    assert!(!stored_file(&bobs).exists());
    f.delete(&f.alice, carols_id).await;
    assert!(f.list(&f.alice).await.is_empty());

    let code = f.delete_code(&f.bob, bobs_id).await;
    assert_eq!(code, "NOT_FOUND");
    let code = f
        .delete_code(&f.alice, &uuid::Uuid::now_v7().to_string())
        .await;
    assert_eq!(code, "NOT_FOUND");
}

/// With custom emoji turned off on the instance, every root answers `FEATURE_DISABLED` naming
/// the feature, before authentication, and nothing changes.
#[tokio::test]
async fn custom_emoji_roots_answer_feature_disabled_when_the_instance_turns_it_off() {
    let f = setup().await;
    let emoji = f.create(&f.bob, "party").await;
    let emoji_id = emoji["id"].as_str().expect("id");
    let gated = f
        .ctx
        .build_schema_with_disabled(&[ServerFeature::CustomEmoji]);

    let calls = [
        (CREATE_EMOJI, create_vars(&f.server_id, "more", &image(16))),
        (DELETE_EMOJI, json!({ "emojiId": emoji_id })),
        (CUSTOM_EMOJIS, json!({ "serverId": f.server_id })),
    ];
    for (query, vars) in calls {
        for request in [
            f.bob.request(query, vars.clone()),
            anonymous_request(query, vars.clone()),
        ] {
            let res = gated.execute(request).await;
            let ext = res
                .errors
                .first()
                .and_then(|e| e.extensions.as_ref())
                .unwrap_or_else(|| panic!("{query}: expected an error with extensions"));
            assert_eq!(
                ext.get("code"),
                Some(&async_graphql::Value::from("FEATURE_DISABLED")),
                "{query}"
            );
            assert_eq!(
                ext.get("feature"),
                Some(&async_graphql::Value::from("custom_emoji")),
                "{query}"
            );
        }
    }
    assert_eq!(f.list(&f.bob).await, vec![emoji]);
}
