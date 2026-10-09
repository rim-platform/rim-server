# Key identity v1 (Stage 3, Track 7.2, first server PR)

_Spec written 2026-10-09 with the first Stage 3 server PR. Implements the "keys" part of
`ROADMAP.md` Track 7.2 and the decisions in `docs/decisions/decentralization-2026-10-08.md`:
a user is a key pair, the user id is stable with a signed key log, the web app is a linked
device, and servers are interchangeable relays. This document is the contract between the
server (`rim_identity`, `KeyAuthService`) and every client that signs statements._

_Updated 2026-10-09 by the algorithm agility PR (`docs/decisions/post-quantum-hybrid-2026-10-09.md`):
statement format version 2 tags every key and signature with its algorithm, payload caps fit
hybrid post-quantum keys, and the server indexes device keys by a hash. Version 1 statements
stay valid. See "Algorithms and statement versions"._

## What this PR ships

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

Not in this PR (later Stage 3 PRs): key-only signup without a password or email, QR device
linking, the 12-word recovery phrase, the client UI, plugin manifest signing, and remote users.

## Keys and encodings

- All keys today are **Ed25519**. Key and signature bytes are **unpadded base64url** (RFC 4648
  §5, no `=`), written with an algorithm tag in front in version 2 statements (next section).
  Decoders are strict: padding, non-canonical trailing bits, wrong lengths, small-order
  ("weak") keys and non-canonical point encodings are rejected. Signatures are checked with
  `verify_strict`, so malleable signatures fail.
- A person holds three kinds of key:
  - the **identity key**, which signs the key log, device certificates, revocations and the
    profile card. It stays off day-to-day devices where possible;
  - the **recovery key**, kept offline (later: derived from the recovery phrase). It can
    replace a lost identity key;
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

### Device certificate

```json
{"v":2,"type":"device","principalId":"rimp1_…","deviceId":"<uuid>","deviceKey":"ed25519:…","name":"Laptop","keySeq":0,"issuedAt":"…"}
```

Signed by the identity key at `keySeq`, which must be the head of the log when the server
receives it. `deviceId` is chosen by the client (UUID v7 recommended). `name` is trimmed, 1 to
64 characters, no control characters. Per account, a `deviceId` and a `deviceKey` are each used
once, **revoked devices included**: a revoked device comes back only with a new id and key. A
key counts as used whichever version spelled it.

### Device revocation

```json
{"v":2,"type":"device-revoke","principalId":"rimp1_…","deviceId":"<uuid>","signer":"identity","issuedAt":"…"}
```

`"signer": "identity"` (current identity key, any device) or `"signer": "device"` (the device
being revoked signs its own retirement, so a device can sign itself out for good without the
identity key). Revoking is idempotent: the first signed revocation is kept.

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
   device, which is `FORBIDDEN` so the client can tell the person to link the device again.
   Failed attempts are limited to 20 per IP per 15 minutes; successful logins do not count.

**Accepted audiences** come from `RIM_KEY_LOGIN_AUDIENCES` (comma-separated `host[:port]`).
When it is unset, the host of `RIM_PUBLIC_BASE_URL` is used; when that is unset too,
`localhost:PORT` and `127.0.0.1:PORT` (development only). The server logs the list at start-up.
A host behind a proxy on a public name must set one of the two variables.

`keyLoginChallenge` and `loginWithKey` are exempt from the CSRF header check, because they are
used without a session; both only act on a fresh signature, which a forged cross-site request
cannot produce. `rotateIdentityKey` is exempt only when the request carries **no session
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

- An **identity rotation** needs a session of that account. Devices stay enrolled.
- A **recovery rotation** works **without a session** (the person lost every device). It must
  come with a certificate for a new device under the new identity key. The server appends the
  entry, **revokes every device and ends every session** of the account, then enrolls the new
  device, which can sign in right away. Anonymous attempts count against the same per-IP
  failure limit as logins, whether they succeed or not. An anonymous rotation for a principal
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
| `user.kind` | `local` (default) or `remote`; `remote` is reserved for Stage 5 |
| `user.principal_id`, `user.identity_key` | Null until enrollment, set together; `principal_id` unique; `identity_key` is the current identity key in tagged form |
| `user.home_relay` | Reserved for Stage 5 |
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
type User { principalId: String … }            # null until the account enrolls a key
type DeviceKey { publicKey: String … }         # tagged: "ed25519:…"

input SignedStatementInput { payload: String!  signature: String! }
type SignedStatement { payload: String!  signature: String! }

type Query { myKeyIdentity: KeyIdentity! }
type Mutation {
  keyLoginChallenge: KeyLoginChallenge!
  loginWithKey(proof: SignedStatementInput!): LoginPayload!
  enrollIdentityKey(input: EnrollIdentityKeyInput!): KeyIdentity!
  addDeviceKey(certificate: SignedStatementInput!): DeviceKey!
  revokeDeviceKey(revocation: SignedStatementInput!): DeviceKey!
  rotateIdentityKey(input: RotateIdentityKeyInput!): KeyIdentity!
  publishProfileCard(card: SignedStatementInput!): ProfileCard!
}
```

Malformed or unverifiable statements fail with code `VALIDATION` and a short reason such as
`signature does not verify` or `key log entry has seq 2, expected 1`.

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
   mapping notification promised in plugin-protocol-v1 ("Today's ids are provisional"), so a
   plugin can rewrite stored ids.
3. Other people's ids map the same way when their `User.principalId` is first seen non-null. An
   account that never enrolls keeps its `rimp0_` id.

The client work is a separate rim-ui PR; the server only exposes `principalId`.

## Deferred

- `user.password_hash` and `user.email` stay NOT NULL; key-only signup comes in a later PR.
- Usernames stay unique across the table; uniqueness among local users only, and rows of kind
  `remote`, come with Stage 5.
- Password sessions that existed before enrollment are not ended automatically. The client can
  offer "sign out other sessions" after enrolling.

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
