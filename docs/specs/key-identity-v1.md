# Key identity v1

_Status: implemented, normative. Follows `docs/decisions/decentralization-2026-10-08.md`:
a user is a key pair, the user id is stable with a signed key log, the web app is a linked
device, and servers are interchangeable relays. This document is the contract between the
server (`rim_identity`, `KeyAuthService`) and every client that signs statements._

_Algorithm agility (`docs/decisions/post-quantum-hybrid-2026-10-09.md`): statement format
version 2 tags every key and signature with its algorithm, payload caps fit
hybrid post-quantum keys, and the server indexes device keys by a hash. Version 1 statements
stay valid. See "Algorithms and statement versions"._

_Key-only signup: `signupWithKey` creates an account from a key
identity alone, with no password and no email, so `user.password_hash` and `user.email` are
nullable. See "Key-only signup"._

## Scope

- **`rim_identity`**, a dependency-light crate that verifies every signed statement below. It
  has no database, HTTP or GraphQL code, so the Tauri client and later the relay can reuse it.
- **Storage**: key columns on the existing `user` row, plus `key_log_entry`, `device_key`,
  `profile_card` and `key_challenge` tables, and `session.device_key_id`.
- **`KeyAuthService`** with GraphQL mutations to enroll a key on an existing account, add and
  revoke devices, rotate or recover the identity key, publish a profile card, and sign in with a
  device key through a single-use challenge. Key login issues the same session cookie as
  password login.
- **Password sign-in and password reset are refused** for an account once it has enrolled a
  key.

Defined elsewhere: device linking (`device-linking-v1.md`), plugin manifest signing
(`workspace-plugins-v1.md`) and remote users (`server-to-server-v1.md`). The recovery phrase and
the key UI are client features of rim-ui.

## Keys and encodings

- All keys today are **Ed25519**. Key and signature bytes are **unpadded base64url** (RFC 4648
  §5, no `=`), written with an algorithm tag in front in version 2 statements (next section).
  Decoders are strict: padding, non-canonical trailing bits, wrong lengths, small-order
  ("weak") keys and non-canonical point encodings are rejected. Signatures are checked with
  `verify_strict`, so malleable signatures fail.
