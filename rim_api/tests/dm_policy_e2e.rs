//! Who may start a DM, on real servers: a workspace member who chose requests gets a fellow
//! member's DM as a request while the owner's arrives, and whoever messaged first takes the
//! DM back directly; only the owner sets a workspace's rules
//! and a closing floor binds each member's choice; a server closed by its operator serves no
//! identity, takes no DM from a stranger here or elsewhere and starts none with another server;
//! a stranger's DM request past the daily limit is refused when it is started and reaches no
//! one; a contact added on one's own still counts against the daily invites to other servers,
//! while someone whose DM request was accepted does not; a partner server's people reach this
//! one's while a server that is not a partner reads no identity; a member who leaves a group
//! they accepted keeps no accepted request for it; a group of the default workspace reaches each
//! member as their DM settings say, and never one who blocked its starter; the recipient of a DM
//! request reads it first and posts nothing into it, nor reacts or types there, until they
//! accept it; a request of a conversation here that lapses keeps it out of view until the
//! sender's next message files it again or the recipient posts into it; a workspace DM or group
//! started by someone the user blocked leaves their list and live events until the unblock.
//!
//! Run with:
//! ```sh
//! cargo test -p rim_api --features e2e-tests -- --test-threads=1
//! ```
#![cfg(feature = "e2e-tests")]

mod support;

use std::time::Duration;

use rim_server::config::RIMConfig;
use rim_services::relay::service::RelayService;
use sea_orm::ConnectionTrait;
use serde_json::{Value, json};

use support::nodes::{Node, databases_with, start};
use support::relay::{
    CREATE, Person, REQUESTS, RESET, eventually, open_registration, sign_up, sign_up_without_card,
    user_id, verify_item,
};
use support::ws::Socket;

const CREATE_SERVER: &str =
    r#"mutation Create($input: CreateServerInput!) { createServer(input: $input) { id } }"#;
const CREATE_INVITE: &str = r#"
mutation Invite($input: CreateInviteInput!) { createInvite(input: $input) { inviteToken } }"#;
const JOIN: &str = r#"
mutation Join($input: JoinServerByInviteInput!) { joinServerByInvite(input: $input) { serverId } }"#;
const RULES: &str = "query($s: ID!) { server(serverId: $s) { dmOutsideFloor dmMemberDefault } }";
const SET_RULES: &str = r#"
mutation($s: ID!, $f: DmOutsideFloor!, $d: DmMembers!) {
  setWorkspaceDmPolicy(workspaceId: $s, outsideFloor: $f, memberDefault: $d) {
    workspaceId outsideFloor memberDefault
  }
}"#;
const SETTINGS_FIELDS: &str = "outside effectiveOutside \
     outsideFloorFrom { kind workspaceId workspaceName } \
     workspaces { workspaceId members memberDefault }";
const SERVER_INFO: &str = "query { serverInfo { dmInbound dmOutbound dmPartners } }";
const REMOTE_IDENTITY: &str = r#"
query($p: String!, $r: String) { remoteIdentity(principalId: $p, relayUrl: $r) { principalId } }"#;
/// A server id no test server has.
const ELSEWHERE: &str = "rims1_OFPDNXTV1splCcKcgz1vWYj6Mg-lbUollHQwXTkYEBw";

fn my_settings() -> String {
    format!("query {{ myDmSettings {{ {SETTINGS_FIELDS} }} }}")
}

fn set_outside() -> String {
    format!("mutation($o: DmOutside!) {{ setMyDmSettings(outside: $o) {{ {SETTINGS_FIELDS} }} }}")
}

fn set_members() -> String {
    format!(
        "mutation($s: ID!, $m: DmMembers!) {{ setMyWorkspaceDmSetting(workspaceId: $s, members: $m) {{ {SETTINGS_FIELDS} }} }}"
    )
}

/// One server with open registration, configured by `tweak`.
async fn one_server(tweak: impl FnOnce(&mut RIMConfig)) -> Node {
    let (database, _) = databases_with(RESET).await;
    start(&database, |c| {
        open_registration(c);
        tweak(c);
    })
    .await
}

/// A workspace named `name` that `owner` creates, with `members` joined by invite.
async fn workspace(owner: &Person, name: &str, members: &[&Person]) -> String {
    let id = owner
        .ok(CREATE_SERVER, json!({ "input": { "name": name } }))
        .await["createServer"]["id"]
        .as_str()
        .expect("id")
        .to_string();
    for member in members {
        let token = owner
            .ok(CREATE_INVITE, json!({ "input": { "serverId": id } }))
            .await["createInvite"]["inviteToken"]
            .clone();
        member
            .ok(JOIN, json!({ "input": { "inviteToken": token } }))
            .await;
    }
    id
}

/// Start a DM with `other` in `server`, a workspace or the direct space.
async fn dm(node: &Node, from: &Person, server: &str, other: &Person) -> Value {
    let other_id = user_id(node, &other.principal()).await;
    from.post(
        CREATE,
        json!({ "input": { "serverId": server, "otherUserId": other_id.to_string() } }),
    )
    .await
}

fn created(body: &Value) -> String {
    assert!(body.get("errors").is_none(), "errors: {body}");
    body["data"]["createConversation"]["id"]
        .as_str()
        .expect("id")
        .to_string()
}

fn code(body: &Value) -> &str {
    body["errors"][0]["extensions"]["code"]
        .as_str()
        .unwrap_or_else(|| panic!("expected an error: {body}"))
}

