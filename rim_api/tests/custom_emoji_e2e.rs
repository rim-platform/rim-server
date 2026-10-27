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
use sea_orm::EntityTrait;
use serde_json::{Value, json};

use rim_api::AppSchema;
use rim_core::custom_emoji::model::CustomEmojiId;
use rim_services::ServiceError;
use rim_services::auth::dto::RegistrationMode;
use rim_services::custom_emoji::command::CustomEmojiImageQuery;
use rim_services::{Actor, ServiceResult};

use support::graphql::{
    Caller, anonymous_request, assert_each_refused, assert_each_refused_as, exec_err_code, exec_ok,
};
use support::roles::{Roles, create_channel};
use support::{TestCtx, unknown_id};

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

const SEND_MESSAGE: &str = r#"
mutation Send($input: SendMessageInput!) { msg: sendMessage(input: $input) { id } }
"#;

const TOGGLE_REACTION: &str = r#"
mutation React($input: ToggleReactionInput!) { toggleReaction(input: $input) { emoji count } }
"#;

/// Where the test provider's local storage keeps uploaded files.
const STORAGE_ROOT: &str = "/tmp/rim_api_e2e_test_storage";

/// The largest image the server accepts, in bytes.
const MAX_IMAGE_BYTES: usize = 256 * 1024;

struct Fixture {
    schema: AppSchema,
    ws: Roles,
    ctx: TestCtx,
}

async fn setup() -> Fixture {
    let ctx = TestCtx::new().await;
    Fixture {
        ws: ctx.roles("emoji").await,
        schema: ctx.build_schema(RegistrationMode::InviteOnly).await,
        ctx,
    }
}

/// `bytes` bytes of image data, base64-encoded.
fn image(bytes: usize) -> String {
    base64::engine::general_purpose::STANDARD.encode(vec![0x89u8; bytes])
}

fn create_vars(server_id: &str, name: &str, image_base64: &str) -> Value {
    json!({ "input": { "serverId": server_id, "name": name, "imageBase64": image_base64 } })
}

fn delete_vars(emoji_id: &str) -> Value {
    json!({ "emojiId": emoji_id })
}

fn id(emoji: &Value) -> &str {
    emoji["id"].as_str().expect("emoji id")
}

impl Fixture {
    /// Where the emoji's image is stored: its row's storage key under the storage root.
    async fn stored_file(&self, emoji: &Value) -> PathBuf {
        let emoji_id: uuid::Uuid = id(emoji).parse().expect("emoji uuid");
        let row = entity::custom_emoji::Entity::find_by_id(emoji_id)
            .one(&self.ctx.conn)
            .await
            .expect("emoji row query")
            .expect("emoji row");
        PathBuf::from(STORAGE_ROOT).join(row.image_url)
    }

    /// The emoji's image as `who` reads it through the service.
    async fn image(&self, who: &Caller, emoji_id: &str) -> ServiceResult<Vec<u8>> {
        let emoji_id = CustomEmojiId::from(emoji_id.parse::<uuid::Uuid>().expect("emoji uuid"));
        self.ctx
            .build_provider()
            .custom_emoji_service()
            .image(CustomEmojiImageQuery {
                actor: Actor::new(who.user_id),
                emoji_id,
            })
            .await
    }

    fn create_request(
        &self,
        who: &Caller,
        name: &str,
        image_base64: &str,
    ) -> async_graphql::Request {
        who.request(
            CREATE_EMOJI,
            create_vars(&self.ws.server_id, name, image_base64),
        )
    }

    /// Add a small emoji named `name` as `who` and return it.
    async fn create(&self, who: &Caller, name: &str) -> Value {
        let data = exec_ok(&self.schema, self.create_request(who, name, &image(16))).await;
        data["emoji"].clone()
    }

    async fn create_code(&self, who: &Caller, name: &str, image_base64: &str) -> String {
        exec_err_code(&self.schema, self.create_request(who, name, image_base64)).await
    }

    async fn list(&self, who: &Caller) -> Vec<Value> {
        let data = exec_ok(
            &self.schema,
            who.request(CUSTOM_EMOJIS, json!({ "serverId": self.ws.server_id })),
        )
        .await;
        data["emojis"].as_array().expect("emojis").clone()
    }

    async fn delete_code(&self, who: &Caller, emoji_id: &str) -> String {
        exec_err_code(
            &self.schema,
            who.request(DELETE_EMOJI, delete_vars(emoji_id)),
        )
        .await
    }