- A person holds three kinds of key:
  - the **identity key**, which signs the key log, device certificates, revocations and the
    profile card. It stays off day-to-day devices where possible;
  - the **recovery key**, kept offline (derived from the 12-word recovery phrase, see "Recovery
    kit"). It can replace a lost identity key;
  - one **device key** per device, certified by the identity key. Only device keys sign in.
- The identity key must differ from the recovery key, and a device key must differ from both.

## Algorithms and statement versions

Every payload names its format version in `"v"`. A server accepts versions **1 and 2**; any
other value is refused with `unsupported statement version N`. **Clients write version 2.**
Version 1 stays accepted so that statements signed before version 2 existed keep verifying,
and nothing a server stores is ever re-signed.

The two versions differ **only in how keys and signatures are spelled**; field names, context
bytes, caps and every rule below are the same.

| | Version 1 | Version 2 |
|---|---|---|
| Key (`identityKey`, `recoveryKey`, `deviceKey`) | `<base64url>`, Ed25519 implied | `<algorithm>:<base64url>` |
| `signature` | `<base64url>`, Ed25519 implied | `<algorithm>:<base64url>` |
| Algorithms | Ed25519 only | any the server implements |

- **Tagged form.** `<algorithm>` is 1 to 32 characters of `a-z`, `0-9` and `-`, followed by
  one `:` (not in the base64url alphabet, so the first colon splits). Today the only algorithm
  is `ed25519` (32-byte key, 64-byte signature), for example
  `ed25519:iojj3XQJ8ZX9UtstPLpdcspnCb8dlBIb83SIAbQPb1w`. A version 2 key or signature without a
  tag, and a version 1 key or signature with one, is a `VALIDATION` error
  (`identityKey is not in its canonical encoding`). A well-formed name the server does not
  implement is refused with a message naming it (for example
  `deviceKey names an unsupported algorithm` followed by the name), so a client can tell
  "newer than this server" from a malformed value.
- **A signature's algorithm must be its key's.** A signature tagged with one algorithm never
  verifies under a key of another (`signature algorithm does not match the signing key`).
- **One key, two spellings.** A key is the same key in either version: `KeyState`, device
  uniqueness and the `deviceKey must differ…` checks compare algorithm and key bytes, not text.
  So a version 1 inception's recovery key and the same key tagged in a version 2 rotation match.
- **Mixed logs.** A key log may start in version 1 and continue in version 2 (an existing
  identity moves to the new format, and later to a new algorithm, with an ordinary rotation
  signed by its current key). It never goes back: once an entry is version 2, a later
  version 1 entry is refused (`a version 1 statement cannot follow a version 2 key log entry`).
  Device certificates, revocations, profile cards and login proofs may be either version
  whatever the log's version; a version 1 statement can only carry and be checked with
  Ed25519 keys.
- **The principal id is derived the same way in both versions and for every algorithm**: it
  is a hash of the inception payload bytes, so rotating to a new version or algorithm keeps
  it. The same keys incepted once as version 1 and once as version 2 give two different ids
  (the payloads differ); a client incepts once and keeps that log.
- **Canonical text.** Servers store and return keys in the tagged form, whichever version
  carried them (`DeviceKey.publicKey`, `user.identity_key`).
- **Hashes stay SHA-256.** `principalId`, `prev` and the device key hash are SHA-256, which
  keeps 128-bit security against quantum search; they are not tagged.

**Planned algorithm (not implemented).** Per the hybrid post-quantum decision, identity,
recovery and device keys later move to **Ed25519 + ML-DSA-44** (FIPS 204), expected under the
name `ed25519-mldsa44`: the key and signature carry both parts (32 + 1,312 bytes of key,
64 + 2,420 bytes of signature) and a signature is valid only when both parts verify. Until a
server implements it, that name is refused like any unknown algorithm. The payload caps below
already fit it, including a profile card listing 32 such device keys.

## Signed statements

Every statement travels as `{ payload, signature }`:

- `payload` is the **exact UTF-8 JSON text that was signed**. Servers verify the signature over
  these bytes (they parse the payload first only to learn which key signed it), and store and
  return the text unchanged. Nothing is ever
  re-serialized, so there is no canonical JSON form: a client may order keys and space the JSON
  however it likes, and must keep the text it signed.
- `signature` is the signature over `context || payload`, spelled as the payload's version
  requires, where `context` names the statement's purpose, so a signature made for one purpose
  never verifies as another:

| Purpose | Context bytes | Signed by | Max payload |
|---|---|---|---|
| Key log entry | `rim.keylog.v1\0` | identity key (inception, identity rotation) or recovery key (recovery rotation) | 16 KiB |
| Device certificate | `rim.device.v1\0` | current identity key | 16 KiB |
| Device revocation | `rim.device-revoke.v1\0` | current identity key, or the revoked device itself | 16 KiB |
| Profile card | `rim.profile-card.v1\0` | current identity key | 128 KiB |
| Login proof | `rim.login.v1\0` | a device key | 16 KiB |

`\0` is one zero byte. The contexts end in `v1` in both statement versions: they name the
purpose, and the version is inside the signed payload. The caps hold for both versions and are
sized for hybrid keys: a tagged Ed25519 + ML-DSA-44 key is about 1.8 KB of text (ML-DSA-87
about 3.5 KB), a rotation names two keys, and a card lists up to 32 devices (about 60 KB with
ML-DSA-44, 115 KB with ML-DSA-87). Every payload has `"v"` (1 or 2) and a `"type"`. Unknown fields, duplicate
fields and a wrong `type` are rejected. The examples below are version 2; a version 1 payload
is the same with `"v":1` and bare keys. Timestamps are RFC 3339 and informational only: servers
do not trust client clocks, so nothing depends on them.

### Key log and principal id

The key log is the person's history of identity keys. Entry 0 is the **inception**:

```json
{"v":2,"type":"inception","seq":0,"identityKey":"ed25519:…","recoveryKey":"ed25519:…","createdAt":"2026-10-09T00:00:00Z"}
```

signed by its own `identityKey`. The **principal id** is fixed by it forever:

```
principalId = "rimp1_" + base64url(SHA-256("rim.principal.v1\0" || inception payload bytes))
```

So the id commits to both the first identity key and the recovery key, survives every rotation,
and cannot be derived for keys other than those in that exact inception. The log and
certificates are public, so a server additionally requires proof that the enrolling device holds
its key (see Enrollment) before it lets an account carry a principal id.

Every later entry is a **rotation**:

```json
{"v":2,"type":"rotate","principalId":"rimp1_…","seq":1,"prev":"…","identityKey":"ed25519:…","recoveryKey":"ed25519:…","signer":"identity","createdAt":"…"}
```

- `seq` is the previous entry's `seq + 1`; `prev` is base64url(SHA-256(previous entry's payload
  bytes)); `principalId` is the log's principal id; `identityKey` must change. `v` is at least
  the previous entry's.
- `"signer": "identity"`: signed by the current identity key. The recovery key must stay the
  same. Existing devices stay enrolled (their certificates name an older `keySeq`, which is
  fine: a certificate is checked once, when the device is added).
- `"signer": "recovery"`: signed by the current recovery key. The recovery key may change.
  This is how a person who lost their identity key gets back in.
- In every entry the identity key differs from the recovery key, and an entry with a field this
  section does not name is refused, by servers and clients alike.

**Key log head pinning (clients).** A client keeps, per principal, the `seq` and payload hash of
the head of the longest log it verified. The first log it verifies for a principal pins its head;
a log served later must contain the pinned entry at its `seq` (the same payload hash), and the
pin then moves to the new head. A log that is shorter, or that holds another entry at the pinned
`seq`, is refused: the client says the person's identity changed in a way RIM cannot verify, and
treats that principal's devices as unverified.

### Device certificate

