//! Pipeline runs on a real server: the evaluator task `rim_server` starts at boot receives
//! every workspace event and runs the matching pipelines whose creator may read the event. A
//! firing shows in the pipeline's `lastFiredAt`, to its creator only, and in the audit row of
//! each command it ran.
//!
//! Run with:
//! ```sh
//! cargo test -p rim_api --features e2e-tests -- --test-threads=1
//! ```
#![cfg(feature = "e2e-tests")]

mod support;

use std::time::Duration;

use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter};
use serde_json::{Value, json};

use support::TestCtx;
use support::graphql::{Call, Caller, exec_ok};
use support::nodes::{Node, S2S_TABLES, start};
use support::relay::eventually;
use support::roles::{Roles, create_channel};

const CREATE_PIPELINE: &str = r#"
mutation Create($input: CreatePipelineInput!) { pipeline: createPipeline(input: $input) { id } }
"#;

const TOGGLE_PIPELINE: &str = r#"
mutation Toggle($pipelineId: ID!, $enabled: Boolean!) {
  pipeline: togglePipeline(pipelineId: $pipelineId, enabled: $enabled) { id }
}
"#;

const PIPELINES: &str = r#"
query Pipelines($serverId: ID!) { pipelines(serverId: $serverId) { id enabled lastFiredAt } }
"#;

const SEND_MESSAGE: &str = r#"
mutation Send($input: SendMessageInput!) { msg: sendMessage(input: $input) { id } }
"#;

const GET_STATE: &str = r#"
query Get($serverId: ID!, $key: String!) {
  entry: workspaceState(serverId: $serverId, key: $key, scope: SERVER) { value }
}
"#;

/// The read-only command a probe pipeline runs; pipelines may run read-only commands only.
const PROBE: &str = "state.get";
/// The read-only command the control pipeline runs.
const CONTROL: &str = "state.list";

struct Fixture {
    node: Node,
    ws: Roles,
    channel: String,
    _ctx: TestCtx,
}

/// The shared role cast on a freshly booted server, with a public channel alice created.
async fn setup() -> Fixture {
    let ctx = TestCtx::new().await;
    ctx.conn
        .execute_unprepared(S2S_TABLES)
        .await
        .expect("truncate");
    let ws = ctx.roles("runs").await;
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
    let node = start(&url, |_| {}).await;
    let channel = create_channel(&node.schema, &ws.alice, &ws.server_id, "general", false).await;
    Fixture {
        node,
        ws,
        channel,
        _ctx: ctx,
    }
}

impl Fixture {
    async fn ok(&self, who: &Caller, call: Call) -> Value {
        exec_ok(&self.node.schema, who.call(call)).await
    }

    /// Create, as `who`, a pipeline on new messages that runs `command_id` with `args`,
    /// merged with `extra` (a filter, a cooldown), and return its id.
    async fn pipeline(&self, who: &Caller, command_id: &str, args: Value, extra: Value) -> String {
        let mut input = json!({
            "serverId": self.ws.server_id,
            "name": command_id,
            "trigger": { "kind": "NEW_MESSAGE" },
            "actions": [{ "commandId": command_id, "args": args }],
        });
        for (k, v) in extra.as_object().expect("extra").iter() {
            input[k] = v.clone();
        }
        let data = self
            .ok(who, (CREATE_PIPELINE, json!({ "input": input })))
            .await;
        data["pipeline"]["id"].as_str().expect("id").to_string()
    }

    /// A pipeline as alice that runs the probe command on every new message.
    async fn probe(&self, extra: Value) -> String {
        let args = json!({ "server_id": self.ws.server_id, "key": "probe" });
        self.pipeline(&self.ws.alice, PROBE, args, extra).await
    }

    /// A pipeline as alice that runs the control command on every new message, used to tell
    /// when the evaluator has finished an earlier event.
    async fn control(&self) -> String {
        let args = json!({ "server_id": self.ws.server_id, "prefix": "" });
        self.pipeline(&self.ws.alice, CONTROL, args, json!({}))
            .await
    }

    async fn toggle(&self, id: &str, enabled: bool) {
        let vars = json!({ "pipelineId": id, "enabled": enabled });
        self.ok(&self.ws.alice, (TOGGLE_PIPELINE, vars)).await;
    }

