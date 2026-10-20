//! Two RIM servers as relays of their users and hosts of their direct spaces: an encrypted DM
//! across relays from creation through Welcome, Commit, a message, an edit and a read state;
//! the request inbox accepted, ignored and blocked, and an expired request refused; queued host
//! events pushed on `relayStream` and a gap pulled from the host's outbox; forwarded requests
//! with replayed and tampered device requests; forwarded GraphQL under the host's batch cap and
//! a mutation budget per device; the roles' refusals; and a dead relay that gets no new outbox
//! rows until its first success, after which its streams show the gap. Clients talk to their own
//! relay only, over HTTP, and check every host event and forwarded answer against the host's
//! key. The public relay: proof-of-work
//! signups refused, accepted and replaced by an invite; a reported DM request listed and
//! resolved by the operator; a principal blocked on its relay signed out and refused signup,
//! login and delivery; and a peer blocked both ways. A member's accepted row on their relay is
//! filed again from a resent invite after it expired, and goes when they leave the group.
//!
//! Run with:
//! ```sh
//! cargo test -p rim_api --features e2e-tests -- --test-threads=1
//! ```
#![cfg(feature = "e2e-tests")]

mod support;

use std::time::Duration;

use async_graphql::Request;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use futures_util::StreamExt;
use rim_api::auth_context::AuthActorCtx;
use rim_core::user::model::UserId;
use rim_server::config::RIMConfig;
use rim_services::relay::service::RelayService;
use sea_orm::{ConnectionTrait, DbBackend, Statement};
use serde_json::{Value, json};
use uuid::Uuid;