```json
{"v":2,"type":"device","principalId":"rimp1_…","deviceId":"<uuid>","deviceKey":"ed25519:…","name":"Laptop","keySeq":0,"issuedAt":"…"}
```

Signed by the identity key at `keySeq`, which must be the head of the log when the server
receives it. Clients accept a certificate at an older `keySeq` (devices stay enrolled across
identity rotations) unless a recovery rotation follows that `keySeq`: a recovery voids what the
lost key signed, as for plugin manifests. `deviceId` is chosen by the client (UUID v7 recommended). `name` is trimmed, 1 to
64 characters, no control characters. Per account, a `deviceId` and a `deviceKey` are each used
once, **revoked devices included**: a revoked device comes back only with a new id and key. A
key counts as used whichever version spelled it.

### Device revocation

```json
{"v":2,"type":"device-revoke","principalId":"rimp1_…","deviceId":"<uuid>","signer":"identity","issuedAt":"…"}
```

`"signer": "identity"` (current identity key, any device) or `"signer": "device"` (the device
being revoked signs its own retirement, so a device can sign itself out for good without the
identity key). Revoking is idempotent: the first signed revocation is kept. Clients verify a
revocation the server lists (by an identity key of the log, the one current when it was signed,
or by the revoked device's own key) and remember a device seen revoked, so a server that later
omits the revocation changes nothing on that client.

### Profile card

```json
{"v":2,"type":"profile-card","principalId":"rimp1_…","version":3,"keySeq":0,"displayName":"Ada",
 "relays":["https://relay.rim.chat"],"devices":[{"deviceId":"<uuid>","deviceKey":"ed25519:…"}],"issuedAt":"…"}
```

What a person publishes about themselves, portable between servers. Signed by the current
identity key at the current `keySeq`. `version` is 1 to 2³¹−1 and a server stores a card only if
its version is greater than the stored one. `displayName` 1 to 80 characters; at most 8 unique
`http(s)` relay URLs of at most 256 bytes; at most 32 devices with unique ids. The server does
not check the listed devices against its device table: readers verify each device through its
certificate.

### Login proof

```json
{"v":2,"type":"login","challengeId":"<uuid>","nonce":"…","audience":"rim.example.com","principalId":"rimp1_…","deviceId":"<uuid>"}
```

Signed by the device key. `challengeId` and `nonce` come from `keyLoginChallenge`. `audience` is
the `host[:port]` the client connects to (lowercase is safest; servers compare it
case-insensitively), so a server that receives a proof cannot relay it to another server.

## Server rules

### Enrollment

`enrollIdentityKey(input: { keyLog, device, proof })`, signed in with a password session:

1. The whole log (at most 256 entries) is replayed and verified from the inception; the device
   certificate must be signed by the head identity key.
2. `proof` is a `login` statement (same format and context as for challenge login) signed by
   the enrolling device's key over a fresh `keyLoginChallenge`, naming this log's principal id,
   the certificate's device id and this server's audience. The challenge is consumed. Without
   it, anyone who had seen a person's log and a device certificate (every server they use
   holds them) could enroll that principal on another account elsewhere.
3. The account must not have a principal id yet, and no other account on this server may have
   this one (both `CONFLICT`).
4. The server stores the log and the device, sets `user.principal_id` and `user.identity_key`,
   and binds **the enrolling session to the new device**, so revoking that device later ends it.
   Other existing password sessions stay valid until they expire or are signed out.
5. From then on the password no longer signs in (`login` answers `UNAUTHENTICATED`, with the
   same timing as a wrong password), `requestPasswordReset` sends nothing, and an outstanding
   reset link is refused with `FORBIDDEN`.

After enrolling, the client offers to sign out other sessions: `logoutOtherSessions` ends every
session of the account but the caller's and answers how many.

### Key-only signup

`signupWithKey(input: { username, displayName, keyLog, device, proof, instanceInviteToken,
workspaceInviteToken })`, no session needed, creates an account from a key
identity alone: no password, no email.

1. The registration mode applies as for `signupWithInvite`. In invite-only mode exactly one
   token is required, checked and consumed as by the password signup of its kind: an instance
   invite creates an account with no workspace (`serverId` null), a workspace invite also joins
   its server. An instance invite bound to an email address admits only a password signup with
   that address, so it refuses a key-only signup with `VALIDATION`. In open mode no token may be given, and the signup
   counts against the same per-IP limit as an open password signup. A wrong number of tokens is
   `VALIDATION`.
2. The identity is verified as at enrollment (rules 1 and 2): the whole log, at most 256
   entries, replays from the inception; the certificate is signed by the head identity key; the
   proof is a `login` statement by that device over a fresh `keyLoginChallenge`, naming this
   log's principal id, the certificate's device id and this server's audience. The challenge is
   consumed. No other account here may hold the principal id (`CONFLICT`). The username follows
   the password signup's rules and must be free (`CONFLICT`); `displayName` is trimmed, at most
   80 characters, and defaults to the username.
