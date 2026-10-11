//! Resolver-level e2e tests of end-to-end encryption over a real database, with a small MLS
//! client built on OpenMLS: devices upload key packages, a member creates the group of an
//! encrypted DM, the other joins from the Welcome, and both exchange, edit, delete and report
//! encrypted messages through the server.
//!
//! Run with:
//! ```sh
//! cargo test -p rim_api --features e2e-tests -- --test-threads=1
//! ```
#![cfg(feature = "e2e-tests")]

mod support;

use std::sync::{Arc, Mutex};

use async_graphql::{Request, Variables};
use openmls::prelude::*;
use sea_orm::ConnectionTrait;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use rim_api::AppSchema;
use rim_api::auth_context::AuthActorCtx;
use rim_core::auth::repository::SessionRepository;
use rim_core::key_identity::model::DeviceKeyId;
use rim_core::key_identity::repository::KeyIdentityRepository;
use rim_core::server::model::Server;
use rim_core::server_member::model::ServerMemberRole;
use rim_core::user::model::User;
use rim_identity::testing::{TestIdentity, device_self_revoke, signing_key};
use rim_identity::{KeyState, StatementVersion};
use rim_infra_db::sea_orm_impl::key_identity::SeaOrmKeyIdentityRepository;
use rim_infra_db::sea_orm_impl::session::SeaOrmSessionRepository;
use rim_infra_services::email::{EmailMessage, EmailSender};
use rim_services::auth::dto::RegistrationMode;
use rim_services::provider::AppServiceProvider;

use support::graphql::{
    Call, Caller, anonymous_request, assert_each_refused, exec_err_code, exec_ok, merged,
};
use support::identity::statement;
use support::mls::{
    MlsDevice, SUITE, b64, capabilities, commit_aad, content, hmac, join_config, unb64,
};
use support::{FRANKING_KEY, TestCtx, unknown_id};

const CREATE_CONVERSATION: &str = r#"
mutation Create($input: CreateConversationInput!) {
  createConversation(input: $input) { id encryptedAt }
}"#;
const ADD_MEMBERS: &str = r#"
mutation Add($serverId: ID!, $conversationId: ID!, $userIds: [ID!]!) {
  addConversationMembers(serverId: $serverId, conversationId: $conversationId, userIds: $userIds) { id }
}"#;
const CONVERSATION: &str = r#"
query Conversation($serverId: ID!, $conversationId: ID!) {
  conversation(serverId: $serverId, conversationId: $conversationId) {
    encryptedAt
    mlsGroup {
      groupId incarnation cipherSuite epoch
      devices { userId principalId deviceId addedEpoch }
      pendingAdds { deviceId hasKeyPackage }
      pendingRemoves { deviceId }
      readdRequests { deviceId }
    }
  }
}"#;
const UPLOAD: &str = r#"
mutation Upload($keyPackages: [String!]!) {
  uploadMlsKeyPackages(keyPackages: $keyPackages) { cipherSuite available lastResortNotAfter }
}"#;
const STATUS: &str = "query { myMlsKeyPackageStatus { cipherSuite available } }";
const CLAIM: &str = r#"
mutation Claim($input: ClaimMlsKeyPackagesInput!) {
  claimMlsKeyPackages(input: $input) { principalId deviceId keyPackage keyPackageRef lastResort }
}"#;
const SUBMIT: &str = r#"
mutation Submit($input: SubmitMlsCommitInput!) {
  submitMlsCommit(input: $input) { id incarnation epoch senderDeviceId }
}"#;
const COMMITS: &str = r#"
query Commits($serverId: ID!, $conversationId: ID!, $incarnation: Int!, $afterEpoch: Int) {
  mlsCommits(serverId: $serverId, conversationId: $conversationId, incarnation: $incarnation,
             afterEpoch: $afterEpoch) { commits { epoch message } hasMore }
}"#;
const WELCOMES: &str = "query { myMlsWelcomes { id conversationId incarnation epoch message } }";
const ACK: &str = "mutation Ack($ids: [ID!]!) { ackMlsWelcomes(ids: $ids) }";
const MESSAGE_FIELDS: &str = "id text authorId forwardedFromMessageId mentionedUserIds \
     encryption { incarnation epoch senderDeviceId ciphertext frankingReceipt frankingKeyId sentAtUs }";
const CHANGES: &str = r#"
query Changes($input: MessageChangesInput!) {
  messageChanges(input: $input) {
    changes { kind version message { id text isDeleted encryption { epoch ciphertext frankingReceipt frankingKeyId sentAtUs senderDeviceId } } }
    cursor
  }
}"#;
const DELETE: &str = r#"
mutation Delete($input: DeleteMessageInput!) { deleteMessage(input: $input) }"#;
const REACT: &str = r#"
mutation React($input: ToggleReactionInput!) { toggleReaction(input: $input) { emoji count } }"#;
const REPORT: &str = r#"
mutation Report($input: ReportEncryptedMessageInput!) { reportEncryptedMessage(input: $input) }"#;
const SEND_PLAIN: &str = r#"
mutation Send($input: SendMessageInput!) { sendMessage(input: $input) { id forwardedFromMessageId } }"#;
const REPLY_PLAIN: &str = r#"
mutation Reply($input: ReplyToMessageInput!) { replyToMessage(input: $input) { id } }"#;
const EDIT_PLAIN: &str = r#"
mutation Edit($input: EditMessageInput!) { editMessage(input: $input) { id } }"#;
const FORWARD: &str = r#"
mutation Forward($input: ForwardMessageInput!) { forwardMessage(input: $input) { id } }"#;
const SEARCH: &str = r#"
query Search($input: SearchMessagesInput!) { searchMessages(input: $input) { items { id conversationId } } }"#;
const UPDATE_SETTINGS: &str = r#"
mutation Settings($serverId: ID!, $input: UpdateNotificationSettingsInput!) {
  updateNotificationSettings(serverId: $serverId, input: $input) { digest }
}"#;
const CREATE_ATTACHMENT: &str = r#"
mutation Attach($input: CreateAttachmentInput!) {
  createAttachment(input: $input) { attachmentId uploadToken }
}"#;
const MESSAGE_ATTACHMENTS: &str = r#"
query Message($serverId: ID!, $conversationId: ID!, $messageId: ID!) {
  message(serverId: $serverId, conversationId: $conversationId, messageId: $messageId) {
    attachments { id filename contentType sizeBytes blobSha256 }
  }
}"#;
const READD: &str = r#"
mutation Readd($input: RequestMlsReaddInput!) { requestMlsReadd(input: $input) }"#;
const PRINCIPAL_DEVICES: &str = r#"
query Devices($ids: [String!]!) { principalDevices(principalIds: $ids) { principalId devices { deviceId mlsActive } } }"#;

/// Execute `request`, which must fail, and return its first error's code and extensions.
async fn exec_err(schema: &AppSchema, request: Request) -> (String, Value) {
    let res = schema.execute(request).await;
    let err = res.errors.first().expect("an error");
    let extensions = serde_json::to_value(&err.extensions).expect("extensions");
    (
        extensions["code"].as_str().unwrap_or_default().to_string(),
        extensions,
    )
}

// ── The MLS test client ─────────────────────────────────────────────────────────────────

/// A signed-in device: its session and its MLS side.
struct Member {
    user: User,
    caller: Caller,
    device_key_id: DeviceKeyId,
    /// The seed of the person's identity, which certifies their devices.
    seed: u8,
    mls: MlsDevice,
}

impl std::ops::Deref for Member {
    type Target = MlsDevice;

    fn deref(&self) -> &MlsDevice {
        &self.mls
    }
}

impl std::ops::DerefMut for Member {
    fn deref_mut(&mut self) -> &mut MlsDevice {
        &mut self.mls
    }
}

impl Member {
    /// A request from this device's session.
    fn request(&self, query: &str, variables: Value) -> Request {
        Request::new(query)
            .variables(Variables::from_json(variables))
            .data(
                AuthActorCtx::new(Some(self.user.id), Some(self.caller.session_id))
                    .with_device_key_id(Some(self.device_key_id)),
            )
    }
}

// ── Fixture ─────────────────────────────────────────────────────────────────────────────

/// A user who enrolled the identity from `seed`, signed in on its first device.
async fn member(ctx: &TestCtx, schema: &AppSchema, name: &str, seed: u8) -> Member {
    let enrolled = ctx.enrolled(schema, name, seed).await;
    let session = SeaOrmSessionRepository::new(ctx.db.clone())
        .get_by_id(enrolled.caller.session_id)
        .await
        .expect("session")
        .expect("session row");
    Member {
        user: enrolled.user,
        caller: enrolled.caller,
        device_key_id: session
            .device_key_id
            .expect("the enrolling session is bound"),
        seed,
        mls: MlsDevice::new(
            enrolled.device_id,
            enrolled.state.principal_id,
            enrolled.device,
            signing_key(seed.wrapping_add(150)),
        ),
    }
}

async fn upload(schema: &AppSchema, m: &Member, one_time: usize) -> Value {
    let mut packages: Vec<String> = (0..one_time).map(|_| m.key_package(false)).collect();
    packages.push(m.key_package(true));
    exec_ok(
        schema,
        m.request(UPLOAD, json!({ "keyPackages": packages })),
    )
    .await["uploadMlsKeyPackages"]
        .clone()
}

struct Fixture {
    ctx: TestCtx,
    schema: AppSchema,
    provider: Arc<dyn AppServiceProvider>,
    server: Server,
    alice: Member,
    bob: Member,
}

