//! Through-the-stack e2e tests for `Message.postedVia`: a message an `AGENT` call of
//! `executeCommand` posts is marked, every other way of posting leaves it null, and an edit keeps
//! the mark.
//!
//! Run with:
//! ```sh
//! cargo test -p rim_api --features e2e-tests -- --test-threads=1
//! ```
#![cfg(feature = "e2e-tests")]

mod support;

use serde_json::{Value, json};

use rim_api::AppSchema;
use rim_services::auth::dto::RegistrationMode;

use support::TestCtx;
use support::graphql::{Caller, anonymous_request, exec_ok};
use support::roles::{Roles, create_channel};

const EXECUTE: &str = r#"
mutation Execute($input: ExecuteCommandInput!) {
  result: executeCommand(input: $input) {
    status payload confirmationRequired { token argsHash }
  }
}
"#;

const SEND: &str = r#"
mutation Send($input: SendMessageInput!) { msg: sendMessage(input: $input) { id postedVia } }
"#;

const EDIT: &str = r#"
mutation Edit($input: EditMessageInput!) { msg: editMessage(input: $input) { id postedVia } }
"#;

const MESSAGE: &str = r#"
query One($s: ID!, $c: ID!, $m: ID!) {
  message(serverId: $s, conversationId: $c, messageId: $m) { id text postedVia }
}
"#;

const CHANGES: &str = r#"
query Changes($s: ID!, $c: ID!) {
  messageChanges(input: { serverId: $s, conversations: [{ conversationId: $c }] }) {
    changes { message { id postedVia } }
  }
}
"#;

struct Fixture {
    schema: AppSchema,
    ws: Roles,
    channel_id: String,
}

async fn setup() -> Fixture {
    let ctx = TestCtx::new().await;
    let ws = ctx.roles("postedvia").await;
    let schema = ctx.build_schema(RegistrationMode::InviteOnly).await;
    let channel_id = create_channel(&schema, &ws.alice, &ws.server_id, "general", false).await;
    Fixture {
        schema,
        ws,
        channel_id,
    }
}

impl Fixture {
    /// Runs `executeCommand(input)` as `who` and, when it asks for a confirmation, again with
    /// the issued proof, as the person confirming would; returns the successful result.
    async fn execute_confirmed(&self, who: &Caller, input: Value) -> Value {
        let first = exec_ok(
            &self.schema,
            who.request(EXECUTE, json!({ "input": input })),
        )
        .await;
        let result = if first["result"]["status"] == json!("CONFIRMATION_REQUIRED") {
            let challenge = &first["result"]["confirmationRequired"];
            let mut proved = input.clone();
            proved["confirmation"] = json!({ "inBandToken": {
                "token": challenge["token"], "argsHash": challenge["argsHash"],
            } });
            exec_ok(
                &self.schema,
                who.request(EXECUTE, json!({ "input": proved })),
            )
            .await
        } else {
            first
        };
        assert_eq!(result["result"]["status"], json!("SUCCESS"), "{result}");
        result["result"].clone()
    }

    /// Posts `text` to the channel with `chat.post_message` as `who`, called with
    /// `initiator`, and returns the new message's id.
    async fn post_with(&self, who: &Caller, initiator: &str, text: &str) -> String {
        let result = self
            .execute_confirmed(
                who,
                json!({
                    "commandId": "chat.post_message",
                    "args": {
                        "server_id": self.ws.server_id,
                        "conversation_id": self.channel_id,
                        "text": text,
                    },
                    "commandContext": { "workspaceId": self.ws.server_id },
                    "initiator": initiator,
                }),
            )
            .await;
        result["payload"]["message_id"]
            .as_str()
            .expect("message id")
            .to_string()
    }