3. In one transaction the server creates the user with `password_hash` and `email` null and
   `principal_id` and `identity_key` set, stores the log and the device, consumes the token (or
   joins the server), and creates a session **bound to the device**, recording the device's
   `lastUsedAt`. It answers `{ userId, serverId }` and sets the same session and CSRF cookies as
   `signupWithInvite`. Of two signups racing for one principal, the unique index refuses the
   second (`CONFLICT`).
4. A key-only account has no password. `login` answers `UNAUTHENTICATED` with the same timing as
   a wrong password, `requestPasswordReset` sends nothing, and `resetPassword` is `FORBIDDEN`, as
   for an enrolled account.
5. Nothing emails an account without an email address: notification digests skip it.

### Challenge login

1. `keyLoginChallenge` (no session) returns `{ challengeId, nonce, expiresAt }`: 32 random bytes,
   valid for **120 seconds**, single use. At most 60 challenges per IP per 5 minutes.
2. The client signs a login proof with its device key and calls `loginWithKey(proof)`.
3. The server finds the account by `principalId` and the device by `deviceId`, verifies the
   signature with that device's key, checks the audience against its accepted audiences, and
   claims the challenge atomically (unused, unexpired, same nonce).
4. On success it locks the user row, reads the device again (a revocation that committed in the
   meantime refuses the login), creates a session bound to the device, records the device's
   `lastUsedAt`, and sets the same session and CSRF cookies as `login`.
5. Every refusal is `UNAUTHENTICATED` with no detail, except a valid proof from a **revoked**
   device, which is `FORBIDDEN` so the client can tell the person to link the device again. A
   proof is valid here only when it passes every check of step 3: revocation is looked at after
   the audience and the challenge (which the attempt uses up), so an old proof, a replayed one or
   one made for another server never shows whether the device was revoked.
   Failed attempts are limited to 20 per IP per 15 minutes; successful logins do not count.

**Accepted audiences** come from `RIM_KEY_LOGIN_AUDIENCES` (comma-separated `host[:port]`).
When it is unset, the host of `RIM_PUBLIC_BASE_URL` is used; when that is unset too,
`localhost:PORT` and `127.0.0.1:PORT`, but only when `RIM_SERVER_HOST` is `localhost` or a
loopback address: a server bound to any other address (`0.0.0.0` included) with neither variable
set refuses to start, with a message naming both. The server logs the list at start-up.

`keyLoginChallenge`, `loginWithKey` and `signupWithKey` are exempt from the CSRF header check,
because they are used without a session; `loginWithKey` and `signupWithKey` only act on a fresh
signature, which a forged cross-site request cannot produce. `rotateIdentityKey` is exempt only when the request carries **no session
cookie** (recovery, signed out). With a session cookie it needs the CSRF header like any other
mutation, because an identity rotation is accepted on the strength of that session.

### Devices

- `addDeviceKey(certificate)`: signed in; the certificate must be signed by the current identity
  key at the current `keySeq`.
- `revokeDeviceKey(revocation)`: signed in; marks the device revoked, keeps the signed
  revocation, and **ends every session of that device**.
- `myKeyIdentity` lists the log, every device (revoked ones included) and the card.

### Rotation and recovery

`rotateIdentityKey(input: { entry, device })`:

- An **identity rotation** needs a session of that account. Devices stay enrolled. It is
  refused (`VALIDATION`, `a key log may have at most 256 entries`) when the log already has 256
  entries, the same cap as at enrollment, so a log grown by identity rotations can always be
  enrolled on another server.
- A **recovery rotation** works **without a session** (the person lost every device). It must
  come with a certificate for a new device under the new identity key. The server appends the
  entry, **revokes every device and ends every session** of the account, then enrolls the new
  device, which can sign in right away. The 256-entry cap does not stop a recovery: being
  locked out of the account is worse than a log too long to carry elsewhere. A log that
  recoveries took past 256 entries is refused by enrollment on other servers. Anonymous attempts
  count against the same per-IP failure limit as logins, whether they succeed or not. An anonymous rotation for a principal
  this server does not know is `UNAUTHENTICATED`; other refusals carry the validation reason,
  so they show whether a principal is enrolled here. That is not secret: `User.principalId` is
  visible to signed-in users.
- A rotation that does not extend the current head (wrong `seq` or `prev`) is a `VALIDATION`
  error. Two rotations racing for the same `seq` are serialized by the row lock below, so the
  second one sees the new head and fails the same way; fetch `myKeyIdentity` and sign again.

Every write that depends on the log (enroll, add, revoke, rotate, publish) runs in one
transaction with the user row locked (`SELECT … FOR UPDATE`), so two of them for one account
never interleave.

## Storage