use support::mls::content;
use support::nodes::{Node, databases_with, start};
use support::relay::{
    ACK_WELCOMES, CLAIM, CREATE, EDIT_ENCRYPTED, HEADS, Item, MARK_READ, PUBLISH, Person,
    RELAY_INFO, REQUESTS, RESET, SEND_ENCRYPTED, SUBMIT, WELCOMES, anonymous_direct_space,
    device_key_id, drained, eventually, key_signup, keys, login_proof, open_registration, relays,
    sign_up, sign_up_with_card, statement, user_id, verify_item,
};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_encrypted_dm_crosses_relays_from_invite_to_read_state() {
    let (a, b) = relays().await;
    let alice = sign_up(&a, "alice", 1).await;
    let mut bob = sign_up(&b, "bob", 2).await;

    // Alice's relay fetches Bob's identity from his relay, verified.
    let identity = alice
        .ok(
            "query($p: String!, $r: String) { remoteIdentity(principalId: $p, relayUrl: $r) { principalId servedBy keyLog { payload } devices { deviceId mlsActive } card { payload } } }",
            json!({ "p": bob.principal(), "r": b.origin }),
        )
        .await["remoteIdentity"]
        .clone();
    assert_eq!(identity["servedBy"], json!(b.id()));
    assert_eq!(
        identity["devices"][0]["deviceId"],
        json!(bob.device_id().to_string())
    );
    assert_eq!(identity["devices"][0]["mlsActive"], json!(true));
    assert!(identity["card"]["payload"].is_string());

    // Alice starts an encrypted DM in her relay's direct space.
    let direct = alice.direct_space().await;
    let conv = alice.start_dm(&bob, &b, true).await;

    // Bob's relay files it as a request: Alice is not his contact.
    let request = bob.pending_request(&alice).await;
    assert_eq!(request["hostServerId"], json!(a.id()));
    assert_eq!(request["conversationId"], json!(conv));
    let held: Vec<Item> = request["heldItems"]
        .as_array()
        .expect("held")
        .iter()
        .map(|i| verify_item(&a, i))
        .collect();
    assert!(!held.is_empty() && held.len() <= 3);
    let invite = &held[0];
    assert_eq!(invite.event["type"], "DmInvite");
    assert_eq!(invite.stream, format!("u:{}", bob.principal()));
    assert_eq!(invite.event["from_principal_id"], json!(alice.principal()));
    assert_eq!(invite.event["from_card"], request["card"]);
    assert!(
        bob.items(&a, None).await.is_empty(),
        "held items wait for the request"
    );

    let accepted = bob
        .ok(
            "mutation($id: ID!) { acceptDmRequest(id: $id) { status } }",
            json!({ "id": request["id"] }),
        )
        .await;
    assert_eq!(accepted["acceptDmRequest"]["status"], "ACCEPTED");
    let contacts = bob
        .ok("query { contacts { principalId relayUrls } }", json!({}))
        .await;
    assert_eq!(
        contacts["contacts"][0]["principalId"],
        json!(alice.principal())
    );
    assert_eq!(contacts["contacts"][0]["relayUrls"], json!([a.origin]));
    let released = bob.items(&a, None).await;
    assert_eq!(released[0].event["type"], "DmInvite");
    let created = released
        .iter()
        .find(|i| i.event["type"] == "ConversationCreated")
        .expect("ConversationCreated");
    assert_eq!(created.stream, format!("c:{conv}"));
    assert_eq!(created.seq, Some(1));

    // Alice claims Bob's device through his relay: he accepted her, so a one-time package.
    let claimed = alice
        .ok(
            CLAIM,
            json!({ "input": {
                "serverId": direct,
                "conversationId": conv,
                "devices": [{ "principalId": bob.principal(), "deviceId": bob.device_id() }],
            } }),
        )
        .await["claimMlsKeyPackages"]
        .clone();
    assert_eq!(claimed[0]["lastResort"], json!(false));
    let mut alice = alice;
    let conversation_id: Uuid = conv.parse().expect("uuid");
    let bob_principal = bob.principal();
    let (commit, welcome) = alice.mls.create_group(
        conversation_id,
        &[(
            bob_principal.as_str(),
            bob.device_id(),
            claimed[0]["keyPackage"].as_str().expect("package"),
        )],
    );
    alice
        .ok(
            SUBMIT,
            json!({ "input": {
                "serverId": direct, "conversationId": conv, "incarnation": 1, "epoch": 0,
                "cipherSuite": 0x004f, "commit": commit, "welcome": welcome,
            } }),
        )
        .await;
    alice.mls.merge_pending();

    // The Welcome notice reaches Bob's device through his relay; he fetches the Welcome from
    // the home, through his relay, and acknowledges it.
    let notice = bob
        .item_of(&a, "MlsWelcomeAvailable", |i| {
            i.event["device_id"] == json!(bob.device_id().to_string())
        })
        .await;
    assert_eq!(notice.stream, format!("u:{}", bob.principal()));
    let welcomes = bob.remote(&a, WELCOMES, json!({})).await["myMlsWelcomes"].clone();
    assert_eq!(welcomes[0]["conversationId"], json!(conv));
    bob.mls
        .join(welcomes[0]["message"].as_str().expect("welcome"));
    let acked = bob
        .remote(&a, ACK_WELCOMES, json!({ "ids": [welcomes[0]["id"]] }))
        .await;
    assert_eq!(acked["ackMlsWelcomes"], json!(1));

    // Alice writes and edits; Bob decrypts both from host-signed events on his relay.
    let message_id = Uuid::now_v7();
    let body = content(message_id, 0, "hello across relays", &[]);
    let envelope = alice.mls.encrypt(message_id, &body, [3; 32], &[]);
    alice
        .ok(
            SEND_ENCRYPTED,
            json!({ "input": { "serverId": direct, "conversationId": conv, "messageId": message_id, "envelope": envelope } }),
        )
        .await;
    let new_message = bob
        .item_of(&a, "NewMessage", |i| {
            i.event["message"]["id"] == json!(message_id.to_string())
        })
        .await;
    let ciphertext = new_message.event["message"]["encryption"]["ciphertext"]
        .as_str()
        .expect("ciphertext")
        .to_string();
    let (received, _) = bob.mls.decrypt(&ciphertext, &message_id.to_string());
    assert_eq!(received, body);

    let edited = content(message_id, 1, "hello across relays, edited", &[]);
    let envelope = alice.mls.encrypt(message_id, &edited, [4; 32], &[]);
    alice
        .ok(
            EDIT_ENCRYPTED,
            json!({ "input": { "serverId": direct, "conversationId": conv, "messageId": message_id, "envelope": envelope } }),
        )
        .await;
    let updated = bob
        .item_of(&a, "MessageUpdated", |i| {
            i.event["message"]["id"] == json!(message_id.to_string())
        })
        .await;
    assert!(updated.seq > new_message.seq, "one stream, in order");
    let ciphertext = updated.event["message"]["encryption"]["ciphertext"]
        .as_str()
        .expect("ciphertext")
        .to_string();
    let (received, _) = bob.mls.decrypt(&ciphertext, &message_id.to_string());
    assert_eq!(received, edited);

    // Bob marks it read at the home; the read state comes back on his own stream.
    let read = bob
        .remote(
            &a,
            MARK_READ,
            json!({ "input": { "workspaceId": direct, "conversationId": conv, "readAt": chrono::Utc::now().to_rfc3339() } }),
        )
        .await;
    assert_eq!(read["markAsRead"], json!(true));
    let state = bob
        .item_of(&a, "ReadStateUpdated", |i| {
            i.event["conversation_id"] == json!(conv)
        })
        .await;
    assert_eq!(state.stream, format!("u:{}", bob.principal()));

    // Stream heads at the home: his streams, not the members-only `w`.
    let heads = bob
        .remote(
            &a,
            HEADS,
            json!({ "serverId": direct, "streams": [format!("c:{conv}"), format!("u:{}", bob.principal()), "w"] }),
        )
        .await["streamHeads"]
        .clone();
    let heads = heads.as_array().expect("heads");
    assert_eq!(heads.len(), 2, "a guest does not see w");
    assert_eq!(
        heads[0]["seq"],
        json!(updated.seq.expect("seq").to_string())
    );

    // Acknowledged items leave the queue once every device passed them.
    let last = bob.items(&a, None).await.last().expect("items").id;
    bob.ok(
        "mutation($u: ID!) { ackRelayItems(upTo: $u) }",
        json!({ "u": last.to_string() }),
    )
    .await;
    assert!(bob.items(&a, None).await.is_empty());
    b.s2s.relay.cleanup().await.expect("cleanup");
    let left =
        b.db.query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT count(*) AS n FROM relay_item WHERE request_id IS NULL",
        ))
        .await
        .expect("query")
        .expect("row")
        .try_get::<i64>("", "n")
        .expect("n");
    assert_eq!(left, 0);

    a.stop().await;
    b.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dm_requests_are_held_for_strangers_ignored_and_blocked() {
    let (a, b) = relays().await;
    let mut alice = sign_up(&a, "alice", 1).await;
    let bob = sign_up(&b, "bob", 2).await;
    let carol = sign_up(&a, "carol", 3).await;

    // A stranger's claim gets the last-resort package only.
    let conv = alice.start_dm(&bob, &b, true).await;
    let direct = alice.direct_space().await;
    let request = bob.pending_request(&alice).await;
    let claimed = alice
        .ok(
            CLAIM,
            json!({ "input": {
                "serverId": direct,
                "conversationId": conv,
                "devices": [{ "principalId": bob.principal(), "deviceId": bob.device_id() }],
            } }),
        )
        .await["claimMlsKeyPackages"]
        .clone();
    assert_eq!(claimed[0]["lastResort"], json!(true));
    let bob_principal = bob.principal();
    let (commit, welcome) = alice.mls.create_group(
        conv.parse().expect("uuid"),
        &[(
            bob_principal.as_str(),
            bob.device_id(),
            claimed[0]["keyPackage"].as_str().expect("package"),
        )],
    );
    alice
        .ok(
            SUBMIT,
            json!({ "input": {
                "serverId": direct, "conversationId": conv, "incarnation": 1, "epoch": 0,
                "cipherSuite": 0x004f, "commit": commit, "welcome": welcome,
            } }),
        )
        .await;
    eventually("three held items", || async {
        let held = bob.pending_request(&alice).await["heldItems"]
            .as_array()
            .expect("held")
            .len();
        assert!(held <= 3, "later items are dropped");
        (held == 3).then_some(())
    })
    .await;
    drained(&a).await;

    // Ignoring deletes the held items and drops what follows.
    let ignored = bob
        .ok(
            "mutation($id: ID!) { ignoreDmRequest(id: $id) { status heldItems { id hostServerId workspaceId envelope { payload signature } } } }",
            json!({ "id": request["id"] }),
        )
        .await;
    assert_eq!(ignored["ignoreDmRequest"]["status"], "IGNORED");
    let kept = ignored["ignoreDmRequest"]["heldItems"]
        .as_array()
        .expect("held");
    assert_eq!(kept.len(), 1, "only the invite stays");
    assert_eq!(verify_item(&a, &kept[0]).event["type"], "DmInvite");
    let code = bob
        .err(
            "mutation($id: ID!) { acceptDmRequest(id: $id) { status } }",
            json!({ "id": request["id"] }),
        )
        .await;
    assert_eq!(code, "CONFLICT");
    alice.mls.merge_pending();
    let message_id = Uuid::now_v7();
    let envelope = alice.mls.encrypt(
        message_id,
        &content(message_id, 0, "hi?", &[]),
        [5; 32],
        &[],
    );
    alice
        .ok(
            SEND_ENCRYPTED,
            json!({ "input": { "serverId": direct, "conversationId": conv, "messageId": message_id, "envelope": envelope } }),
        )
        .await;
    drained(&a).await;
    assert!(
        bob.items(&a, None).await.is_empty(),
        "an ignored conversation stays out"
    );

    // Blocking ignores pending requests and refuses later invites.
    carol.start_dm(&bob, &b, true).await;
    let from_carol = bob.pending_request(&carol).await;
    let blocked = bob
        .ok(
            "mutation($p: String!) { blockPrincipal(principalId: $p) }",
            json!({ "p": carol.principal() }),
        )
        .await;
    assert_eq!(blocked["blockPrincipal"], json!(true));
    let requests = bob.ok(REQUESTS, json!({ "status": "IGNORED" })).await;
    assert!(
        requests["dmRequests"]
            .as_array()
            .expect("requests")
            .iter()
            .any(|r| r["id"] == from_carol["id"])
    );
    let direct = carol.direct_space().await;
    carol
        .ok(
            CREATE,
            json!({ "input": {
                "serverId": direct,
                "memberIds": [],
                "remoteMembers": [{ "principalId": bob.principal(), "relayUrls": [b.origin] }],
                "title": "group",
                "encrypted": true,
            } }),
        )
        .await;
    drained(&a).await;
    let pending = bob.ok(REQUESTS, json!({ "status": "PENDING" })).await;
    assert_eq!(
        pending["dmRequests"],
        json!([]),
        "a blocked sender files nothing"
    );
    let report = bob
        .ok(
            "mutation($id: ID!, $r: String!) { reportDmRequest(id: $id, reason: $r) }",
            json!({ "id": from_carol["id"], "r": "spam" }),
        )
        .await;
    assert_eq!(report["reportDmRequest"], json!(true));
    let unblocked = bob
        .ok(
            "mutation($p: String!) { unblockPrincipal(principalId: $p) }",
            json!({ "p": carol.principal() }),
        )
        .await;
    assert_eq!(unblocked["unblockPrincipal"], json!(true));

    a.stop().await;
    b.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_expired_dm_request_cannot_be_accepted_ignored_or_reported() {
    let (a, b) = relays().await;
    let alice = sign_up(&a, "alice", 1).await;
    let bob = sign_up(&b, "bob", 2).await;
    alice.start_dm(&bob, &b, true).await;
    let request = bob.pending_request(&alice).await;
    let id: Uuid = request["id"].as_str().expect("id").parse().expect("uuid");
    b.db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE relay_dm_request SET expires_at = now() - interval '1 minute' WHERE id = $1",
        [id.into()],
    ))
    .await
    .expect("expire the request");

    for mutation in [
        "mutation($id: ID!) { acceptDmRequest(id: $id) { status } }",
        "mutation($id: ID!) { ignoreDmRequest(id: $id) { status } }",
        "mutation($id: ID!) { reportDmRequest(id: $id, reason: \"spam\") }",
    ] {
        assert_eq!(
            bob.err(mutation, json!({ "id": request["id"] })).await,
            "NOT_FOUND",
            "{mutation}"
        );
    }

    a.stop().await;
    b.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn relay_stream_pushes_host_events_and_a_gap_is_pulled_from_the_outbox() {
    let (a, b) = relays().await;
    let mut alice = sign_up(&a, "alice", 1).await;
    let bob = sign_up(&b, "bob", 2).await;
    // A contact's conversation flows without a request.
    bob.ok(
        "mutation($p: String!, $r: [String!]!) { addContact(principalId: $p, relayUrls: $r) { principalId } }",
        json!({ "p": alice.principal(), "r": [a.origin] }),
    )
    .await;

    let bob_user = user_id(&b, &bob.principal()).await;
    let bob_device = device_key_id(&b, bob.device_id()).await;
    let mut stream =
        b.schema
            .execute_stream(Request::new("subscription { relayStream }").data(
                AuthActorCtx::new(Some(bob_user), None).with_device_key_id(Some(bob_device)),
            ));

    let conv = alice.start_dm(&bob, &b, true).await;
    let direct = alice.direct_space().await;
    alice.create_group_with(&bob, &direct, &conv).await;
    let first_id = alice.send_encrypted(&direct, &conv, "first").await;
    let mut seen = Vec::new();
    while !seen.iter().any(|i: &Item| i.event["type"] == "NewMessage") {
        let response = tokio::time::timeout(Duration::from_secs(20), stream.next())
            .await
            .expect("an item in time")
            .expect("the stream stays open");
        assert!(response.errors.is_empty(), "{:?}", response.errors);
        let text = response.data.into_json().expect("json")["relayStream"]
            .as_str()
            .expect("json string")
            .to_string();
        let pushed: Value = serde_json::from_str(&text).expect("item json");
        assert_eq!(pushed["type"], "RelayItem");
        seen.push(verify_item(&a, &pushed));
    }
    assert!(
        seen.iter().any(|i| i.event["type"] == "DmInvite"),
        "a contact's invite flows"
    );
    let first = seen
        .iter()
        .find(|i| i.event["type"] == "NewMessage")
        .expect("NewMessage")
        .clone();
    assert_eq!(first.event["message"]["id"], json!(first_id.to_string()));

    // Bob's relay misses a delivery; it pulls the gap from Alice's relay's outbox. Holding
    // deliveries waits for the sender to finish, so no acknowledgement in flight resets it.
    drained(&a).await;
    a.db.execute_unprepared(
        "UPDATE s2s_peer_delivery SET next_attempt_at = now() + interval '1 hour', failures = 9",
    )
    .await
    .expect("hold deliveries");
    let second_id = json!(
        alice
            .send_encrypted(&direct, &conv, "second")
            .await
            .to_string()
    );
    tokio::time::sleep(Duration::from_secs(2)).await;
    let before = bob.items(&a, Some(first.id)).await;
    assert!(
        before.iter().all(|i| i.event["message"]["id"] != second_id),
        "the delivery is held"
    );
    let pulled = b.s2s.relay.pull_outbox(a.id()).await.expect("pull");
    assert!(pulled >= 1);
    let after = bob.items(&a, Some(first.id)).await;
    let second = after
        .iter()
        .find(|i| i.event["message"]["id"] == second_id)
        .expect("the pulled item");
    assert_eq!(
        second.seq,
        first.seq.map(|s| s + 1),
        "the conversation stream has no gap"
    );
    assert_eq!(
        b.s2s.relay.pull_outbox(a.id()).await.expect("pull"),
        0,
        "nothing above what was received"
    );

    a.stop().await;
    b.stop().await;
}

