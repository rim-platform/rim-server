# End-to-end encryption with MLS v1 (Stage 4, S4-0)

_Spec written 2026-10-09 for Stage 4 (`docs/plans/stage-4-2026-10-09.md`, `ROADMAP.md` Track 7.3).
Library, providers, cipher suite and test vectors: `docs/decisions/mls-library-2026-10-09.md`.
Read `key-identity-v1.md` (keys, tagged encodings, statements) and `device-linking-v1.md` first.
This document is the contract between the server (S4-1), the `rim_mls` crate (S4-2), the desktop
client (S4-3, S4-4, S4-5) and the web client (S4-6)._

## Summary

1. **DMs and groups** are encrypted; channels, private ones included, stay plaintext. New DMs and
   groups are encrypted when every member is ready; existing ones are upgraded once, by a member
   device, behind a visible "encrypted since" marker. Old plaintext stays.
2. One MLS group per conversation, suite **0x004F** (X25519 + ML-KEM-768, AES-128-GCM, Ed25519).
   Members are **devices**. The server owns conversation membership; the group follows it.
3. Credentials: a per-device **MLS signature key** certified by the device key (an `mls-key`
   statement), chained to the key log. Clients verify every leaf.
4. Each device keeps 20 one-time **key packages** and one last-resort package on the server,
   30-day lifetime. The server validates them on upload and hands them out once.
5. The server is the **sequencer**: one Commit per `(conversation, incarnation, epoch)`, enforced by
   a row lock and a unique index; the loser gets `STALE_EPOCH`. Application messages are accepted
   only in the current epoch, so the change feed is in epoch order.
6. Every MLS message is a PrivateMessage. Its cleartext **authenticated data** carries what the
   server may read: mentions, the franking commitment, and the membership summary of a Commit.
   The server checks the summary against the members and the key packages it issued.
7. An encrypted message is a `message` row with `text` null and an MLS envelope. Edits replace the
   envelope (same row, new version); deletes are server tombstones; reactions, pins, threads and
   forward origins are server metadata.
8. A client decrypts each message **once**, in one transaction with storing its plaintext. A new
   device joins through a Welcome and reads older history from the drive archive.
9. Attachments: AES-256-GCM under a per-file secret, uploaded as opaque blobs checked by SHA-256.
10. **Franking**: an HMAC commitment in the authenticated data, a server receipt over it, and a
    report that reveals one message's content and franking key to the moderators.

## Scope

| Conversation | Encrypted in v1 | Why |
|---|---|---|
| `Direct` | Yes | Two members, no shared history for later joiners, nothing server-side depends on its text |
| `Group` | Yes | Member-only by construction (`is_private` is always false for groups; the server has no other reader) |
| `Channel`, private | No | Channels are where server search, pipelines, alerts, incidents, bots and the agent work, and where later joiners read full history; MLS gives a new member nothing from before it joined. Encrypted private channels need history sharing and are deferred |
| `Channel`, public | No | Readable by every workspace member by design |

- **Ready member.** A user whose `principal_id` is set and who has at least one *MLS-active* device:
  not revoked, with a key package uploaded in the last 60 days (`device_key.last_key_package_upload_at`,
  set by every upload; key package rows themselves expire after 30 days). Bots, ingest tokens, the
  CLI and password-only accounts are never ready.
- **An encrypted conversation** has `conversation.encrypted_at` set. It is set when an encrypted
  conversation is created (`createConversation(input: { encrypted: true })`) or when the first
  Commit of an existing plaintext DM or group is accepted ("Migration"). It is never cleared.
- In an encrypted conversation the server refuses plaintext writes (`sendMessage`,
  `replyToMessage`, `editMessage` on an encrypted row, `forwardMessage` into it: `VALIDATION`,
  `this conversation is encrypted`), and `createConversation(encrypted: true)` and
  `addConversationMembers` refuse members who are not ready (`VALIDATION`,
  `every member of an encrypted conversation needs a device set up for encryption`).
- **Group devices**: the devices that are leaves of the current MLS group, as the server records
  them from accepted Commits (`mls_group_device`). **Desired devices**: the MLS-active devices of
  the conversation's current members. **Pending adds** are desired devices not in the group;
  **pending removes** are group devices no longer desired (member left, removed, kicked or banned;
  device revoked; inactive for 60 days).
- At most **100 members and 512 devices** per encrypted conversation (`VALIDATION` beyond).

## Identity and credentials

### The `mls-key` statement

A statement in the `key-identity-v1.md` sense (exact signed text, version 2 tagged keys, unknown
and duplicate fields refused), signed by the **device key** over
`"rim.mls-key.v1\0" || payload`, payload at most 16 KiB:

```json
{"v":2,"type":"mls-key","principalId":"rimp1_…","deviceId":"<uuid>","mlsKey":"ed25519:…","issuedAt":"…"}
```

- `mlsKey` is the device's MLS signature key: Ed25519 (the suite's signature scheme), generated by
  `rim_mls`, kept with the group state (never in the archive). One per device; a device that loses
  it generates a new key and statement and is re-added ("Lost state").
- The MLS leaf's credential is a **basic credential** whose `identity` is the UTF-8 JSON
  `{"payload":"<payload text>","signature":"<tagged signature>"}` (appendix A.2); the leaf's
  `signature_key` must equal the raw bytes of `mlsKey`.
- `rim_identity` gains the statement type and its verifier; servers and clients use the same code.

### Verifying a leaf

A client accepts a leaf (in a Welcome's tree, an Add, or a re-added leaf) only when all hold:

1. `identity` parses as above and the payload is a valid `mls-key` statement.
2. `keyLog(principalId)` replays (`key-identity-v1.md`), and `principalDevices` lists `deviceId` with
   a certificate that verifies against that log at its `keySeq` and has no revocation.
3. The `mls-key` signature verifies under that certificate's `deviceKey`, and its `principalId` and
   `deviceId` equal the certificate's.
4. `mlsKey` equals the leaf's `signature_key`.
5. The principal is a current member of the conversation (from the server's member list, mapped by
   `User.principalId`), and no other leaf of the group has the same `(principalId, deviceId)`.

Clients cache verified `(principalId, deviceId, mlsKey)` triples and recheck the device list when a
`MlsMembershipChanged` event arrives. The server checks rules 1, 3 and 4 at key package upload,
against the uploading session's own device.

### A leaf that does not verify

The client still processes the Commit or Welcome (refusing it would fork the group), then:
refetches the key log and device list once; if the leaf still fails, it marks the conversation
**"unverified device present"** naming the principal and device id, **stops sending** in it, and
commits a Remove of that leaf at once (no ranking delay). If the server refuses the Remove (it
believes the device is desired), the warning stays until the leaf goes away; the person can still
read, and the UI says that a device RIM cannot verify can read new messages.

### Device list changes

| Event | Effect | Who commits |
|---|---|---|
| A device is linked (`device-linking-v1.md`) and uploads key packages | Pending add | Ranked group devices, or the member's own other device first (rank 0) |
| `revokeDeviceKey` | Pending remove | Ranked group devices; any sender first |
| Identity rotation (`signer: identity`) | None: devices stay enrolled | — |
| Recovery rotation (all devices revoked, a new one enrolled) | Pending removes, then a pending add | Ranked group devices |
| Member added / removed / kicked / banned / leaves | Pending adds / removes for all their devices | The device that made the change, else ranked |
| Device inactive 60 days | Pending remove; re-added when it uploads key packages again | Ranked |
| Re-add request ("Lost state") | Remove + Add of that device in one Commit | Ranked |
| Self-update (post-compromise security) | An empty Commit with an update path, at most every 7 days per device | The device itself, at a random time within a day of becoming due |

**Ranking.** Pending changes come from `Conversation.mlsGroup`. Every group device not in pending
removes sorts the group devices' ids (lowercase UUID text, ascending) and takes its position as its
rank. It waits `min(rank, 8) × 15 s` plus 0 to 3 s of random jitter, refetches the group, and if a
change it can make is still pending and its local epoch equals the server's, it claims key packages
and commits **all** pending changes at once. A device that is about to send an application message
commits pending changes first, without waiting (the server refuses its message otherwise, see
"Sequencing"). Racing devices are harmless: one Commit wins, the others get `STALE_EPOCH`.

## Key packages

- **Per device**: 20 unclaimed one-time packages and one last-resort package (OpenMLS
  `mark_as_last_resort`), each with the `lifetime` extension `not_before = now − 1 h`,
  `not_after = now + 30 days`. Capabilities: suite 0x004F, basic credentials.
- **Rotation**: on start and every 12 hours while running, a device tops up to 20 when fewer than
  10 remain, uploads a new last-resort package when the current one expires within 7 days or was
  claimed more than 5 times, and deletes private keys of its packages that expired or were
  replaced more than 7 days ago. Uploading is also the device's MLS-activity signal.
- **Server validation at upload**: the bytes decode as an `MLSMessage` carrying a `KeyPackage`; the
  suite is in the server's accepted list (`[0x004F]`); OpenMLS validation passes (signature, protocol
  version); the credential is a basic credential whose `mls-key` statement verifies under **the
  uploading session's device key** and names that session's principal and device; the leaf
  signature key equals `mlsKey`; `not_after` is in the future and at most 90 days away; the
  `KeyPackageRef` (OpenMLS `hash_ref`, 32 bytes for this suite) is new. A new last-resort package
  replaces the device's previous one in the same transaction.