| Where | What |
|---|---|
| `user.kind` | `local` (default) or `remote`; `remote` is a person whose relay is another server (`server-to-server-v1.md`) |
| `user.principal_id`, `user.identity_key` | Null until enrollment, set together; `principal_id` unique; `identity_key` is the current identity key in tagged form |
| `user.password_hash`, `user.email` | Null for a key-only account. `CHECK (password_hash IS NOT NULL OR principal_id IS NOT NULL)`: a key-only account always has a principal. Migration `key_only_signup`'s `down` refuses to run while a row has either null |
| `user.home_relay` | The relay of a `remote` user (`server-to-server-v1.md`); null for local users |
| `key_log_entry` | `(user_id, seq)` primary key, `entry_type` inception or rotate, payload, signature |
| `device_key` | `public_key` in tagged form, `key_hash` = SHA-256 of that text (32 bytes), certificate as signed, `key_seq`, `added_at`, `last_used_at`, `revoked_at` and the signed revocation; unique `(user_id, device_id)` and `(user_id, key_hash)` |
| `profile_card` | One row per user, the newest card as signed |
| `key_challenge` | Nonce, expiry, `used_at`; expired rows are deleted when new challenges are issued |
| `session.device_key_id` | The device a session belongs to; null for password sessions not bound to a device |

The plan called the log table `key_rotation`; it is `key_log_entry` because it also holds the
inception. The device key index is on `key_hash` rather than on the key text, so it stays 32
bytes per row whatever the algorithm (a hybrid key's text is close to the btree row limit).
Migration `key_identity_algorithm_agility` prefixed every stored key with `ed25519:` and
computed `key_hash` for existing rows; its `down` strips the prefix again and refuses to run
while a key of another algorithm is stored. Signed payloads are never rewritten. Keys live on the existing `user` row, so none of the tables that reference the user
id move.

## GraphQL

```graphql
type User { principalId: String … }            # null until the account enrolls a key; set at key-only signup
type DeviceKey { publicKey: String … }         # tagged: "ed25519:…"

input SignedStatementInput { payload: String!  signature: String! }
type SignedStatement { payload: String!  signature: String! }

input SignupWithKeyInput {
  username: String!
  displayName: String
  keyLog: [SignedStatementInput!]!
  device: SignedStatementInput!
  proof: SignedStatementInput!
  instanceInviteToken: String
  workspaceInviteToken: String
}

type Query {
  myKeyIdentity: KeyIdentity!
  keyLog(principalId: String!): [KeyLogEntry!]!   # public, no session; [] when not enrolled here
}
type Mutation {
  keyLoginChallenge: KeyLoginChallenge!
  loginWithKey(proof: SignedStatementInput!): LoginPayload!
  enrollIdentityKey(input: EnrollIdentityKeyInput!): KeyIdentity!
  signupWithKey(input: SignupWithKeyInput!): SignupWithInvitePayloadGql!   # { userId, serverId }
  addDeviceKey(certificate: SignedStatementInput!): DeviceKey!
  revokeDeviceKey(revocation: SignedStatementInput!): DeviceKey!
  rotateIdentityKey(input: RotateIdentityKeyInput!): KeyIdentity!
  publishProfileCard(card: SignedStatementInput!): ProfileCard!
  logoutOtherSessions: Int!                       # sessions ended; the caller's stays
}
```

Malformed or unverifiable statements fail with code `VALIDATION` and a short reason such as
`signature does not verify` or `key log entry has seq 2, expected 1`. A payload that is not JSON
of the statement's shape (unknown, duplicate or missing fields, wrong types) is refused with the
fixed reason `statement payload is not valid`; the JSON parser's own message goes to the server
log only.

`keyLog` needs no session. The log is public (every server a person uses holds it), and a person
who lost every device needs its head (`seq` and the hash of the last payload) to sign a recovery
rotation. It answers `[]` for a principal no account here has (and for anything longer than 64
characters, which is not a principal id). It is not rate-limited: unlike the anonymous rotation,
it checks no secret, and principal ids are 256-bit hashes that cannot be guessed, so answering
whether one is enrolled here tells nothing a holder of the id does not already know.

## Plugin principal ids: `rimp0_` to `rimp1_`

Plugins see `{ principalId, displayName }` (`docs/specs/plugin-protocol-v1.md`). Until now the
client computed a **provisional** id, `rimp0_` + base64url(SHA-256(
`"rim.principal.provisional.v1\0"` + the lowercased account UUID)), in
`rim-ui/src-tauri/src/plugin_host/protocol.rs`.

This PR adds `User.principalId`. The mapping happens **once, in the client**:

1. When the signed-in user's `principalId` turns non-null (after enrollment on this device, or
   on the first fetch on another device), the plugin host computes the account's old `rimp0_`
   id the same way as before and records `rimp0_… → rimp1_…` in its local store.
2. It sends running plugins `rim.principal.changed` with the new principal, plus the old-to-new
   mapping in `rim.principals.remapped` (plugin-protocol-v1, section 11), so a plugin can
   rewrite stored ids. It sends the mapping again after every handshake, so a plugin that was
   not running at the switch still gets it.
3. Other people's ids map the same way when their `User.principalId` is first seen non-null. An
   account that never enrolls keeps its `rimp0_` id.

The desktop client does this; the server only exposes `principalId`.

## Desktop client (rim-ui)

What the desktop app does with these statements. Other clients (the web app as a linked device,
the CLI) may choose differently, except for the recovery kit format, which every client that
offers recovery must read.