    async fn send(&self, text: &str) {
        self.send_to(&self.channel, text).await;
    }

    /// Send `text` as alice to `conversation_id`.
    async fn send_to(&self, conversation_id: &str, text: &str) {
        let vars = json!({ "input": {
            "serverId": self.ws.server_id, "conversationId": conversation_id, "text": text,
        } });
        self.ok(&self.ws.alice, (SEND_MESSAGE, vars)).await;
    }

    /// The pipeline `id` as `pipelines` returns it to alice.
    async fn stored(&self, id: &str) -> Value {
        self.stored_as(&self.ws.alice, id).await
    }

    /// The pipeline `id` as `pipelines` returns it to `who`.
    async fn stored_as(&self, who: &Caller, id: &str) -> Value {
        let data = self
            .ok(who, (PIPELINES, json!({ "serverId": self.ws.server_id })))
            .await;
        data["pipelines"]
            .as_array()
            .expect("pipelines")
            .iter()
            .find(|p| p["id"] == id)
            .expect("pipeline listed")
            .clone()
    }

    /// The audit rows of `command_id` in the workspace.
    async fn audit(&self, command_id: &str) -> Vec<entity::command_audit_log::Model> {
        let server: uuid::Uuid = self.ws.server_id.parse().expect("server id");
        entity::command_audit_log::Entity::find()
            .filter(entity::command_audit_log::Column::ServerId.eq(server))
            .filter(entity::command_audit_log::Column::CommandId.eq(command_id))
            .all(&self.node.db)
            .await
            .expect("audit rows")
    }

    /// Wait until `command_id` has exactly `count` audit rows.
    async fn wait_for_runs(&self, command_id: &str, count: usize) {
        eventually(&format!("{count} runs of {command_id}"), || async {
            (self.audit(command_id).await.len() == count).then_some(())
        })
        .await;
    }