async fn fixture() -> Fixture {
    let ctx = TestCtx::new().await;
    let schema = ctx.build_schema(RegistrationMode::InviteOnly).await;
    let provider = ctx.build_provider();
    let server = ctx.create_server("e2ee").await;
    let alice = member(&ctx, &schema, "alice", 1).await;
    let bob = member(&ctx, &schema, "bob", 2).await;
    ctx.add_owner(&server, &alice.user).await;
    ctx.add_member(&server, &bob.user, ServerMemberRole::Member)
        .await;
    Fixture {
        ctx,
        schema,
        provider,
        server,
        alice,
        bob,
    }
}

impl Fixture {
    fn server_id(&self) -> String {
        self.server.id.to_string()
    }

    async fn create_dm(&self, encrypted: bool) -> String {
        let data = exec_ok(
            &self.schema,
            self.alice.request(
                CREATE_CONVERSATION,
                json!({ "input": {
                    "serverId": self.server_id(),
                    "otherUserId": self.bob.user.id.to_string(),
                    "encrypted": encrypted,
                } }),
            ),
        )
        .await;
        data["createConversation"]["id"]
            .as_str()
            .expect("id")
            .to_string()
    }

    /// An encrypted DM whose group alice created and bob joined from the Welcome.
    async fn encrypted_dm(&mut self) -> String {
        upload(&self.schema, &self.alice, 1).await;
        let status = upload(&self.schema, &self.bob, 3).await;
        assert_eq!(status["cipherSuite"], json!(0x004f));
        assert!(status["lastResortNotAfter"].is_string());
        let conv = self.create_dm(true).await;

        // Alice claims bob's key package and creates the group.
        let claimed = exec_ok(
            &self.schema,
            self.alice.request(
                CLAIM,
                json!({ "input": {
                    "serverId": self.server_id(),
                    "conversationId": conv,
                    "devices": [{ "principalId": self.bob.principal_id, "deviceId": self.bob.device_id }],
                } }),
            ),
        )
        .await["claimMlsKeyPackages"]
            .clone();
        assert_eq!(claimed.as_array().map(Vec::len), Some(1));
        assert_eq!(claimed[0]["lastResort"], json!(false));
        let package = MlsMessageIn::tls_deserialize_exact_bytes(&unb64(
            claimed[0]["keyPackage"].as_str().expect("keyPackage"),
        ))
        .expect("decode");
        let MlsMessageBodyIn::KeyPackage(package) = package.extract() else {
            panic!("not a key package");
        };
        let package = package
            .validate(self.alice.provider.crypto(), ProtocolVersion::Mls10)
            .expect("valid");
        let key_package_ref = package
            .hash_ref(self.alice.provider.crypto())
            .expect("ref")
            .as_slice()
            .to_vec();
        assert_eq!(b64(&key_package_ref), claimed[0]["keyPackageRef"]);

        let conversation_id: Uuid = conv.parse().expect("uuid");
        let config = MlsGroupCreateConfig::builder()
            .ciphersuite(SUITE)
            .capabilities(capabilities())
            .wire_format_policy(PURE_CIPHERTEXT_WIRE_FORMAT_POLICY)
            .use_ratchet_tree_extension(true)
            .padding_size(256)
            .max_past_epochs(2)
            .build();
        let mut group_id = b"rim1".to_vec();
        group_id.extend_from_slice(conversation_id.as_bytes());
        group_id.extend_from_slice(&1u32.to_be_bytes());
        let mut group = MlsGroup::new_with_group_id(
            &self.alice.provider,
            &self.alice.signer,
            &config,
            GroupId::from_slice(&group_id),
            self.alice.credential(),
        )
        .expect("group");
        group.set_aad(commit_aad(&[(&self.bob.mls, key_package_ref)], &[]));
        let (commit, welcome, _) = group
            .add_members(&self.alice.provider, &self.alice.signer, &[package])
            .expect("add");
        let data = exec_ok(
            &self.schema,
            self.alice.request(
                SUBMIT,
                json!({ "input": {
                    "serverId": self.server_id(),
                    "conversationId": conv,
                    "incarnation": 1,
                    "epoch": 0,
                    "cipherSuite": 0x004f,
                    "commit": b64(&commit.to_bytes().expect("commit")),
                    "welcome": b64(&welcome.to_bytes().expect("welcome")),
                } }),
            ),
        )
        .await;
        assert_eq!(data["submitMlsCommit"]["epoch"], json!(0));
        assert_eq!(
            data["submitMlsCommit"]["senderDeviceId"],
            json!(self.alice.device_id.to_string())
        );
        group
            .merge_pending_commit(&self.alice.provider)
            .expect("merge");
        self.alice.group = Some(group);

        // Bob joins from the Welcome and acknowledges it.
        let welcomes =
            exec_ok(&self.schema, self.bob.request(WELCOMES, json!({}))).await["myMlsWelcomes"]
                .clone();
        assert_eq!(welcomes.as_array().map(Vec::len), Some(1));
        assert_eq!(welcomes[0]["conversationId"], json!(conv));
        assert_eq!(welcomes[0]["epoch"], json!(1));
        let welcome = MlsMessageIn::tls_deserialize_exact_bytes(&unb64(
            welcomes[0]["message"].as_str().expect("message"),
        ))
        .expect("decode");
        let MlsMessageBodyIn::Welcome(welcome) = welcome.extract() else {
            panic!("not a Welcome");
        };
        let staged =
            StagedWelcome::new_from_welcome(&self.bob.provider, &join_config(), welcome, None)
                .expect("staged");
        let joined = staged.into_group(&self.bob.provider).expect("join");
        assert_eq!(joined.group_id().as_slice(), group_id.as_slice());
        self.bob.group = Some(joined);
        // Another device cannot acknowledge bob's Welcome.
        let foreign = exec_ok(
            &self.schema,
            self.alice
                .request(ACK, json!({ "ids": [welcomes[0]["id"].clone()] })),
        )
        .await;
        assert_eq!(foreign["ackMlsWelcomes"], json!(0));
        let acked = exec_ok(
            &self.schema,
            self.bob
                .request(ACK, json!({ "ids": [welcomes[0]["id"].clone()] })),
        )
        .await;
        assert_eq!(acked["ackMlsWelcomes"], json!(1));
        let none = exec_ok(&self.schema, self.bob.request(WELCOMES, json!({}))).await;
        assert_eq!(none["myMlsWelcomes"], json!([]));
        conv
    }

    async fn group(&self, conv: &str) -> Value {
        exec_ok(
            &self.schema,
            self.alice.request(
                CONVERSATION,
                json!({ "serverId": self.server_id(), "conversationId": conv }),
            ),
        )
        .await["conversation"]
            .clone()
    }

    fn send_request(
        &mut self,
        who: Who,
        conv: &str,
        id: Uuid,
        body: &str,
        mentions: &[&str],
    ) -> (Request, Vec<u8>, [u8; 32]) {
        let fk: [u8; 32] = std::array::from_fn(|i| i as u8 ^ id.as_bytes()[i % 16]);
        let content = content(id, 0, body, mentions);
        let server_id = self.server_id();
        let m = self.member_mut(who);
        let envelope = m.encrypt(id, &content, fk, mentions);
        let request = m.request(
            &format!("mutation Send($input: SendEncryptedMessageInput!) {{ sendEncryptedMessage(input: $input) {{ {MESSAGE_FIELDS} }} }}"),
            json!({ "input": {
                "serverId": server_id,
                "conversationId": conv,
                "messageId": id,
                "envelope": envelope,
            } }),
        );
        (request, content, fk)
    }

    fn member_mut(&mut self, who: Who) -> &mut Member {
        match who {
            Who::Alice => &mut self.alice,
            Who::Bob => &mut self.bob,
        }
    }

    /// Bob reads the conversation's changes after `after` and returns them.
    async fn changes(&self, who: Who, conv: &str, after: Option<&str>) -> Value {
        let m = match who {
            Who::Alice => &self.alice,
            Who::Bob => &self.bob,
        };
        exec_ok(
            &self.schema,
            m.request(
                CHANGES,
                json!({ "input": {
                    "serverId": self.server_id(),
                    "conversations": [{ "conversationId": conv, "after": after }],
                } }),
            ),
        )
        .await["messageChanges"][0]
            .clone()
    }
}

impl Fixture {
    /// A second device of bob's, certified and with key packages, in no group.
    async fn bobs_phone(&self) -> Member {
        let identity = TestIdentity::with_version(self.bob.seed, StatementVersion::V2);
        let state = KeyState::from_inception(&identity.inception()).expect("inception");
        let device_id = Uuid::now_v7();
        let key = signing_key(self.bob.seed.wrapping_add(70));
        let cert = identity.device_cert(&state, device_id, &key, "Phone");
        exec_ok(
            &self.schema,
            self.bob
                .caller
                .request(ADD_DEVICE, json!({ "certificate": statement(&cert) })),
        )
        .await;
        let row = SeaOrmKeyIdentityRepository::new(self.ctx.db.clone())
            .get_device(self.bob.user.id, device_id)
            .await
            .expect("get device")
            .expect("device row");
        let phone = Member {
            user: self.bob.user.clone(),
            caller: self.ctx.caller(&self.bob.user).await,
            device_key_id: row.id,
            seed: self.bob.seed,
            mls: MlsDevice::new(
                device_id,
                state.principal_id,
                key,
                signing_key(self.bob.seed.wrapping_add(170)),
            ),
        };
        upload(&self.schema, &phone, 1).await;
        phone
    }
}

#[derive(Clone, Copy)]
enum Who {
    Alice,
    Bob,
}

/// Captures every email the digest job sends.
#[derive(Clone, Default)]
struct Outbox(Arc<Mutex<Vec<EmailMessage>>>);