    async fn delete(&self, who: &Caller, emoji_id: &str) {
        let data = exec_ok(
            &self.schema,
            who.request(DELETE_EMOJI, delete_vars(emoji_id)),
        )
        .await;
        assert_eq!(data["deleteCustomEmoji"], json!(true));
    }
}

/// Every custom emoji root answers `UNAUTHENTICATED` without a session.
#[tokio::test]
async fn custom_emoji_roots_refuse_anonymous_callers() {
    let f = setup().await;
    let calls = [
        (
            CREATE_EMOJI,
            create_vars(&f.ws.server_id, "party", &image(16)),
        ),
        (DELETE_EMOJI, delete_vars(&unknown_id())),
        (CUSTOM_EMOJIS, json!({ "serverId": f.ws.server_id })),
    ];
    assert_each_refused(&f.schema, &calls, "UNAUTHENTICATED", anonymous_request).await;
}

/// A user outside the workspace can neither list, add nor delete its emoji.
#[tokio::test]
async fn custom_emoji_roots_refuse_non_members() {
    let f = setup().await;
    let emoji = f.create(&f.ws.bob, "party").await;
    let calls = [
        (
            CREATE_EMOJI,
            create_vars(&f.ws.server_id, "intrude", &image(16)),
        ),
        (DELETE_EMOJI, delete_vars(id(&emoji))),
        (CUSTOM_EMOJIS, json!({ "serverId": f.ws.server_id })),
    ];
    assert_each_refused_as(&f.schema, &f.ws.olive, &calls, "FORBIDDEN").await;
    assert_eq!(f.list(&f.ws.alice).await, vec![emoji]);
}

/// Any member, a guest included, may add an emoji. The answer carries every field, the image is
/// stored under the workspace and named by its route, and every member lists it.
#[tokio::test]
async fn create_custom_emoji_returns_the_emoji_and_lists_it_for_members() {
    let f = setup().await;
    let before = chrono::Utc::now();
    let emoji = f.create(&f.ws.bob, "party_parrot").await;

    assert!(emoji["id"].is_string());
    assert_eq!(emoji["serverId"], json!(f.ws.server_id));
    assert_eq!(emoji["name"], json!("party_parrot"));
    assert_eq!(
        emoji["imageUrl"],
        json!(format!("/api/emoji/{}", id(&emoji)))
    );
    let stored = f.stored_file(&emoji).await;
    assert!(
        stored.starts_with(
            PathBuf::from(STORAGE_ROOT)
                .join(&f.ws.server_id)
                .join("emoji")
        ),
        "{stored:?}"
    );
    assert_eq!(emoji["uploadedBy"], json!(f.ws.bob.user_id.to_string()));
    let created_at: chrono::DateTime<chrono::Utc> = emoji["createdAt"]
        .as_str()
        .expect("createdAt")
        .parse()
        .expect("RFC 3339");
    assert!(created_at >= before - chrono::Duration::seconds(1));
    assert_eq!(
        std::fs::read(stored).expect("stored image"),
        vec![0x89u8; 16]
    );

    let guest_emoji = f.create(&f.ws.gina, "wave").await;
    assert_eq!(
        guest_emoji["uploadedBy"],
        json!(f.ws.gina.user_id.to_string())
    );

    for who in [&f.ws.alice, &f.ws.bob, &f.ws.gina] {
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
        let code = f.create_code(&f.ws.bob, name, &image(16)).await;
        assert_eq!(code, "VALIDATION", "{name:?}");
    }
    assert!(f.list(&f.ws.bob).await.is_empty());

    f.create(&f.ws.bob, &"a".repeat(64)).await;
    f.create(&f.ws.bob, "_Under_score9").await;
    assert_eq!(f.list(&f.ws.bob).await.len(), 2);
}

/// The image must be valid base64 of one byte up to 256 KiB; anything else answers `VALIDATION`.
#[tokio::test]
async fn create_custom_emoji_validates_the_image() {
    let f = setup().await;
    let code = f
        .create_code(&f.ws.bob, "big", &image(MAX_IMAGE_BYTES + 1))
        .await;
    assert_eq!(code, "VALIDATION");
    let code = f.create_code(&f.ws.bob, "empty", "").await;
    assert_eq!(code, "VALIDATION");
    let code = f.create_code(&f.ws.bob, "garbled", "not base64!").await;
    assert_eq!(code, "VALIDATION");
    assert!(f.list(&f.ws.bob).await.is_empty());

    let data = exec_ok(
        &f.schema,
        f.create_request(&f.ws.bob, "largest", &image(MAX_IMAGE_BYTES)),
    )
    .await;
    assert_eq!(data["emoji"]["name"], json!("largest"));
}