    /// Wait until pipeline `id` reads `enabled`.
    async fn wait_until_enabled_is(&self, id: &str, enabled: bool) {
        eventually(&format!("pipeline enabled = {enabled}"), || async {
            (self.stored(id).await["enabled"] == enabled).then_some(())
        })
        .await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_matching_message_runs_the_action_as_the_pipeline_creator() {
    let f = setup().await;
    let args = json!({ "server_id": f.ws.server_id, "key": "probe" });
    let id = f.pipeline(&f.ws.bob, PROBE, args, json!({})).await;
    assert_eq!(
        f.stored_as(&f.ws.bob, &id).await["lastFiredAt"],
        Value::Null
    );

    f.send("hello").await;
    f.wait_for_runs(PROBE, 1).await;

    let run = &f.audit(PROBE).await[0];
    assert_eq!(run.actor_user_id, uuid::Uuid::from(f.ws.bob.user_id));
    assert_eq!(run.initiator, "plugin");
    assert_eq!(run.outcome, "success");
    eventually("lastFiredAt", || async {
        f.stored_as(&f.ws.bob, &id).await["lastFiredAt"]
            .as_str()
            .map(|_| ())
    })
    .await;
    assert_eq!(
        f.stored(&id).await["lastFiredAt"],
        Value::Null,
        "only the creator reads when the pipeline fired"
    );
    f.node.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_pipeline_cannot_write_because_its_writes_need_a_confirmation() {
    let f = setup().await;
    let args = json!({ "server_id": f.ws.server_id, "key": "written", "value": 1 });
    let id = f.pipeline(&f.ws.alice, "state.set", args, json!({})).await;
    f.control().await;

    f.send("hello").await;
    f.send("barrier").await;
    f.wait_for_runs(CONTROL, 2).await;

    let data = f
        .ok(
            &f.ws.alice,
            (
                GET_STATE,
                json!({ "serverId": f.ws.server_id, "key": "written" }),
            ),
        )
        .await;
    assert_eq!(data["entry"], Value::Null, "the write was skipped");
    assert!(
        f.stored(&id).await["lastFiredAt"].is_string(),
        "the pipeline still counts as fired"
    );
    f.node.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_message_the_filter_rejects_does_not_fire_the_pipeline() {
    let f = setup().await;
    let filter = json!({ "filter": { "conditions": [
        { "conditionType": "MessageContains", "pattern": "deploy" },
    ] } });
    let id = f.probe(filter).await;
    f.control().await;

    f.send("hello").await;
    f.send("barrier").await;
    f.wait_for_runs(CONTROL, 2).await;
    assert!(f.audit(PROBE).await.is_empty());
    assert_eq!(f.stored(&id).await["lastFiredAt"], Value::Null);

    f.send("deploy done").await;
    f.wait_for_runs(PROBE, 1).await;
    f.node.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_disabled_pipeline_does_not_fire() {
    let f = setup().await;
    let id = f.probe(json!({})).await;
    f.toggle(&id, false).await;
    f.control().await;

    f.send("hello").await;
    f.send("barrier").await;
    f.wait_for_runs(CONTROL, 2).await;
    assert!(f.audit(PROBE).await.is_empty());
    assert_eq!(f.stored(&id).await["lastFiredAt"], Value::Null);
    f.node.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_cooldown_skips_a_second_message_and_lets_one_through_after_it() {
    let f = setup().await;
    f.probe(json!({ "cooldownSecs": 1 })).await;
    f.control().await;

    f.send("first").await;
    f.send("inside the cooldown").await;
    f.wait_for_runs(CONTROL, 2).await;
    assert_eq!(f.audit(PROBE).await.len(), 1);

    tokio::time::sleep(Duration::from_millis(1100)).await;
    f.send("after the cooldown").await;
    f.wait_for_runs(PROBE, 2).await;
    f.node.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_eleventh_firing_within_a_minute_disables_the_pipeline() {
    let f = setup().await;
    let id = f.probe(json!({})).await;

    for i in 0..11 {
        f.send(&format!("message {i}")).await;
    }
    f.wait_until_enabled_is(&id, false).await;
    assert_eq!(
        f.audit(PROBE).await.len(),
        10,
        "ten firings ran their action"
    );

    // Switched back on inside the same minute, it is disabled again at its next firing.
    f.toggle(&id, true).await;
    f.send("after re-enabling").await;
    f.wait_until_enabled_is(&id, false).await;
    assert_eq!(f.audit(PROBE).await.len(), 10);
    f.node.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failing_action_does_not_stop_the_other_pipelines() {
    let f = setup().await;
    let broken = f
        .pipeline(&f.ws.alice, "state.set", json!({}), json!({}))
        .await;
    let probe = f.probe(json!({})).await;

    f.send("hello").await;
    f.wait_for_runs(PROBE, 1).await;
    for id in [&broken, &probe] {
        eventually("lastFiredAt", || async {
            f.stored(id).await["lastFiredAt"].as_str().map(|_| ())
        })
        .await;
    }
    f.node.stop().await;
}

/// A member's pipeline does not react to a private channel they are not in, even when the
/// message matches its filter, and reacts to the same message in a public channel.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_pipeline_ignores_a_private_channel_its_creator_is_not_in() {
    let f = setup().await;
    let private = create_channel(&f.node.schema, &f.ws.alice, &f.ws.server_id, "board", true).await;
    let args = json!({ "server_id": f.ws.server_id, "key": "probe" });
    let filter = json!({ "filter": { "conditions": [
        { "conditionType": "MessageContains", "pattern": "layoffs" },
    ] } });
    let id = f.pipeline(&f.ws.bob, PROBE, args, filter).await;
    f.control().await;

    f.send_to(&private, "layoffs on friday").await;
    f.send_to(&private, "barrier").await;
    f.wait_for_runs(CONTROL, 2).await;
    assert!(
        f.audit(PROBE).await.is_empty(),
        "the private message fired it"
    );
    assert_eq!(
        f.stored_as(&f.ws.bob, &id).await["lastFiredAt"],
        Value::Null
    );

    f.send("layoffs on friday").await;
    f.wait_for_runs(PROBE, 1).await;
    eventually("lastFiredAt", || async {
        f.stored_as(&f.ws.bob, &id).await["lastFiredAt"]
            .as_str()
            .map(|_| ())
    })
    .await;
    f.node.stop().await;
}