/// Outbox rows `host` holds for `relay` above what the relay acknowledged.
async fn waiting_for(host: &Node, relay: &Node) -> i64 {
    host.db
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT count(*) AS n FROM s2s_outbox WHERE peer_server_id = $1 AND id > \
             (SELECT COALESCE(max(delivered_through), 0) FROM s2s_peer_delivery \
              WHERE peer_server_id = $1)",
            [relay.id().into()],
        ))
        .await
        .expect("query")
        .expect("row")
        .try_get::<i64>("", "n")
        .expect("n")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_dead_relay_gets_nothing_new_until_its_first_success_and_its_streams_show_the_gap() {
    let (a, b) = relays().await;
    let mut alice = sign_up(&a, "alice", 1).await;
    let bob = sign_up(&b, "bob", 2).await;
    bob.ok(
        "mutation($p: String!, $r: [String!]!) { addContact(principalId: $p, relayUrls: $r) { principalId } }",
        json!({ "p": alice.principal(), "r": [a.origin] }),
    )
    .await;
    let conv = alice.start_dm(&bob, &b, true).await;
    let direct = alice.direct_space().await;
    alice.create_group_with(&bob, &direct, &conv).await;
    let first_id = json!(
        alice
            .send_encrypted(&direct, &conv, "first")
            .await
            .to_string()
    );
    let first = bob
        .item_of(&a, "NewMessage", |i| i.event["message"]["id"] == first_id)
        .await;
    drained(&a).await;

    // Every delivery to Bob's relay has failed for over a day. The writer reads a relay's
    // state again after 10 seconds.
    a.db.execute_unprepared(&format!(
        "UPDATE s2s_peer_delivery SET next_attempt_at = now() + interval '1 hour', \
         failures = 40, failing_since = now() - interval '25 hours' \
         WHERE peer_server_id = '{}'",
        b.id()
    ))
    .await
    .expect("a dead relay");
    tokio::time::sleep(Duration::from_secs(11)).await;
    let second_id = json!(
        alice
            .send_encrypted(&direct, &conv, "second")
            .await
            .to_string()
    );
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(
        waiting_for(&a, &b).await,
        0,
        "nothing is queued for a dead relay"
    );

    // The next attempt is a probe with nothing to send; its success ends the failures.
    a.db.execute_unprepared("UPDATE s2s_peer_delivery SET next_attempt_at = now()")
        .await
        .expect("attempt now");
    eventually("the probe to succeed", || async {
        let row =
            a.db.query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT failures = 0 AND failing_since IS NULL AS ok FROM s2s_peer_delivery \
                 WHERE peer_server_id = $1",
                [b.id().into()],
            ))
            .await
            .expect("query")
            .expect("row");
        row.try_get::<bool>("", "ok").expect("ok").then_some(())
    })
    .await;
    let third_id = json!(
        alice
            .send_encrypted(&direct, &conv, "third")
            .await
            .to_string()
    );
    let third = bob
        .item_of(&a, "NewMessage", |i| i.event["message"]["id"] == third_id)
        .await;
    let (first_seq, third_seq) = (first.seq.expect("seq"), third.seq.expect("seq"));
    assert!(
        third_seq >= first_seq + 2,
        "the conversation stream shows the gap the app reconciles from the host: {first_seq} then {third_seq}"
    );
    assert!(
        bob.items(&a, None)
            .await
            .iter()
            .all(|i| i.event["message"]["id"] != second_id),
        "the event written while the relay was dead never reaches it"
    );

    a.stop().await;
    b.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn forwarded_requests_refuse_replays_tampering_and_relays_a_card_does_not_name() {
    let (a, b) = relays().await;
    let alice = sign_up(&a, "alice", 1).await;
    let mut bob = sign_up(&b, "bob", 2).await;
    alice.start_dm(&bob, &b, true).await;
    bob.pending_request(&alice).await;
    let direct = alice.direct_space().await;

    let query = json!({ "query": "query($s: ID!) { streamHeads(serverId: $s, streams: [\"w\"]) { seq } }", "variables": { "s": direct } });
    let body = serde_json::to_vec(&query).expect("json");
    let (answer, seq) = bob
        .forward(&a, "POST", "/api", body.clone(), None, None)
        .await;
    assert_eq!(answer.status, 200);
    assert!(answer.host_signed);

    // The same seq again: a replay, with the highest seq so the client moves past it.
    let (replayed, _) = bob
        .forward(&a, "POST", "/api", body.clone(), None, Some(seq))
        .await;
    assert_eq!(replayed.status, 409);
    assert!(replayed.host_signed, "refusals are host-signed too");
    assert_eq!(replayed.body["error"], "replay");
    assert_eq!(replayed.body["highestSeq"], json!(seq.to_string()));

    // Another body than the one signed.
    let (tampered, _) = bob
        .forward(
            &a,
            "POST",
            "/api",
            body.clone(),
            Some(br#"{"query":"query { relayInfo { serverId } }"}"#.to_vec()),
            None,
        )
        .await;
    assert_eq!(tampered.status, 403);
    assert_eq!(tampered.body["error"], "forbidden");

    // Account mutations stay at a person's own relay.
    let card = bob.identity.profile_card(
        &bob.state,
        9,
        "Bob",
        &[b.origin.as_str()],
        &[(bob.device_id(), &bob.device_key)],
    );
    let publish =
        serde_json::to_vec(&json!({ "query": PUBLISH, "variables": { "card": statement(&card) } }))
            .expect("json");
    let (refused, _) = bob.forward(&a, "POST", "/api", publish, None, None).await;
    assert_eq!(refused.status, 200);
    assert_eq!(refused.body["errors"][0]["extensions"]["code"], "FORBIDDEN");

    // A request without the device headers never reaches the host's handlers.
    let raw = reqwest::Client::new()
        .post(format!("{}/api/remote/{}/api", b.origin, a.id()))
        .header(
            "cookie",
            format!("rim_session={}; rim_csrf={}", bob.session, bob.csrf),
        )
        .header("x-csrf-token", &bob.csrf)
        .body(body.clone())
        .send()
        .await
        .expect("send");
    assert_eq!(raw.status(), 400);

    // Once Bob's card stops naming his relay, the host refuses what it forwards.
    bob.publish_card(2, &["https://elsewhere.example"]).await;
    let bob_on_a = user_id(&a, &bob.principal()).await;
    eventually("the pushed card", || async {
        let row =
            a.db.query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT version FROM profile_card WHERE user_id = $1",
                [Uuid::from(bob_on_a).into()],
            ))
            .await
            .expect("query")?;
        (row.try_get::<i64>("", "version").expect("version") == 2).then_some(())
    })
    .await;
    let (stranger, _) = bob.forward(&a, "POST", "/api", body, None, None).await;
    assert_eq!(stranger.status, 403);
    assert_eq!(stranger.body["error"], "not_delegated");

    a.stop().await;
    b.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn forwarded_graphql_keeps_the_hosts_batch_cap_and_a_mutation_budget_per_device() {
    let (a_url, b_url) = databases_with(RESET).await;
    let a = start(&a_url, |config| {
        open_registration(config);
        config.api_max_batch = Some(2);
        config.rate_mutations_per_min_per_session = Some(10);
    })
    .await;
    let b = start(&b_url, open_registration).await;
    let alice = sign_up(&a, "alice", 1).await;
    let mut bob = sign_up(&b, "bob", 2).await;
    let mut carol = sign_up(&b, "carol", 3).await;
    alice.start_dm(&bob, &b, true).await;
    alice.start_dm(&carol, &b, true).await;
    bob.pending_request(&alice).await;
    carol.pending_request(&alice).await;
    let direct = alice.direct_space().await;
    let heads = json!({ "query": "query($s: ID!) { streamHeads(serverId: $s, streams: [\"w\"]) { seq } }", "variables": { "s": direct } });

    // The relay allows 20 requests in a batch; the host allows 2.
    let three = serde_json::to_vec(&json!([heads, heads, heads])).expect("json");
    let (refused, _) = bob.forward(&a, "POST", "/api", three, None, None).await;
    assert_eq!(refused.status, 400);
    assert!(refused.host_signed);
    assert_eq!(refused.body, json!({ "error": "batch_too_large" }));
    let two = serde_json::to_vec(&json!([heads, heads])).expect("json");
    let (answer, _) = bob.forward(&a, "POST", "/api", two, None, None).await;
    assert_eq!(answer.status, 200, "{}", answer.body);
    assert_eq!(answer.body.as_array().map(Vec::len), Some(2));

    // Six mutation fields of the ten a minute: charged though none of them exists.
    let six = serde_json::to_vec(&json!({
        "query": "mutation { a: nope b: nope c: nope d: nope e: nope f: nope }",
    }))
    .expect("json");
    let (charged, _) = bob
        .forward(&a, "POST", "/api", six.clone(), None, None)
        .await;
    assert_eq!(charged.status, 200);
    assert_ne!(
        charged.body["errors"][0]["extensions"]["code"],
        "RATE_LIMITED"
    );
    let (limited, _) = bob
        .forward(&a, "POST", "/api", six.clone(), None, None)
        .await;
    assert_eq!(limited.status, 200);
    assert!(limited.host_signed);
    assert_eq!(
        limited.body["errors"][0]["extensions"]["code"], "RATE_LIMITED",
        "{}",
        limited.body
    );
    assert!(limited.body["errors"][0]["extensions"]["retryAfterSeconds"].as_u64() > Some(0));

    // Another person's device on the same relay has its own budget.
    let (own, _) = carol.forward(&a, "POST", "/api", six, None, None).await;
    assert_eq!(own.status, 200);
    assert_ne!(
        own.body["errors"][0]["extensions"]["code"], "RATE_LIMITED",
        "{}",
        own.body
    );

    a.stop().await;
    b.stop().await;
}