/// A name is unique within a workspace (`CONFLICT`, storing no image) but may repeat in another
/// workspace.
#[tokio::test]
async fn create_custom_emoji_refuses_a_duplicate_name() {
    let f = setup().await;
    f.create(&f.ws.bob, "party").await;
    let code = f.create_code(&f.ws.carol, "party", &image(16)).await;
    assert_eq!(code, "CONFLICT");
    assert_eq!(f.list(&f.ws.bob).await.len(), 1);
    let stored = std::fs::read_dir(
        PathBuf::from(STORAGE_ROOT)
            .join(&f.ws.server_id)
            .join("emoji"),
    )
    .expect("emoji dir")
    .count();
    assert_eq!(stored, 1, "the refused upload leaves no file");

    exec_ok(
        &f.schema,
        f.ws.olive.request(
            CREATE_EMOJI,
            create_vars(&f.ws.other_server_id, "party", &image(16)),
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
    let bobs = f.create(&f.ws.bob, "bobs").await;
    let carols = f.create(&f.ws.carol, "carols").await;

    let bobs_file = f.stored_file(&bobs).await;

    for who in [&f.ws.mona, &f.ws.carol, &f.ws.gina] {
        assert_eq!(f.delete_code(who, id(&bobs)).await, "FORBIDDEN");
    }
    assert!(bobs_file.exists());

    f.delete(&f.ws.bob, id(&bobs)).await;
    assert!(!bobs_file.exists());
    f.delete(&f.ws.alice, id(&carols)).await;
    assert!(f.list(&f.ws.alice).await.is_empty());

    assert_eq!(f.delete_code(&f.ws.bob, id(&bobs)).await, "NOT_FOUND");
    let unknown = unknown_id();
    assert_eq!(f.delete_code(&f.ws.alice, &unknown).await, "NOT_FOUND");
}

/// A member, a guest included, reads an emoji's image; an outsider, and anyone once the emoji
/// is deleted, gets `NotFound`.
#[tokio::test]
async fn custom_emoji_image_is_served_to_members_only() {
    let f = setup().await;
    let emoji = f.create(&f.ws.bob, "party").await;

    assert_eq!(
        f.image(&f.ws.gina, id(&emoji)).await.expect("guest reads"),
        vec![0x89u8; 16]
    );
    let err = f.image(&f.ws.olive, id(&emoji)).await.unwrap_err();
    assert!(matches!(err, ServiceError::NotFound), "{err:?}");

    f.delete(&f.ws.bob, id(&emoji)).await;
    let err = f.image(&f.ws.bob, id(&emoji)).await.unwrap_err();
    assert!(matches!(err, ServiceError::NotFound), "{err:?}");
}

/// A `:name:` reaction works for a custom emoji of the workspace and answers `VALIDATION` for a
/// name the workspace has no emoji for, and for every name while custom emoji are disabled.
#[tokio::test]
async fn a_shortcode_reaction_names_one_of_the_workspace_custom_emoji() {
    let f = setup().await;
    f.create(&f.ws.bob, "party_parrot").await;
    let channel = create_channel(&f.schema, &f.ws.alice, &f.ws.server_id, "lobby", false).await;
    let sent = exec_ok(
        &f.schema,
        f.ws.alice.request(
            SEND_MESSAGE,
            json!({ "input": {
                "serverId": f.ws.server_id, "conversationId": channel, "text": "hello",
            } }),
        ),
    )
    .await;
    let message_id = sent["msg"]["id"].as_str().expect("id").to_string();
    let react = |emoji: &str| {
        f.ws.alice.request(
            TOGGLE_REACTION,
            json!({ "input": {
                "serverId": f.ws.server_id, "conversationId": channel, "messageId": message_id,
                "emoji": emoji,
            } }),
        )
    };

    let data = exec_ok(&f.schema, react(":party_parrot:")).await;
    assert_eq!(
        data["toggleReaction"],
        json!([{ "emoji": ":party_parrot:", "count": 1 }])
    );
    assert_eq!(
        exec_err_code(&f.schema, react(":verify_at_evil_co:")).await,
        "VALIDATION"
    );

    let disabled = f
        .ctx
        .build_schema_with_disabled(&[rim_core::server_feature::model::ServerFeature::CustomEmoji]);
    assert_eq!(
        exec_err_code(&disabled, react(":party:")).await,
        "VALIDATION"
    );
}