- **Where keys live.** Every secret goes through the app's one OS keyring helper
  (`src-tauri/src/secret_store.rs`), one entry per principal: the device key, and on the device
  that enrolled the identity, the identity key. The identity key stays on that device until device
  linking can move it off; the device key alone signs in. Keys never reach the webview: it asks the
  Tauri side for a signed statement and sends that.
- **Enrollment** (Settings > Security, signed in with a password): the app creates the identity,
  recovery and device keys, shows the recovery kit once, and only enrolls after the person
  confirms they saved it. The device and identity keys are written to the keyring **before**
  `enrollIdentityKey` is sent, so a crash in between never leaves an enrolled account without its
  keys; keys whose principal the server never accepted are removed. The recovery key is never
  stored. The app then publishes a profile card (display name, this server's URL, the active
  devices).
- **Key-only signup** (the join pages, next to the password form, with the page's token if it
  has one): the person picks a username, an optional display name and this device's name; the
  app creates the keys as for enrollment, shows the recovery kit once, and only sends
  `signupWithKey` after the person confirms they saved it. The keys are written to the keyring
  **before** it is sent. When it fails, the app reads `keyLog` for the new principal: an empty log
  means the server refused, so the keys are removed and the draft stays in memory, letting the
  person retry with the same keys (and the kit they saved) or discard them; a log that holds the
  principal means an attempt whose answer was lost created the account, so the app writes the
  draft's keys back if a failed attempt removed them and signs in with the device key (also when
  the person discards the draft); with no answer the keys stay for a later sign-in. On success the
  keys are confirmed and linked to the returned user id, the profile card is published, and the
  app opens the account as after a password signup (a workspace invite whose answer was lost
  opens the workspace its preview named). Unconfirmed keys whose principal has no key log on the
  server (a signup or enrollment that never reached it) are removed when signing in with them
  fails. The web app keeps password signup only.
- **Other sessions.** Right after an enrollment, Settings > Security offers "Sign out other
  sessions" (`logoutOtherSessions`); Account settings offers it for every account, on the
  desktop and the web.
- **Audience.** The `host[:port]` of the GraphQL endpoint URL as written, lowercased, the same
  rule the server applies to `RIM_PUBLIC_BASE_URL`.
- **Sign-in.** On a cold start whose session cookie is gone or refused, the app signs in with the
  device key of the account it was last signed in to. After an explicit sign-out it waits on the
  sign-in screen, which offers "Sign in with this device" for every account this device holds a
  key for on this server. A `FORBIDDEN` answer says the device was revoked; the
  sign-in screen then offers to remove that device's keys from this computer.
- **Statement version.** The app writes version 2 (tagged keys and signatures) and reads both.
  Keys and signatures are opaque text outside the one module that signs and verifies, so a hybrid
  algorithm is added there alone. A version 1 log (an identity enrolled by an older client) is
  extended in version 2 by its next rotation.
- **Identity rotation** keeps the recovery key and every device. The new identity key is stored as
  pending, with the `seq` its rotation creates, before `rotateIdentityKey` is sent. A verified log
  whose head names it promotes it; a log that already holds that `seq` with another key drops it;
  a log that ends before it (a stale copy, or the rotation still in flight) leaves it pending, and
  a retry signs the same pending key again, so a rotation whose answer was lost never loses the
  key it put on the server.
- **Identity key backup.** With the history archive set up, a device that holds the identity
  key keeps a copy of it in the archive, sealed under a backup key that only the recovery kit
  gives (`HKDF-SHA256(ikm = the phrase's 16 bytes, salt = "rim.recovery-phrase.v1",
  info = "identity backup key")`, or the same over a version 1 kit's 32-byte seed); the device
  keeps that backup key in its keyring record, never the phrase. It also wraps the archive key
  under the identity key. A linked device takes the identity key over in Settings > Security
  with the recovery phrase: the phrase must match the log's `recoveryKey` and the backed-up key
  the head's `identityKey`; nothing is signed or revoked. Format: rim-ui
  `docs/specs/history-archive-v1.md`, "Identity key backup".
- **Profile card.** Lists this server's URL as the one relay and the account's active devices. A
  device is listed only if its certificate verifies against the replayed log at its `keySeq`, so
  a server cannot add a key of its own to the card the identity key signs.
- **Enrollment in flight.** While `enrollIdentityKey` has not answered, the server saying "not
  enrolled" proves nothing, so nothing removes keys on that basis (closing Settings mid-way keeps
  them; a later check removes them if the enrollment never landed).

### Recovery kit

Shown once at enrollment, as text the person saves. Clients write version 2; readers accept
both versions, as a whole kit or as the bare secret alone (a 12-word phrase or a `RK1-` key),
and tell them apart by content, not by the header line. Either way the secret is a 32-byte seed
used as the Ed25519 recovery key (`SigningKey::from_bytes`); the key log's `recoveryKey` is its
public key. Nothing is migrated: an account enrolled with a version 1 kit recovers with its
`RK1-` key.

**Version 2, the recovery phrase.**