#[async_trait::async_trait]
impl EmailSender for Outbox {
    async fn send(&self, message: EmailMessage) -> anyhow::Result<()> {
        self.0.lock().expect("outbox").push(message);
        Ok(())
    }
}

/// A report revealing one message's content, franking key and receipt.
fn report(
    server_id: &str,
    conv: &str,
    message_id: Uuid,
    content: &[u8],
    fk: &[u8],
    encryption: &Value,
    sender: &Member,
) -> Value {
    json!({ "input": {
        "serverId": server_id,
        "conversationId": conv,
        "messageId": message_id,
        "reason": "abuse",
        "content": b64(content),
        "frankingKey": b64(fk),
        "receipt": encryption["frankingReceipt"],
        "receiptKeyId": encryption["frankingKeyId"],
        "senderUserId": sender.user.id.to_string(),
        "senderDeviceId": encryption["senderDeviceId"],
        "sentAtUs": encryption["sentAtUs"],
    } })
}

// ── Tests ───────────────────────────────────────────────────────────────────────────────

/// Create, Welcome, send with a mention, decrypt from the feed, repeat, edit, react, report
/// (valid, tampered, wrong key, twice, after a delete), delete, search and the digest.
#[tokio::test]
async fn an_encrypted_dm_carries_messages_edits_reactions_and_reports() {
    let mut f = fixture().await;
    // The test provider's franking key, so the receipt can be recomputed here.
    let server_key = FRANKING_KEY.to_vec();
    let conv = f.encrypted_dm().await;

    let group = f.group(&conv).await;
    assert!(group["encryptedAt"].is_string());
    let mls = &group["mlsGroup"];
    assert_eq!(mls["incarnation"], json!(1));
    assert_eq!(mls["cipherSuite"], json!(0x004f));
    assert_eq!(mls["epoch"], json!(1));
    assert_eq!(mls["devices"].as_array().map(Vec::len), Some(2));
    assert_eq!(mls["pendingAdds"], json!([]));
    assert_eq!(mls["pendingRemoves"], json!([]));

    // Alice sends, mentioning bob.
    let id = Uuid::now_v7();
    let bob_principal = f.bob.principal_id.clone();
    let (request, sent_content, fk) =
        f.send_request(Who::Alice, &conv, id, "hi bob", &[&bob_principal]);
    let sent = exec_ok(&f.schema, request).await["sendEncryptedMessage"].clone();
    assert_eq!(sent["id"], json!(id.to_string()));
    assert_eq!(sent["text"], Value::Null);
    assert_eq!(sent["mentionedUserIds"], json!([f.bob.user.id.to_string()]));
    let encryption = sent["encryption"].clone();
    assert_eq!(encryption["epoch"], json!(1));
    assert_eq!(
        encryption["senderDeviceId"],
        json!(f.alice.device_id.to_string())
    );
    assert_eq!(encryption["frankingKeyId"], json!(1));

    // The receipt is HMAC(K, context || ids || sentAtUs || commitment), recomputed here.
    let sent_at_us: i64 = encryption["sentAtUs"]
        .as_str()
        .expect("sentAtUs")
        .parse()
        .expect("i64");
    let commitment = hmac(&fk, &[b"rim.e2ee.franking.v1\0", &sent_content]);
    let conversation_id: Uuid = conv.parse().expect("uuid");
    let receipt = hmac(
        &server_key,
        &[
            b"rim.e2ee.franking-receipt.v1\0",
            id.as_bytes(),
            conversation_id.as_bytes(),
            Uuid::from(f.alice.user.id).as_bytes(),
            f.alice.device_id.as_bytes(),
            &sent_at_us.to_be_bytes(),
            &commitment,
        ],
    );
    assert_eq!(encryption["frankingReceipt"], json!(b64(&receipt)));

    // Sending the same id and bytes again answers the same message; other bytes conflict.
    let resend = f.alice.request(
        &format!("mutation Send($input: SendEncryptedMessageInput!) {{ sendEncryptedMessage(input: $input) {{ {MESSAGE_FIELDS} }} }}"),
        json!({ "input": {
            "serverId": f.server_id(), "conversationId": conv, "messageId": id,
            "envelope": encryption["ciphertext"],
        } }),
    );
    let again = exec_ok(&f.schema, resend).await;
    assert_eq!(again["sendEncryptedMessage"]["encryption"], encryption);
    let other = f.alice.encrypt(id, b"other", [9; 32], &[]);
    let conflict = f.alice.request(
        "mutation Send($input: SendEncryptedMessageInput!) { sendEncryptedMessage(input: $input) { id } }",
        json!({ "input": { "serverId": f.server_id(), "conversationId": conv, "messageId": id, "envelope": other } }),
    );
    assert_eq!(exec_err_code(&f.schema, conflict).await, "CONFLICT");

    // Bob decrypts it from the change feed.
    let feed = f.changes(Who::Bob, &conv, None).await;
    let change = &feed["changes"][0];
    assert_eq!(change["kind"], json!("CREATED"));
    assert_eq!(change["message"]["text"], Value::Null);
    let ciphertext = change["message"]["encryption"]["ciphertext"]
        .as_str()
        .expect("ciphertext")
        .to_string();
    let (received, received_fk) = f.bob.decrypt(&ciphertext, &id.to_string());
    assert_eq!(received, sent_content);
    assert_eq!(received_fk, fk);
    let cursor = feed["cursor"].as_str().expect("cursor").to_string();

    // Alice edits it: the same row, a new envelope, an EDITED entry.
    let edited_content = content(id, 1, "hi bob, edited", &[]);
    let edit_fk = [7u8; 32];
    let envelope = f.alice.encrypt(id, &edited_content, edit_fk, &[]);
    let edit = f.alice.request(
        &format!("mutation Edit($input: EditEncryptedMessageInput!) {{ editEncryptedMessage(input: $input) {{ {MESSAGE_FIELDS} }} }}"),
        json!({ "input": { "serverId": f.server_id(), "conversationId": conv, "messageId": id, "envelope": envelope } }),
    );
    let edited = exec_ok(&f.schema, edit).await["editEncryptedMessage"].clone();
    assert_eq!(
        edited["mentionedUserIds"],
        json!([]),
        "an edit replaces the mention rows"
    );
    let feed = f.changes(Who::Bob, &conv, Some(&cursor)).await;
    let change = &feed["changes"][0];
    assert_eq!(change["kind"], json!("EDITED"));
    let edited_encryption = change["message"]["encryption"].clone();
    let (received, _) = f.bob.decrypt(
        edited_encryption["ciphertext"]
            .as_str()
            .expect("ciphertext"),
        &id.to_string(),
    );
    assert_eq!(received, edited_content);

    // Reactions stay server metadata.
    let react = f.bob.request(
        REACT,
        json!({ "input": { "serverId": f.server_id(), "conversationId": conv, "messageId": id, "emoji": "👍" } }),
    );
    assert_eq!(
        exec_ok(&f.schema, react).await["toggleReaction"][0]["count"],
        json!(1)
    );

    // Bob reports the edit he holds.
    let server_id = f.server_id();
    let tampered = report(
        &server_id,
        &conv,
        id,
        b"something else",
        &edit_fk,
        &edited_encryption,
        &f.alice,
    );
    assert_eq!(
        exec_err_code(&f.schema, f.bob.request(REPORT, tampered)).await,
        "VALIDATION"
    );
    let wrong_key = report(
        &server_id,
        &conv,
        id,
        &edited_content,
        &[8; 32],
        &edited_encryption,
        &f.alice,
    );
    assert_eq!(
        exec_err_code(&f.schema, f.bob.request(REPORT, wrong_key)).await,
        "VALIDATION"
    );
    let valid = report(
        &server_id,
        &conv,
        id,
        &edited_content,
        &edit_fk,
        &edited_encryption,
        &f.alice,
    );
    let data = exec_ok(&f.schema, f.bob.request(REPORT, valid.clone())).await;
    assert_eq!(data["reportEncryptedMessage"], json!(true));
    assert_eq!(
        exec_err_code(&f.schema, f.bob.request(REPORT, valid)).await,
        "CONFLICT"
    );
    let stored: Option<Vec<u8>> = f
        .ctx
        .conn
        .query_one_raw(sea_orm::Statement::from_sql_and_values(
            sea_orm::DbBackend::Postgres,
            "SELECT franked_content FROM message_report WHERE message_id = $1",
            [id.into()],
        ))
        .await
        .expect("query")
        .expect("report row")
        .try_get("", "franked_content")
        .expect("column");
    assert_eq!(stored.as_deref(), Some(edited_content.as_slice()));

    // A second message, deleted by its sender before bob reports it.
    let second = Uuid::now_v7();
    let (request, second_content, second_fk) =
        f.send_request(Who::Alice, &conv, second, "regret", &[]);
    let second_message = exec_ok(&f.schema, request).await["sendEncryptedMessage"].clone();
    let feed = f
        .changes(
            Who::Bob,
            &conv,
            Some(feed["cursor"].as_str().expect("cursor")),
        )
        .await;
    let (received, _) = f.bob.decrypt(
        feed["changes"][0]["message"]["encryption"]["ciphertext"]
            .as_str()
            .expect("ciphertext"),
        &second.to_string(),
    );
    assert_eq!(received, second_content);
    let delete = f.alice.request(
        DELETE,
        json!({ "input": { "serverId": f.server_id(), "conversationId": conv, "messageId": second } }),
    );
    exec_ok(&f.schema, delete).await;
    let feed = f
        .changes(
            Who::Bob,
            &conv,
            Some(feed["cursor"].as_str().expect("cursor")),
        )
        .await;
    let tombstone = &feed["changes"][0];
    assert_eq!(tombstone["kind"], json!("DELETED"));
    assert_eq!(
        tombstone["message"]["encryption"]["ciphertext"],
        Value::Null
    );
    assert_eq!(
        tombstone["message"]["encryption"]["frankingReceipt"],
        second_message["encryption"]["frankingReceipt"],
        "franking survives the delete"
    );
    let after_delete = report(
        &server_id,
        &conv,
        second,
        &second_content,
        &second_fk,
        &second_message["encryption"],
        &f.alice,
    );
    let data = exec_ok(&f.schema, f.bob.request(REPORT, after_delete)).await;
    assert_eq!(data["reportEncryptedMessage"], json!(true));

    // Search never returns encrypted rows, even for an operator-only query.
    for query in ["hi", "from:alice"] {
        let search = f.alice.request(
            SEARCH,
            json!({ "input": { "serverId": f.server_id(), "query": query } }),
        );
        let items = exec_ok(&f.schema, search).await["searchMessages"]["items"].clone();
        assert!(
            items
                .as_array()
                .expect("items")
                .iter()
                .all(|i| i["conversationId"] != json!(conv)),
            "{query}: {items}"
        );
    }

    // Bob's digest names the sender, never the content.
    exec_ok(
        &f.schema,
        f.bob.request(
            UPDATE_SETTINGS,
            json!({ "serverId": f.server_id(), "input": {
                "defaultLevel": "ALL", "keywords": [], "dnd": null, "digest": "HOURLY",
            } }),
        ),
    )
    .await;
    let outbox = Outbox::default();
    let observers =
        rim_services::events::Observers::new(Arc::new(rim_services::events::NoopBroadcaster));
    let jobs = rim_infra_services::notification::DefaultNotificationJobs::new(
        rim_infra_services::AppRepos::with_handle(&f.ctx.db).for_notification_jobs(),
        Arc::new(outbox.clone()),
        Arc::new(rim_infra_services::presence::PresenceManager::new(
            Arc::new(observers),
        )),
        None,
    );
    use rim_infra_services::notification::NotificationJobs;
    let run = jobs
        .run_digest(chrono::Utc::now() + chrono::Duration::hours(2))
        .await
        .expect("digest");
    assert_eq!(run.sent, 1);
    let emails = outbox.0.lock().expect("outbox");
    assert!(
        emails[0].body_text.contains("Encrypted message from alice"),
        "{}",
        emails[0].body_text
    );
    assert!(!emails[0].body_text.contains("hi bob"));
}