- **Claiming**: `claimMlsKeyPackages` takes `(principalId, deviceId)` pairs for one conversation
  and returns one package per device, in one transaction: the oldest unclaimed one-time package
  (`FOR UPDATE SKIP LOCKED`), else the last-resort package (not consumed). A device with neither
  (or, for a re-add, with no one-time package) is left out of the answer. The caller must be a
  session device that is a group device, or, while the conversation has no group, the session of
  a conversation member. Every target must be a pending add or a re-add target of that
  conversation, in the group's suite; when the caller may start a new incarnation ("Incarnations")
  every MLS-active device of a member but the caller's is a target, in the preferred suite. The
  claim records `(claimed_at, claimed_by_device_key_id, claimed_for_conversation_id)`; a Commit may
  use a package claimed for its conversation by its committer for one hour. A last-resort row
  keeps only its latest claim, so a Commit uses it only while that claim is the committer's.
- **Last resort** packages are never used for re-adds (see "Lost state").
- An accepted Commit deletes the one-time packages it added. Claimed one-time rows are deleted 7
  days after the claim; expired rows hourly.

## Groups

### Group id and configuration

The MLS group id is 24 bytes: ASCII `rim1`, the conversation UUID's 16 bytes, the incarnation as
a big-endian u32 (appendix A.1). Clients refuse a Welcome, Commit or message whose group id does not
name the conversation and incarnation they expect.

| OpenMLS setting | Value |
|---|---|
| Cipher suite | `MLS_128_MLKEM768X25519_AES128GCM_SHA256_Ed25519` (0x004F) |
| Wire format policy | `PURE_CIPHERTEXT_WIRE_FORMAT_POLICY`: Commits are PrivateMessages too |
| Ratchet tree | `use_ratchet_tree_extension(true)`: the Welcome carries the tree |
| Padding | `padding_size(256)` |
| Sender ratchet | out-of-order tolerance 32, maximum forward distance 10,000 |
| Past epochs | `max_past_epochs(2)` |
| Proposals | None stand-alone: every change is a Commit with its proposals inline |
| Authenticated data | Set before every message (`set_aad`), see "Authenticated data". S4-2's first tests confirm that OpenMLS frames it into Commits as well as application messages |

### Creating a group

1. The creator's client calls `createConversation(input: { …, encrypted: true })`. The server
   checks that every member is ready, creates the conversation with `encrypted_at = created_at`,
   and no group. A DM pair that already has a DM gets that conversation back, as today; a plaintext
   one is then upgraded as in "Migration".
2. It calls `claimMlsKeyPackages` for every desired device but its own, creates the group at epoch
   0 with incarnation 1, adds them, and sends the Commit with the Welcome through
   `submitMlsCommit(incarnation: 1, epoch: 0, cipherSuite: 79)`.
3. The server creates `mls_group` (epoch 1 after the Commit), records the group devices (the
   committer and the adds), stores the Welcome for each added device, and emits
   `MlsWelcomeAvailable` to their users.
4. If no group exists 60 seconds after creation (the creator crashed), any member device may do
   step 2; the first accepted Commit wins.

The server refuses an incarnation's first Commit unless every member has at least one device among
the resulting group devices (`VALIDATION`, `every member needs at least one device in the group`),
and unless the committer is MLS-active itself. A conversation's first group (incarnation 1) is
created by the device of the member "Migration" names, or, for a conversation created encrypted,
by the creator's device during the first 60 seconds and any member's after (`FORBIDDEN`
otherwise).

### Welcome delivery and joining

- A Welcome is stored once with one recipient row per added device, until the device acknowledges
  it (`ackMlsWelcomes`) or for 30 days. `myMlsWelcomes` returns the session device's unacknowledged
  Welcomes, oldest first.
