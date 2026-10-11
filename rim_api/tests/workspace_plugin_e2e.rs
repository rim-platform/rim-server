//! Resolver-level e2e test for workspace plugin registries over a real database: the known
//! bundle uploaded through the bundle service, the known statement added, read, updated and
//! removed over GraphQL.
//!
//! The HTTP layer of the bundle endpoints (status codes, headers, CSRF) and the CSRF check of
//! the mutations are tested in `rim_server`; this test drives the same service calls.
//!
//! Run with:
//! ```sh
//! cargo test -p rim_api --features e2e-tests -- --test-threads=1
//! ```
#![cfg(feature = "e2e-tests")]

mod support;

use std::sync::Arc;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use rim_api::AppSchema;
use rim_core::server::model::Server;
use rim_core::server_member::model::ServerMemberRole;
use rim_identity::StatementVersion;
use rim_identity::testing::{
    KNOWN_PLUGIN_BUNDLE, KNOWN_PLUGIN_MANIFEST_PAYLOAD, KNOWN_PLUGIN_MANIFEST_SIGNATURE,
    TestIdentity,
};
use rim_services::auth::dto::RegistrationMode;
use rim_services::events::{WorkspaceEvent, WorkspacePluginChange};
use rim_services::provider::AppServiceProvider;
use rim_services::workspace_plugin::command::{DownloadPluginBundleQuery, UploadPluginBundleCmd};
use rim_services::{Actor, ServiceError};

use support::graphql::{Caller, anonymous_request, exec_err_code, exec_ok, merged};
use support::{RecordingBroadcaster, TestCtx};

const SHA: &str = "5b154b3eb45c3ebc44f6820ba30a389ce85b43dea34bcbf652b9b1a5416fbe85";
const PLUGIN: &str = "dev.example.hello-web";

const ENTRY_FIELDS: &str = "id serverId pluginId version displayName runtimeKind status \
     source { kind bundleUrl bundleSha256 bundleSize gitUrl gitCommit gitTreeSha256 } \
     manifest statement { payload signature } publisherPrincipalId publisherName \
     publisherKeySeq publisherKeyLog { payload signature } signatureStatus \
     approvedBy { id } approvedAt updatedAt";

fn list_query() -> String {
    format!(
        "query List($serverId: ID!) {{ workspacePlugins(serverId: $serverId) {{ {ENTRY_FIELDS} }} }}"
    )
}

fn add_mutation() -> String {
    format!(
        "mutation Add($input: AddWorkspacePluginInput!) {{ addWorkspacePlugin(input: $input) {{ {ENTRY_FIELDS} }} }}"
    )
}

fn update_mutation() -> String {
    format!(
        "mutation Update($input: UpdateWorkspacePluginInput!) {{ updateWorkspacePlugin(input: $input) {{ {ENTRY_FIELDS} }} }}"
    )
}

const CREATE_DM: &str = r#"
mutation CreateDm($input: CreateConversationInput!) { createConversation(input: $input) { id } }"#;

const REMOVE: &str = r#"
mutation Remove($serverId: ID!, $pluginId: String!) { removeWorkspacePlugin(serverId: $serverId, pluginId: $pluginId) }"#;

fn known_statement() -> Value {
    json!({ "payload": KNOWN_PLUGIN_MANIFEST_PAYLOAD, "signature": KNOWN_PLUGIN_MANIFEST_SIGNATURE })
}