/// Plaintext sends, replies, edits and forwards are refused in an encrypted DM; forwarding is
/// sending the content again, with the origin recorded.
#[tokio::test]
async fn plaintext_writes_are_refused_in_an_encrypted_dm() {
    let mut f = fixture().await;
    let conv = f.encrypted_dm().await;
    let id = Uuid::now_v7();
    let (request, _, _) = f.send_request(Who::Alice, &conv, id, "secret", &[]);
    exec_ok(&f.schema, request).await;

    let server_id = f.server_id();
    let refusals = [
        (
            SEND_PLAIN,
            json!({ "input": { "serverId": server_id, "conversationId": conv, "text": "plain" } }),
        ),
        (
            REPLY_PLAIN,
            json!({ "input": { "serverId": server_id, "conversationId": conv, "rootMessageId": id, "text": "plain" } }),
        ),
        (
            EDIT_PLAIN,
            json!({ "input": { "serverId": server_id, "conversationId": conv, "messageId": id, "text": "plain" } }),
        ),
    ];
    for (query, variables) in refusals {
        let (code, _) = exec_err(&f.schema, f.alice.request(query, variables)).await;
        assert_eq!(code, "VALIDATION", "{query}");
    }

    // A channel to forward to and from.
    let channel = exec_ok(
        &f.schema,
        f.alice.request(
            CREATE_CONVERSATION,
            json!({ "input": { "serverId": server_id, "isChannel": true, "title": "general" } }),
        ),
    )
    .await["createConversation"]["id"]
        .as_str()
        .expect("id")
        .to_string();
    let plain = exec_ok(
        &f.schema,
        f.alice.request(SEND_PLAIN, json!({ "input": { "serverId": server_id, "conversationId": channel, "text": "hello world" } })),
    )
    .await["sendMessage"]["id"]
        .clone();
    for (source_conv, target_conv, message) in [
        (&conv, &channel, json!(id)),
        (&channel, &conv, plain.clone()),
    ] {
        let forward = f.alice.request(
            FORWARD,
            json!({ "input": {
                "serverId": server_id,
                "sourceConversationId": source_conv,
                "targetConversationId": target_conv,
                "messageId": message,
            } }),
        );
        assert_eq!(exec_err_code(&f.schema, forward).await, "VALIDATION");
    }

    // Forwarding the decrypted content into the channel records the origin.
    let forwarded = exec_ok(
        &f.schema,
        f.alice.request(
            SEND_PLAIN,
            json!({ "input": { "serverId": server_id, "conversationId": channel, "text": "secret", "forwardedFromMessageId": id } }),
        ),
    )
    .await;
    assert_eq!(
        forwarded["sendMessage"]["forwardedFromMessageId"],
        json!(id.to_string())
    );
    // A source the caller cannot read is refused.
    let unknown = f.alice.request(
        SEND_PLAIN,
        json!({ "input": { "serverId": server_id, "conversationId": channel, "text": "x", "forwardedFromMessageId": Uuid::now_v7() } }),
    );
    assert_eq!(exec_err_code(&f.schema, unknown).await, "NOT_FOUND");
    // Into the encrypted DM, a forward is an encrypted message naming its origin.
    let into_dm = Uuid::now_v7();
    let content = content(into_dm, 0, "hello world", &[]);
    let envelope = f.alice.encrypt(into_dm, &content, [3; 32], &[]);
    let data = exec_ok(
        &f.schema,
        f.alice.request(
            &format!("mutation Send($input: SendEncryptedMessageInput!) {{ sendEncryptedMessage(input: $input) {{ {MESSAGE_FIELDS} }} }}"),
            json!({ "input": {
                "serverId": server_id, "conversationId": conv, "messageId": into_dm,
                "envelope": envelope, "forwardedFromMessageId": plain,
            } }),
        ),
    )
    .await;
    assert_eq!(
        data["sendEncryptedMessage"]["forwardedFromMessageId"],
        plain
    );

    // A plaintext conversation takes no envelopes.
    let not_encrypted = f.alice.request(
        "mutation Send($input: SendEncryptedMessageInput!) { sendEncryptedMessage(input: $input) { id } }",
        json!({ "input": { "serverId": server_id, "conversationId": channel, "messageId": Uuid::now_v7(), "envelope": envelope } }),
    );
    assert_eq!(exec_err_code(&f.schema, not_encrypted).await, "VALIDATION");
}

/// Messages are accepted in the current epoch only, and wait for a removal to be committed.
#[tokio::test]
async fn messages_follow_the_epoch_and_wait_for_removals() {
    let mut f = fixture().await;
    let conv = f.encrypted_dm().await;
    let server_id = f.server_id();

    // Bob prepares a message in epoch 1; alice's self-update moves the group to epoch 2.
    let late = Uuid::now_v7();
    let (late_request, late_content, _) = f.send_request(Who::Bob, &conv, late, "late", &[]);
    let commit = f.alice.self_update();
    let submit = |epoch: i64, commit: &str| {
        json!({ "input": {
            "serverId": server_id, "conversationId": conv, "incarnation": 1, "epoch": epoch, "commit": commit,
        } })
    };
    exec_ok(&f.schema, f.alice.request(SUBMIT, submit(1, &commit))).await;
    f.alice.merge_pending();
    // The same epoch again is stale.
    let (code, extensions) = exec_err(&f.schema, f.alice.request(SUBMIT, submit(1, &commit))).await;
    assert_eq!(code, "STALE_EPOCH");
    assert_eq!(
        (
            extensions["incarnation"].clone(),
            extensions["epoch"].clone()
        ),
        (json!(1), json!(2))
    );

    let (code, extensions) = exec_err(&f.schema, late_request).await;
    assert_eq!(code, "STALE_EPOCH");
    assert_eq!(extensions["epoch"], json!(2));

    // Bob catches up from the Commit list and sends the same content again, same id.
    let commits = exec_ok(
        &f.schema,
        f.bob.request(COMMITS, json!({ "serverId": server_id, "conversationId": conv, "incarnation": 1, "afterEpoch": 0 })),
    )
    .await["mlsCommits"]
        .clone();
    assert_eq!(commits["commits"][0]["epoch"], json!(1));
    assert_eq!(commits["hasMore"], json!(false));
    f.bob
        .process_commit(commits["commits"][0]["message"].as_str().expect("message"));
    assert_eq!(f.bob.epoch(), 2);
    let envelope = f.bob.encrypt(late, &late_content, [5; 32], &[]);
    let resent = f.bob.request(
        &format!("mutation Send($input: SendEncryptedMessageInput!) {{ sendEncryptedMessage(input: $input) {{ {MESSAGE_FIELDS} }} }}"),
        json!({ "input": { "serverId": server_id, "conversationId": conv, "messageId": late, "envelope": envelope } }),
    );
    assert_eq!(
        exec_ok(&f.schema, resent).await["sendEncryptedMessage"]["encryption"]["epoch"],
        json!(2)
    );

    // Bob's device goes quiet for two months: it is due for removal, and alice must commit
    // that before she sends.
    f.ctx
        .conn
        .execute_unprepared(&format!(
            "UPDATE device_key SET last_key_package_upload_at = now() - interval '61 days' WHERE id = '{}'",
            Uuid::from(f.bob.device_key_id)
        ))
        .await
        .expect("age the device");
    let group = f.group(&conv).await;
    assert_eq!(
        group["mlsGroup"]["pendingRemoves"],
        json!([{ "deviceId": f.bob.device_id.to_string() }])
    );
    let blocked = Uuid::now_v7();
    let (request, _, _) = f.send_request(Who::Alice, &conv, blocked, "wait", &[]);
    let (code, extensions) = exec_err(&f.schema, request).await;
    assert_eq!(code, "MLS_COMMIT_REQUIRED");
    assert_eq!(extensions["reason"], json!("pendingRemovals"));

    let bob_leaf = f
        .alice
        .group()
        .members()
        .find(|m| m.signature_key == f.bob.signer.0.verifying_key().to_bytes())
        .expect("bob's leaf")
        .index;
    let alice = &mut f.alice.mls;
    let group = alice.group.as_mut().expect("group");
    group.set_aad(commit_aad(&[], &[&f.bob.mls]));
    let (commit, _, _) = group
        .remove_members(&alice.provider, &alice.signer, &[bob_leaf])
        .expect("remove");
    let epoch = f.alice.epoch();
    exec_ok(
        &f.schema,
        f.alice.request(
            SUBMIT,
            submit(epoch, &b64(&commit.to_bytes().expect("commit"))),
        ),
    )
    .await;
    f.alice.merge_pending();
    let group = f.group(&conv).await;
    assert_eq!(group["mlsGroup"]["pendingRemoves"], json!([]));
    assert_eq!(
        group["mlsGroup"]["devices"].as_array().map(Vec::len),
        Some(1)
    );
    let (request, _, _) = f.send_request(Who::Alice, &conv, blocked, "now", &[]);
    exec_ok(&f.schema, request).await;
}