/// A multipart body with one file field.
fn multipart_file(boundary: &str, filename: &str, content_type: &str, data: &[u8]) -> Vec<u8> {
    let mut body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\n\
         Content-Type: {content_type}\r\n\r\n"
    )
    .into_bytes();
    body.extend_from_slice(data);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    body
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn avatars_upload_and_download_through_the_forward_path() {
    const BOUNDARY: &str = "rim-test-boundary";
    let (a, b) = relays().await;
    let alice = sign_up(&a, "alice", 1).await;
    let mut bob = sign_up(&b, "bob", 2).await;
    let mut carol = sign_up(&b, "carol", 3).await;
    alice.start_dm(&bob, &b, true).await;
    bob.pending_request(&alice).await;
    let multipart = format!("multipart/form-data; boundary={BOUNDARY}");

    // Bob stores his avatar at Alice's server through his relay.
    let png = b"\x89PNG\r\n\x1a\nbob";
    let (uploaded, _) = bob
        .forward_typed(
            &a,
            "POST",
            "/api/files/avatar",
            multipart_file(BOUNDARY, "me.png", "image/png", png),
            None,
            None,
            &multipart,
        )
        .await;
    assert_eq!(uploaded.status, 200, "{}", uploaded.body);
    assert!(uploaded.host_signed);
    let bob_on_a = user_id(&a, &bob.principal()).await;
    assert_eq!(
        uploaded.body["path"],
        json!(format!("avatars/{bob_on_a}/avatar.png"))
    );

    // Bob makes it his avatar at Alice's server, with a status; Alice's server stores both.
    let avatar_url = uploaded.body["url"].as_str().expect("url").to_string();
    let set = bob
        .remote(
            &a,
            r#"mutation($input: UpdateMyProfileInput!) {
                updateMyProfile(input: $input) { id avatarUrl statusText }
            }"#,
            json!({ "input": { "avatarUrl": avatar_url, "statusText": "on the road" } }),
        )
        .await;
    assert_eq!(set["updateMyProfile"]["id"], json!(bob_on_a.to_string()));
    assert_eq!(set["updateMyProfile"]["avatarUrl"], json!(avatar_url));
    let row =
        a.db.query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"SELECT avatar_url, status_text, display_name FROM "user" WHERE id = $1"#,
            [Uuid::from(bob_on_a).into()],
        ))
        .await
        .expect("query")
        .expect("bob's row");
    assert_eq!(
        row.try_get::<Option<String>>("", "avatar_url")
            .expect("avatar"),
        Some(avatar_url.clone())
    );
    assert_eq!(
        row.try_get::<Option<String>>("", "status_text")
            .expect("status"),
        Some("on the road".to_string())
    );
    let card_name = row
        .try_get::<Option<String>>("", "display_name")
        .expect("name");

    // His display name comes from his profile card: setting it at Alice's server is refused.
    let body = serde_json::to_vec(&json!({
        "query": r#"mutation { updateMyProfile(input: { displayName: "Robert", avatarUrl: "" }) { id } }"#,
    }))
    .expect("json");
    let (refused, _) = bob.forward(&a, "POST", "/api", body, None, None).await;
    assert_eq!(refused.status, 200);
    assert!(refused.host_signed);
    assert_eq!(
        refused.body["errors"][0]["extensions"]["code"], "FORBIDDEN",
        "{}",
        refused.body
    );
    let row =
        a.db.query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"SELECT avatar_url, display_name FROM "user" WHERE id = $1"#,
            [Uuid::from(bob_on_a).into()],
        ))
        .await
        .expect("query")
        .expect("bob's row");
    assert_eq!(
        row.try_get::<Option<String>>("", "avatar_url")
            .expect("avatar"),
        Some(avatar_url),
        "a refused mutation changes nothing"
    );
    assert_eq!(
        row.try_get::<Option<String>>("", "display_name")
            .expect("name"),
        card_name
    );

    // Alice's avatar, stored on her own server, reaches Bob through the same path.
    let alice_png = b"\x89PNG\r\n\x1a\nalice";
    let stored: Value = alice
        .http
        .post(format!("{}/api/files/avatar", a.origin))
        .header(
            "cookie",
            format!("rim_session={}; rim_csrf={}", alice.session, alice.csrf),
        )
        .header("x-csrf-token", &alice.csrf)
        .header("content-type", &multipart)
        .body(multipart_file(BOUNDARY, "me.png", "image/png", alice_png))
        .send()
        .await
        .expect("send")
        .json()
        .await
        .expect("json");
    let alice_path = format!("/api/files/{}", stored["path"].as_str().expect("path"));
    let (downloaded, _) = bob
        .forward(&a, "GET", &alice_path, Vec::new(), None, None)
        .await;
    assert_eq!(downloaded.status, 200);
    assert!(downloaded.host_signed);
    assert_eq!(downloaded.bytes, alice_png);
    let (own, _) = bob
        .forward(
            &a,
            "GET",
            &format!("/api/files/avatars/{bob_on_a}/avatar.png"),
            Vec::new(),
            None,
            None,
        )
        .await;
    assert_eq!(own.bytes, png);

    // Only avatars are served.
    for path in ["/api/files/other/x", "/api/files/uploads/x"] {
        let (denied, _) = bob.forward(&a, "GET", path, Vec::new(), None, None).await;
        assert_eq!(denied.status, 403, "{path}");
        assert!(denied.host_signed);
    }

    // A device request for another path than the one forwarded never reaches the handler.
    let (replaced, _) = bob
        .forward_typed(
            &a,
            "POST",
            "/api/files/avatar",
            multipart_file(BOUNDARY, "me.png", "image/png", png),
            Some(multipart_file(
                BOUNDARY,
                "me.png",
                "image/png",
                b"\x89PNGother",
            )),
            None,
            &multipart,
        )
        .await;
    assert_eq!(replaced.status, 403);
    assert_eq!(replaced.body["error"], "forbidden");

    // Carol is no one at Alice's server: the host refuses to run her requests.
    let (stranger, _) = carol
        .forward(&a, "GET", &alice_path, Vec::new(), None, None)
        .await;
    assert_eq!(stranger.status, 403);
    assert_eq!(stranger.body["error"], "forbidden");

    a.stop().await;
    b.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn relay_and_host_roles_refuse_what_they_do_not_run() {
    let (first, second) = databases_with(RESET).await;
    let host = start(&first, |c| {
        open_registration(c);
        c.role = Some("host".to_string());
    })
    .await;
    let relay = start(&second, |c| {
        open_registration(c);
        c.role = Some("relay".to_string());
    })
    .await;
    let carol = sign_up(&relay, "carol", 3).await;
    let dave = sign_up(&host, "dave", 4).await;

    // A relay hosts no new workspaces; a host keeps no contacts.
    let code = carol
        .err(
            "mutation { createServer(input: { name: \"w\" }) { id } }",
            json!({}),
        )
        .await;
    assert_eq!(code, "FORBIDDEN");
    let code = dave
        .err("query { contacts { principalId } }", json!({}))
        .await;
    assert_eq!(code, "FORBIDDEN");
    let info = dave.ok(RELAY_INFO, json!({})).await;
    assert_eq!(info["relayInfo"]["roles"], json!(["host"]));

    // A host forwards nothing for its users and takes no deliveries.
    let remote = reqwest::Client::new()
        .post(format!("{}/api/remote/{}/api", host.origin, relay.id()))
        .header(
            "cookie",
            format!("rim_session={}; rim_csrf={}", dave.session, dave.csrf),
        )
        .header("x-csrf-token", &dave.csrf)
        .body("{}")
        .send()
        .await
        .expect("send");
    assert_eq!(remote.status(), 403);
    relay
        .s2s
        .state
        .peers
        .pin(&host.origin, Some(&host.id()))
        .await
        .expect("pin");
    let answer = relay
        .s2s
        .client
        .send(rim_services::s2s::dto::S2sRequest {
            peer_server_id: host.id(),
            method: rim_services::s2s::dto::S2sMethod::Post,
            path: "/deliver".into(),
            body: br#"{"items":[]}"#.to_vec(),
            content_type: None,
            headers: vec![],
        })
        .await
        .expect("a signed answer");
    assert_eq!(answer.status, 503);
    assert_eq!(answer.error_code().as_deref(), Some("role_disabled"));
    // A relay serves its users' identities to any pinned peer.
    host.s2s
        .state
        .peers
        .pin(&relay.origin, Some(&relay.id()))
        .await
        .expect("pin");
    let identity = host
        .s2s
        .client
        .send(rim_services::s2s::dto::S2sRequest {
            peer_server_id: relay.id(),
            method: rim_services::s2s::dto::S2sMethod::Get,
            path: format!("/principals/{}", carol.principal()),
            body: vec![],
            content_type: None,
            headers: vec![],
        })
        .await
        .expect("a signed answer");
    assert_eq!(identity.status, 200);
    let document: Value = serde_json::from_slice(&identity.body).expect("json");
    assert_eq!(document["principalId"], json!(carol.principal()));
    assert_eq!(document["username"], json!("carol"));

    host.stop().await;
    relay.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_remote_user_signing_up_here_becomes_local_and_leaving_a_relay_clears_it() {
    let (a, b) = relays().await;
    let alice = sign_up(&a, "alice", 1).await;
    let bob = sign_up(&b, "bob", 2).await;
    alice.start_dm(&bob, &b, true).await;
    bob.pending_request(&alice).await;
    let remote = user_id(&a, &bob.principal()).await;
    let row = |node: &Node, id: UserId| {
        let db = node.db.clone();
        async move {
            let row = db
                .query_one_raw(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    r#"SELECT kind, username, (SELECT count(*) FROM server_member m
                         JOIN server s ON s.id = m.server_id AND s.kind = 'direct'
                         WHERE m.user_id = u.id) AS direct
                       FROM "user" u WHERE id = $1"#,
                    [Uuid::from(id).into()],
                ))
                .await
                .expect("query")
                .expect("row");
            (
                row.try_get::<String>("", "kind").expect("kind"),
                row.try_get::<String>("", "username").expect("username"),
                row.try_get::<i64>("", "direct").expect("direct"),
            )
        }
    };
    let (kind, username, direct) = row(&a, remote).await;
    assert_eq!(kind, "remote");
    assert!(username.starts_with("r_"));
    assert_eq!(direct, 1);
    // Bob's handle comes from his relay: his username there and its handle domain, the host
    // of its origin. Alice's is her username at her own relay's domain.
    let home =
        a.db.query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"SELECT home_username, home_handle_domain FROM "user" WHERE id = $1"#,
            [Uuid::from(remote).into()],
        ))
        .await
        .expect("query")
        .expect("row");
    assert_eq!(
        (
            home.try_get::<String>("", "home_username")
                .expect("username"),
            home.try_get::<String>("", "home_handle_domain")
                .expect("domain"),
        ),
        ("bob".to_string(), "127.0.0.1".to_string())
    );
    assert_eq!(
        alice.ok("query { me { handle } }", json!({})).await["me"]["handle"],
        json!("@alice:127.0.0.1")
    );

    // Bob signs up on Alice's relay with the same key and device: the row becomes his.
    sign_up_with_card(&a, "bobby", 2, 2).await;
    assert_eq!(user_id(&a, &bob.principal()).await, remote);
    assert_eq!(
        row(&a, remote).await,
        ("local".to_string(), "bobby".to_string(), 1)
    );

    // A successor names the conversation it replaces.
    let dave = sign_up(&b, "dave", 5).await;
    let direct = alice.direct_space().await;
    let replaced = Uuid::now_v7();
    let successor = alice
        .ok(
            CREATE,
            json!({ "input": {
                "serverId": direct,
                "remoteMembers": [{ "principalId": dave.principal(), "relayUrls": [b.origin] }],
                "encrypted": true,
                "predecessor": { "hostServerId": b.id(), "conversationId": replaced },
            } }),
        )
        .await["createConversation"]["id"]
        .clone();
    let predecessor = alice
        .ok(
            "query($s: ID!, $c: ID!) { conversation(serverId: $s, conversationId: $c) { conversationType predecessor { hostServerId conversationId } } }",
            json!({ "s": direct, "c": successor }),
        )
        .await["conversation"]
        .clone();
    assert_eq!(
        predecessor["conversationType"], "DIRECT",
        "one remote member is a DM"
    );
    assert_eq!(
        predecessor["predecessor"],
        json!({ "hostServerId": b.id(), "conversationId": replaced.to_string() })
    );

    // Leaving his own relay deletes his requests and contacts there.
    bob.ok(
        "mutation($p: String!, $r: [String!]!) { addContact(principalId: $p, relayUrls: $r) { principalId } }",
        json!({ "p": alice.principal(), "r": [a.origin] }),
    )
    .await;
    let left = bob.ok("mutation { leaveRelay }", json!({})).await;
    assert_eq!(left["leaveRelay"], json!(true));
    assert_eq!(
        bob.ok("query { contacts { principalId } }", json!({}))
            .await["contacts"],
        json!([])
    );
    assert_eq!(bob.ok(REQUESTS, json!({})).await["dmRequests"], json!([]));

    a.stop().await;
    b.stop().await;
}