- The device joins (`StagedWelcome`), checks the group id, verifies every leaf ("Verifying a leaf"),
  fetches and processes the Commits created in its join epoch and later, then acknowledges. A Welcome
  it cannot open (the key package's private key is gone) leads to a re-add request.
- **A new device's history.** The feed returns the conversation's older rows too; their epochs are
  before the device's join epoch (or their incarnation is older), so they cannot be decrypted
  here. The client shows them as one collapsed line, "Earlier messages were sent before this device
  joined", with "Restore from history archive" when an archive is set up (`history-archive-v1.md`);
  restored rows replace the placeholders. Nothing is ever shown as a decryption failure.

### Leaving, removal and the group's end

- The server changes membership exactly as today (`leaveConversation`, `removeConversationMember`,
  kick, ban). The member's devices become pending removes; another device commits them.
- A device that processes its own removal deletes its group state and keeps its plaintext cache
  until the change feed answers `NO_ACCESS`, which drops it as for any conversation.
- Deleting a conversation cascades to its MLS rows. DMs and groups cannot be archived.

### Lost state and re-add requests

A device whose group state is missing or corrupt, that cannot process a Commit, or that is behind
past the server's Commit retention, signs a **re-add request** with its device key over
`"rim.mls-readd.v1\0" || payload` and sends it with `requestMlsReadd`:

```json
{"v":2,"type":"mls-readd","principalId":"rimp1_…","deviceId":"<uuid>","conversationId":"<uuid>","incarnation":1,"lastEpoch":41,"nonce":"<16 random bytes>","issuedAt":"…"}
```

`lastEpoch` is the last epoch it processed, or `null` with no state. Before asking, it uploads
fresh key packages and discards its old group state. The server accepts a request only from a
device that is still a group device of the incarnation it names, which must be the current one
(an older one is `STALE_EPOCH`), with `lastEpoch` at most the group's epoch. Protections against a server forcing re-adds:

| Rule | Stops |
|---|---|
| The request is signed by the device key; committers verify it against the device's certificate before acting | A server fabricating requests |
| A re-add uses a **one-time** key package claimed now, never a last-resort package | Forcing reuse of an old last-resort init key that may have leaked |
| A committer honors at most one request per device and conversation per 24 hours, and the server accepts at most 3 per day | Churn and key package draining |
| The re-added leaf is verified like any other | Substituting another key |

While waiting, the conversation shows "This device can't read new messages until another device
re-adds it". A re-add does not restore earlier messages; the cache and the archive do.

### Incarnations

An incarnation is a fresh MLS group for the same conversation (group id with the next incarnation
number, epoch 0). `submitMlsCommit` with `incarnation = current + 1` and `epoch = 0` is accepted
from a group device of the current incarnation only when one holds:

- **Suite migration**: `cipherSuite` is the server's preferred suite and differs from the current
  incarnation's, and every claimed package offers it;
- **Recovery**: the committer has a re-add request on the current incarnation older than 10
  minutes and no Commit was accepted on it since that request;
- **No devices left**: no group device of the current incarnation is MLS-active for a current
  member. Then the committer need not be a group device: any MLS-active device of a member may
  start the next incarnation.

The old `mls_group` row gets `retired_at`; every message, Commit and Welcome of it is refused from
then on (`STALE_EPOCH` naming the new incarnation). Clients show "Encryption was reset by
<name>'s <device>" at that point in the timeline.

## Sequencing

The server is the MLS delivery service and sequencer. It decodes the outer `MLSMessage`
(protocol version, wire format, and the PrivateMessage header fields `group_id`, `epoch`,
`content_type`, `authenticated_data`) and never decrypts.

### What the server checks

| Check | Application message | Commit | Refusal |
|---|---|---|---|
| Session bound to a device | ✓ | ✓ | `FORBIDDEN` (`this session is not bound to a device`) |
| Session device is a group device of the current incarnation and not a pending remove | ✓ | ✓ (except an incarnation's first Commit, see "Incarnations") | `FORBIDDEN` |
| Wire format PrivateMessage; `content_type` application resp. commit | ✓ | ✓ | `VALIDATION` |
| `group_id` = this conversation's current incarnation | ✓ | ✓ | `STALE_EPOCH` if older, else `VALIDATION` |
| `epoch` = current epoch (and ≥ 1 for application messages) | ✓ | ✓ | `STALE_EPOCH` |
| No pending removes | ✓ | — | `MLS_COMMIT_REQUIRED` (`reason: "pendingRemovals"`) |
| Authenticated data parses and passes its rules | ✓ | ✓ | `VALIDATION` |
| Size caps ("Limits") | ✓ | ✓ | `VALIDATION` |

### One Commit per epoch

In one transaction:

```sql
SELECT epoch FROM mls_group
 WHERE conversation_id = $1 AND incarnation = $2 AND retired_at IS NULL
 FOR UPDATE;
-- epoch must equal the Commit's epoch, else STALE_EPOCH (current epoch and incarnation in the error)
INSERT INTO mls_commit (id, conversation_id, incarnation, epoch, …) VALUES (…);  -- uq_mls_commit_epoch
UPDATE mls_group SET epoch = epoch + 1 WHERE conversation_id = $1 AND incarnation = $2;
-- apply the summary to mls_group_device, consume the Add key packages, store the Welcome
```

The row lock serializes Commits; the unique index `(conversation_id, incarnation, epoch)` is the
backstop, and a unique violation is also answered `STALE_EPOCH`. Application messages take the
same row `FOR SHARE` and check the epoch inside their insert transaction. So every application
message of epoch *e* commits before the Commit that ends *e*, and a client that has seen a Commit
already sees every message of the epoch it ended.

### Ordering

- Application messages are ordered by the message `version` (the change feed's per-conversation
  counter, `message-change-feed.md`), which is epoch-monotone by the locking above. The feed is the
  only path through which a client decrypts.
- Commits are ordered by epoch: `mlsCommits(conversationId, incarnation, afterEpoch)`.
- **Client loop**, per conversation: fetch new Commits; drain the feed; for each entry, if its epoch
  is above the local epoch, apply Commits up to it first; decrypt; when the feed is drained, apply
  the remaining Commits. Commits from realtime events only trigger this loop.
- **Own Commits.** The client keeps its Commit pending until the server answers. Accepted: merge.
  `STALE_EPOCH`: clear the pending Commit, run the loop, recompute what is still needed, retry.
  No answer: fetch the Commit at that epoch; if it is the client's own (`OwnPendingCommit`), merge,
  else clear and process the other.
- **Own application messages** are not decryptable by their sender (`OwnPrivateMessage`): the
  sending device stores the plaintext under the message id it chose before sending. On
  `STALE_EPOCH` it runs the loop and sends the same content again in the new epoch with the same
  message id.
- **A Commit the client cannot process** (OpenMLS refuses it): the device stops at that epoch,
  sends a re-add request, and shows the waiting notice. A Commit that processes but whose
  authenticated-data summary does not match its proposals is merged, and the client warns that
  the committing device misbehaved and runs "A leaf that does not verify" for every leaf.

### Replay and gaps

- MLS deletes each message key after use, so a replayed ciphertext fails to decrypt; the client
  logs it and ignores it. Application data names the message id, which must equal the row id, so a
  ciphertext cannot be moved to another row.
- Per-sender MLS generations skip legitimately (edits replace rows, deletes remove ciphertext), so
  a generation gap is not an error. Withheld Commits are detected (epochs are consecutive and a
  later application message names a higher epoch); withheld application messages can only be
  detected by the feed's cursor contract, not proven. A relay (Stage 5) that withholds them is
  the residual risk `relay-proxied-workspaces-2026-10-09.md` accepts.

## The message envelope

### Rows and tables

| MLS message | Stored as |
|---|---|
| Application message | A `message` row: `text` null, `encrypted` true, `mls_ciphertext` the `MLSMessage` bytes, `mls_incarnation`, `mls_epoch`, `sender_device_key_id`, `franking_commitment`, `franking_receipt`, `franking_key_id` |
| Commit | An `mls_commit` row |
| Welcome | An `mls_welcome` row plus one `mls_welcome_recipient` row per added device |
| Key package | An `mls_key_package` row |
| Proposal | Not used |

There is no content-type column on `message`: only application messages are rows, so the
timeline, the change feed and every existing message query keep their meaning. Handshake messages
are not timeline items and have their own order (epochs).

**Message ids are chosen by the client** (UUID v7) for encrypted messages, so the sender can store
its plaintext under the id before the server answers. A repeated `sendEncryptedMessage` with the
same id and the same envelope bytes answers the existing message; with other bytes `CONFLICT`.

### Application data

The MLS application message's plaintext is binary: `0x01` (format version), the 32-byte
**franking key**, then the **content**: UTF-8 JSON with the statement rules (unknown and duplicate
fields refused, every field present, at most 64 KiB):

```json
{"v":1,"type":"message","id":"<uuid>","rev":0,"threadRootId":null,"body":"hi <@rimp1_…>",
 "mentions":["rimp1_…"],"mentionAll":false,"replyTo":null,"forwardedFrom":null,
 "attachments":[],"linkPreviews":[]}
```

| Field | Rule |
|---|---|
| `id` | The message id; must equal the row id and the authenticated data's `messageId` |
| `rev` | 0 when sent, +1 per edit; a client never replaces a revision it holds with a lower one |
| `threadRootId` | The thread root's message id for a thread reply, else `null`; must match the row's thread |
| `body` | 0 to 10,000 characters; mentions are `<@principalId>` tokens |
| `mentions` | The distinct principal ids of the body's `<@…>` tokens, in order of first appearance, at most 100 |
| `mentionAll` | `true` when the body notifies everyone (`@here`, `@channel`) |
| `replyTo` | `{"messageId":"<uuid>"}` for a timeline reply, else `null`. Encrypted only: the server's `parent_message_id` stays null |
| `forwardedFrom` | `{"messageId","conversationId","authorPrincipalId","sentAt"}` or `null`: the forwarder's claim about the origin, shown as "Forwarded by <sender>" |
| `attachments` | At most 10 entries, see "Attachments" |
| `linkPreviews` | At most 3 `{"url","title","description","siteName"}` (strings or `null`, each at most 2,000 characters), fetched by the sender's client; receivers never fetch |

A body, mention list, thread or id that breaks a rule makes the message **invalid**: the client
shows "A message from <sender> could not be verified" and never its content. So does a content
whose franking commitment does not match.

### Authenticated data

The PrivateMessage's `authenticated_data`, readable by the server and signed by the sender's MLS
key inside the encryption, is UTF-8 JSON with the same strict rules:

```json
{"v":1,"type":"application","messageId":"<uuid>","franking":"<base64url, 32 bytes>","mentions":["rimp1_…"],"mentionAll":false}
{"v":1,"type":"commit","adds":[{"principalId":"rimp1_…","deviceId":"<uuid>","keyPackageRef":"<base64url>"}],"removes":[{"principalId":"rimp1_…","deviceId":"<uuid>"}]}
```

- **Application**: at most 8 KiB. The server checks `messageId` against the request (a UUID v7),
  `franking` is 32 bytes, every mention is a principal id, distinct, at most 100. The receiver checks after decrypting that `messageId`,
  `mentions` and `mentionAll` equal the content's and that `franking` equals the recomputed
  commitment.
- **Commit**: at most 256 KiB, no device added or removed twice. The server checks: every add is a
  pending add or a re-add target, uses a key package claimed for this conversation by this device
  in the last hour, named exactly by `keyPackageRef`, of the group's suite, and belonging to that
  device; every remove is a group device that is a pending remove or is re-added in the same
  Commit with a re-add request outstanding; the committer is not removed. A Commit with adds
  carries a Welcome of the group's suite whose `new_member` refs are exactly the adds'; one
  without adds carries none. The receiver checks that the staged Commit's Add proposals have exactly these
  `KeyPackageRef`s and its Remove proposals remove exactly these devices.

### Edits, deletes and metadata

- **Edit**: `editEncryptedMessage` carries a new application message in the current epoch whose
  content has the same `id`, a higher `rev`, and the complete new message (not a diff: a device
  that never decrypted the original can render the edit). The server replaces `mls_ciphertext`,
  `mls_epoch`, `sender_device_key_id`, the franking columns and the mention rows; the change-feed
  trigger bumps `version` and sets `edited_at`. Only the author (any of their devices) may edit; the
  receiver checks that the sender's principal is the row author's.
- **Delete**: `deleteMessage` as today. The server sets `is_deleted`, nulls `mls_ciphertext`, and
  keeps the franking columns. No application message is sent: the server is the authority on
  deletion (it can drop a row anyway), and a moderator outside a group must be able to delete.
- **Server metadata, visible to the server**: reactions (emoji, who, when), pins (which message,
  who), thread membership (`thread_id`, the thread rows), the forward origin
  (`forwarded_from_message_id`), read states, `has_attachments`, attachment sizes, sender device,
  send and edit times, mentions (below). Reactions and pins stay metadata so the server-side
  counters, events and `pinnedMessages` keep working; what they leak is the emoji and that someone
  reacted to or pinned a given message, not the message's content.

### Clients: decrypt once, store, render

- A message decrypts **once** per device (MLS deletes each key after use). `rim_mls` keeps OpenMLS
  state in the cache database, so decrypting an entry, storing its plaintext row and advancing the
  ratchet commit in one transaction. The timeline of an encrypted conversation reads from the
  cache only.
- Cache rows carry the plaintext as `text`, the rest as today, plus the envelope's metadata. The
  upsert rule stays "higher version wins", except that an entry this device cannot decrypt (before
  its join, older incarnation) is stored only as a placeholder where no plaintext row exists, and
  never replaces one. `RESET` on an encrypted conversation keeps plaintext rows (they cannot be
  refetched) and re-reads the feed over them.
- `edited_at` and `is_deleted` come from the feed as for plaintext; a tombstone deletes the plaintext.
- The archive keeps decrypted `text` and adds `payload.e2ee` (`incarnation`, `epoch`,
  `senderDeviceId`, `frankingKey`, `receipt`, `receiptKeyId`, `sentAtUs`), an additive field in
  archive version 1 that S4-3 adds to `history-archive-v1.md`. `sig` stays `null`. Group state and
  MLS keys are never archived.

### The change feed

`messageChanges` is unchanged; encrypted rows come back with `text: null` and the new
`Message.encryption` field. Encrypted entries are epoch-monotone in `version` order. An edit appears
as one `EDITED` entry carrying the newest envelope.

## Mentions and notifications

- The server stores mention rows from the authenticated data: each principal id is mapped to the
  member with that `user.principal_id`; ids that are not current members are ignored; the author is
  skipped. `mentionAll` is treated as `@channel` / `@here`.
- Notification rules run as today with what the server knows: `direct`, `mention`,
  `thread_reply` and `all` work; `keyword` cannot (the desktop client matches keywords after
  decrypting and shows a local notification, without an inbox entry). Edits replace mention rows
  and do not notify again.
- Digests and any email say "Encrypted message from <name>"; `NotificationDue` is unchanged.
- **What it leaks**: who was mentioned in which message. Accepted, because the server already knows
  every member, every message's sender and time, and a DM's notifications are per message anyway;
  without it encrypted conversations would never notify mentions in groups.
- The server uses the mention list **only** for mention rows and notification routing. A sender can
  lie in it, but receivers refuse a message whose list differs from its content, so a lie costs the
  sender their message.

## Attachments

- **Cipher**: AES-256-GCM. Per file a random 32-byte `secret`;
  `key = HKDF-SHA256(ikm = secret, no salt, info = "rim.e2ee.attachment.v1 key", L = 32)`,
  `nonce = HKDF-SHA256(ikm = secret, no salt, info = "rim.e2ee.attachment.v1 nonce", L = 12)`,
  AAD `"rim.e2ee.attachment.v1"`, one encryption per secret. The blob is the ciphertext with the
  16-byte tag appended. AES-GCM rather than XChaCha20-Poly1305 because WebCrypto has it (the web
  client decrypts up to 25 MB natively and fast) and the desktop already links `aes-gcm`; with a
  fresh key per file the nonce cannot repeat.
- **Content entry**: `{"blobSha256","secret","name","mime","size"}`: SHA-256 of the blob,
  the secret, the file name (1 to 255 characters), the media type as the sender saw it, and the
  plaintext size. The commitment covers the entry, and the blob hash pins the blob, so one blob
  decrypts to one file for every member and for a moderator (AES-GCM is not key-committing; the
  key and the blob are both fixed by the committed content).
- **Upload** through the existing flow, after the message is sent: `createAttachment(input:
  { messageId, filename: "encrypted", contentType: "application/octet-stream", sizeBytes,
  blobSha256 })`, the upload to `/api/files/upload`, `finalizeAttachment`. In an encrypted
  conversation `blobSha256` is required, the file name and type must be exactly those, and the
  upload handler accepts `application/octet-stream` only there and refuses bytes whose SHA-256 or
  size differ. In a plaintext conversation `blobSha256` is refused. The server stores the size and
  the hash, sniffs nothing, and generates nothing from the bytes.
- **Download**: `Attachment.blobSha256` matches a content entry; `/api/attachments/{id}/download`
  serves `application/octet-stream` named `encrypted.bin`; the client checks the hash, decrypts,
  checks the size, and only then hands the file to the UI with the content entry's name and type.
- **Limits**: blob at most `MAX_ATTACHMENT_BYTES` (25 MiB), so plaintext at most 25 MiB − 16 bytes.
  No thumbnails in v1: images up to 5 MB are downloaded and shown automatically.

## Franking

A report proves one message's content to the moderators without the server having read it.

1. **Sender**: picks a random 32-byte franking key `fk` per message and edit, and computes the
   commitment `c = HMAC-SHA256(fk, "rim.e2ee.franking.v1\0" || content)` over the exact content
   bytes. `fk` travels inside the application data; `c` in the authenticated data.
2. **Server on receipt**: stores `c` and a receipt
   `r = HMAC-SHA256(K, "rim.e2ee.franking-receipt.v1\0" || message id || conversation id ||
   sender user id || sender device id || sent_at_us || c)` (ids as 16 raw UUID bytes, the device id
   being the client device id, `sent_at_us` the row's `sent_at` in microseconds as a big-endian
   i64), where `K` is the server's franking key `franking_key_id` (32 random bytes in
   `franking_key`, created on first use). `r` and `sentAtUs` are delivered in `Message.encryption`.
3. **Receiver**: recomputes `c` and refuses the message as invalid on a mismatch, so every message a
   member can see is reportable. `fk` must be exactly 32 bytes (HMAC's key padding would otherwise
   let two keys collide). It keeps `fk`, `r` and `sentAtUs` with the plaintext.
4. **Report** (`reportEncryptedMessage`): the reporter sends the content bytes, `fk`, the receipt,
   its key id, the sender ids and `sentAtUs`. The server recomputes `c` from the content and `fk`,
   recomputes `r` with its key, and compares in constant time. It needs no message row, so a message
   the sender deleted stays reportable. A valid report stores the content in `message_report`
   (`franked_content`, `franking_verified_at`); an invalid one is refused (`VALIDATION`, `the report
   does not match the message`). The usual report rules hold (one per reporter and message).
5. **Moderator view** (S4-5): the body as plain text with mentions resolved, the attachment names and
   sizes (a moderator's client downloads a reported attachment by its `blobSha256` through a
   report-scoped route and decrypts it with the secret from the verified content), sender,
   device, conversation and time, and the line "Verified: this is the exact message <sender>'s
   device sent. Other messages in the conversation are not shown and are not verified." A verified
   report lets a moderator delete that message even outside the conversation.

**Limits.** A report proves one message, not its context: what it replied to, what came before, or
whether the reporter provoked it (a reporter may report several messages, each proven alone). The
revealed content is stored on the server from then on. Franking adds no deniability loss: MLS
messages are already signed by the sender's MLS key, so any member holding a plaintext and its
signature could convince a third party. Message deniability is deferred.

## Server features in encrypted conversations

| Feature | Behavior |
|---|---|
| Full-text search | Encrypted rows have no text and no search vector; `searchMessages` never returns them. Local search covers them ("on this device") |
| Link previews | No server fetch (`spawn_link_preview_fetch` skipped). The sender's client fetches and embeds them; receivers never fetch |
| Pipelines | Triggers still fire on message events with metadata only; `MessageContains` never matches an encrypted message |
| The agent | No server-side context or summaries; `chat.search_messages` never returns encrypted rows; `chat.post_message` into an encrypted conversation is refused; `chat.create_group` creates a plaintext group that the creator's desktop then upgrades ("Migration"). The client agent reads through the message service (below), off by default per conversation, with a warning that the text goes to the model provider |
| Notifications | From the authenticated mentions and "new message"; digests say "Encrypted message"; keyword notifications are client-local |
| Slow mode | Channels only, so not applicable |
| Moderation | Kick, ban, timeout, membership and roles work; timed-out members cannot send envelopes or commit; reports go through franking (`reportMessage` refuses an encrypted message, `reportEncryptedMessage` a plaintext one, and the receipt's sender must be the row's author); a moderator delete removes the row's ciphertext |
| Pins, reactions, custom emoji | Server metadata, unchanged |
| Threads | `thread_id` and thread rows are metadata; replies are encrypted messages with `threadRootMessageId`; `thread_reply` notifications work |
| Forwarding | `forwardMessage` refuses an encrypted source or target (`VALIDATION`). The client forwards by sending new content: `sendEncryptedMessage(forwardedFromMessageId)` into an encrypted conversation, or, after a confirmation that the text will be posted unencrypted, `sendMessage(forwardedFromMessageId)` into a plaintext one. The server checks the caller can read the source and records the origin |
| Read states, unread counts, typing, presence | Unchanged |
| Incidents, on-call, alerts | Not available: declaring an incident in an encrypted conversation is refused; alerts only post to channels |
| Bots, webhooks, ingest | Cannot be members, so cannot post |
| Export and admin tools | Encrypted rows are exported as metadata without content; audit rows hold ids only, as today |

## Plugins and the client message service

S4-4 implements the client message service; this is its contract.

- **Capabilities.** `chat.read` and `chat.write` cover plaintext conversations.
  `chat.read.encrypted` and `chat.write.encrypted` cover encrypted ones and require the plain
  capability as well. Each is its own line in the consent prompt; the encrypted ones read
  "Read your encrypted DMs and groups: this plugin sees their decrypted text and can send it
  anywhere its other permissions allow." Web and desktop plugins get the same names.
- **What the service returns**: `{ id, conversationId, threadId, author: { principalId,
  displayName }, text, sentAt, editedAt, encrypted, attachments: [{ name, mime, size }] }`, read from
  the local cache. Never franking keys, attachment secrets, MLS state, ciphertext or keys of any
  kind. Plugins cannot fetch attachment bytes in v1.
- **Writes** go through the same send path as the person's own messages and are encrypted by the
  client.
- **Audit**: the host records, per plugin, when each grant was given and revoked and a daily count of
  encrypted messages read, shown in the plugin's settings. Revoking stops the plugin, as for every
  capability.

## Migration of existing conversations

- **Who upgrades**: in a DM either member's device; in a group a device of the creator
  (`created_by`), or of any member when the creator is null or no longer a member. The device must be
  a desktop device (it needs `rim_mls`; the web client after S4-6).
- **When**: the eligible device upgrades automatically when it opens the conversation and every
  member is ready. While someone is not, the conversation shows "Not encrypted yet: <name> has not
  set up encryption on a desktop device".
- **How**: as "Creating a group" step 2 on the existing conversation. The server sets
  `encrypted_at` in the transaction that accepts incarnation 1's first Commit, and emits
  `ConversationUpdated`.
- **The marker** is `Conversation.encryptedAt`: clients render "Messages from here on are end-to-end
  encrypted" at that point, and a quiet "sent before encryption" note on older messages. No
  message row is created.
- **Old plaintext stays** on the server, readable by members as before (members added later can
  still read it). People can delete their own old messages.

## The web client (S4-6)

- The same `rim_mls` crate in wasm with the libcrux provider's portable code. OpenMLS's storage
  trait is synchronous and IndexedDB is not, so the crate keeps state in memory and writes the
  entries an operation changed to IndexedDB before the operation's result is used (no ciphertext is
  sent, and no plaintext shown, before the write lands).