/// The publisher's key log: the version 2 inception of the identity from seed 1.
fn publisher_key_log() -> Value {
    let inception = TestIdentity::with_version(1, StatementVersion::V2).inception();
    json!([{ "payload": inception.payload, "signature": inception.signature }])
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

struct World {
    schema: AppSchema,
    provider: Arc<dyn AppServiceProvider>,
    events: Arc<RecordingBroadcaster>,
    server: Server,
    owner: Caller,
    member: Caller,
    guest: Caller,
    outsider: Caller,
}

async fn world(ctx: &TestCtx) -> World {
    let (schema, provider, events) = ctx.build_recording(RegistrationMode::InviteOnly);
    let server = ctx.create_server("acme").await;
    let owner = ctx.create_user("owner").await;
    let member = ctx.create_user("member").await;
    let guest = ctx.create_user("guest").await;
    let outsider = ctx.create_user("outsider").await;
    ctx.add_owner(&server, &owner).await;
    ctx.add_member(&server, &member, ServerMemberRole::Member)
        .await;
    ctx.add_member(&server, &guest, ServerMemberRole::Guest)
        .await;
    World {
        schema,
        provider,
        events,
        owner: ctx.caller(&owner).await,
        member: ctx.caller(&member).await,
        guest: ctx.caller(&guest).await,
        outsider: ctx.caller(&outsider).await,
        server,
    }
}

impl World {
    fn server_id(&self) -> String {
        self.server.id.to_string()
    }

    async fn upload(&self, by: Caller) -> Result<bool, ServiceError> {
        self.provider
            .plugin_bundle_service()
            .upload(UploadPluginBundleCmd {
                actor: Actor::new(by.user_id),
                server_id: self.server.id,
                bytes: KNOWN_PLUGIN_BUNDLE.to_vec(),
                sha256: sha256_hex(KNOWN_PLUGIN_BUNDLE),
            })
            .await
            .map(|uploaded| {
                assert_eq!(uploaded.sha256, SHA);
                assert_eq!(uploaded.size, 1605);
                assert_eq!(
                    uploaded.url,
                    format!("/api/plugin-bundles/{}/{SHA}", self.server.id)
                );
                uploaded.created
            })
    }

    async fn download(&self, by: Caller) -> Result<Vec<u8>, ServiceError> {
        self.provider
            .plugin_bundle_service()
            .download(DownloadPluginBundleQuery {
                actor: Actor::new(by.user_id),
                server_id: self.server.id,
                sha256: SHA.to_string(),
            })
            .await
    }

    fn add_input(&self, statement: Value) -> Value {
        json!({ "input": {
            "serverId": self.server_id(),
            "statement": statement,
            "status": "RECOMMENDED",
            "publisherKeyLog": publisher_key_log(),
        }})
    }

    fn plugin_events(&self) -> Vec<WorkspacePluginChange> {
        self.events
            .events()
            .into_iter()
            .filter_map(|e| match e {
                WorkspaceEvent::WorkspacePluginChanged {
                    workspace_id,
                    plugin_id,
                    change,
                } => {
                    assert_eq!(workspace_id, self.server.id);
                    assert_eq!(plugin_id, PLUGIN);
                    Some(change)
                }
                _ => None,
            })
            .collect()
    }
}

#[tokio::test]
async fn a_signed_plugin_goes_through_the_registry() {
    let ctx = TestCtx::new().await;
    let w = world(&ctx).await;

    // The bundle: stored once (201), then found (200).
    assert!(w.upload(w.owner).await.expect("upload"));
    assert!(!w.upload(w.owner).await.expect("upload again"));
    assert!(matches!(
        w.upload(w.member).await,
        Err(ServiceError::PermissionDenied)
    ));

    // The known statement, with the inception as the publisher's log.
    let added = exec_ok(
        &w.schema,
        w.owner
            .request(&add_mutation(), w.add_input(known_statement())),
    )
    .await;
    let entry = &added["addWorkspacePlugin"];
    assert_eq!(entry["pluginId"], PLUGIN);
    assert_eq!(entry["version"], "0.1.0");
    assert_eq!(entry["displayName"], "Hello Web");
    assert_eq!(entry["runtimeKind"], "WEB_SANDBOXED");
    assert_eq!(entry["status"], "RECOMMENDED");
    assert_eq!(entry["signatureStatus"], "VALID");
    assert_eq!(entry["serverId"], w.server_id());
    assert_eq!(
        entry["source"],
        json!({
            "kind": "BUNDLE",
            "bundleUrl": format!("/api/plugin-bundles/{}/{SHA}", w.server_id()),
            "bundleSha256": SHA,
            "bundleSize": 1605,
            "gitUrl": null,
            "gitCommit": null,
            "gitTreeSha256": null,
        })
    );
    assert_eq!(entry["statement"], known_statement());
    assert_eq!(
        entry["publisherPrincipalId"],
        "rimp1_txDLbnipi5i3kCJtJwBOUixzv_MY8OnU0mcy--Tyje0"
    );
    assert_eq!(entry["publisherName"], "Ada");
    assert_eq!(entry["publisherKeySeq"], 0);
    assert_eq!(entry["publisherKeyLog"], publisher_key_log());
    assert_eq!(entry["approvedBy"]["id"], w.owner.user_id.to_string());
    let manifest = entry["manifest"].as_str().expect("manifest");
    assert!(manifest.contains(r#""plugin_id":"dev.example.hello-web""#));

    // Members and guests read the registry and download the bundle; others do not. A guest
    // sees the approver only once they share a conversation, like the member list.
    let listed_by = |reader: Caller| {
        exec_ok(
            &w.schema,
            reader.request(&list_query(), json!({ "serverId": w.server_id() })),
        )
    };
    let without_approver = merged(entry.clone(), json!({ "approvedBy": null }));
    for (reader, seen) in [(w.member, entry), (w.guest, &without_approver)] {
        assert_eq!(listed_by(reader).await["workspacePlugins"], json!([seen]));
        assert_eq!(
            w.download(reader).await.expect("download"),
            KNOWN_PLUGIN_BUNDLE
        );
    }
    exec_ok(
        &w.schema,
        w.guest.request(
            CREATE_DM,
            json!({ "input": { "serverId": w.server_id(), "otherUserId": w.owner.user_id.to_string() } }),
        ),
    )
    .await;
    assert_eq!(listed_by(w.guest).await["workspacePlugins"], json!([entry]));
    assert_eq!(
        exec_err_code(
            &w.schema,
            w.outsider
                .request(&list_query(), json!({ "serverId": w.server_id() })),
        )
        .await,
        "FORBIDDEN"
    );
    assert!(matches!(
        w.download(w.outsider).await,
        Err(ServiceError::NotFound)
    ));
    assert_eq!(
        exec_err_code(
            &w.schema,
            anonymous_request(&list_query(), json!({ "serverId": w.server_id() })),
        )
        .await,
        "UNAUTHENTICATED"
    );

    // A member may not add; a tampered signature does not verify; a second add conflicts.
    assert_eq!(
        exec_err_code(
            &w.schema,
            w.member
                .request(&add_mutation(), w.add_input(known_statement())),
        )
        .await,
        "FORBIDDEN"
    );
    let mut tampered = known_statement();
    tampered["signature"] = json!(KNOWN_PLUGIN_MANIFEST_SIGNATURE.replacen('F', "G", 1));
    let res = w
        .schema
        .execute(w.owner.request(&add_mutation(), w.add_input(tampered)))
        .await;
    assert_eq!(res.errors[0].message, "signature does not verify");
    assert_eq!(
        exec_err_code(
            &w.schema,
            w.owner
                .request(&add_mutation(), w.add_input(known_statement())),
        )
        .await,
        "CONFLICT"
    );

    // A status update emits the event.
    let updated = exec_ok(
        &w.schema,
        w.owner.request(
            &update_mutation(),
            json!({ "input": {
                "serverId": w.server_id(),
                "pluginId": PLUGIN,
                "status": "REQUIRED",
            }}),
        ),
    )
    .await;
    assert_eq!(updated["updateWorkspacePlugin"]["status"], "REQUIRED");
    assert_eq!(
        w.plugin_events(),
        [WorkspacePluginChange::Added, WorkspacePluginChange::Updated]
    );

    // Removal, then the entry is gone.
    let removed = exec_ok(
        &w.schema,
        w.owner.request(
            REMOVE,
            json!({ "serverId": w.server_id(), "pluginId": PLUGIN }),
        ),
    )
    .await;
    assert_eq!(removed["removeWorkspacePlugin"], true);
    assert_eq!(
        exec_err_code(
            &w.schema,
            w.owner.request(
                REMOVE,
                json!({ "serverId": w.server_id(), "pluginId": PLUGIN }),
            ),
        )
        .await,
        "NOT_FOUND"
    );
    assert_eq!(
        exec_err_code(
            &w.schema,
            w.owner.request(
                &update_mutation(),
                json!({ "input": {
                    "serverId": w.server_id(),
                    "pluginId": PLUGIN,
                    "status": "RECOMMENDED",
                }}),
            ),
        )
        .await,
        "NOT_FOUND"
    );
    assert_eq!(
        w.plugin_events(),
        [
            WorkspacePluginChange::Added,
            WorkspacePluginChange::Updated,
            WorkspacePluginChange::Removed
        ]
    );
    let listed = exec_ok(
        &w.schema,
        w.member
            .request(&list_query(), json!({ "serverId": w.server_id() })),
    )
    .await;
    assert_eq!(listed["workspacePlugins"], json!([]));
}

#[tokio::test]
async fn the_default_url_needs_the_bundle_uploaded_here() {
    let ctx = TestCtx::new().await;
    let w = world(&ctx).await;
    let res = w
        .schema
        .execute(
            w.owner
                .request(&add_mutation(), w.add_input(known_statement())),
        )
        .await;
    assert_eq!(
        res.errors[0].message,
        "bundle 5b154b3e… is not uploaded to this workspace"
    );

    // Without a log, a publisher not enrolled here cannot be verified.
    let mut input = w.add_input(known_statement());
    input["input"]
        .as_object_mut()
        .expect("input")
        .remove("publisherKeyLog");
    w.upload(w.owner).await.expect("upload");
    assert_eq!(
        exec_err_code(&w.schema, w.owner.request(&add_mutation(), input)).await,
        "VALIDATION"
    );
}