/// The conversations `person` sees in `direct`'s list.
async fn listed(person: &Person, direct: &str) -> Vec<String> {
    person
        .ok(
            "query($s: ID!) { inboxConversations(serverId: $s, dir: AFTER, limit: 50) { items { id } } }",
            json!({ "s": direct }),
        )
        .await["inboxConversations"]["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|c| c["id"].as_str().expect("id").to_string())
        .collect()
}

/// The members `person` sees in `direct`'s directory, with their roles.
async fn directory(person: &Person, direct: &str) -> Value {
    person
        .ok(
            "query($s: ID!) { serverMembers(serverId: $s) { principalId memberRole } }",
            json!({ "s": direct }),
        )
        .await["serverMembers"]
        .clone()
}

/// A directory listing only `person`, as a guest.
fn guest(person: &Person) -> Value {
    json!([{ "principalId": person.principal(), "memberRole": "GUEST" }])
}

/// What `person` sees of `conversation` in `direct`: its unread count, its notifications, and
/// the notification bell's count.
async fn signals(person: &Person, direct: &str, conversation: &str) -> (i64, usize, i64) {
    let data = person
        .ok(
            "query($s: ID!) { unreadCounts(workspaceId: $s) { conversationId count } \
             myNotifications(serverId: $s, dir: AFTER, limit: 50) { items { conversationId } } \
             myUnreadNotificationCount(serverId: $s) }",
            json!({ "s": direct }),
        )
        .await;
    let unread = data["unreadCounts"]
        .as_array()
        .expect("counts")
        .iter()
        .find(|c| c["conversationId"] == json!(conversation))
        .map_or(0, |c| c["count"].as_i64().expect("count"));
    let notifications = data["myNotifications"]["items"]
        .as_array()
        .expect("notifications")
        .iter()
        .filter(|n| n["conversationId"] == json!(conversation))
        .count();
    let bell = data["myUnreadNotificationCount"].as_i64().expect("bell");
    (unread, notifications, bell)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_strangers_dm_with_a_member_of_the_home_lands_in_its_request_inbox() {
    let (a, b) = relays().await;
    let alice = sign_up(&a, "alice", 1).await;
    let mut bob = sign_up(&b, "bob", 2).await;
    let carol = sign_up(&a, "carol", 3).await;
    let mut dave = sign_up(&b, "dave", 5).await;
    // Carol's DMs make Bob and Dave members of A's direct space.
    carol.start_dm(&bob, &b, true).await;
    carol.start_dm(&dave, &b, true).await;
    alice
        .ok(
            "mutation($p: String!, $r: [String!]!) { addContact(principalId: $p, relayUrls: $r) { principalId } }",
            json!({ "p": dave.principal(), "r": [b.origin] }),
        )
        .await;
    // relayInfo answers without a session.
    let direct = anonymous_direct_space(&a).await;

    // A signed-in relayInfo is a first use of the direct space: Alice is a guest there before
    // any DM, sees only herself, sees no owner, and lists an empty inbox.
    assert_eq!(alice.direct_space().await, direct);
    assert_eq!(directory(&alice, &direct).await, guest(&alice));
    let space = alice
        .ok(
            "query($s: ID!) { server(serverId: $s) { owner { username } } }",
            json!({ "s": direct }),
        )
        .await;
    assert_eq!(space["server"]["owner"], Value::Null);
    assert_eq!(listed(&alice, &direct).await, Vec::<String>::new());

    // Without relayInfo, listing the inbox is the first use.
    let erin = sign_up(&a, "erin", 6).await;
    assert_eq!(listed(&erin, &direct).await, Vec::<String>::new());
    assert_eq!(directory(&erin, &direct).await, guest(&erin));

    // A conversation with someone on another relay is encrypted only.
    let code = alice
        .err(
            CREATE,
            json!({ "input": {
                "serverId": direct,
                "remoteMembers": [{ "principalId": bob.principal(), "relayUrls": [b.origin] }],
            } }),
        )
        .await;
    assert_eq!(code, "VALIDATION");
    let message = alice
        .post(
            CREATE,
            json!({ "input": {
                "serverId": direct,
                "encrypted": false,
                "remoteMembers": [{ "principalId": bob.principal(), "relayUrls": [b.origin] }],
            } }),
        )
        .await["errors"][0]["message"]
        .clone();
    assert_eq!(
        message,
        "conversations with people on other servers are encrypted"
    );

    let create_with_alice = json!({ "input": {
        "serverId": direct,
        "encrypted": true,
        "remoteMembers": [{ "principalId": alice.principal(), "relayUrls": [a.origin] }],
    } });

    // Bob, a stranger to Alice, starts a DM with her at her own relay: it is a request.
    let from_bob =
        bob.remote(&a, CREATE, create_with_alice.clone()).await["createConversation"]["id"]
            .as_str()
            .expect("id")
            .to_string();
    let request = alice.pending_request(&bob).await;
    assert_eq!(request["hostServerId"], json!(a.id()));
    assert_eq!(request["conversationId"], json!(from_bob));
    let invite = verify_item(&a, &request["heldItems"][0]);
    assert_eq!(invite.event["type"], "DmInvite");
    assert_eq!(invite.stream, format!("u:{}", alice.principal()));
    assert!(
        !listed(&alice, &direct).await.contains(&from_bob),
        "a request is not a live conversation"
    );

    // Bob writes before Alice decides: her notification is stored, but her unread count, her
    // notifications and the bell leave the conversation out until she accepts.
    let claimed = bob
        .remote(
            &a,
            CLAIM,
            json!({ "input": {
                "serverId": direct,
                "conversationId": from_bob,
                "devices": [{ "principalId": alice.principal(), "deviceId": alice.device_id() }],
            } }),
        )
        .await["claimMlsKeyPackages"]
        .clone();
    let alice_principal = alice.principal();
    let (commit, welcome) = bob.mls.create_group(
        from_bob.parse().expect("uuid"),
        &[(
            alice_principal.as_str(),
            alice.device_id(),
            claimed[0]["keyPackage"].as_str().expect("package"),
        )],
    );
    bob.remote(
        &a,
        SUBMIT,
        json!({ "input": {
            "serverId": direct, "conversationId": from_bob, "incarnation": 1, "epoch": 0,
            "cipherSuite": 0x004f, "commit": commit, "welcome": welcome,
        } }),
    )
    .await;
    bob.mls.merge_pending();
    let message_id = Uuid::now_v7();
    let envelope = bob.mls.encrypt(
        message_id,
        &content(message_id, 0, "hello", &[]),
        [7; 32],
        &[],
    );
    bob.remote(
        &a,
        SEND_ENCRYPTED,
        json!({ "input": { "serverId": direct, "conversationId": from_bob, "messageId": message_id, "envelope": envelope } }),
    )
    .await;
    let alice_id = user_id(&a, &alice.principal()).await;
    eventually("Alice's notification row", || async {
        let row = a
            .db
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT count(*) AS n FROM user_notification WHERE user_id = $1 AND message_id = $2",
                [Uuid::from(alice_id).into(), message_id.into()],
            ))
            .await
            .expect("query")
            .expect("row");
        (row.try_get::<i64>("", "n").expect("n") == 1).then_some(())
    })
    .await;
    assert_eq!(
        signals(&alice, &direct, &from_bob).await,
        (0, 0, 0),
        "a pending request adds no unread count and no notification"
    );

    // Dave, her contact, starts one too: it is live at once.
    let from_dave = dave.remote(&a, CREATE, create_with_alice).await["createConversation"]["id"]
        .as_str()
        .expect("id")
        .to_string();
    tokio::time::sleep(Duration::from_millis(500)).await;
    let pending = alice.ok(REQUESTS, json!({ "status": "PENDING" })).await;
    assert!(
        pending["dmRequests"]
            .as_array()
            .expect("requests")
            .iter()
            .all(|r| r["fromPrincipalId"] != json!(dave.principal())),
        "a contact files no request"
    );
    assert!(listed(&alice, &direct).await.contains(&from_dave));

    // Accepting Bob's request makes his conversation live.
    alice
        .ok(
            "mutation($id: ID!) { acceptDmRequest(id: $id) { status } }",
            json!({ "id": request["id"] }),
        )
        .await;
    assert!(listed(&alice, &direct).await.contains(&from_bob));
    assert_eq!(
        signals(&alice, &direct, &from_bob).await,
        (1, 1, 1),
        "an accepted conversation counts"
    );

    a.stop().await;
    b.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_sdl_names_every_relay_type_and_field() {
    let (first, _) = databases_with(RESET).await;
    let node = start(&first, open_registration).await;
    let sdl = reqwest::get(format!("{}/api/sdl", node.origin))
        .await
        .expect("sdl")
        .text()
        .await
        .expect("text");
    for needle in [
        "type RelayInfo",
        "type RemoteServer",
        "type RemotePrincipalDevice",
        "type RemoteIdentity",
        "type Contact",
        "enum DmRequestStatus",
        "type RelayItem ",
        "type RelayItemPage",
        "type DmRequest ",
        "type StreamHead",
        "input RemoteMemberInput",
        "input PredecessorInput",
        "type ConversationPredecessor",
        "predecessor: ConversationPredecessor",
        "remoteMembers: [RemoteMemberInput!]",
        "predecessor: PredecessorInput",
        "relayInfo: RelayInfo!",
        "remoteServer(origin: String!): RemoteServer!",
        "remoteIdentity(principalId: String!, relayUrl: String): RemoteIdentity!",
        "contacts: [Contact!]!",
        "dmRequests(status: DmRequestStatus): [DmRequest!]!",
        "relayItems(after: ID, limit: Int): RelayItemPage!",
        "streamHeads(serverId: ID!, streams: [String!]!): [StreamHead!]!",
        "addContact(principalId: String!, relayUrls: [String!]!): Contact!",
        "removeContact(principalId: String!): Boolean!",
        "acceptDmRequest(id: ID!): DmRequest!",
        "ignoreDmRequest(id: ID!): DmRequest!",
        "blockPrincipal(principalId: String!): Boolean!",
        "unblockPrincipal(principalId: String!): Boolean!",
        "reportDmRequest(id: ID!, reason: String!): Boolean!",
        "ackRelayItems(upTo: ID!): Boolean!",
        "leaveRelay: Boolean!",
        "relayStream(after: ID): String!",
        "\tproofOfWork: String\n",
        "\tOPEN\n\t\"\"\"\n\t`RIM_REGISTRATION_POW_BITS`",
        "\tPOW\n}",
    ] {
        assert!(sdl.contains(needle), "the SDL lacks {needle:?}");
    }
    node.stop().await;
}

const LOGIN: &str = r#"
mutation Login($proof: SignedStatementInput!) { loginWithKey(proof: $proof) { userId } }"#;

/// The first error code of a GraphQL answer.
fn error_code(body: &Value) -> &str {
    body["errors"][0]["extensions"]["code"]
        .as_str()
        .unwrap_or_else(|| panic!("expected an error: {body}"))
}

/// The configuration the operator's commands run with on a node's database.
fn operator_config(database_url: &str) -> RIMConfig {
    RIMConfig {
        database_url: database_url.to_string(),
        ..Default::default()
    }
}

async fn operator(database_url: &str, group: &str, args: &[&str]) -> String {
    rim_server::abuse_cmd::run_with(
        &operator_config(database_url),
        group,
        args.iter().map(|a| a.to_string()),
    )
    .await
    .unwrap_or_else(|e| panic!("rim_server {group} {args:?}: {e}"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pow_signups_are_accepted_refused_or_take_an_invite() {
    const BITS: u32 = 8;
    let (first, _) = databases_with(RESET).await;
    let node = start(&first, |c| c.registration_pow_bits = Some(BITS)).await;
    let http = reqwest::Client::new();

    // The descriptor and `registrationMode` say how to sign up.
    let discovery: Value = http
        .get(format!("{}/.well-known/rim-server", node.origin))
        .send()
        .await
        .expect("discovery")
        .json()
        .await
        .expect("json");
    let descriptor: Value = serde_json::from_str(
        discovery["descriptor"]["payload"]
            .as_str()
            .expect("payload"),
    )
    .expect("descriptor");
    assert_eq!(
        descriptor["registration"],
        json!({ "mode": "pow", "powBits": BITS })
    );
    let mode: Value = http
        .post(format!("{}/api", node.origin))
        .json(&json!({ "query": "query { registrationMode }" }))
        .send()
        .await
        .expect("mode")
        .json()
        .await
        .expect("json");
    assert_eq!(mode["data"]["registrationMode"], "POW");

    let ada = keys(11);
    let inception = ada.identity.inception().payload;
    let nonce = rim_s2s::pow::solve_pow(&node.id(), &inception, BITS);
    // A weak nonce, one for another server, none at all, and both a token and a nonce: refused.
    let weak = (0u64..)
        .map(u64::to_be_bytes)
        .find(|n| {
            rim_s2s::pow::check_pow(&node.id(), &inception, &URL_SAFE_NO_PAD.encode(n), BITS)
                .is_err()
        })
        .expect("a weak nonce");
    let elsewhere = rim_s2s::pow::solve_pow(
        "rims1_T5LLFxkrobgyvlf3WVxuvLlfqYSZAssRFgmvCfI2R-c",
        &inception,
        BITS,
    );
    for extra in [
        json!({ "proofOfWork": URL_SAFE_NO_PAD.encode(weak) }),
        json!({}),
        json!({ "proofOfWork": URL_SAFE_NO_PAD.encode(nonce), "instanceInviteToken": "rim_inv" }),
    ] {
        let (body, _) = key_signup(&http, &node, "ada", &ada, extra.clone()).await;
        assert_eq!(error_code(&body), "VALIDATION", "{extra}: {body}");
    }
    if rim_s2s::pow::check_pow(
        &node.id(),
        &inception,
        &URL_SAFE_NO_PAD.encode(elsewhere),
        BITS,
    )
    .is_err()
    {
        let (body, _) = key_signup(
            &http,
            &node,
            "ada",
            &ada,
            json!({ "proofOfWork": URL_SAFE_NO_PAD.encode(elsewhere) }),
        )
        .await;
        assert_eq!(error_code(&body), "VALIDATION", "{body}");
    }
    let (body, _) = key_signup(
        &http,
        &node,
        "ada",
        &ada,
        json!({ "proofOfWork": URL_SAFE_NO_PAD.encode(nonce) }),
    )
    .await;
    assert!(body.get("errors").is_none(), "{body}");
    let ada_id: UserId = body["data"]["signupWithKey"]["userId"]
        .as_str()
        .expect("user id")
        .parse::<Uuid>()
        .expect("uuid")
        .into();

    // An instance invite stands in for the work.
    use rim_core::auth::repository::InstanceInviteRepository;
    rim_infra_db::sea_orm_impl::instance_invite::SeaOrmInstanceInviteRepository::new(
        node.db.clone().into(),
    )
    .create(rim_core::auth::model::NewInstanceInvite {
        created_by_user_id: Some(ada_id),
        token_hash: rim_auth::session_token::hash_token("rim_inv_grace"),
        expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
        max_uses: 1,
        email: None,
    })
    .await
    .expect("instance invite");
    let (body, _) = key_signup(
        &http,
        &node,
        "grace",
        &keys(12),
        json!({ "instanceInviteToken": "rim_inv_grace" }),
    )
    .await;
    assert!(body.get("errors").is_none(), "{body}");
    node.stop().await;
}

/// The operator's `invite` command on `database_url`.
async fn invite(database_url: &str, args: &[&str]) -> String {
    rim_server::invite_cmd::run_with(
        &operator_config(database_url),
        args.iter().map(|a| a.to_string()),
    )
    .await
    .unwrap_or_else(|e| panic!("rim_server invite {args:?}: {e}"))
}

/// The token of the signup link `rim_server invite create` printed first.
fn invite_token(created: &str) -> String {
    let link = created.lines().next().expect("a link");
    link.rsplit_once("/join/")
        .expect("a join link")
        .1
        .to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_pure_relays_operator_hands_out_signup_invites() {
    let (first, _) = databases_with(RESET).await;
    // Invite-only registration on a server that runs only the relay role: no one owns a
    // workspace, so no one can issue an invite over GraphQL.
    let node = start(&first, |c| c.role = Some("relay".to_string())).await;
    let http = reqwest::Client::new();

    let (body, _) = key_signup(&http, &node, "ada", &keys(21), json!({})).await;
    assert!(body.get("errors").is_some(), "invite-only: {body}");

    let created = invite(
        &first,
        &["create", "--max-uses", "1", "--public-url", &node.origin],
    )
    .await;
    assert!(
        created.starts_with(&format!("{}/join/rim_iinv_", node.origin)),
        "{created}"
    );
    assert!(created.contains("issued by operator"), "{created}");
    let token = invite_token(&created);
    let (body, _) = key_signup(
        &http,
        &node,
        "ada",
        &keys(21),
        json!({ "instanceInviteToken": token }),
    )
    .await;
    assert!(body.get("errors").is_none(), "{body}");

    // The invite is used up.
    let (body, _) = key_signup(
        &http,
        &node,
        "grace",
        &keys(22),
        json!({ "instanceInviteToken": token }),
    )
    .await;
    assert!(body.get("errors").is_some(), "a used invite: {body}");
    let listed = invite(&first, &["list"]).await;
    assert!(listed.contains("used 1/1"), "{listed}");

    // A revoked invite admits no one.
    let created = invite(&first, &["create", "--max-uses", "5"]).await;
    let id = created
        .lines()
        .nth(1)
        .and_then(|line| line.split_whitespace().next())
        .expect("the invite id")
        .to_string();
    assert!(invite(&first, &["revoke", &id]).await.contains(&id));
    let (body, _) = key_signup(
        &http,
        &node,
        "grace",
        &keys(22),
        json!({ "instanceInviteToken": invite_token(&created) }),
    )
    .await;
    assert!(body.get("errors").is_some(), "a revoked invite: {body}");
    assert!(!invite(&first, &["list"]).await.contains(&id));

    node.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reported_request_is_listed_and_a_blocked_principal_is_signed_out_and_refused() {
    let (first, second) = databases_with(RESET).await;
    let a = start(&first, open_registration).await;
    let b = start(&second, open_registration).await;
    let alice = sign_up(&a, "alice", 1).await;
    let bob = sign_up(&b, "bob", 2).await;

    // Bob reports Alice's request; the operator of his relay lists and resolves it.
    alice.start_dm(&bob, &b, true).await;
    let request = bob.pending_request(&alice).await;
    let reported = bob
        .ok(
            "mutation($id: ID!, $r: String!) { reportDmRequest(id: $id, reason: $r) }",
            json!({ "id": request["id"], "r": "unsolicited ads" }),
        )
        .await;
    assert_eq!(reported["reportDmRequest"], json!(true));
    let listed = operator(&second, "abuse-reports", &["list", "--open"]).await;
    let request_id = request["id"].as_str().expect("id");
    for needle in [
        alice.principal(),
        a.id(),
        request_id.to_string(),
        "unsolicited ads".to_string(),
        "open".to_string(),
    ] {
        assert!(listed.contains(&needle), "{needle} in {listed}");
    }
    let report_id = listed.split_whitespace().next().expect("report id");
    let resolved = operator(&second, "abuse-reports", &["resolve", report_id]).await;
    assert!(resolved.contains("resolved"), "{resolved}");
    assert_eq!(
        operator(&second, "abuse-reports", &["list", "--open"]).await,
        ""
    );

    // The operator blocks Bob: his session ends and he cannot sign in again.
    let blocked = operator(
        &second,
        "principal",
        &["block", &bob.principal(), "--reason", "spam wave"],
    )
    .await;
    assert!(blocked.contains("ended 1 sessions"), "{blocked}");
    let body = bob
        .post("query { contacts { principalId } }", json!({}))
        .await;
    assert_eq!(error_code(&body), "UNAUTHENTICATED", "{body}");
    let http = reqwest::Client::new();
    let proof = login_proof(&http, &b, &keys(2)).await;
    let body: Value = http
        .post(format!("{}/api", b.origin))
        .json(&json!({ "query": LOGIN, "variables": { "proof": statement(&proof) } }))
        .send()
        .await
        .expect("login")
        .json()
        .await
        .expect("json");
    assert_eq!(error_code(&body), "FORBIDDEN", "{body}");
    assert!(
        operator(&second, "principal", &["list"])
            .await
            .contains(&bob.principal())
    );

    // A blocked principal cannot sign up.
    operator(
        &second,
        "principal",
        &[
            "block",
            &keys(3).state.principal_id,
            "--reason",
            "known spammer",
        ],
    )
    .await;
    let (body, _) = key_signup(&http, &b, "carol", &keys(3), json!({})).await;
    assert_eq!(error_code(&body), "FORBIDDEN", "{body}");

    // Nothing more is delivered to Bob: a new conversation from Alice files no request.
    let direct = alice.direct_space().await;
    let group = alice
        .ok(
            CREATE,
            json!({ "input": {
                "serverId": direct,
                "memberIds": [],
                "remoteMembers": [{ "principalId": bob.principal(), "relayUrls": [b.origin] }],
                "title": "after the block",
                "encrypted": true,
            } }),
        )
        .await["createConversation"]["id"]
        .as_str()
        .expect("group id")
        .to_string();
    drained(&a).await;
    let filed =
        b.db.query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT count(*) AS n FROM relay_dm_request WHERE conversation_id = $1",
            [group.parse::<Uuid>().expect("uuid").into()],
        ))
        .await
        .expect("query")
        .expect("row")
        .try_get::<i64>("", "n")
        .expect("n");
    assert_eq!(filed, 0, "a blocked recipient gets nothing");

    // Unblocked, Bob signs in again.
    let unblocked = operator(&second, "principal", &["unblock", &bob.principal()]).await;
    assert!(unblocked.contains("unblocked"), "{unblocked}");
    let proof = login_proof(&http, &b, &keys(2)).await;
    let body: Value = http
        .post(format!("{}/api", b.origin))
        .json(&json!({ "query": LOGIN, "variables": { "proof": statement(&proof) } }))
        .send()
        .await
        .expect("login")
        .json()
        .await
        .expect("json");
    assert!(body.get("errors").is_none(), "{body}");
    a.stop().await;
    b.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_blocked_peer_gets_403_both_ways_until_unblocked() {
    use rim_services::s2s::dto::{PeerBlockChange, S2sClientError, S2sMethod, S2sRequest};

    let (a, b) = relays().await;
    let ping = |to: &Node| S2sRequest {
        peer_server_id: to.id(),
        method: S2sMethod::Get,
        path: "/ping".into(),
        body: vec![],
        content_type: None,
        headers: vec![],
    };
    // B pins A and pings it, so A pins B.
    b.s2s
        .state
        .peers
        .pin(&a.origin, Some(&a.id()))
        .await
        .expect("pin");
    let answer = b
        .s2s
        .client
        .send(ping(&a))
        .await
        .expect("a verified answer");
    assert_eq!(answer.status, 200);

    let peers = &a.s2s.state.peers;
    assert_eq!(
        peers.set_blocked(&b.id(), true).await.expect("block"),
        PeerBlockChange::Changed
    );
    assert_eq!(
        peers.set_blocked(&b.id(), true).await.expect("again"),
        PeerBlockChange::Unchanged
    );
    let refused = b.s2s.client.send(ping(&a)).await.expect("a signed refusal");
    assert_eq!(refused.status, 403);
    let body: Value = serde_json::from_slice(&refused.body).expect("json");
    assert_eq!(body["error"], "peer_not_allowed");
    // A no longer talks to B either.
    assert!(matches!(
        a.s2s.client.send(ping(&b)).await,
        Err(S2sClientError::PeerBlocked)
    ));

    assert_eq!(
        peers.set_blocked(&b.id(), false).await.expect("unblock"),
        PeerBlockChange::Changed
    );
    let answer = b
        .s2s
        .client
        .send(ping(&a))
        .await
        .expect("a verified answer");
    assert_eq!(answer.status, 200);
    assert_eq!(
        peers
            .set_blocked("rims1_OFPDNXTV1splCcKcgz1vWYj6Mg-lbUollHQwXTkYEBw", true)
            .await
            .expect("unknown"),
        PeerBlockChange::NotPinned
    );
    a.stop().await;
    b.stop().await;
}

/// Signs `event` as `host`'s own event on Bob's stream at `seq`, delivers it to `relay` as item
/// `id`, and returns the refusal reasons.
async fn deliver_as(host: &Node, relay: &Node, bob: &Person, id: u64, event: Value) -> Vec<String> {
    use rim_s2s::host::{HostEventFields, sign_host_event};
    use rim_services::s2s::dto::{S2sMethod, S2sRequest};

    let identity = host.identity();
    let workspace_id = anonymous_direct_space(host).await;
    let stream = format!("u:{}", bob.principal());
    let issued_at = rim_s2s::timestamp(chrono::Utc::now());
    let event = event.to_string();
    let envelope = sign_host_event(
        &identity.signer,
        &HostEventFields {
            host: identity.server_id(),
            key_seq: identity.state.seq,
            workspace_id: &workspace_id,
            stream: &stream,
            seq: Some(id),
            event_json: &event,
            issued_at: &issued_at,
        },
    );
    let body = json!({ "items": [{
        "id": id.to_string(),
        "envelope": statement(&envelope),
        "recipients": [bob.principal()],
        "deviceId": null,
    }] });
    let response = host
        .s2s
        .client
        .send(S2sRequest {
            peer_server_id: relay.id(),
            method: S2sMethod::Post,
            path: "/deliver".to_string(),
            body: serde_json::to_vec(&body).expect("json"),
            content_type: None,
            headers: Vec::new(),
        })
        .await
        .expect("delivered");
    assert_eq!(response.status, 200);
    let answer: Value = serde_json::from_slice(&response.body).expect("json");
    answer["refused"]
        .as_array()
        .expect("refused")
        .iter()
        .map(|r| r["reason"].as_str().expect("reason").to_string())
        .collect()
}

/// A host that names someone whose card does not list it, or forges their card, is refused
/// `invalid_invite`, so it borrows no contact's standing; a sender nobody can check is a
/// stranger's request.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_host_cannot_file_an_invite_in_the_name_of_someone_it_does_not_speak_for() {
    let (a, b) = relays().await;
    let bob = sign_up(&b, "bob", 2).await;
    let erin = sign_up(&b, "erin", 3).await;
    a.s2s
        .state
        .peers
        .pin(&b.origin, Some(&b.id()))
        .await
        .expect("pin");
    b.s2s
        .state
        .peers
        .pin(&a.origin, Some(&a.id()))
        .await
        .expect("pin");
    bob.ok(
        "mutation($p: String!, $r: [String!]!) { addContact(principalId: $p, relayUrls: $r) { principalId } }",
        json!({ "p": erin.principal(), "r": [b.origin] }),
    )
    .await;
    let invite = |from: &str, card: Value| {
        json!({
            "type": "DmInvite",
            "workspace_id": Uuid::now_v7(),
            "conversation_id": Uuid::now_v7(),
            "from_principal_id": from,
            "from_card": card,
            "member_principal_ids": [from, bob.principal()],
        })
    };

    // A names Erin, Bob's contact on B, with her own card, which lists B only.
    let erins_card = erin
        .identity
        .profile_card(&erin.state, 2, "Erin", &[b.origin.as_str()], &[]);
    assert_eq!(
        deliver_as(
            &a,
            &b,
            &bob,
            1,
            invite(&erin.principal(), statement(&erins_card))
        )
        .await,
        vec!["invalid_invite"],
        "the card does not list the host"
    );

    // A card that names A but is not signed by Erin's key.
    let mallory = support::relay::keys(9);
    let forged =
        mallory
            .identity
            .profile_card(&mallory.state, 3, "Erin", &[a.origin.as_str()], &[]);
    let forged = json!({ "payload": erins_card.payload, "signature": forged.signature });
    assert_eq!(
        deliver_as(&a, &b, &bob, 2, invite(&erin.principal(), forged)).await,
        vec!["invalid_invite"],
        "a card Erin did not sign"
    );

    // A principal A does not serve, named as Bob's contact: nobody can check the card, so it
    // is a stranger's request, not a live conversation.
    let minted =
        mallory
            .identity
            .profile_card(&mallory.state, 1, "Mallory", &[a.origin.as_str()], &[]);
    bob.ok(
        "mutation($p: String!, $r: [String!]!) { addContact(principalId: $p, relayUrls: $r) { principalId } }",
        json!({ "p": mallory.identity.principal_id(), "r": [a.origin] }),
    )
    .await;
    assert_eq!(
        deliver_as(
            &a,
            &b,
            &bob,
            3,
            invite(&mallory.identity.principal_id(), statement(&minted))
        )
        .await,
        Vec::<String>::new(),
        "a principal the host does not serve"
    );

    // Only that request was filed, pending, and nothing reached Bob's queue.
    let requests = bob.ok(REQUESTS, json!({ "status": null })).await["dmRequests"].clone();
    let requests = requests.as_array().expect("requests");
    assert_eq!(requests.len(), 1, "{requests:?}");
    assert_eq!(
        requests[0]["fromPrincipalId"],
        json!(mallory.identity.principal_id())
    );
    assert_eq!(requests[0]["status"], "PENDING");
    let items = bob
        .ok(support::relay::ITEMS, json!({ "after": null }))
        .await;
    assert_eq!(items["relayItems"]["items"], json!([]));

    a.stop().await;
    b.stop().await;
}

/// Whether `node` holds an accepted row for `conversation`, and the host's id for its user.
async fn accepted_row(node: &Node, conversation: &str) -> Option<Option<Uuid>> {
    node.db
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT host_user_id FROM relay_dm_request \
             WHERE conversation_id = $1 AND status = 'accepted'",
            [conversation.parse::<Uuid>().expect("uuid").into()],
        ))
        .await
        .expect("query")
        .map(|row| row.try_get::<Option<Uuid>>("", "host_user_id").expect("id"))
}

/// How many of `host`'s `ReadStateUpdated` items for `conversation` wait in `person`'s queue.
async fn read_states(person: &Person, host: &Node, conversation: &str) -> usize {
    person
        .items(host, None)
        .await
        .into_iter()
        .filter(|i| {
            i.event["type"] == "ReadStateUpdated"
                && i.event["conversation_id"] == json!(conversation)
        })
        .count()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_members_accepted_row_is_filed_again_after_it_expires_and_goes_when_they_leave() {
    const LEAVE: &str =
        "mutation($s: ID!, $c: ID!) { leaveConversation(serverId: $s, conversationId: $c) }";
    let (a, b) = relays().await;
    let alice = sign_up(&a, "alice", 1).await;
    let mut bob = sign_up(&b, "bob", 2).await;
    let carol = sign_up(&a, "carol", 3).await;
    let direct = alice.direct_space().await;
    let group = alice
        .ok(
            CREATE,
            json!({ "input": {
                "serverId": direct,
                "memberIds": [],
                "remoteMembers": [
                    { "principalId": bob.principal(), "relayUrls": [b.origin] },
                    { "principalId": carol.principal(), "relayUrls": [a.origin] },
                ],
                "title": "group",
                "encrypted": true,
            } }),
        )
        .await["createConversation"]["id"]
        .as_str()
        .expect("group id")
        .to_string();
    let request = bob.pending_request(&alice).await;
    assert_eq!(request["conversationId"], json!(group));
    bob.ok(
        "mutation($id: ID!) { acceptDmRequest(id: $id) { status } }",
        json!({ "id": request["id"] }),
    )
    .await;
    assert_eq!(
        accepted_row(&b, &group).await,
        Some(None),
        "accepted, before the host named Bob"
    );

    // An event on Bob's own stream gives the row the id the host gives him.
    bob.remote(
        &a,
        MARK_READ,
        json!({ "input": { "workspaceId": direct, "conversationId": group, "readAt": chrono::Utc::now().to_rfc3339() } }),
    )
    .await;
    let bob_at_a = user_id(&a, &bob.principal()).await;
    eventually("the row to learn Bob's id at the host", || async {
        (accepted_row(&b, &group).await == Some(Some(bob_at_a.into()))).then_some(())
    })
    .await;

    // After a long silence the row expires. The next item is refused, the home sends the
    // invite again, Bob's relay files it accepted (Alice became his contact) and later items
    // flow.
    b.db.execute_unprepared(&format!(
        "UPDATE relay_dm_request SET expires_at = now() - interval '1 day' \
         WHERE conversation_id = '{group}'"
    ))
    .await
    .expect("expire");
    b.s2s.relay.cleanup().await.expect("cleanup");
    assert_eq!(accepted_row(&b, &group).await, None);
    let before = read_states(&bob, &a, &group).await;
    let mark_read = json!({ "input": { "workspaceId": direct, "conversationId": group, "readAt": chrono::Utc::now().to_rfc3339() } });
    bob.remote(&a, MARK_READ, mark_read.clone()).await;
    eventually("the invite to be filed again", || async {
        accepted_row(&b, &group).await.map(|_| ())
    })
    .await;
    assert_eq!(
        read_states(&bob, &a, &group).await,
        before,
        "the refused item is not queued"
    );
    bob.remote(&a, MARK_READ, mark_read).await;
    eventually("a later item to reach Bob", || async {
        (read_states(&bob, &a, &group).await == before + 1).then_some(())
    })
    .await;
    eventually("the row to learn Bob's id again", || async {
        (accepted_row(&b, &group).await == Some(Some(bob_at_a.into()))).then_some(())
    })
    .await;

    // Leaving the group at its host deletes the row; Bob still gets the event that removes him.
    bob.remote(&a, LEAVE, json!({ "s": direct, "c": group }))
        .await;
    let removed = bob
        .item_of(&a, "MemberRemovedFromConversation", |i| {
            i.event["conversation_id"] == json!(group)
        })
        .await;
    assert_eq!(removed.event["user_id"], json!(bob_at_a.to_string()));
    eventually("the accepted row to go", || async {
        accepted_row(&b, &group).await.is_none().then_some(())
    })
    .await;

    a.stop().await;
    b.stop().await;
}