```
RIM recovery kit (v2)
Account id: rimp1_nz1POml8JmCMcFDECrtu60ibohPKnywhcExmOPMvTUo
Recovery phrase: word1 word2 … word12
```

- **Entropy**: 16 random bytes from the OS.
- **Phrase**: the BIP39 encoding of those bytes with the English word list: 128 bits plus a 4-bit
  checksum (the first 4 bits of SHA-256 of the entropy), 12 words of 11 bits each.
- **Seed**: `HKDF-SHA256(ikm = the 16 entropy bytes, salt = "rim.recovery-phrase.v1",
  info = "recovery key")`, expanded to 32 bytes. It is **not** BIP39's PBKDF2 seed: a passphrase
  and 2,048 rounds add nothing to 128 random bits.
- **Reading**: case, extra spaces and line breaks are ignored, and so is numbering in front of a
  word (`1.`, `1)`, or a number on its own, as copied from a numbered list). The kit's header
  and account id lines and the `Recovery phrase:` label are skipped. A phrase that is not 12
  words is refused; so is a word outside the list, with a message naming its position and the
  word as typed; so is a phrase whose checksum fails (most typos and swapped words).

Known-answer vector (never use it for a real account):

| Item | Value |
|---|---|
| Entropy | `000102030405060708090a0b0c0d0e0f` |
| Phrase | `abandon amount liar amount expire adjust cage candy arch gather drum buyer` |
| Recovery key | `ed25519:K2c3c55QDKnCUKWO8yTdZm25JIT20PKr5vaNC91F4cw` |

**Version 1, the recovery key.**

```
RIM recovery kit (v1)
Account id: rimp1_nz1POml8JmCMcFDECrtu60ibohPKnywhcExmOPMvTUo
Recovery key: RK1-…   (14 groups of 4 characters)
```

The recovery key is `RK1-` and the RFC 4648 base32 (uppercase, no padding) of the 32-byte
recovery seed followed by the first 3 bytes of `SHA-256("rim.recovery-kit.v1\0" || seed)`, in
groups of 4 joined by `-`. Readers ignore case, spaces and `-`, read `0` as `O` and `1` as `I`,
and refuse a key whose check bytes do not match. Version 2 replaced this format for new
enrollments; readers keep accepting it. A text holds a version 1 key when a line other than the
account id's has a word starting with `RK1`; a phrase never does.

**Recovering** (signed out, every device lost): the app reads the kit, fetches `keyLog` for the
account id, replays it, checks that the head's `recoveryKey` is the kit's, and signs a rotation
with `"signer": "recovery"` that keeps the same recovery key and names a new identity key, plus a
certificate for a new device at the new `seq`. It stores the new keys, sends `rotateIdentityKey`
without a session, and signs in with the new device key. Every other device and session of the
account is signed out by the server.

Whether a recovery landed is read from the key log, not from the error: when `rotateIdentityKey`
fails, the app fetches `keyLog` again. If the log ends at its rotation it signs in; if the log
answers without it, the attempt's keys are removed and any keys this device held for the account
before the attempt are put back; with no answer the keys stay for the next try.

## Deferred

- QR device linking (a new device gets its certificate from the old one through the server as a
  courier) is specified in `device-linking-v1.md`.
- Usernames stay unique across the whole table; rows of kind `remote` get an `r_` username
  (`server-to-server-v1.md`).

## Appendix A: known-answer vectors (version 1)

Checked by `rim_identity/src/vectors.rs`. Keys are `SigningKey::from_bytes([seed; 32])`; never
use these seeds for real keys.

| Item | Value |
|---|---|
| Identity key (seed 1) | `iojj3XQJ8ZX9UtstPLpdcspnCb8dlBIb83SIAbQPb1w` |
| Recovery key (seed 101) | `1i8Bah79Hk_feT60LNhEceG6nwzwTRKHtcxx9hYofLg` |
| Device key (seed 20) | `IIKL9cW9ystoSGMzbCAvtVmdpIvlWWYVdCFwcFvsqfc` |
| Principal id | `rimp1_nz1POml8JmCMcFDECrtu60ibohPKnywhcExmOPMvTUo` |
| `prev` for the first rotation | `o1oFYkMXfjtQJnG6as5GURR0cnzeYNYv5Br-LaeI78I` |

Inception payload and signature:

```
{"v":1,"type":"inception","seq":0,"identityKey":"iojj3XQJ8ZX9UtstPLpdcspnCb8dlBIb83SIAbQPb1w","recoveryKey":"1i8Bah79Hk_feT60LNhEceG6nwzwTRKHtcxx9hYofLg","createdAt":"2026-10-09T00:00:00Z"}
vf3zjxbHUJ8l9Oe6ayFGZZszVH1c_FVgO04F_4PqkfsYu9h8jAgbPKhAIAYHlQxg-7yAO5w1OEMmcAGcXptVDQ
```

Device certificate (device id `00000000-0000-0000-0000-000000000001`, name `Laptop`):