- **Plaintext store**: the web app has no message cache today; decrypt-once requires one, so S4-6
  adds an IndexedDB store of decrypted messages for encrypted conversations.
- **Keys**: the MLS signature key and HPKE private keys are raw bytes in IndexedDB, because OpenMLS
  signs and decapsulates synchronously; the device key stays a non-extractable WebCrypto key and
  signs only `mls-key` statements and re-add requests. There is no keyring.
- **Key packages** are refreshed only while a tab is open. A browser unused for 60 days leaves the
  groups and is re-added when it comes back; what was sent meanwhile is not readable there.
- The weaker-storage notice of `web-linked-device.md` is extended: "Encrypted conversations in this
  browser are only as safe as this browser profile."

## Storage

One migration, additive (generated with `sea migrate generate add_e2ee_mls`):

```sql
ALTER TABLE conversation ADD COLUMN encrypted_at timestamptz NULL;

CREATE TABLE franking_key (
    id integer PRIMARY KEY, secret bytea NOT NULL CHECK (octet_length(secret) = 32),
    created_at timestamptz NOT NULL DEFAULT now());

CREATE TABLE mls_key_package (
    id uuid PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES "user" (id) ON DELETE CASCADE,
    device_key_id uuid NOT NULL REFERENCES device_key (id) ON DELETE CASCADE,
    cipher_suite integer NOT NULL, key_package_ref bytea NOT NULL, key_package bytea NOT NULL,
    last_resort boolean NOT NULL DEFAULT false, not_after timestamptz NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(), claimed_at timestamptz NULL,
    claimed_by_device_key_id uuid NULL REFERENCES device_key (id) ON DELETE SET NULL,
    claimed_for_conversation_id uuid NULL REFERENCES conversation (id) ON DELETE SET NULL,
    CONSTRAINT uq_mls_key_package_ref UNIQUE (key_package_ref));
CREATE INDEX idx_mls_key_package_available ON mls_key_package (device_key_id, cipher_suite, created_at)
    WHERE claimed_at IS NULL AND NOT last_resort;
CREATE UNIQUE INDEX uq_mls_key_package_last_resort ON mls_key_package (device_key_id, cipher_suite)
    WHERE last_resort;
CREATE INDEX idx_mls_key_package_not_after ON mls_key_package (not_after);

CREATE TABLE mls_group (
    conversation_id uuid NOT NULL REFERENCES conversation (id) ON DELETE CASCADE,
    incarnation integer NOT NULL CHECK (incarnation >= 1),
    group_id bytea NOT NULL, cipher_suite integer NOT NULL,
    epoch bigint NOT NULL,                      -- the epoch the next Commit must be created in
    created_by_device_key_id uuid NULL REFERENCES device_key (id) ON DELETE SET NULL,
    created_at timestamptz NOT NULL DEFAULT now(), retired_at timestamptz NULL,
    PRIMARY KEY (conversation_id, incarnation), CONSTRAINT uq_mls_group_group_id UNIQUE (group_id));
CREATE UNIQUE INDEX uq_mls_group_current ON mls_group (conversation_id) WHERE retired_at IS NULL;

CREATE TABLE mls_group_device (
    conversation_id uuid NOT NULL, incarnation integer NOT NULL,
    device_key_id uuid NOT NULL REFERENCES device_key (id) ON DELETE CASCADE,
    user_id uuid NOT NULL REFERENCES "user" (id) ON DELETE CASCADE, added_epoch bigint NOT NULL,
    PRIMARY KEY (conversation_id, incarnation, device_key_id),
    FOREIGN KEY (conversation_id, incarnation) REFERENCES mls_group ON DELETE CASCADE);

CREATE TABLE mls_commit (
    id uuid PRIMARY KEY, conversation_id uuid NOT NULL, incarnation integer NOT NULL,
    epoch bigint NOT NULL,                      -- created in this epoch; the group moves to epoch + 1
    sender_user_id uuid NULL REFERENCES "user" (id) ON DELETE SET NULL,
    sender_device_key_id uuid NULL REFERENCES device_key (id) ON DELETE SET NULL,
    message bytea NOT NULL, created_at timestamptz NOT NULL DEFAULT now(),
    FOREIGN KEY (conversation_id, incarnation) REFERENCES mls_group ON DELETE CASCADE,
    CONSTRAINT uq_mls_commit_epoch UNIQUE (conversation_id, incarnation, epoch));

CREATE TABLE mls_welcome (
    id uuid PRIMARY KEY, commit_id uuid NOT NULL REFERENCES mls_commit (id) ON DELETE CASCADE,
    conversation_id uuid NOT NULL REFERENCES conversation (id) ON DELETE CASCADE,
    incarnation integer NOT NULL,
    epoch bigint NOT NULL,                      -- the epoch the new devices join at
    message bytea NOT NULL, created_at timestamptz NOT NULL DEFAULT now(),
    expires_at timestamptz NOT NULL);
CREATE TABLE mls_welcome_recipient (
    welcome_id uuid NOT NULL REFERENCES mls_welcome (id) ON DELETE CASCADE,
    device_key_id uuid NOT NULL REFERENCES device_key (id) ON DELETE CASCADE,
    user_id uuid NOT NULL REFERENCES "user" (id) ON DELETE CASCADE, acked_at timestamptz NULL,
    PRIMARY KEY (welcome_id, device_key_id));
CREATE INDEX idx_mls_welcome_recipient_device ON mls_welcome_recipient (device_key_id)
    WHERE acked_at IS NULL;

CREATE TABLE mls_readd_request (
    id uuid PRIMARY KEY, conversation_id uuid NOT NULL, incarnation integer NOT NULL,
    device_key_id uuid NOT NULL REFERENCES device_key (id) ON DELETE CASCADE,
    payload text NOT NULL, signature text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(), fulfilled_epoch bigint NULL,
    FOREIGN KEY (conversation_id, incarnation) REFERENCES mls_group ON DELETE CASCADE);

ALTER TABLE message
    ADD COLUMN encrypted boolean NOT NULL DEFAULT false,
    ADD COLUMN mls_ciphertext bytea NULL,
    ADD COLUMN mls_incarnation integer NULL,
    ADD COLUMN mls_epoch bigint NULL,
    ADD COLUMN sender_device_key_id uuid NULL REFERENCES device_key (id) ON DELETE SET NULL,
    ADD COLUMN franking_commitment bytea NULL,
    ADD COLUMN franking_receipt bytea NULL,
    ADD COLUMN franking_key_id integer NULL REFERENCES franking_key (id),
    ADD CONSTRAINT chk_message_encrypted_text CHECK (NOT encrypted OR text IS NULL),
    ADD CONSTRAINT chk_message_encrypted_envelope
        CHECK (encrypted = (mls_epoch IS NOT NULL AND mls_incarnation IS NOT NULL));
ALTER TABLE message_attachment ADD COLUMN blob_sha256 bytea NULL;
ALTER TABLE message_report
    ADD COLUMN franked_content bytea NULL, ADD COLUMN franking_verified_at timestamptz NULL;
```