    /// The message `id` as `who` reads it.
    async fn message(&self, who: &Caller, id: &str) -> Value {
        exec_ok(
            &self.schema,
            who.request(
                MESSAGE,
                json!({ "s": self.ws.server_id, "c": self.channel_id, "m": id }),
            ),
        )
        .await["message"]
            .clone()
    }

    /// `postedVia` of the message `id` in the channel's change feed, as `who` reads it.
    async fn feed_posted_via(&self, who: &Caller, id: &str) -> Value {
        let data = exec_ok(
            &self.schema,
            who.request(
                CHANGES,
                json!({ "s": self.ws.server_id, "c": self.channel_id }),
            ),
        )
        .await;
        data["messageChanges"][0]["changes"]
            .as_array()
            .expect("changes")
            .iter()
            .find(|change| change["message"]["id"] == json!(id))
            .map(|change| change["message"]["postedVia"].clone())
            .expect("the message is in the feed")
    }
}

/// A message an `AGENT` call posts reads back `postedVia: AGENT` to everyone in the channel,
/// in a lookup and in the change feed.
#[tokio::test]
async fn a_message_the_agent_posts_is_marked() {
    let f = setup().await;
    let id = f.post_with(&f.ws.bob, "AGENT", "posted by the agent").await;

    for reader in [&f.ws.bob, &f.ws.carol] {
        assert_eq!(
            f.message(reader, &id).await,
            json!({ "id": id, "text": "posted by the agent", "postedVia": "AGENT" })
        );
        assert_eq!(f.feed_posted_via(reader, &id).await, json!("AGENT"));
    }
}

/// `sendMessage` and `executeCommand` with any initiator but `AGENT` post an unmarked message.
#[tokio::test]
async fn other_ways_of_posting_are_not_marked() {
    let f = setup().await;
    let sent = exec_ok(
        &f.schema,
        f.ws.bob.request(
            SEND,
            json!({ "input": {
                "serverId": f.ws.server_id, "conversationId": f.channel_id, "text": "typed",
            } }),
        ),
    )
    .await;
    assert_eq!(sent["msg"]["postedVia"], Value::Null);
    let sent_id = sent["msg"]["id"].as_str().expect("id").to_string();
    assert_eq!(
        f.message(&f.ws.carol, &sent_id).await["postedVia"],
        Value::Null
    );

    for initiator in ["PALETTE", "MACRO", "KEYBIND", "PLUGIN", "CLI", "UI"] {
        let id = f.post_with(&f.ws.bob, initiator, initiator).await;
        assert_eq!(
            f.message(&f.ws.carol, &id).await["postedVia"],
            Value::Null,
            "{initiator}"
        );
    }
}

/// Editing the agent's message, from the person's own client, keeps the mark.
#[tokio::test]
async fn an_edit_keeps_the_mark() {
    let f = setup().await;
    let id = f.post_with(&f.ws.bob, "AGENT", "first draft").await;

    let edited = exec_ok(
        &f.schema,
        f.ws.bob.request(
            EDIT,
            json!({ "input": {
                "serverId": f.ws.server_id, "conversationId": f.channel_id,
                "messageId": id, "text": "fixed by hand",
            } }),
        ),
    )
    .await;

    assert_eq!(edited["msg"]["postedVia"], json!("AGENT"));
    assert_eq!(
        f.message(&f.ws.carol, &id).await,
        json!({ "id": id, "text": "fixed by hand", "postedVia": "AGENT" })
    );
    assert_eq!(f.feed_posted_via(&f.ws.carol, &id).await, json!("AGENT"));
}

/// `serverInfo.capabilities` lists `message_posted_via`.
#[tokio::test]
async fn the_server_lists_message_posted_via() {
    let f = setup().await;
    let data = exec_ok(
        &f.schema,
        anonymous_request("{ serverInfo { capabilities } }", json!({})),
    )
    .await;
    assert!(
        data["serverInfo"]["capabilities"]
            .as_array()
            .expect("capabilities")
            .contains(&json!("message_posted_via"))
    );
}