```
{"v":1,"type":"device","principalId":"rimp1_nz1POml8JmCMcFDECrtu60ibohPKnywhcExmOPMvTUo","deviceId":"00000000-0000-0000-0000-000000000001","deviceKey":"IIKL9cW9ystoSGMzbCAvtVmdpIvlWWYVdCFwcFvsqfc","name":"Laptop","keySeq":0,"issuedAt":"2026-10-09T00:00:00Z"}
tt6UNl3tRCwupWIYEfnWzqW9qVqDsJju2dA7d2CdNqdIZ0piVeP_Q6Y1lLfA4YN-26tsdaFea-rM_I8Qy9H6Cg
```

Login proof (challenge `00000000-0000-0000-0000-000000000009`, nonce `bm9uY2U`, audience
`localhost:8080`):

```
{"v":1,"type":"login","challengeId":"00000000-0000-0000-0000-000000000009","nonce":"bm9uY2U","audience":"localhost:8080","principalId":"rimp1_nz1POml8JmCMcFDECrtu60ibohPKnywhcExmOPMvTUo","deviceId":"00000000-0000-0000-0000-000000000001"}
jB_BFFAn7e-FMh8wCSWhnCcPzMjoe1fTKwlCoGfE0p1OYkhZWjV4J1c_AgeIhKpWOnLYyB0PqstAEYmaCITXBw
```

## Appendix B: known-answer vectors (version 2)

Checked by `rim_identity/src/vectors.rs`. Same keys and fields as appendix A, written as
version 2 statements. The principal id differs from appendix A because the inception payload
differs.

| Item | Value |
|---|---|
| Identity key (seed 1) | `ed25519:iojj3XQJ8ZX9UtstPLpdcspnCb8dlBIb83SIAbQPb1w` |
| Device key (seed 20) | `ed25519:IIKL9cW9ystoSGMzbCAvtVmdpIvlWWYVdCFwcFvsqfc` |
| Principal id | `rimp1_txDLbnipi5i3kCJtJwBOUixzv_MY8OnU0mcy--Tyje0` |

Inception payload and signature:

```
{"v":2,"type":"inception","seq":0,"identityKey":"ed25519:iojj3XQJ8ZX9UtstPLpdcspnCb8dlBIb83SIAbQPb1w","recoveryKey":"ed25519:1i8Bah79Hk_feT60LNhEceG6nwzwTRKHtcxx9hYofLg","createdAt":"2026-10-09T00:00:00Z"}
ed25519:kGvSf1hR28cdKP2Smwmy2BHEdP7wmKwy0At-DtrUs8mYhfzF0jjHTjpeX4DAzXv5HuCVwFEYWM1DjN-6wkU2Ag
```

Device certificate (device id `00000000-0000-0000-0000-000000000001`, name `Laptop`):

```
{"v":2,"type":"device","principalId":"rimp1_txDLbnipi5i3kCJtJwBOUixzv_MY8OnU0mcy--Tyje0","deviceId":"00000000-0000-0000-0000-000000000001","deviceKey":"ed25519:IIKL9cW9ystoSGMzbCAvtVmdpIvlWWYVdCFwcFvsqfc","name":"Laptop","keySeq":0,"issuedAt":"2026-10-09T00:00:00Z"}
ed25519:0HjRzn6gQ2q-vCHSkdOPK2ykGGBUpW2fYdDuEdTzf6OxsWzjXeNNpRSVgzPxOEfbYzZ9o9DWqp4NxcTEDhOaDA
```

Login proof (challenge `00000000-0000-0000-0000-000000000009`, nonce `bm9uY2U`, audience
`localhost:8080`):

```
{"v":2,"type":"login","challengeId":"00000000-0000-0000-0000-000000000009","nonce":"bm9uY2U","audience":"localhost:8080","principalId":"rimp1_txDLbnipi5i3kCJtJwBOUixzv_MY8OnU0mcy--Tyje0","deviceId":"00000000-0000-0000-0000-000000000001"}
ed25519:3FSH7M9uz0sP42GA0gKPso6JmGSx32Y1WZ-oKBW0VcVWANGEEjkDjaEtDpHNIjdEgC5fFA2jbfzSmuzhW7k1Dg
```

Mixed log: the appendix A log (version 1 inception) extended by a version 2 identity rotation
to the key from seed 2. Its `prev` is appendix A's; the principal id stays appendix A's.

```
{"v":2,"type":"rotate","principalId":"rimp1_nz1POml8JmCMcFDECrtu60ibohPKnywhcExmOPMvTUo","seq":1,"prev":"o1oFYkMXfjtQJnG6as5GURR0cnzeYNYv5Br-LaeI78I","identityKey":"ed25519:gTl3Dqh9F19Wo1Rmw0x-zMuNipG07jeiXfYPW4_Js5Q","recoveryKey":"ed25519:1i8Bah79Hk_feT60LNhEceG6nwzwTRKHtcxx9hYofLg","signer":"identity","createdAt":"2026-10-09T00:00:00Z"}
ed25519:D2moo9U9OjVL9zed4lM0A5cHoGQOVEtUfgKfAmOn0P18x74NEbh4FL4f3PbulQzoVuGAGOEJUZq3R2ZNpeUdCg
```