The migration also adds `device_key.last_key_package_upload_at timestamptz NULL`, the MLS
activity signal (key package rows are deleted when they expire, so their age cannot carry it).
The change-feed function also sets `edited_at` when `mls_ciphertext` changes to a new envelope
(clearing it on delete is not an edit), and the update trigger gains
`OR NEW.mls_ciphertext IS DISTINCT FROM OLD.mls_ciphertext` in its `WHEN`. `down` restores
the previous function and trigger, then drops the new columns, indexes and tables; it refuses to
run while an encrypted message exists (rolling back would leave rows with neither text nor
ciphertext). Retention: Commits 90 days, Welcomes 30 days, claimed key packages 7 days, re-add requests
once fulfilled or after 7 days, `franked_content` 90 days after the report is resolved.

Code: an `e2ee` module in `rim_core` (repository traits), `rim_services` (`E2eeService`, DTOs, an
`MlsWire` trait for decoding and key package validation), `rim_infra_services` (the service and the
OpenMLS-backed `MlsWire`), `rim_infra_db`. The auth context gains the session's `device_key_id`
(`session.device_key_id`), which `Actor` does not carry today.

## GraphQL

Binary values are unpadded base64url strings; `MLSMessage` bytes are the RFC 9420 `MLSMessage`
encoding (key packages, Welcomes and PrivateMessages alike). Epochs fit `Int` (an incarnation is
re-created long before 2³¹).