/// Without a device-bound session nothing MLS works; an encrypted conversation without a group
/// asks for a Commit; unready members cannot be in an encrypted conversation.
#[tokio::test]
async fn sessions_groups_and_members_must_be_ready() {
    let mut f = fixture().await;
    // A second, password session of alice's, bound to no device.
    let session = SeaOrmSessionRepository::new(f.ctx.db.clone())
        .create_session(rim_core::auth::model::NewSession {
            user_id: f.alice.user.id,
            token_hash: format!("e2e-password-session-{}", f.alice.user.id),
            expires_at: chrono::Utc::now() + chrono::Duration::days(1),
            user_agent: None,
            ip: None,
        })
        .await
        .expect("session");
    let password_session = Caller {
        user_id: f.alice.user.id,
        session_id: session.id,
    };
    let (code, _) = exec_err(&f.schema, password_session.request(STATUS, json!({}))).await;
    assert_eq!(code, "FORBIDDEN");
    let packages = vec![f.alice.key_package(false)];
    let (code, _) = exec_err(
        &f.schema,
        password_session.request(UPLOAD, json!({ "keyPackages": packages })),
    )
    .await;
    assert_eq!(code, "FORBIDDEN");

    // Bob has uploaded nothing yet: he is not ready for an encrypted DM.
    upload(&f.schema, &f.alice, 1).await;
    let refused = f.alice.request(
        CREATE_CONVERSATION,
        json!({ "input": { "serverId": f.server_id(), "otherUserId": f.bob.user.id.to_string(), "encrypted": true } }),
    );
    let res = f.schema.execute(refused).await;
    assert_eq!(
        res.errors[0].message,
        "every member of an encrypted conversation needs a device set up for encryption"
    );
    let status = exec_ok(&f.schema, f.alice.request(STATUS, json!({}))).await;
    assert_eq!(status["myMlsKeyPackageStatus"][0]["available"], json!(1));

    // Once bob uploads, an encrypted group can be created; it has no MLS group yet.
    upload(&f.schema, &f.bob, 1).await;
    let group = exec_ok(
        &f.schema,
        f.alice.request(
            CREATE_CONVERSATION,
            json!({ "input": { "serverId": f.server_id(), "memberIds": [f.bob.user.id.to_string()], "encrypted": true } }),
        ),
    )
    .await["createConversation"]
        .clone();
    assert!(group["encryptedAt"].is_string());
    let group_id = group["id"].as_str().expect("id").to_string();
    let view = f.group(&group_id).await;
    assert_eq!(view["mlsGroup"], Value::Null);

    // An envelope there asks for the group first.
    f.encrypted_dm().await;
    let id = Uuid::now_v7();
    let content = content(id, 0, "x", &[]);
    let envelope = f.alice.encrypt(id, &content, [1; 32], &[]);
    let (code, extensions) = exec_err(
        &f.schema,
        f.alice.request(
            "mutation Send($input: SendEncryptedMessageInput!) { sendEncryptedMessage(input: $input) { id } }",
            json!({ "input": { "serverId": f.server_id(), "conversationId": group_id, "messageId": id, "envelope": envelope } }),
        ),
    )
    .await;
    assert_eq!(code, "MLS_COMMIT_REQUIRED");
    assert_eq!(extensions["reason"], json!("noGroup"));

    // A member without a ready device cannot be added to an encrypted group.
    let carol = f.ctx.create_user("carol").await;
    f.ctx
        .add_member(&f.server, &carol, ServerMemberRole::Member)
        .await;
    let res = f
        .schema
        .execute(f.alice.request(
            ADD_MEMBERS,
            json!({ "serverId": f.server_id(), "conversationId": group_id, "userIds": [carol.id.to_string()] }),
        ))
        .await;
    assert_eq!(
        res.errors[0].message,
        "every member of an encrypted conversation needs a device set up for encryption"
    );

    // Workspace peers see each other's certified devices.
    let devices = exec_ok(
        &f.schema,
        f.alice
            .request(PRINCIPAL_DEVICES, json!({ "ids": [f.bob.principal_id] })),
    )
    .await;
    assert_eq!(
        devices["principalDevices"][0]["devices"][0]["deviceId"],
        json!(f.bob.device_id.to_string())
    );
    assert_eq!(
        devices["principalDevices"][0]["devices"][0]["mlsActive"],
        json!(true)
    );
}

const ADD_DEVICE: &str = r#"
mutation Add($certificate: SignedStatementInput!) { addDeviceKey(certificate: $certificate) { deviceId } }"#;
const REVOKE: &str = r#"
mutation Revoke($revocation: SignedStatementInput!) { revokeDeviceKey(revocation: $revocation) { revokedAt } }"#;

/// Roots that act as the session's device in conversation `$c` of workspace `$s`.
const GROUP_DEVICE_ROOTS: [&str; 5] = [
    "mutation($s: ID!, $c: ID!, $d: [MlsDeviceRefInput!]!) { claimMlsKeyPackages(input: { \
     serverId: $s, conversationId: $c, devices: $d }) { deviceId } }",
    "mutation($s: ID!, $c: ID!, $commit: String!) { submitMlsCommit(input: { serverId: $s, \
     conversationId: $c, incarnation: 1, epoch: 1, commit: $commit }) { epoch } }",
    "mutation($s: ID!, $c: ID!, $new: ID!, $envelope: String!) { sendEncryptedMessage(input: { \
     serverId: $s, conversationId: $c, messageId: $new, envelope: $envelope }) { id } }",
    "mutation($s: ID!, $c: ID!, $m: ID!, $edit: String!) { editEncryptedMessage(input: { \
     serverId: $s, conversationId: $c, messageId: $m, envelope: $edit }) { id } }",
    "mutation($s: ID!, $c: ID!, $readd: SignedStatementInput!) { requestMlsReadd(input: { \
     serverId: $s, conversationId: $c, request: $readd }) }",
];

/// Roots that need conversation membership and no device.
const CONVERSATION_MLS_ROOTS: [&str; 2] = [
    "query($s: ID!, $c: ID!, $limit: Int) { mlsCommits(serverId: $s, conversationId: $c, \
     incarnation: 1, limit: $limit) { hasMore } }",
    "mutation($s: ID!, $c: ID!, $m: ID!, $u: ID!, $dev: ID!) { reportEncryptedMessage(input: { \
     serverId: $s, conversationId: $c, messageId: $m, reason: \"abuse\", content: \"AA\", \
     frankingKey: \"AA\", receipt: \"AA\", receiptKeyId: 1, senderUserId: $u, \
     senderDeviceId: $dev, sentAtUs: \"1\" }) }",
];

/// Roots that act on the session's device alone.
const SESSION_DEVICE_ROOTS: [&str; 4] = [
    "query { myMlsKeyPackageStatus { available } }",
    "query { myMlsWelcomes { id } }",
    "mutation($kp: [String!]!) { uploadMlsKeyPackages(keyPackages: $kp) { available } }",
    "mutation($welcomes: [ID!]!) { ackMlsWelcomes(ids: $welcomes) }",
];