/// The DMs and groups `person` sees in `server`.
async fn listed(person: &Person, server: &str) -> Vec<String> {
    person
        .ok(
            "query($s: ID!) { inboxConversations(serverId: $s, dir: AFTER, limit: 50) { items { id } } }",
            json!({ "s": server }),
        )
        .await["inboxConversations"]["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|c| c["id"].as_str().expect("id").to_string())
        .collect()
}

/// What `person` reads as `dmRequestPending` of `conversation` in `server`.
async fn request_pending(person: &Person, server: &str, conversation: &str) -> Value {
    person
        .ok(
            "query($s: ID!, $c: ID!) { conversation(serverId: $s, conversationId: $c) { dmRequestPending } }",
            json!({ "s": server, "c": conversation }),
        )
        .await["conversation"]["dmRequestPending"]
        .clone()
}

async fn direct_space(person: &Person) -> String {
    person.direct_space().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_co_members_dm_is_a_request_when_the_recipient_chose_requests() {
    let a = one_server(|_| {}).await;
    let alice = sign_up(&a, "alice", 1).await;
    let bob = sign_up(&a, "bob", 2).await;
    let carol = sign_up(&a, "carol", 3).await;
    let acme = workspace(&alice, "Acme", &[&bob, &carol]).await;

    // A workspace starts with no floor and its members' DMs direct.
    let rules = bob.ok(RULES, json!({ "s": acme })).await;
    assert_eq!(
        rules["server"],
        json!({ "dmOutsideFloor": "NONE", "dmMemberDefault": "DIRECT" })
    );
    let direct = direct_space(&bob).await;
    let rules = bob.ok(RULES, json!({ "s": direct })).await;
    assert_eq!(
        rules["server"],
        json!({ "dmOutsideFloor": null, "dmMemberDefault": null }),
        "the direct space has no rules"
    );

    // Bob takes DMs from Acme's members as requests.
    let settings = bob
        .ok(&set_members(), json!({ "s": acme, "m": "REQUESTS" }))
        .await;
    assert_eq!(
        settings["setMyWorkspaceDmSetting"]["workspaces"],
        json!([{ "workspaceId": acme, "members": "REQUESTS", "memberDefault": "DIRECT" }])
    );

    // Carol, a member, starts a DM with him in the workspace: it is a request.
    let from_carol = created(&dm(&a, &carol, &acme, &bob).await);
    let request = bob.pending_request(&carol).await;
    assert_eq!(request["hostServerId"], json!(a.id()));
    assert_eq!(request["conversationId"], json!(from_carol));
    let invite = verify_item(&a, &request["heldItems"][0]);
    assert_eq!(invite.event["type"], "DmInvite");
    assert_eq!(invite.event["workspace_id"], json!(acme));
    assert!(
        !listed(&bob, &acme).await.contains(&from_carol),
        "a request is not a live conversation"
    );
    assert!(listed(&carol, &acme).await.contains(&from_carol));
    // Carol sees her DM wait for Bob; Bob, its recipient, has nothing pending from her side.
    assert_eq!(
        request_pending(&carol, &acme, &from_carol).await,
        json!(true)
    );
    assert_eq!(
        request_pending(&bob, &acme, &from_carol).await,
        json!(false)
    );
    // A channel has no DM request at all.
    let channels = carol
        .ok(
            "query($s: ID!) { channels(serverId: $s, dir: AFTER, limit: 10) { items { id dmRequestPending } } }",
            json!({ "s": acme }),
        )
        .await;
    let channel = &channels["channels"]["items"][0];
    assert!(channel["id"].is_string(), "{channels}");
    assert_eq!(channel["dmRequestPending"], Value::Null);

    // Alice, the owner, is never held.
    let from_alice = created(&dm(&a, &alice, &acme, &bob).await);
    tokio::time::sleep(Duration::from_millis(500)).await;
    let pending = bob.ok(REQUESTS, json!({ "status": "PENDING" })).await;
    let from: Vec<&Value> = pending["dmRequests"]
        .as_array()
        .expect("requests")
        .iter()
        .map(|r| &r["fromPrincipalId"])
        .collect();
    assert_eq!(from, vec![&json!(carol.principal())]);
    assert!(listed(&bob, &acme).await.contains(&from_alice));

    // Accepting makes Carol's DM live.
    bob.ok(
        "mutation($id: ID!) { acceptDmRequest(id: $id) { status } }",
        json!({ "id": request["id"] }),
    )
    .await;
    assert!(listed(&bob, &acme).await.contains(&from_carol));
    assert_eq!(
        request_pending(&carol, &acme, &from_carol).await,
        json!(false)
    );

    // Carol messaged Bob first, so he is her contact: his DM back is no request, even once she
    // takes Acme's members' DMs as requests.
    let contacts = carol
        .ok("query { contacts { principalId } }", json!({}))
        .await;
    assert_eq!(
        contacts["contacts"],
        json!([{ "principalId": bob.principal() }])
    );
    carol
        .ok(&set_members(), json!({ "s": acme, "m": "REQUESTS" }))
        .await;
    let back = created(&dm(&a, &bob, &direct, &carol).await);
    tokio::time::sleep(Duration::from_millis(500)).await;
    let pending = carol.ok(REQUESTS, json!({ "status": "PENDING" })).await;
    assert_eq!(pending["dmRequests"], json!([]));
    assert!(listed(&carol, &direct).await.contains(&back));

    a.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_request_takes_nothing_from_its_recipient_until_they_accept_it() {
    const SEND: &str = "mutation($s: ID!, $c: ID!, $t: String!) { \
         sendMessage(input: { serverId: $s, conversationId: $c, text: $t }) { id } }";
    const REACT: &str = "mutation($s: ID!, $c: ID!, $m: ID!) { \
         toggleReaction(input: { serverId: $s, conversationId: $c, messageId: $m, emoji: \"👍\" }) { emoji } }";
    const TYPING: &str =
        "mutation($s: ID!, $c: ID!) { startTyping(serverId: $s, conversationId: $c) }";
    let a = one_server(|_| {}).await;
    let alice = sign_up(&a, "alice", 1).await;
    let bob = sign_up(&a, "bob", 2).await;
    let carol = sign_up(&a, "carol", 3).await;
    let acme = workspace(&alice, "Acme", &[&bob, &carol]).await;
    bob.ok(&set_members(), json!({ "s": acme, "m": "REQUESTS" }))
        .await;

    // Carol's DM is a request for Bob; she writes into it.
    let from_carol = created(&dm(&a, &carol, &acme, &bob).await);
    let request = bob.pending_request(&carol).await;
    let hello = carol
        .ok(SEND, json!({ "s": acme, "c": from_carol, "t": "hello" }))
        .await["sendMessage"]["id"]
        .clone();

    // Bob may read it first, but posts nothing into it before he decides.
    let refused = bob
        .post(SEND, json!({ "s": acme, "c": from_carol, "t": "hi back" }))
        .await;
    assert_eq!(code(&refused), "FORBIDDEN", "{refused}");
    assert!(
        refused["errors"][0]["message"]
            .as_str()
            .is_some_and(|m| m.contains("accept the message request")),
        "{refused}"
    );
    let reaction = bob
        .post(REACT, json!({ "s": acme, "c": from_carol, "m": hello }))
        .await;
    assert_eq!(code(&reaction), "FORBIDDEN", "{reaction}");
    let typing = bob.ok(TYPING, json!({ "s": acme, "c": from_carol })).await;
    assert_eq!(
        typing["startTyping"],
        json!(false),
        "no typing reaches Carol"
    );
    // Carol, who started it, is not held.
    let carols_typing = carol
        .ok(TYPING, json!({ "s": acme, "c": from_carol }))
        .await;
    assert_eq!(carols_typing["startTyping"], json!(true));

    // Once he accepts, he replies.
    bob.ok(
        "mutation($id: ID!) { acceptDmRequest(id: $id) { status } }",
        json!({ "id": request["id"] }),
    )
    .await;
    bob.ok(SEND, json!({ "s": acme, "c": from_carol, "t": "hi back" }))
        .await;
    bob.ok(REACT, json!({ "s": acme, "c": from_carol, "m": hello }))
        .await;

    a.stop().await;
}

const BLOCK: &str = "mutation($p: String!) { blockPrincipal(principalId: $p) }";
const UNBLOCK: &str = "mutation($p: String!) { unblockPrincipal(principalId: $p) }";
const SEND: &str = "mutation($s: ID!, $c: ID!, $t: String!) { \
     sendMessage(input: { serverId: $s, conversationId: $c, text: $t }) { id } }";
const REACT: &str = "mutation($s: ID!, $c: ID!, $m: ID!) { \
     toggleReaction(input: { serverId: $s, conversationId: $c, messageId: $m, emoji: \"👍\" }) { emoji } }";
const MESSAGES: &str = r#"query($s: ID!, $c: ID!) {
  messages(serverId: $s, conversationId: $c, dir: AFTER, limit: 50, includeDeleted: true) {
    items { id text }
  }
}"#;

/// The texts of the messages `person` reads in `conversation` of `server`.
async fn texts(person: &Person, server: &str, conversation: &str) -> Vec<String> {
    person
        .ok(MESSAGES, json!({ "s": server, "c": conversation }))
        .await["messages"]["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|m| m["text"].as_str().expect("text").to_string())
        .collect()
}

/// The ids of `person`'s requests in `status`.
async fn request_ids(person: &Person, status: &str) -> Vec<Value> {
    person.ok(REQUESTS, json!({ "status": status })).await["dmRequests"]
        .as_array()
        .expect("requests")
        .iter()
        .map(|r| r["id"].clone())
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unblocked_senders_next_message_is_a_new_request_without_what_the_block_refused() {
    let a = one_server(|_| {}).await;
    let alice = sign_up(&a, "alice", 1).await;
    let bob = sign_up(&a, "bob", 2).await;
    let carol = sign_up(&a, "carol", 3).await;
    let direct = direct_space(&alice).await;

    let conv = created(&dm(&a, &alice, &direct, &bob).await);
    let hello = alice
        .ok(SEND, json!({ "s": direct, "c": conv, "t": "hello" }))
        .await;
    let request = bob.pending_request(&alice).await;

    let blocked = bob.ok(BLOCK, json!({ "p": alice.principal() })).await;
    assert_eq!(blocked["blockPrincipal"], json!(true));
    assert!(request_ids(&bob, "PENDING").await.is_empty());

    let refused = alice
        .post(
            SEND,
            json!({ "s": direct, "c": conv, "t": "while blocked" }),
        )
        .await;
    assert_eq!(code(&refused), "FORBIDDEN", "{refused}");
    assert!(
        refused["errors"][0]["message"]
            .as_str()
            .is_some_and(|m| m.contains("they do not take direct messages from you")),
        "{refused}"
    );
    let reaction = alice
        .post(
            REACT,
            json!({ "s": direct, "c": conv, "m": hello["sendMessage"]["id"] }),
        )
        .await;
    assert_eq!(code(&reaction), "FORBIDDEN", "{reaction}");

    let unblocked = bob.ok(UNBLOCK, json!({ "p": alice.principal() })).await;
    assert_eq!(unblocked["unblockPrincipal"], json!(true));
    assert!(request_ids(&bob, "PENDING").await.is_empty());
    assert!(request_ids(&bob, "IGNORED").await.is_empty());
    assert!(!listed(&bob, &direct).await.contains(&conv));

    alice
        .ok(SEND, json!({ "s": direct, "c": conv, "t": "after" }))
        .await;
    let again = bob.pending_request(&alice).await;
    assert_eq!(again["id"], request["id"]);
    assert_eq!(again["heldItems"].as_array().expect("held").len(), 2);
    assert!(
        again["createdAt"].as_str() > request["createdAt"].as_str(),
        "the request is filed anew: {request} then {again}"
    );
    assert!(!listed(&bob, &direct).await.contains(&conv));

    bob.ok(
        "mutation($id: ID!) { acceptDmRequest(id: $id) { status } }",
        json!({ "id": request["id"] }),
    )
    .await;
    assert!(listed(&bob, &direct).await.contains(&conv));
    assert_eq!(texts(&bob, &direct, &conv).await, ["hello", "after"]);
    bob.ok(SEND, json!({ "s": direct, "c": conv, "t": "hi back" }))
        .await;

    // Unblocking someone who has no request changes nothing.
    let carols = bob.ok(BLOCK, json!({ "p": carol.principal() })).await;
    assert_eq!(carols["blockPrincipal"], json!(true));
    let lifted = bob.ok(UNBLOCK, json!({ "p": carol.principal() })).await;
    assert_eq!(lifted["unblockPrincipal"], json!(true));
    let again = bob.ok(UNBLOCK, json!({ "p": carol.principal() })).await;
    assert_eq!(again["unblockPrincipal"], json!(false));

    a.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_accepted_dm_takes_nothing_from_a_blocked_member_and_carries_on_after_the_unblock() {
    let a = one_server(|_| {}).await;
    let alice = sign_up(&a, "alice", 1).await;
    let bob = sign_up(&a, "bob", 2).await;
    let carol = sign_up(&a, "carol", 3).await;
    let direct = direct_space(&alice).await;

    let (bob_id, carol_id) = (
        user_id(&a, &bob.principal()).await,
        user_id(&a, &carol.principal()).await,
    );
    let group = alice
        .ok(
            CREATE,
            json!({ "input": {
                "serverId": direct,
                "memberIds": [bob_id.to_string(), carol_id.to_string()],
                "title": "group",
            } }),
        )
        .await["createConversation"]["id"]
        .as_str()
        .expect("id")
        .to_string();
    let group_request = bob.pending_request(&alice).await;
    assert_eq!(group_request["conversationId"], json!(group));

    // Accepting the DM makes Alice a contact; the group request stays undecided.
    let conv = created(&dm(&a, &alice, &direct, &bob).await);
    alice
        .ok(SEND, json!({ "s": direct, "c": conv, "t": "before" }))
        .await;
    let dm_request = eventually("the DM request", || async {
        let pending = bob.ok(REQUESTS, json!({ "status": "PENDING" })).await;
        pending["dmRequests"]
            .as_array()
            .and_then(|r| r.iter().find(|r| r["conversationId"] == json!(conv)))
            .cloned()
    })
    .await;
    bob.ok(
        "mutation($id: ID!) { acceptDmRequest(id: $id) { status } }",
        json!({ "id": dm_request["id"] }),
    )
    .await;
    bob.ok(BLOCK, json!({ "p": alice.principal() })).await;
    let refused = alice
        .post(
            SEND,
            json!({ "s": direct, "c": conv, "t": "while blocked" }),
        )
        .await;
    assert_eq!(code(&refused), "FORBIDDEN", "{refused}");
    assert!(
        !listed(&bob, &direct).await.contains(&conv),
        "Alice started the DM, so a block takes it out of Bob's list"
    );

    bob.ok(UNBLOCK, json!({ "p": alice.principal() })).await;
    assert!(listed(&bob, &direct).await.contains(&conv));
    assert_eq!(texts(&bob, &direct, &conv).await, ["before"]);
    alice
        .ok(SEND, json!({ "s": direct, "c": conv, "t": "back" }))
        .await;
    assert_eq!(texts(&bob, &direct, &conv).await, ["before", "back"]);
    assert!(
        request_ids(&bob, "PENDING").await.is_empty(),
        "the accepted DM files no request, and the group's waits for its next item"
    );

    alice
        .ok(SEND, json!({ "s": direct, "c": group, "t": "hello all" }))
        .await;
    let again = bob.pending_request(&alice).await;
    assert_eq!(again["conversationId"], json!(group));
    assert_eq!(again["id"], group_request["id"]);

    a.stop().await;
}

/// Expire every request of `conversation` and run the relay's cleanup at once.
async fn lapse(node: &Node, conversation: &str) {
    node.db
        .execute_unprepared(&format!(
            "UPDATE relay_dm_request SET expires_at = now() - interval '1 second' \
             WHERE conversation_id = '{conversation}'"
        ))
        .await
        .expect("expire");
    node.s2s.relay.cleanup().await.expect("cleanup");
}

/// `person`'s pending request from `from` once it holds `held` items.
async fn holding(person: &Person, from: &Person, held: usize) -> Value {
    eventually("a request holding its items", || async {
        let request = person.pending_request(from).await;
        (request["heldItems"].as_array().map(Vec::len) == Some(held)).then_some(request)
    })
    .await
}

/// Assert that the first item `request` holds is the invite of `conversation` from `host`.
fn holds_the_invite_first(host: &Node, request: &Value, conversation: &str) {
    let invite = verify_item(host, &request["heldItems"][0]);
    assert_eq!(invite.event["type"], json!("DmInvite"), "{request}");
    assert_eq!(invite.event["conversation_id"], json!(conversation));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unanswered_request_here_never_surfaces_its_conversation() {
    let a = one_server(|_| {}).await;
    let alice = sign_up(&a, "alice", 1).await;
    let bob = sign_up(&a, "bob", 2).await;
    let direct = direct_space(&bob).await;

    let conv = created(&dm(&a, &alice, &direct, &bob).await);
    alice
        .ok(SEND, json!({ "s": direct, "c": conv, "t": "hello" }))
        .await;
    // The invite, the conversation's creation and the message are all held before the request
    // lapses, so no event of the conversation reaches the inbox after it is cleared.
    let request = holding(&bob, &alice, 3).await;

    lapse(&a, &conv).await;
    assert!(request_ids(&bob, "PENDING").await.is_empty());
    assert!(request_ids(&bob, "IGNORED").await.is_empty());
    assert!(
        !listed(&bob, &direct).await.contains(&conv),
        "a lapsed request does not surface its conversation"
    );
    let stale = bob
        .post(
            "mutation($id: ID!) { acceptDmRequest(id: $id) { status } }",
            json!({ "id": request["id"] }),
        )
        .await;
    assert_eq!(code(&stale), "NOT_FOUND", "{stale}");
    assert_eq!(request_pending(&alice, &direct, &conv).await, json!(true));

    alice
        .ok(SEND, json!({ "s": direct, "c": conv, "t": "again" }))
        .await;
    let again = holding(&bob, &alice, 2).await;
    assert_eq!(again["id"], request["id"]);
    holds_the_invite_first(&a, &again, &conv);
    assert_eq!(texts(&bob, &direct, &conv).await, ["hello", "again"]);

    bob.ok(
        "mutation($id: ID!) { ignoreDmRequest(id: $id) { status } }",
        json!({ "id": request["id"] }),
    )
    .await;
    lapse(&a, &conv).await;
    assert!(!listed(&bob, &direct).await.contains(&conv));
    alice
        .ok(SEND, json!({ "s": direct, "c": conv, "t": "third" }))
        .await;
    let third = holding(&bob, &alice, 2).await;
    holds_the_invite_first(&a, &third, &conv);

    lapse(&a, &conv).await;
    bob.ok(SEND, json!({ "s": direct, "c": conv, "t": "hi" }))
        .await;
    assert!(
        listed(&bob, &direct).await.contains(&conv),
        "posting takes the conversation back"
    );
    assert!(request_ids(&bob, "PENDING").await.is_empty());
    assert_eq!(request_pending(&alice, &direct, &conv).await, json!(false));

    a.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_lapsed_request_from_someone_blocked_stays_ignored() {
    let a = one_server(|_| {}).await;
    let bob = sign_up(&a, "bob", 2).await;
    let carol = sign_up(&a, "carol", 3).await;
    let direct = direct_space(&bob).await;

    let conv = created(&dm(&a, &carol, &direct, &bob).await);
    carol
        .ok(SEND, json!({ "s": direct, "c": conv, "t": "hello" }))
        .await;
    let request = bob.pending_request(&carol).await;
    bob.ok(BLOCK, json!({ "p": carol.principal() })).await;

    lapse(&a, &conv).await;
    assert!(
        request_ids(&bob, "IGNORED").await.contains(&request["id"]),
        "a request from someone blocked stays ignored"
    );
    assert!(!listed(&bob, &direct).await.contains(&conv));
    let refused = carol
        .post(SEND, json!({ "s": direct, "c": conv, "t": "spam" }))
        .await;
    assert_eq!(code(&refused), "FORBIDDEN", "{refused}");

    bob.ok(UNBLOCK, json!({ "p": carol.principal() })).await;
    assert!(
        !request_ids(&bob, "IGNORED").await.contains(&request["id"]),
        "the unblock found its invite and cleared it"
    );
    carol
        .ok(SEND, json!({ "s": direct, "c": conv, "t": "after" }))
        .await;
    let again = holding(&bob, &carol, 2).await;
    assert_eq!(again["id"], request["id"]);
    holds_the_invite_first(&a, &again, &conv);

    a.stop().await;
}

const CREATE_CHANNEL: &str = "mutation($s: ID!, $title: String!) { \
     createConversation(input: { serverId: $s, isChannel: true, title: $title }) { id } }";
const JOIN_CHANNEL: &str =
    "mutation($s: ID!, $c: ID!) { joinChannel(serverId: $s, conversationId: $c) { id } }";
const UNREAD: &str = "query($s: ID!) { myUnreadNotificationCount(serverId: $s) }";

/// A group in `server` that `creator` starts with `members`.
async fn group(node: &Node, creator: &Person, server: &str, members: &[&Person]) -> String {
    let mut ids = Vec::new();
    for member in members {
        ids.push(user_id(node, &member.principal()).await.to_string());
    }
    created(
        &creator
            .post(
                CREATE,
                json!({ "input": { "serverId": server, "memberIds": ids } }),
            )
            .await,
    )
}

/// `person`'s `workspaceEvents` of `server` on a socket of its own.
async fn events_of(node: &Node, person: &Person, server: &str) -> Socket {
    let mut socket = Socket::connect(&node.origin, &person.session).await;
    socket
        .subscribe(
            "events",
            "subscription($id: ID!) { workspaceEvents(workspaceId: $id) }",
            json!({ "id": server }),
        )
        .await;
    socket
}

/// The events `socket` streams until one contains `marker`, which is not among them.
async fn events_until(socket: &mut Socket, marker: &str) -> Vec<String> {
    let mut seen = Vec::new();
    loop {
        let answer = socket.receive().await;
        let text = answer["payload"]["data"]["workspaceEvents"]
            .as_str()
            .unwrap_or_else(|| panic!("an event: {answer}"))
            .to_string();
        if text.contains(marker) {
            return seen;
        }
        seen.push(text);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_workspace_dm_and_group_a_blocked_person_started_are_hidden_until_the_unblock() {
    let a = one_server(|_| {}).await;
    let alice = sign_up(&a, "alice", 1).await;
    let bob = sign_up(&a, "bob", 2).await;
    let carol = sign_up(&a, "carol", 3).await;
    let acme = workspace(&alice, "Acme", &[&bob, &carol]).await;

    let dm = created(&dm(&a, &alice, &acme, &bob).await);
    let alices = group(&a, &alice, &acme, &[&bob, &carol]).await;
    let channel = alice
        .ok(CREATE_CHANNEL, json!({ "s": acme, "title": "lounge" }))
        .await["createConversation"]["id"]
        .as_str()
        .expect("id")
        .to_string();
    bob.ok(JOIN_CHANNEL, json!({ "s": acme, "c": channel }))
        .await;
    let carols = group(&a, &carol, &acme, &[&alice, &bob]).await;
    for conversation in [&dm, &alices] {
        alice
            .ok(SEND, json!({ "s": acme, "c": conversation, "t": "hi" }))
            .await;
    }
    let listed_before = listed(&bob, &acme).await;
    for conversation in [&dm, &alices, &carols] {
        assert!(listed_before.contains(conversation), "{listed_before:?}");
    }
    let unread_before = eventually("Bob's notifications", || async {
        let n = bob.ok(UNREAD, json!({ "s": acme })).await["myUnreadNotificationCount"]
            .as_i64()
            .expect("count");
        (n > 0).then_some(n)
    })
    .await;

    bob.ok(BLOCK, json!({ "p": alice.principal() })).await;
    let bobs_list = listed(&bob, &acme).await;
    assert!(!bobs_list.contains(&dm) && !bobs_list.contains(&alices));
    assert!(bobs_list.contains(&carols));
    assert!(listed(&carol, &acme).await.contains(&alices));
    let refused = bob
        .post(SEND, json!({ "s": acme, "c": alices, "t": "from bob" }))
        .await;
    assert_eq!(code(&refused), "FORBIDDEN", "{refused}");
    assert!(
        refused["errors"][0]["message"]
            .as_str()
            .is_some_and(|m| m.contains("accept the message request")),
        "{refused}"
    );
    bob.ok(SEND, json!({ "s": acme, "c": carols, "t": "in carols" }))
        .await;
    bob.ok(
        SEND,
        json!({ "s": acme, "c": channel, "t": "in the lounge" }),
    )
    .await;
    let refused = alice
        .post(SEND, json!({ "s": acme, "c": dm, "t": "while blocked" }))
        .await;
    assert_eq!(code(&refused), "FORBIDDEN", "{refused}");
    assert!(
        refused["errors"][0]["message"]
            .as_str()
            .is_some_and(|m| m.contains("they do not take direct messages from you")),
        "{refused}"
    );
    let unread_after = bob.ok(UNREAD, json!({ "s": acme })).await["myUnreadNotificationCount"]
        .as_i64()
        .expect("count");
    assert!(
        unread_after < unread_before,
        "the hidden DM's notification is not counted: {unread_before} then {unread_after}"
    );

    // Live events: Alice's message into her group reaches Carol but not Bob.
    let mut bobs_events = events_of(&a, &bob, &acme).await;
    let mut carols_events = events_of(&a, &carol, &acme).await;
    for (n, socket) in [&mut bobs_events, &mut carols_events]
        .into_iter()
        .enumerate()
    {
        let marker = format!("listening {n}");
        let mut heard = false;
        for _ in 0..50 {
            carol
                .ok(SEND, json!({ "s": acme, "c": carols, "t": marker }))
                .await;
            if let Some(answer) = socket
                .receive_within(Duration::from_millis(300))
                .await
                .filter(|a| a.to_string().contains(&marker))
            {
                assert_eq!(answer["type"], "next", "{answer}");
                heard = true;
                break;
            }
        }
        assert!(heard, "the subscription listens");
    }
    alice
        .ok(SEND, json!({ "s": acme, "c": alices, "t": "secret plans" }))
        .await;
    carol
        .ok(
            SEND,
            json!({ "s": acme, "c": carols, "t": "visible marker" }),
        )
        .await;
    let bob_saw = events_until(&mut bobs_events, "visible marker").await;
    assert!(
        !bob_saw.iter().any(|e| e.contains(&alices)),
        "nothing of the hidden group reaches Bob: {bob_saw:?}"
    );
    events_until(&mut carols_events, "secret plans").await;
    assert_eq!(texts(&carol, &acme, &alices).await, ["hi", "secret plans"]);

    bob.ok(UNBLOCK, json!({ "p": alice.principal() })).await;
    let bobs_list = listed(&bob, &acme).await;
    assert!(bobs_list.contains(&dm) && bobs_list.contains(&alices));
    bob.ok(SEND, json!({ "s": acme, "c": alices, "t": "back" }))
        .await;

    a.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn only_the_owner_sets_a_workspaces_rules_and_a_closing_floor_binds_its_members() {
    let a = one_server(|_| {}).await;
    let alice = sign_up(&a, "alice", 1).await;
    let bob = sign_up(&a, "bob", 2).await;
    let dave = sign_up(&a, "dave", 4).await;
    let acme = workspace(&alice, "Acme", &[&bob]).await;
    let rules = json!({ "s": acme, "f": "CLOSED", "d": "REQUESTS" });

    assert_eq!(bob.err(SET_RULES, rules.clone()).await, "FORBIDDEN");
    assert_eq!(dave.err(SET_RULES, rules.clone()).await, "FORBIDDEN");
    let set = alice.ok(SET_RULES, rules).await;
    assert_eq!(
        set["setWorkspaceDmPolicy"],
        json!({ "workspaceId": acme, "outsideFloor": "CLOSED", "memberDefault": "REQUESTS" })
    );
    assert_eq!(
        bob.ok(RULES, json!({ "s": acme })).await["server"],
        json!({ "dmOutsideFloor": "CLOSED", "dmMemberDefault": "REQUESTS" })
    );

    // Bob's choice stays `REQUESTS`, but Acme's floor refuses DMs from outside, and says so.
    let settings = bob.ok(&my_settings(), json!({})).await["myDmSettings"].clone();
    assert_eq!(settings["outside"], "REQUESTS");
    assert_eq!(settings["effectiveOutside"], "NONE");
    assert_eq!(
        settings["outsideFloorFrom"],
        json!({ "kind": "WORKSPACE", "workspaceId": acme, "workspaceName": "Acme" })
    );
    assert_eq!(
        settings["workspaces"],
        json!([{ "workspaceId": acme, "members": "REQUESTS", "memberDefault": "REQUESTS" }])
    );
    let below = bob.post(&set_outside(), json!({ "o": "REQUESTS" })).await;
    assert_eq!(code(&below), "VALIDATION", "{below}");
    let none = bob.ok(&set_outside(), json!({ "o": "NONE" })).await;
    assert_eq!(none["setMyDmSettings"]["outside"], "NONE");
    let direct = direct_space(&bob).await;
    assert_eq!(
        bob.err(&set_members(), json!({ "s": direct, "m": "DIRECT" }))
            .await,
        "NOT_FOUND",
        "the direct space is no workspace of his"
    );

    // Dave shares no workspace with Bob: his DM is refused when he starts it.
    let refused = dm(&a, &dave, &direct, &bob).await;
    assert_eq!(code(&refused), "FORBIDDEN", "{refused}");

    a.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_closed_server_takes_no_dm_from_outside_and_starts_none_with_another_server() {
    let (first, second) = databases_with(RESET).await;
    let b = start(&second, open_registration).await;
    let a = start(&first, |c| {
        open_registration(c);
        c.dm_inbound = Some("none".to_string());
        c.dm_outbound = Some("none".to_string());
    })
    .await;
    let alice = sign_up(&a, "alice", 1).await;
    let bob = sign_up(&b, "bob", 2).await;
    let carol = sign_up(&a, "carol", 3).await;

    assert_eq!(
        alice.ok(SERVER_INFO, json!({})).await["serverInfo"],
        json!({ "dmInbound": "NONE", "dmOutbound": "NONE", "dmPartners": [] })
    );
    let settings = alice.ok(&my_settings(), json!({})).await["myDmSettings"].clone();
    assert_eq!(settings["effectiveOutside"], "NONE");
    assert_eq!(settings["outsideFloorFrom"]["kind"], "OPERATOR");

    // Bob's relay reads no identity of Alice, so he cannot start a DM with her.
    assert_eq!(
        bob.err(
            REMOTE_IDENTITY,
            json!({ "p": alice.principal(), "r": a.origin })
        )
        .await,
        "NOT_FOUND"
    );
    let bob_direct = direct_space(&bob).await;
    assert_eq!(
        bob.err(
            CREATE,
            json!({ "input": {
                "serverId": bob_direct,
                "encrypted": true,
                "remoteMembers": [{ "principalId": alice.principal(), "relayUrls": [a.origin] }],
            } }),
        )
        .await,
        "NOT_FOUND"
    );

    // Alice starts no DM with someone on another server.
    let alice_direct = direct_space(&alice).await;
    let outbound = alice
        .post(
            CREATE,
            json!({ "input": {
                "serverId": alice_direct,
                "encrypted": true,
                "remoteMembers": [{ "principalId": bob.principal(), "relayUrls": [b.origin] }],
            } }),
        )
        .await;
    assert_eq!(code(&outbound), "FORBIDDEN", "{outbound}");
    assert_eq!(
        outbound["errors"][0]["message"],
        "this server starts no direct messages with that server"
    );

    // Carol, a stranger to Alice on her own server, is refused as well.
    let local = dm(&a, &carol, &alice_direct, &alice).await;
    assert_eq!(code(&local), "FORBIDDEN", "{local}");

    a.stop().await;
    b.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_strangers_dm_request_past_the_daily_limit_is_refused_and_reaches_no_one() {
    let a = one_server(|c| c.relay_dm_requests_per_day = Some(3)).await;
    let sam = sign_up(&a, "sam", 1).await;
    let direct = direct_space(&sam).await;
    let mut strangers = Vec::new();
    for seed in 10..14u8 {
        strangers.push(sign_up(&a, &format!("stranger{seed}"), seed).await);
    }
    let (last, first) = strangers.split_last().expect("strangers");

    // Sam's first three DMs of the day with strangers are requests.
    for stranger in first {
        created(&dm(&a, &sam, &direct, stranger).await);
    }
    first[2].pending_request(&sam).await;

    // The fourth is refused when it is started, and its recipient sees nothing of it.
    let refused = dm(&a, &sam, &direct, last).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(listed(last, &direct).await, Vec::<String>::new());
    let requests = last.ok(REQUESTS, json!({})).await;
    assert_eq!(requests["dmRequests"], json!([]));
    let notifications = last
        .ok(
            "query($s: ID!) { myNotifications(serverId: $s, dir: AFTER, limit: 50) { items { id } } \
             myUnreadNotificationCount(serverId: $s) }",
            json!({ "s": direct }),
        )
        .await;
    assert_eq!(notifications["myNotifications"]["items"], json!([]));
    assert_eq!(notifications["myUnreadNotificationCount"], json!(0));
    assert_eq!(code(&refused), "RATE_LIMITED", "{refused}");
    let retry = refused["errors"][0]["extensions"]["retryAfterSeconds"]
        .as_i64()
        .expect("retryAfterSeconds");
    assert!((1..=86_400).contains(&retry), "{retry}");

    // Once the recipient holds Sam as a contact, a DM from Sam files no request and is not limited.
    last.ok(
        "mutation($p: String!, $r: [String!]!) { addContact(principalId: $p, relayUrls: $r) { principalId } }",
        json!({ "p": sam.principal(), "r": [a.origin] }),
    )
    .await;
    let delivered = created(&dm(&a, &sam, &direct, last).await);
    assert!(listed(last, &direct).await.contains(&delivered));

    a.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_account_without_a_profile_card_starts_no_conversation_with_someone_here() {
    let a = one_server(|_| {}).await;
    let sam = sign_up_without_card(&a, "sam", 1).await;
    let ada = sign_up(&a, "ada", 2).await;
    let direct = direct_space(&sam).await;

    // Without a card no invite could take the DM through Ada's request inbox: refused.
    let refused = dm(&a, &sam, &direct, &ada).await;
    assert_eq!(code(&refused), "VALIDATION", "{refused}");
    assert!(
        refused["errors"][0]["message"]
            .as_str()
            .is_some_and(|m| m.starts_with("publish a profile card")),
        "{refused}"
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(listed(&ada, &direct).await, Vec::<String>::new());

    // Once Sam publishes one, the DM is a request Ada sees.
    sam.publish_card(1, &[a.origin.as_str()]).await;
    let conversation = created(&dm(&a, &sam, &direct, &ada).await);
    let request = ada.pending_request(&sam).await;
    assert_eq!(request["conversationId"], json!(conversation));

    a.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_contact_added_on_ones_own_still_counts_against_the_daily_invites_to_other_servers() {
    let (first, second) = databases_with(RESET).await;
    let b = start(&second, open_registration).await;
    let a = start(&first, |c| {
        open_registration(c);
        c.relay_dm_requests_per_day = Some(1);
    })
    .await;
    let sam = sign_up(&a, "sam", 1).await;
    let bob = sign_up(&b, "bob", 2).await;
    let carol = sign_up(&b, "carol", 3).await;
    let dave = sign_up(&b, "dave", 4).await;
    let direct = direct_space(&sam).await;
    let dm_with = |other: &Person| {
        json!({ "input": {
            "serverId": direct,
            "encrypted": true,
            "remoteMembers": [{ "principalId": other.principal(), "relayUrls": [b.origin] }],
        } })
    };

    // Sam's one invite of the day to another server goes to Bob.
    created(&sam.post(CREATE, dm_with(&bob)).await);

    // Adding Carol as a contact does not make her known: the next invite is over the quota.
    sam.ok(
        "mutation($p: String!, $r: [String!]!) { addContact(principalId: $p, relayUrls: $r) { principalId } }",
        json!({ "p": carol.principal(), "r": [b.origin] }),
    )
    .await;
    let refused = sam.post(CREATE, dm_with(&carol)).await;
    assert_eq!(code(&refused), "RATE_LIMITED", "{refused}");

    // Once Sam accepted Dave's own DM request, a conversation with him is no invite to count.
    dave.start_dm(&sam, &a, true).await;
    let request = sam.pending_request(&dave).await;
    sam.ok(
        "mutation($id: ID!) { acceptDmRequest(id: $id) { status } }",
        json!({ "id": request["id"] }),
    )
    .await;
    created(&sam.post(CREATE, dm_with(&dave)).await);

    a.stop().await;
    b.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_partners_people_reach_this_server_and_a_server_that_is_no_partner_reads_nothing() {
    let (first, second) = databases_with(RESET).await;
    // B takes DMs from one server it does not share with A.
    let b = start(&second, |c| {
        open_registration(c);
        c.dm_inbound = Some("partners".to_string());
        c.dm_partners = Some(ELSEWHERE.to_string());
    })
    .await;
    let a = start(&first, |c| {
        open_registration(c);
        c.dm_inbound = Some("partners".to_string());
        c.dm_partners = Some(b.id());
    })
    .await;
    let alice = sign_up(&a, "alice", 1).await;
    let bob = sign_up(&b, "bob", 2).await;

    assert_eq!(
        alice.ok(SERVER_INFO, json!({})).await["serverInfo"],
        json!({ "dmInbound": "PARTNERS", "dmOutbound": "ALLOWED", "dmPartners": [b.id()] })
    );

    // Bob, on a partner, starts a DM with Alice: it reaches her as a request.
    let conv = bob.start_dm(&alice, &a, true).await;
    let request = alice.pending_request(&bob).await;
    assert_eq!(request["hostServerId"], json!(b.id()));
    assert_eq!(request["conversationId"], json!(conv));

    // A is no partner of B: B serves A no identity.
    assert_eq!(
        alice
            .err(
                REMOTE_IDENTITY,
                json!({ "p": bob.principal(), "r": b.origin })
            )
            .await,
        "NOT_FOUND"
    );

    a.stop().await;
    b.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_member_who_leaves_a_group_they_accepted_keeps_no_accepted_request() {
    const LEAVE: &str =
        "mutation($s: ID!, $c: ID!) { leaveConversation(serverId: $s, conversationId: $c) }";
    let a = one_server(|_| {}).await;
    let alice = sign_up(&a, "alice", 1).await;
    let bob = sign_up(&a, "bob", 2).await;
    let carol = sign_up(&a, "carol", 3).await;
    let direct = direct_space(&alice).await;
    let (bob_id, carol_id) = (
        user_id(&a, &bob.principal()).await,
        user_id(&a, &carol.principal()).await,
    );
    let group = alice
        .ok(
            CREATE,
            json!({ "input": {
                "serverId": direct,
                "memberIds": [bob_id.to_string(), carol_id.to_string()],
                "title": "group",
            } }),
        )
        .await["createConversation"]["id"]
        .as_str()
        .expect("id")
        .to_string();
    let request = bob.pending_request(&alice).await;
    assert_eq!(request["conversationId"], json!(group));
    bob.ok(
        "mutation($id: ID!) { acceptDmRequest(id: $id) { status } }",
        json!({ "id": request["id"] }),
    )
    .await;
    let accepted = bob.ok(REQUESTS, json!({ "status": "ACCEPTED" })).await;
    assert_eq!(accepted["dmRequests"][0]["conversationId"], json!(group));
    assert!(listed(&bob, &direct).await.contains(&group));

    bob.ok(LEAVE, json!({ "s": direct, "c": group })).await;
    eventually("the accepted request to go", || async {
        let accepted = bob.ok(REQUESTS, json!({ "status": "ACCEPTED" })).await;
        (accepted["dmRequests"] == json!([])).then_some(())
    })
    .await;
    assert!(!listed(&bob, &direct).await.contains(&group));

    a.stop().await;
}

/// In the server's default workspace a group is started like a DM with each newcomer: one who
/// takes the workspace's DMs as requests gets it in their request inbox, one who takes them
/// directly sees it at once, and accepting the request lists it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_default_workspace_group_reaches_each_member_as_their_dm_settings_say() {
    let (database, _) = databases_with(RESET).await;
    let ctx = support::TestCtx::new().await;
    let lobby = ctx.create_server("Lobby").await.id.to_string();
    let a = start(&database, |c| {
        open_registration(c);
        c.default_workspace = Some(lobby.clone());
    })
    .await;
    let alice = sign_up(&a, "alice", 1).await;
    let bob = sign_up(&a, "bob", 2).await;
    let carol = sign_up(&a, "carol", 3).await;
    carol
        .ok(&set_members(), json!({ "s": lobby, "m": "REQUESTS" }))
        .await;
    let (alice_id, carol_id) = (
        user_id(&a, &alice.principal()).await,
        user_id(&a, &carol.principal()).await,
    );

    let group = created(
        &bob.post(
            CREATE,
            json!({ "input": {
                "serverId": lobby,
                "memberIds": [alice_id.to_string(), carol_id.to_string()],
            } }),
        )
        .await,
    );
    let request = carol.pending_request(&bob).await;
    assert_eq!(request["conversationId"], json!(group));
    assert!(
        !listed(&carol, &lobby).await.contains(&group),
        "a request is not a live conversation"
    );
    assert!(listed(&alice, &lobby).await.contains(&group));

    carol
        .ok(
            "mutation($id: ID!) { acceptDmRequest(id: $id) { status } }",
            json!({ "id": request["id"] }),
        )
        .await;
    assert!(listed(&carol, &lobby).await.contains(&group));

    // Dave blocked Bob: Bob neither starts a group with him nor adds him to one.
    let dave = sign_up(&a, "dave", 4).await;
    dave.ok(
        "mutation($p: String!) { blockPrincipal(principalId: $p) }",
        json!({ "p": bob.principal() }),
    )
    .await;
    let dave_id = user_id(&a, &dave.principal()).await;
    let refused = bob
        .post(
            CREATE,
            json!({ "input": {
                "serverId": lobby,
                "memberIds": [alice_id.to_string(), dave_id.to_string()],
            } }),
        )
        .await;
    assert_eq!(code(&refused), "FORBIDDEN", "{refused}");
    let refused = bob
        .post(
            "mutation($s: ID!, $c: ID!, $u: [ID!]!) { \
             addConversationMembers(serverId: $s, conversationId: $c, userIds: $u) { id } }",
            json!({ "s": lobby, "c": group, "u": [dave_id.to_string()] }),
        )
        .await;
    assert_eq!(code(&refused), "FORBIDDEN", "{refused}");

    a.stop().await;
}