```graphql
type Conversation { encryptedAt: DateTime  mlsGroup: MlsGroup }        # both null for plaintext
type MlsGroup {
  groupId: String!  incarnation: Int!  cipherSuite: Int!  epoch: Int!   # epoch: next Commit's epoch
  devices: [MlsDevice!]!  pendingAdds: [MlsPendingDevice!]!  pendingRemoves: [MlsDevice!]!
  readdRequests: [MlsReaddRequest!]!
}
type MlsDevice { userId: ID!  principalId: String!  deviceId: ID!  addedEpoch: Int! }
type MlsPendingDevice { userId: ID!  principalId: String!  deviceId: ID!  hasKeyPackage: Boolean! }
type MlsReaddRequest { principalId: String!  deviceId: ID!  request: SignedStatement!  createdAt: DateTime! }

type MessageEncryption {
  incarnation: Int!  epoch: Int!  senderDeviceId: ID!
  ciphertext: String                     # null once deleted
  frankingReceipt: String!  frankingKeyId: Int!  sentAtUs: String!   # decimal microseconds
}
type Message { encryption: MessageEncryption }                         # null for plaintext rows
type Attachment { blobSha256: String }

type MlsKeyPackageStatus { cipherSuite: Int!  available: Int!  lastResortNotAfter: DateTime  oldestNotAfter: DateTime }
type ClaimedKeyPackage { userId: ID!  principalId: String!  deviceId: ID!  keyPackage: String!  keyPackageRef: String!  lastResort: Boolean! }
type MlsCommit { id: ID!  incarnation: Int!  epoch: Int!  senderPrincipalId: String  senderDeviceId: ID  message: String!  createdAt: DateTime! }
type MlsCommitPage { commits: [MlsCommit!]!  hasMore: Boolean! }
type MlsWelcome { id: ID!  serverId: ID!  conversationId: ID!  incarnation: Int!  epoch: Int!  message: String!  expiresAt: DateTime! }
type PrincipalDevice { deviceId: ID!  name: String!  certificate: SignedStatement!  revokedAt: DateTime  revocation: SignedStatement }
type PrincipalDevices { principalId: String!  devices: [PrincipalDevice!]! }

input CreateConversationInput { encrypted: Boolean }                   # additive; default false
input SendMessageInput { forwardedFromMessageId: ID }                  # additive
input CreateAttachmentInput { blobSha256: String }                     # additive
input MlsDeviceRefInput { principalId: String!  deviceId: ID! }
input ClaimMlsKeyPackagesInput { serverId: ID!  conversationId: ID!  devices: [MlsDeviceRefInput!]! }
input SubmitMlsCommitInput {
  serverId: ID!  conversationId: ID!  incarnation: Int!  epoch: Int!
  cipherSuite: Int                       # required for an incarnation's first Commit (epoch 0)
  commit: String!  welcome: String       # welcome required when the summary has adds
}
input SendEncryptedMessageInput {
  serverId: ID!  conversationId: ID!  messageId: ID!  envelope: String!
  threadRootMessageId: ID  forwardedFromMessageId: ID
}
input EditEncryptedMessageInput { serverId: ID!  conversationId: ID!  messageId: ID!  envelope: String! }
input RequestMlsReaddInput { serverId: ID!  conversationId: ID!  request: SignedStatementInput! }
input ReportEncryptedMessageInput {
  serverId: ID!  conversationId: ID!  messageId: ID!  reason: String!
  content: String!  frankingKey: String!  receipt: String!  receiptKeyId: Int!
  senderUserId: ID!  senderDeviceId: ID!  sentAtUs: String!
}

type Query {
  myMlsKeyPackageStatus: [MlsKeyPackageStatus!]!                       # session device
  myMlsWelcomes: [MlsWelcome!]!                                         # session device, unacknowledged
  mlsCommits(serverId: ID!, conversationId: ID!, incarnation: Int!, afterEpoch: Int, limit: Int): MlsCommitPage!
  principalDevices(principalIds: [String!]!): [PrincipalDevices!]!     # principals sharing a workspace with the caller
}
type Mutation {
  uploadMlsKeyPackages(keyPackages: [String!]!): MlsKeyPackageStatus!
  claimMlsKeyPackages(input: ClaimMlsKeyPackagesInput!): [ClaimedKeyPackage!]!
  submitMlsCommit(input: SubmitMlsCommitInput!): MlsCommit!
  sendEncryptedMessage(input: SendEncryptedMessageInput!): Message!
  editEncryptedMessage(input: EditEncryptedMessageInput!): Message!
  ackMlsWelcomes(ids: [ID!]!): Int!
  requestMlsReadd(input: RequestMlsReaddInput!): Boolean!
  reportEncryptedMessage(input: ReportEncryptedMessageInput!): Boolean!
}
```