fn calls(roots: &[&'static str], variables: &Value) -> Vec<Call> {
    roots.iter().map(|q| (*q, variables.clone())).collect()
}

/// Every E2EE root refuses a caller without a session. A user outside the workspace and a
/// member outside the conversation get `FORBIDDEN` from the conversation's roots; so does a
/// device that is not in the group from the roots that act as a group device, a member editing
/// another's message, and a revoked device from every root that acts as the session's device.
/// An unknown conversation or message is `NOT_FOUND`, a malformed id or page size `VALIDATION`.
#[tokio::test]
async fn mls_roots_refuse_outsiders_devices_outside_the_group_and_revoked_devices() {
    let mut f = fixture().await;
    let conv = f.encrypted_dm().await;
    let server_id = f.server_id();
    let bobs = Uuid::now_v7();
    let (request, _, _) = f.send_request(Who::Bob, &conv, bobs, "mine", &[]);
    exec_ok(&f.schema, request).await;
    // What the refused callers present: alice's envelopes for a new message, for an edit of
    // bob's and for a message nobody sent, and her Commit, none of them sent.
    let (new, ghost) = (Uuid::now_v7(), Uuid::now_v7());
    let envelope = f
        .alice
        .encrypt(new, &content(new, 0, "x", &[]), [1; 32], &[]);
    let edit = f
        .alice
        .encrypt(bobs, &content(bobs, 1, "y", &[]), [2; 32], &[]);
    let ghost_edit = f
        .alice
        .encrypt(ghost, &content(ghost, 1, "z", &[]), [3; 32], &[]);
    let commit = f.alice.self_update();
    let phone = f.bobs_phone().await;
    let readd = rim_identity::testing::device_mls_readd_at(
        &phone.device_key,
        &phone.principal_id,
        phone.device_id,
        conv.parse().expect("uuid"),
        1,
        None,
        &[7; 16],
        &chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    );
    let here = json!({
        "s": server_id, "c": conv, "m": bobs, "new": new, "commit": commit, "envelope": envelope,
        "edit": edit, "readd": statement(&readd), "limit": null,
        "d": [{ "principalId": f.bob.principal_id, "deviceId": f.bob.device_id }],
        "u": f.bob.user.id.to_string(), "dev": f.bob.device_id,
        "kp": [phone.key_package(false)], "welcomes": [Uuid::now_v7()],
    });
    let group_calls = calls(&GROUP_DEVICE_ROOTS, &here);
    let mut conversation_calls = group_calls.clone();
    conversation_calls.extend(calls(&CONVERSATION_MLS_ROOTS, &here));

    let mut every = conversation_calls.clone();
    every.extend(calls(&SESSION_DEVICE_ROOTS, &here));
    every.push((PRINCIPAL_DEVICES, json!({ "ids": [f.bob.principal_id] })));
    assert_each_refused(&f.schema, &every, "UNAUTHENTICATED", anonymous_request).await;

    let olive = member(&f.ctx, &f.schema, "olive", 3).await;
    let carol = member(&f.ctx, &f.schema, "carol", 4).await;
    f.ctx
        .add_member(&f.server, &carol.user, ServerMemberRole::Member)
        .await;
    for who in [&olive, &carol] {
        assert_each_refused(&f.schema, &conversation_calls, "FORBIDDEN", |q, v| {
            who.request(q, v)
        })
        .await;
    }
    let seen = exec_ok(
        &f.schema,
        olive.request(PRINCIPAL_DEVICES, json!({ "ids": [f.bob.principal_id] })),
    )
    .await;
    assert_eq!(
        seen["principalDevices"],
        json!([{ "principalId": f.bob.principal_id, "devices": [] }])
    );
    assert_each_refused(&f.schema, &group_calls, "FORBIDDEN", |q, v| {
        phone.request(q, v)
    })
    .await;
    assert_eq!(
        exec_err_code(
            &f.schema,
            f.alice.request(GROUP_DEVICE_ROOTS[3], here.clone())
        )
        .await,
        "FORBIDDEN"
    );

    let mut unknown = calls(
        &GROUP_DEVICE_ROOTS,
        &merged(here.clone(), json!({ "c": unknown_id() })),
    );
    unknown.extend(calls(
        &CONVERSATION_MLS_ROOTS,
        &merged(here.clone(), json!({ "c": unknown_id() })),
    ));
    let nothing = merged(here.clone(), json!({ "m": ghost, "edit": ghost_edit }));
    unknown.push((GROUP_DEVICE_ROOTS[3], nothing.clone()));
    unknown.push((CONVERSATION_MLS_ROOTS[1], nothing));
    assert_each_refused(&f.schema, &unknown, "NOT_FOUND", |q, v| {
        f.alice.request(q, v)
    })
    .await;
    let mut malformed = calls(
        &GROUP_DEVICE_ROOTS,
        &merged(here.clone(), json!({ "s": "not-an-id" })),
    );
    malformed.extend(calls(
        &CONVERSATION_MLS_ROOTS,
        &merged(here.clone(), json!({ "s": "not-an-id" })),
    ));
    for limit in [0, 101] {
        malformed.push((
            CONVERSATION_MLS_ROOTS[0],
            merged(here.clone(), json!({ "limit": limit })),
        ));
    }
    let bad_device = json!([{ "principalId": f.bob.principal_id, "deviceId": "not-an-id" }]);
    malformed.extend([
        (
            GROUP_DEVICE_ROOTS[0],
            merged(here.clone(), json!({ "d": bad_device })),
        ),
        (
            SESSION_DEVICE_ROOTS[3],
            merged(here.clone(), json!({ "welcomes": ["not-an-id"] })),
        ),
        (
            PRINCIPAL_DEVICES,
            json!({ "ids": vec![f.bob.principal_id.clone(); 101] }),
        ),
    ]);
    assert_each_refused(&f.schema, &malformed, "VALIDATION", |q, v| {
        f.alice.request(q, v)
    })
    .await;

    // Bob's first device retires itself.
    let state = KeyState::from_inception(
        &TestIdentity::with_version(f.bob.seed, StatementVersion::V2).inception(),
    )
    .expect("inception");
    let revocation = device_self_revoke(&f.bob.device_key, &state, f.bob.device_id);
    exec_ok(
        &f.schema,
        f.bob
            .caller
            .request(REVOKE, json!({ "revocation": statement(&revocation) })),
    )
    .await;
    let mut device_calls = group_calls;
    device_calls.extend(calls(&SESSION_DEVICE_ROOTS, &here));
    assert_each_refused(&f.schema, &device_calls, "FORBIDDEN", |q, v| {
        f.bob.request(q, v)
    })
    .await;
}

/// An encrypted conversation's attachments are opaque blobs, checked against their SHA-256.
#[tokio::test]
async fn encrypted_attachments_are_checked_by_hash() {
    let mut f = fixture().await;
    let conv = f.encrypted_dm().await;
    let server_id = f.server_id();
    let id = Uuid::now_v7();
    let (request, _, _) = f.send_request(Who::Alice, &conv, id, "file", &[]);
    exec_ok(&f.schema, request).await;

    // The known-answer blob of `hello, attachment\n` and its hash.
    let blob = (0..34)
        .map(|i| {
            u8::from_str_radix(
                &"096d52fa9b833e37ef33d387fc5ecab34eabb410807b85e90c862a44614c7c9cb163"
                    [i * 2..i * 2 + 2],
                16,
            )
            .expect("hex")
        })
        .collect::<Vec<u8>>();
    let blob_sha256 = "EHFcZ6hUOIdDi32ptdrCXM2PTHQwKF46-0Ls56yQrac";
    assert_eq!(b64(&Sha256::digest(&blob)), blob_sha256);

    let attach = |filename: &str, content_type: &str, hash: Option<&str>| {
        f.alice.request(
            CREATE_ATTACHMENT,
            json!({ "input": {
                "serverId": server_id, "conversationId": conv, "messageId": id,
                "filename": filename, "contentType": content_type, "sizeBytes": 34, "blobSha256": hash,
            } }),
        )
    };
    for request in [
        attach("encrypted", "application/octet-stream", None),
        attach("hello.txt", "text/plain", Some(blob_sha256)),
        attach("encrypted", "application/octet-stream", Some("short")),
    ] {
        assert_eq!(exec_err_code(&f.schema, request).await, "VALIDATION");
    }
    let ticket = exec_ok(
        &f.schema,
        attach("encrypted", "application/octet-stream", Some(blob_sha256)),
    )
    .await["createAttachment"]
        .clone();
    let attachment_id: Uuid = ticket["attachmentId"]
        .as_str()
        .expect("id")
        .parse()
        .expect("uuid");
    let upload_token = ticket["uploadToken"].as_str().expect("token").to_string();

    let attachments = f.provider.message_attachment_service();
    let complete = |data: Vec<u8>, content_type: &str| {
        rim_services::message_attachment::command::CompleteUploadCmd {
            actor_user_id: f.alice.user.id,
            attachment_id: attachment_id.into(),
            upload_token: upload_token.clone(),
            uploaded_filename: "encrypted".to_string(),
            content_type: content_type.to_string(),
            data,
        }
    };
    let mut tampered = blob.clone();
    tampered[0] ^= 1;
    for (data, content_type) in [
        (tampered, "application/octet-stream"),
        (blob[..33].to_vec(), "application/octet-stream"),
        (blob.clone(), "text/plain"),
    ] {
        let err = attachments
            .complete_upload(complete(data, content_type))
            .await
            .unwrap_err();
        assert!(
            matches!(err, rim_services::ServiceError::Validation(_)),
            "{err:?}"
        );
    }
    attachments
        .complete_upload(complete(blob.clone(), "application/octet-stream"))
        .await
        .expect("the blob matches");

    let shown = exec_ok(
        &f.schema,
        f.bob.request(
            MESSAGE_ATTACHMENTS,
            json!({ "serverId": server_id, "conversationId": conv, "messageId": id }),
        ),
    )
    .await["message"]["attachments"][0]
        .clone();
    assert_eq!(shown["id"], json!(attachment_id.to_string()));
    assert_eq!(shown["blobSha256"], json!(blob_sha256));
    assert_eq!(shown["contentType"], json!("application/octet-stream"));
    let download = attachments
        .authorize_download(
            rim_services::message_attachment::command::AuthorizeDownloadQuery {
                actor_user_id: f.bob.user.id,
                attachment_id: attachment_id.into(),
            },
        )
        .await
        .expect("download");
    assert_eq!(
        download.blob_sha256.as_deref(),
        Some(unb64(blob_sha256).as_slice())
    );

    // A plaintext conversation takes no blob hash, and no opaque uploads.
    let channel = exec_ok(
        &f.schema,
        f.alice.request(
            CREATE_CONVERSATION,
            json!({ "input": { "serverId": server_id, "isChannel": true, "title": "files" } }),
        ),
    )
    .await["createConversation"]["id"]
        .as_str()
        .expect("id")
        .to_string();
    let plain = exec_ok(
        &f.schema,
        f.alice.request(
            SEND_PLAIN,
            json!({ "input": { "serverId": server_id, "conversationId": channel, "text": "f" } }),
        ),
    )
    .await["sendMessage"]["id"]
        .clone();
    let with_hash = f.alice.request(
        CREATE_ATTACHMENT,
        json!({ "input": {
            "serverId": server_id, "conversationId": channel, "messageId": plain,
            "filename": "a.txt", "contentType": "text/plain", "sizeBytes": 3, "blobSha256": blob_sha256,
        } }),
    );
    assert_eq!(exec_err_code(&f.schema, with_hash).await, "VALIDATION");
}

const REPORTS: &str = r#"
query Reports($serverId: ID!) {
  messageReports(serverId: $serverId, dir: AFTER, limit: 20) {
    items { id messageId status resolvedAt messageReadable messageText messageAuthorId messageSentAt
            frankingVerifiedAt frankedSenderDeviceId frankedContent frankedContentExpiresAt }
  }
}"#;
const REPORT_ONE: &str = r#"
query Report($serverId: ID!, $reportId: ID!) {
  messageReport(serverId: $serverId, reportId: $reportId) {
    id messageAuthorId messageSentAt frankingVerifiedAt frankedSenderDeviceId frankedContent
  }
}"#;
const RESOLVE: &str = r#"
mutation Resolve($serverId: ID!, $reportId: ID!) {
  resolveReport(serverId: $serverId, reportId: $reportId, outcome: RESOLVED) {
    resolvedAt frankedContent frankedContentExpiresAt
  }
}"#;

/// A moderator outside an encrypted DM reads a verified report, downloads its attachment
/// through the report and deletes the message; a member can do none of it.
#[tokio::test]
async fn a_moderator_outside_the_dm_reads_downloads_and_deletes_a_reported_message() {
    let mut f = fixture().await;
    let conv = f.encrypted_dm().await;
    let server_id = f.server_id();
    let carol = f.ctx.create_user("carol").await;
    f.ctx
        .add_member(&f.server, &carol, ServerMemberRole::Moderator)
        .await;
    let carol = Caller {
        user_id: carol.id,
        session_id: f.ctx.create_session(&carol).await,
    };

    // Alice sends a message naming the appendix A.3 attachment, then uploads its blob.
    let blob_sha256 = "EHFcZ6hUOIdDi32ptdrCXM2PTHQwKF46-0Ls56yQrac";
    let blob: Vec<u8> = (0..34)
        .map(|i| {
            u8::from_str_radix(
                &"096d52fa9b833e37ef33d387fc5ecab34eabb410807b85e90c862a44614c7c9cb163"
                    [i * 2..i * 2 + 2],
                16,
            )
            .expect("hex")
        })
        .collect();
    let id = Uuid::now_v7();
    let mut sent_content: Value =
        serde_json::from_slice(&content(id, 0, "look", &[])).expect("json");
    sent_content["attachments"] = json!([{
        "blobSha256": blob_sha256, "secret": "oKGio6SlpqeoqaqrrK2ur7CxsrO0tba3uLm6u7y9vr8",
        "name": "hello.txt", "mime": "text/plain", "size": 18,
    }]);
    let sent_content = sent_content.to_string().into_bytes();
    let fk = [0x60u8; 32];
    let envelope = f.alice.encrypt(id, &sent_content, fk, &[]);
    let sent = exec_ok(
        &f.schema,
        f.alice.request(
            &format!("mutation Send($input: SendEncryptedMessageInput!) {{ sendEncryptedMessage(input: $input) {{ {MESSAGE_FIELDS} }} }}"),
            json!({ "input": {
                "serverId": server_id, "conversationId": conv, "messageId": id, "envelope": envelope,
            } }),
        ),
    )
    .await["sendEncryptedMessage"]
        .clone();
    let ticket = exec_ok(
        &f.schema,
        f.alice.request(
            CREATE_ATTACHMENT,
            json!({ "input": {
                "serverId": server_id, "conversationId": conv, "messageId": id,
                "filename": "encrypted", "contentType": "application/octet-stream",
                "sizeBytes": 34, "blobSha256": blob_sha256,
            } }),
        ),
    )
    .await["createAttachment"]
        .clone();
    let attachment_id: Uuid = ticket["attachmentId"]
        .as_str()
        .expect("id")
        .parse()
        .expect("uuid");
    let stored = f
        .provider
        .message_attachment_service()
        .complete_upload(
            rim_services::message_attachment::command::CompleteUploadCmd {
                actor_user_id: f.alice.user.id,
                attachment_id: attachment_id.into(),
                upload_token: ticket["uploadToken"].as_str().expect("token").to_string(),
                uploaded_filename: "encrypted".to_string(),
                content_type: "application/octet-stream".to_string(),
                data: blob,
            },
        )
        .await
        .expect("upload")
        .storage_key
        .expect("stored");

    // Bob reports it: a tampered report is refused, the exact one verified.
    let tampered = report(
        &server_id,
        &conv,
        id,
        b"{}",
        &fk,
        &sent["encryption"],
        &f.alice,
    );
    assert_eq!(
        exec_err_code(&f.schema, f.bob.request(REPORT, tampered)).await,
        "VALIDATION"
    );
    let valid = report(
        &server_id,
        &conv,
        id,
        &sent_content,
        &fk,
        &sent["encryption"],
        &f.alice,
    );
    exec_ok(&f.schema, f.bob.request(REPORT, valid)).await;

    // The queue shows the verified report without its content.
    let items = exec_ok(
        &f.schema,
        carol.request(REPORTS, json!({ "serverId": server_id })),
    )
    .await["messageReports"]["items"]
        .clone();
    assert_eq!(items.as_array().map(Vec::len), Some(1));
    let listed = &items[0];
    assert_eq!(listed["messageId"], json!(id.to_string()));
    assert_eq!(listed["messageReadable"], json!(false));
    assert_eq!(listed["messageText"], Value::Null);
    assert_eq!(
        listed["messageAuthorId"],
        json!(f.alice.user.id.to_string())
    );
    assert!(listed["messageSentAt"].is_string());
    assert!(listed["frankingVerifiedAt"].is_string());
    assert_eq!(
        listed["frankedSenderDeviceId"],
        json!(f.alice.device_id.to_string())
    );
    assert_eq!(listed["frankedContent"], Value::Null);
    assert_eq!(listed["frankedContentExpiresAt"], Value::Null);
    let report_id = listed["id"].as_str().expect("id").to_string();

    // A member may not read it; the moderator reads the exact content, and that is audited.
    let one = json!({ "serverId": server_id, "reportId": report_id });
    assert_eq!(
        exec_err_code(&f.schema, f.bob.request(REPORT_ONE, one.clone())).await,
        "FORBIDDEN"
    );
    assert_eq!(
        exec_err_code(
            &f.schema,
            f.bob.request(REPORTS, json!({ "serverId": server_id }))
        )
        .await,
        "FORBIDDEN"
    );
    let opened =
        exec_ok(&f.schema, carol.request(REPORT_ONE, one.clone())).await["messageReport"].clone();
    assert_eq!(opened["frankedContent"], json!(b64(&sent_content)));
    assert_eq!(
        opened["frankedSenderDeviceId"],
        json!(f.alice.device_id.to_string())
    );
    let viewed = f
        .ctx
        .conn
        .query_one_raw(sea_orm::Statement::from_sql_and_values(
            sea_orm::DbBackend::Postgres,
            "SELECT count(*) AS n FROM command_audit_log \
             WHERE command_id = 'e2ee.report_viewed' AND actor_user_id = $1 \
               AND args_redacted->>'report_id' = $2",
            [Uuid::from(carol.user_id).into(), report_id.clone().into()],
        ))
        .await
        .expect("query")
        .expect("row")
        .try_get::<i64>("", "n")
        .expect("count");
    assert_eq!(viewed, 1);

    // The blob comes through the report to the moderator only, and only by its hash.
    let moderation = f.provider.moderation_service();
    let ask = |actor: rim_core::user::model::UserId, hash: &str| {
        rim_services::moderation::command::AuthorizeReportAttachmentQuery {
            actor_user_id: actor,
            report_id: report_id.parse::<Uuid>().expect("uuid").into(),
            blob_sha256: unb64(hash),
        }
    };
    let served = moderation
        .authorize_report_attachment(ask(carol.user_id, blob_sha256))
        .await
        .expect("served");
    assert_eq!(served.storage_key, stored);
    for (actor, hash) in [
        (f.bob.user.id, blob_sha256),
        (carol.user_id, "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8"),
    ] {
        let err = moderation
            .authorize_report_attachment(ask(actor, hash))
            .await
            .unwrap_err();
        assert!(
            matches!(err, rim_services::ServiceError::NotFound),
            "{err:?}"
        );
    }

    // Outside the DM the moderator deletes only the reported message.
    let other = Uuid::now_v7();
    let (request, _, _) = f.send_request(Who::Alice, &conv, other, "unreported", &[]);
    exec_ok(&f.schema, request).await;
    let delete = |message_id: Uuid| {
        carol.request(
            DELETE,
            json!({ "input": { "serverId": server_id, "conversationId": conv, "messageId": message_id } }),
        )
    };
    assert_eq!(exec_err_code(&f.schema, delete(other)).await, "FORBIDDEN");
    exec_ok(&f.schema, delete(id)).await;
    let feed = f.changes(Who::Bob, &conv, None).await;
    let deleted = feed["changes"]
        .as_array()
        .expect("changes")
        .iter()
        .find(|c| c["message"]["id"] == json!(id.to_string()))
        .expect("the reported message")
        .clone();
    assert_eq!(deleted["kind"], json!("DELETED"));
    assert_eq!(deleted["message"]["encryption"]["ciphertext"], Value::Null);

    // Resolving starts the content's 90 days; until then it stays readable.
    let resolved =
        exec_ok(&f.schema, carol.request(RESOLVE, one.clone())).await["resolveReport"].clone();
    assert_eq!(resolved["frankedContent"], Value::Null);
    let resolved_at: chrono::DateTime<chrono::Utc> = resolved["resolvedAt"]
        .as_str()
        .expect("resolvedAt")
        .parse()
        .expect("time");
    let expires_at: chrono::DateTime<chrono::Utc> = resolved["frankedContentExpiresAt"]
        .as_str()
        .expect("expiresAt")
        .parse()
        .expect("time");
    assert_eq!(expires_at - resolved_at, chrono::Duration::days(90));
    let reopened =
        exec_ok(&f.schema, carol.request(REPORT_ONE, one)).await["messageReport"].clone();
    assert_eq!(reopened["frankedContent"], json!(b64(&sent_content)));
}

/// A device whose clock runs a few minutes ahead of the server's still uploads key packages
/// that expire 90 days after its own clock; ten minutes ahead is refused.
#[tokio::test]
async fn finding_10_key_packages_from_a_clock_minutes_ahead_are_accepted() {
    let f = fixture().await;
    let ahead = f.alice.key_package_for(false, 90 * 86_400 + 4 * 60);
    let data = exec_ok(
        &f.schema,
        f.alice.request(UPLOAD, json!({ "keyPackages": [ahead] })),
    )
    .await;
    assert_eq!(data["uploadMlsKeyPackages"]["available"], json!(1));
    let too_far = f.alice.key_package_for(false, 90 * 86_400 + 10 * 60);
    let (code, extensions) = exec_err(
        &f.schema,
        f.alice.request(UPLOAD, json!({ "keyPackages": [too_far] })),
    )
    .await;
    assert_eq!(code, "VALIDATION", "{extensions}");
}

/// A re-add request is accepted once: the same signed request sent again is refused, and so is
/// one issued more than ten minutes ago or one from before the device's latest add.
#[tokio::test]
async fn finding_9_a_replayed_readd_request_is_refused() {
    let mut f = fixture().await;
    let conv = f.encrypted_dm().await;
    let conversation_id: Uuid = conv.parse().expect("uuid");
    let bob = &f.bob;
    let request = |last_epoch: Option<u64>, nonce: u8, issued_at: chrono::DateTime<chrono::Utc>| {
        let signed = rim_identity::testing::device_mls_readd_at(
            &bob.device_key,
            &bob.principal_id,
            bob.device_id,
            conversation_id,
            1,
            last_epoch,
            &[nonce; 16],
            &issued_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        );
        bob.request(
            READD,
            json!({ "input": {
                "serverId": f.server_id(),
                "conversationId": conv,
                "request": statement(&signed),
            } }),
        )
    };
    let now = chrono::Utc::now();
    let (code, extensions) = exec_err(
        &f.schema,
        request(Some(1), 1, now - chrono::Duration::minutes(11)),
    )
    .await;
    assert_eq!(code, "VALIDATION", "{extensions}");
    let (code, extensions) = exec_err(&f.schema, request(Some(0), 2, now)).await;
    assert_eq!(code, "VALIDATION", "{extensions}");

    let data = exec_ok(&f.schema, request(Some(1), 3, now)).await;
    assert_eq!(data["requestMlsReadd"], json!(true));
    let (code, extensions) = exec_err(&f.schema, request(Some(1), 3, now)).await;
    assert_eq!(code, "CONFLICT", "{extensions}");
    let group = f.group(&conv).await;
    assert_eq!(
        group["mlsGroup"]["readdRequests"].as_array().map(Vec::len),
        Some(1)
    );
}

/// A receipt made with the server's franking key over another content does not verify: the
/// reported commitment must be one the server recorded for the message. An edit adds a version,
/// so the version the receiver saw before the edit stays reportable.
#[tokio::test]
async fn finding_7_a_report_must_match_a_stored_revision() {
    let mut f = fixture().await;
    let conv = f.encrypted_dm().await;
    let conversation_id: Uuid = conv.parse().expect("uuid");
    let id = Uuid::now_v7();
    let (request, sent_content, fk) = f.send_request(Who::Alice, &conv, id, "hello", &[]);
    let sent = exec_ok(&f.schema, request).await["sendEncryptedMessage"].clone();
    let encryption = sent["encryption"].clone();
    let sent_at_us: i64 = encryption["sentAtUs"]
        .as_str()
        .expect("sentAtUs")
        .parse()
        .expect("i64");

    // Someone holding the server's key forges a receipt over content alice never sent.
    let forged = content(id, 0, "something alice never said", &[]);
    let forged_commitment = hmac(&fk, &[b"rim.e2ee.franking.v1\0", &forged]);
    let forged_receipt = hmac(
        &FRANKING_KEY,
        &[
            b"rim.e2ee.franking-receipt.v1\0",
            id.as_bytes(),
            conversation_id.as_bytes(),
            Uuid::from(f.alice.user.id).as_bytes(),
            f.alice.device_id.as_bytes(),
            &sent_at_us.to_be_bytes(),
            &forged_commitment,
        ],
    );
    let mut forged_encryption = encryption.clone();
    forged_encryption["frankingReceipt"] = json!(b64(&forged_receipt));
    let server_id = f.server_id();
    let input = report(
        &server_id,
        &conv,
        id,
        &forged,
        &fk,
        &forged_encryption,
        &f.alice,
    );
    let (code, extensions) = exec_err(&f.schema, f.bob.request(REPORT, input)).await;
    assert_eq!(code, "VALIDATION", "{extensions}");

    // Alice edits it. The forged content still matches no version; the version bob saw
    // before the edit does.
    let edited = content(id, 1, "hello, edited", &[]);
    let envelope = f.alice.encrypt(id, &edited, [5u8; 32], &[]);
    let edit = f.alice.request(
        &format!("mutation Edit($input: EditEncryptedMessageInput!) {{ editEncryptedMessage(input: $input) {{ {MESSAGE_FIELDS} }} }}"),
        json!({ "input": { "serverId": server_id, "conversationId": conv, "messageId": id, "envelope": envelope } }),
    );
    exec_ok(&f.schema, edit).await;
    let input = report(
        &server_id,
        &conv,
        id,
        &forged,
        &fk,
        &forged_encryption,
        &f.alice,
    );
    let (code, extensions) = exec_err(&f.schema, f.bob.request(REPORT, input)).await;
    assert_eq!(code, "VALIDATION", "{extensions}");
    let original = report(
        &server_id,
        &conv,
        id,
        &sent_content,
        &fk,
        &encryption,
        &f.alice,
    );
    let data = exec_ok(&f.schema, f.bob.request(REPORT, original)).await;
    assert_eq!(data["reportEncryptedMessage"], json!(true));
    let stored: Option<Vec<u8>> = f
        .ctx
        .conn
        .query_one_raw(sea_orm::Statement::from_sql_and_values(
            sea_orm::DbBackend::Postgres,
            "SELECT franked_content FROM message_report WHERE message_id = $1",
            [id.into()],
        ))
        .await
        .expect("query")
        .expect("report row")
        .try_get("", "franked_content")
        .expect("column");
    assert_eq!(stored.as_deref(), Some(sent_content.as_slice()));
}

/// A message its sender deleted stays reportable: the row keeps its franking columns, and the
/// verified report holds the content.
#[tokio::test]
async fn finding_26_a_deleted_message_is_reported_end_to_end() {
    let mut f = fixture().await;
    let conv = f.encrypted_dm().await;
    let id = Uuid::now_v7();
    let (request, sent_content, fk) = f.send_request(Who::Alice, &conv, id, "regret", &[]);
    let sent = exec_ok(&f.schema, request).await["sendEncryptedMessage"].clone();
    let feed = f.changes(Who::Bob, &conv, None).await;
    let (received, received_fk) = f.bob.decrypt(
        feed["changes"][0]["message"]["encryption"]["ciphertext"]
            .as_str()
            .expect("ciphertext"),
        &id.to_string(),
    );
    assert_eq!(
        (received.as_slice(), received_fk),
        (sent_content.as_slice(), fk)
    );
    let delete = f.alice.request(
        DELETE,
        json!({ "input": { "serverId": f.server_id(), "conversationId": conv, "messageId": id } }),
    );
    exec_ok(&f.schema, delete).await;

    let server_id = f.server_id();
    let input = report(
        &server_id,
        &conv,
        id,
        &received,
        &received_fk,
        &sent["encryption"],
        &f.alice,
    );
    let data = exec_ok(&f.schema, f.bob.request(REPORT, input)).await;
    assert_eq!(data["reportEncryptedMessage"], json!(true));
    let stored: Option<Vec<u8>> = f
        .ctx
        .conn
        .query_one_raw(sea_orm::Statement::from_sql_and_values(
            sea_orm::DbBackend::Postgres,
            "SELECT r.franked_content FROM message_report r JOIN message m ON m.id = r.message_id \
             WHERE r.message_id = $1 AND m.is_deleted",
            [id.into()],
        ))
        .await
        .expect("query")
        .expect("report of the deleted row")
        .try_get("", "franked_content")
        .expect("column");
    assert_eq!(stored.as_deref(), Some(sent_content.as_slice()));
}