- `mlsCommits`: `afterEpoch` exclusive (null from 0), `limit` 1 to 100 (default 50), oldest first;
  caller must be a conversation member. `principalDevices` answers one entry per distinct id asked,
  in order, with an empty device list for a principal who shares no workspace with the caller, at
  most 100 ids per call.
- New objects `E2eeQueries` and `E2eeMutations` are appended to the last query and mutation groups.
  All mutations need the CSRF header. The GraphQL request body must allow at least 8 MiB (a 4 MiB
  Commit is 5.4 MiB of base64url); the server sets no limit of its own on `/api`, so a proxy in
  front of it must not set a lower one.

### Errors

| Code | When | Extensions |
|---|---|---|
| `STALE_EPOCH` (new) | A Commit or message for an epoch or incarnation that is not current | `incarnation`, `epoch` (current) |
| `MLS_COMMIT_REQUIRED` (new) | A message while removals are pending, or into an encrypted conversation without a group | `reason`: `pendingRemovals` or `noGroup` |
| `FORBIDDEN` | Session without a device; device not in the group; claim or commit by a non-member | — |
| `VALIDATION` | Malformed bytes, wrong wire format or content type, authenticated data or summary rules, size caps, plaintext write into an encrypted conversation, unready members | reason text |
| `CONFLICT` | Message id reused with other bytes; second report | — |
| `TIMED_OUT`, `NOT_FOUND`, `RATE_LIMITED` | As for the plaintext equivalents | as today |

### Events

`NewMessage` and `MessageUpdated` carry `message.encryption` (the `MessageEncryption` fields, snake
case) and `text: null`. New variants, delivered to the conversation's members (server-side scope
lists as today):

```json
{"type":"MlsCommitAccepted","workspace_id":"…","conversation_id":"…","incarnation":1,"epoch":7}
{"type":"MlsMembershipChanged","workspace_id":"…","conversation_id":"…"}
{"type":"MlsWelcomeAvailable","workspace_id":"…","conversation_id":"…","user_id":"…","device_id":"…"}
```

`epoch` is the Commit's epoch. `MlsMembershipChanged` fires when pending adds or removes change
(membership, devices, key package activity, re-add requests): on adds, removals, leaves, kicks and
bans in an encrypted conversation, on a device revocation or recovery, on a key package upload by
a device that is not in a group yet, on a re-add request, and with every Commit that adds or
removes devices. `MlsWelcomeAvailable` goes to
`user_id` only; clients ignore it for other devices.

## Limits and audit

| What | Limit |
|---|---|
| Key package | 16 KiB each; 50 per upload; 100 unclaimed per device and suite |
| Claims | 600 devices per user per hour; one-time packages of one device: 100 claims per day, then only its last-resort package |
| Commit | 4 MiB; authenticated data 256 KiB |
| Welcome | 4 MiB |
| Application envelope | 96 KiB; authenticated data 8 KiB; content 64 KiB |
| Re-add requests | 3 per device per conversation per day |
| Encrypted conversation | 100 members, 512 devices |

Rate limiters are the in-memory per-node ones, as for device linking; correctness never depends
on them. Audit (`command_audit_log`, ids only): `e2ee.enable` (incarnation 1 accepted),
`e2ee.reincarnate`, `e2ee.report` (franked report accepted), `admin.delete_message` as today.
Logs carry conversation ids, epochs and device row ids, never bytes.

## Threat model

| Adversary | Can | Cannot |
|---|---|---|
| The server (full database and code) | See metadata: members, devices, timing, sizes, sender devices, mentions, reactions, pins, threads, forward origins; withhold or delay messages and Commits; refuse Commits; delete rows; ask nothing a device would sign | Read content or attachments; add a device of its own (it holds no device key to sign `mls-key`); forge a message; move a ciphertext to another row or conversation; force a last-resort re-add; read reports it did not receive |
| A relay (Stage 5) | Less than the server: it sees ciphertext and its own users' metadata, and can withhold | Everything the server cannot |
| A compromised current device | Read everything its leaf can read, past messages in its cache and archive, until it is revoked and removed; send as that device | Read after its removal (forward secrecy of later epochs); recover messages whose keys were used and deleted, other than through its cache |
| A revoked device | Read messages until its removal Commit is accepted (the server refuses others' messages while the removal is pending, so a sender commits it first) | Read anything after that Commit |
| A member's old, unrevoked device | Read like any device; it is removed after 60 days without a key package upload | — |
| A former member | Keep what its devices decrypted | Read anything after its devices' removal |
| The drive provider | See archive files and sizes (`history-archive-v1.md`) | Read them; group state is never there |

Post-compromise security comes from Commits with update paths (every device at most weekly, and
every membership Commit). Quantum: key agreement is hybrid (X-Wing); signatures and
authentication are classical.

## Test plan

**Server (S4-1).** Unit tests of `E2eeService` with mock repositories for every row of the checks,
errors and summary tables; key package validation with real OpenMLS key packages (bad signature,
wrong device, wrong `mlsKey`, expired, foreign suite, reused ref); the creation, migration and
incarnation rules. `rim_infra_db` (`db-tests`): two concurrent Commits for one epoch, exactly one
wins and the other is `STALE_EPOCH`; an application message racing a Commit lands in epoch order;
`SKIP LOCKED` claims under concurrency never hand one package twice; the last-resort partial
index; the trigger bumps `version` and `edited_at` on a ciphertext change; cascades; migration up
and down (and `down` refusing). `rim_api` e2e (`e2e-tests`) with a test client built on `rim_mls`
fixtures: create an encrypted DM, Welcome, send, edit, delete, react, report with franking (valid,
tampered content, wrong key, message deleted first); plaintext writes refused; pending removal
blocks a send until committed; `forwardMessage` refused; search returns nothing encrypted; digests
say "Encrypted message"; appendix A receipt and attachment hash.

**Crate (S4-2).** The ADR's vector table; appendix A byte for byte; both providers create and process
0x004F groups and interoperate; multi-device simulations against an in-process sequencer: 3 users ×
3 devices, link and revoke devices, members leave and join, racing committers with ranking, a
device offline past retention re-added, lost state, a Commit that cannot be processed, an
incarnation; every leaf-verification failure; authenticated-data mismatches refused; decrypt once
(a second decrypt fails); crash between decrypt and store leaves both undone.

**Clients (S4-3 to S4-6).** End-to-end flows on two desktop instances against a dev server: new DM,
migration with marker, new device joining and restoring from the archive, attachments, local
search, mentions and keyword toasts, reports; plugin grants (S4-4); the web client in a headless
browser with the same flows and the wasm test suite (S4-6).

## Deferred

- Hybrid MLS credential signatures (no composite Ed25519 + ML-DSA-44 MLS suite exists).
- Safety numbers and key transparency, so a person can check another's devices out of band.
- Sender keys or another scheme for groups beyond 512 devices; encrypted private channels and
  history sharing to new members.
- Message deniability; encrypted reactions and pins; client-made thumbnails; streamed or chunked
  attachments above 25 MB; per-conversation plugin grants.
- Groups spanning servers (Stage 5); signed archive messages (`sig`).

## Appendix A: known-answer vectors

Computed with Python `cryptography` 50.0.2 and cross-checked with Node 26 `crypto`. Keys are
`SigningKey::from_bytes([seed; 32])`; the device is `key-identity-v1.md` appendix B's (seed 20,
principal `rimp1_txDLbnipi5i3kCJtJwBOUixzv_MY8OnU0mcy--Tyje0`, device id
`00000000-0000-0000-0000-000000000001`). JSON is written with fields in the order shown and no
spaces; readers do not depend on order. Never use these values for real data.

### A.1 Group id

Conversation `0199d1a0-0000-7000-8000-000000000001`, incarnation 1:
`72696d310199d1a000007000800000000000000100000001` (base64url `cmltMQGZ0aAAAHAAgAAAAAAAAAEAAAAB`).

### A.2 `mls-key` statement and credential identity

MLS key (seed 40): `ed25519:6S62BU_pvGgqG887dZ9lqzikz72BxNHzNC5Myc3tiws`.

```
{"v":2,"type":"mls-key","principalId":"rimp1_txDLbnipi5i3kCJtJwBOUixzv_MY8OnU0mcy--Tyje0","deviceId":"00000000-0000-0000-0000-000000000001","mlsKey":"ed25519:6S62BU_pvGgqG887dZ9lqzikz72BxNHzNC5Myc3tiws","issuedAt":"2026-10-09T00:00:00Z"}
ed25519:6TN4OHOn3VKI00okNthNJOJkuAg1afEO9h-63L9T6VoQxeYA1acXMqTuLBu5_flkzCjPRssEWpP6LkVCi74EDA
```

The credential `identity` is the 382-byte JSON `{"payload":"<the payload above, JSON-escaped>",
"signature":"<the signature above>"}`, SHA-256
`b7334437dc6e0da6588fbfc37540416efc0497c0a75d4eccae1fd92cae969a11`. A key package carrying it, and
its `KeyPackageRef`: to be filled by S4-2 (key packages contain fresh HPKE keys; S4-2 records one
fixed key package as a fixture and its ref).

### A.3 Attachment

| Item | Value |
|---|---|
| Secret (bytes `a0`..`bf`) | `oKGio6SlpqeoqaqrrK2ur7CxsrO0tba3uLm6u7y9vr8` |
| Key | `b03b51201f06749186a1768c665aaa48570ab7c6f42886d09ab5a97116b15b8f` |
| Nonce | `306c91d469a72d1e1b19c711` |
| Plaintext | `hello, attachment\n` (18 bytes) |
| Blob (ciphertext ‖ tag, 34 bytes) | `096d52fa9b833e37ef33d387fc5ecab34eabb410807b85e90c862a44614c7c9cb163` |
| `blobSha256` | `EHFcZ6hUOIdDi32ptdrCXM2PTHQwKF46-0Ls56yQrac` |

### A.4 Content, franking and authenticated data

Message `0199d1a0-0000-7000-8000-000000000002` mentioning appendix A's principal of
`key-identity-v1.md`, with A.3's attachment. Content (480 bytes):

```
{"v":1,"type":"message","id":"0199d1a0-0000-7000-8000-000000000002","rev":0,"threadRootId":null,"body":"hi <@rimp1_nz1POml8JmCMcFDECrtu60ibohPKnywhcExmOPMvTUo>","mentions":["rimp1_nz1POml8JmCMcFDECrtu60ibohPKnywhcExmOPMvTUo"],"mentionAll":false,"replyTo":null,"forwardedFrom":null,"attachments":[{"blobSha256":"EHFcZ6hUOIdDi32ptdrCXM2PTHQwKF46-0Ls56yQrac","secret":"oKGio6SlpqeoqaqrrK2ur7CxsrO0tba3uLm6u7y9vr8","name":"hello.txt","mime":"text/plain","size":18}],"linkPreviews":[]}
```

| Item | Value |
|---|---|
| Franking key (bytes `60`..`7f`) | `YGFiY2RlZmdoaWprbG1ub3BxcnN0dXZ3eHl6e3x9fn8` |
| Commitment `c` | `71ec48866b283644c13508ddc5c8d4fcc72ba51721dcd3b74b16e5a50d5a874d` (`cexIhmsoNkTBNQjdxcjU_McrpRch3NO3SxblpQ1ah00`) |
| Application data (`01` ‖ key ‖ content, 513 bytes), SHA-256 | `2034e24c9ef5d4ad3965448fb6beec5852da1a0c87896bb07f14b3c7f63ab776` |
| The PrivateMessage carrying it | To be filled by S4-2 (randomized; S4-2 checks that decrypting its fixture yields these 513 bytes) |

Authenticated data (220 bytes):

```
{"v":1,"type":"application","messageId":"0199d1a0-0000-7000-8000-000000000002","franking":"cexIhmsoNkTBNQjdxcjU_McrpRch3NO3SxblpQ1ah00","mentions":["rimp1_nz1POml8JmCMcFDECrtu60ibohPKnywhcExmOPMvTUo"],"mentionAll":false}
```

A Commit's authenticated data adding device `…0002` of that principal with the ref of bytes
`c0`..`df`:

```
{"v":1,"type":"commit","adds":[{"principalId":"rimp1_nz1POml8JmCMcFDECrtu60ibohPKnywhcExmOPMvTUo","deviceId":"00000000-0000-0000-0000-000000000002","keyPackageRef":"wMHCw8TFxsfIycrLzM3Oz9DR0tPU1dbX2Nna29zd3t8"}],"removes":[]}
```

### A.5 Franking receipt

Server franking key (bytes `80`..`9f`), sender user `0199d1a0-0000-7000-8000-0000000000aa`, sender
device `…0001`, conversation A.1, message A.4, `sent_at` 2026-10-09T00:00:00.123456Z
(`sentAtUs` `1791504000123456`). Receipt input (133 bytes):

```
72696d2e653265652e6672616e6b696e672d726563656970742e7631000199d1a00000700080000000000000020199d1a00000700080000000000000010199d1a00000700080000000000000aa0000000000000000000000000000000100065d5d078a824071ec48866b283644c13508ddc5c8d4fcc72ba51721dcd3b74b16e5a50d5a874d
```

Receipt: `9c25cd6659b97074696a9445ad323e061debaaf5f97af5ed2f2cd46150d3db75`
(`nCXNZlm5cHRpapRFrTI-Bh3rqvX5evXtLyzUYVDT23U`).

### A.6 Re-add request

```
{"v":2,"type":"mls-readd","principalId":"rimp1_txDLbnipi5i3kCJtJwBOUixzv_MY8OnU0mcy--Tyje0","deviceId":"00000000-0000-0000-0000-000000000001","conversationId":"0199d1a0-0000-7000-8000-000000000001","incarnation":1,"lastEpoch":41,"nonce":"AAECAwQFBgcICQoLDA0ODw","issuedAt":"2026-10-09T00:00:00Z"}
ed25519:UNry-YBfbQv1nUxStNVtA9jirfxTM5rLE-JA40KZGS9c-tCyl90KEq4W1OxmoxL0msvG62Gc5msFvpCI_h_lCw
```
