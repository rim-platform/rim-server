# Device linking v1 (Stage 3, S3-2)

_Spec written 2026-10-09 for the second Stage 3 server PR. Extends `docs/specs/key-identity-v1.md`
(read it first: keys, tagged encodings, device certificates and challenge login are defined
there). This document is the contract between the server (`DeviceLinkService`), the desktop
client and the later web client for giving a new device its own certified device key._

## Summary

1. The old device (signed in, holds the identity key) calls `startDeviceLink` and shows the code
   `rim-link:1:<linkId>:<secret>:<endpoint>` as a QR code and as text. The link lives 10 minutes.
2. The 32-byte secret never reaches the server. The server makes the 16-byte link id and stores
   only its SHA-256.
3. Channel: HKDF-SHA256(secret, salt = link id) with one `info` per direction, then AES-256-GCM
   with a random 96-bit nonce and AAD naming version, direction and link id.
4. The new device (no session) seals an **offer** (device id, tagged device key, name, platform,
   statement version) and sends it with `sendDeviceLinkOffer`. One offer per link.
5. The old device reads it through `deviceLink`, shows the name and an 8-character fingerprint the
   new device also shows, signs a certificate, calls `addDeviceKey`, sends a sealed **answer**
   (the certificate, nothing else) with `sendDeviceLinkAnswer`, then republishes its profile card.
6. The new device polls `deviceLinkAnswer`, verifies the certificate against `keyLog`, stores its
   keys, signs in with `keyLoginChallenge` + `loginWithKey`, and calls `completeDeviceLink`.
7. States: `open` → `offered` → `answered` → `consumed`; `cancelled` and expiry end any live state.
8. Anonymous calls answer `UNAUTHENTICATED` alike for every link they cannot act on, and are
   rate-limited per IP. The server stores sealed bytes and never interprets them.
9. Storage is a `device_link` table behind a repository trait, like `key_challenge`.
10. The answer carries nothing secret in v1, because anyone who saw the code can open it. The
    archive key and moving the identity key are deferred.

## What the channel protects

The server is a courier. It sees the link id, the sealed bytes and their sizes, and which
account started the link. It never sees the secret.

| Adversary | Can | Cannot |
|---|---|---|
| The server, or anyone who learns only the link id | Drop, delay or cancel a link; post a bogus offer (fails to open, link cancelled) | Read the offer or answer; substitute its own device key; turn an offer into an answer |
| Anyone who saw the code (QR photographed, text pasted somewhere) | Everything above; post an offer first; open the answer | Get a certificate unless the person confirms *their* offer on the old device |

The confirmation step on the old device is the defense against the second row. The channel is
symmetric only (AES-256 under a 256-bit secret), so there is no key agreement to record now and
break later; the hybrid post-quantum decision asks nothing of it.

## Flow

```
old device (session)                    server                    new device (no session)
startDeviceLink ───────────────────────► open
show code (QR + text)  · · · · · · · · · · · · · · · · · · · · · ► paste code
                                                    ◄─────────── sendDeviceLinkOffer(sealed offer)
deviceLink (poll) ◄──────────────────── offered                  show fingerprint, poll
open offer, person confirms name + fingerprint
addDeviceKey(certificate) ─────────────► device added
sendDeviceLinkAnswer(sealed answer) ───► answered
publishProfileCard (best effort)                   ────────────► deviceLinkAnswer (poll)
                                                                 open, verify against keyLog
                                                    ◄─────────── keyLoginChallenge, loginWithKey
deviceLink (poll) ◄──────────────────── consumed ◄────────────── completeDeviceLink (session)
```

Either side may call `cancelDeviceLink` at any point before `consumed`.

## Link code

```
rim-link:1:AAECAwQFBgcICQoLDA0ODw:ICEiIyQlJicoKSorLC0uLzAxMjM0NTY3ODk6Ozw9Pj8:http://localhost:8080/api
```

| Part | Value |
|---|---|
| Scheme | `rim-link:` (readers compare it ASCII case-insensitively; writers write lowercase) |
| Version | `1`, decimal. The version of everything in this document: offer, answer, `info`, AAD |
| Link id | 16 bytes, unpadded base64url, exactly 22 characters |
| Secret | 32 bytes from the OS random generator, unpadded base64url, exactly 43 characters |
| Endpoint | The old device's GraphQL endpoint as configured, trailing `/` removed |

Readers:

1. Remove every ASCII whitespace character (chat apps and mail wrap long lines).
2. Refuse anything longer than **334 characters** (78 fixed + a 256-byte endpoint).
3. Strip the scheme and split the rest at the **first three** `:` into version, link id, secret
   and endpoint (the endpoint keeps its own colons).
4. Refuse a version they do not implement with "this code was made by a newer app; update this
   device". Decode link id and secret strictly (no padding, canonical trailing bits, exact length).
5. The endpoint must start with `http://` or `https://`, be 1 to 256 bytes of printable ASCII
   (`0x21` to `0x7E`) and contain no `?`, `#` or `@`.

A QR code encodes exactly this string in byte mode, error correction level M. The example above
is 103 characters.

## Channel cryptography

### Keys

```
K_offer  = HKDF-SHA256(ikm = secret, salt = link id (16 raw bytes), info = "rim.device-link.v1 offer",  L = 32)
K_answer = HKDF-SHA256(ikm = secret, salt = link id (16 raw bytes), info = "rim.device-link.v1 answer", L = 32)
```

### Sealing

- **AES-256-GCM**, 96-bit nonce from the OS random generator, 128-bit tag. AES-GCM rather than
  XChaCha20-Poly1305 (which the desktop uses for the history archive) because WebCrypto has
  AES-GCM and no ChaCha20, and the web client must seal and open with non-extractable keys.
- **AAD** is ASCII `rim.device-link.v1\0<direction>\0<link id text>`, where `<direction>` is
  `offer` or `answer` and `<link id text>` is the 22-character base64url form. Distinct keys and
  distinct AAD mean an offer never opens as an answer and a message never moves to another link.
- **Each side seals exactly one plaintext per link.** A retry resends the same sealed bytes.
  With at most one message per key, a random nonce cannot repeat under a key.

### Sealed message

On the wire, like `SignedStatementInput`:

```json
{ "nonce": "QEFCQ0RFRkdISUpL", "ciphertext": "J8-HezTgXKj9…" }
```

`nonce` is the 12-byte nonce, `ciphertext` is the AES-GCM output **with the 16-byte tag
appended** (what WebCrypto `encrypt` returns), both unpadded base64url.

| | Max plaintext | Max `ciphertext` bytes |
|---|---|---|
| Offer | 16 KiB | 16,400 |
| Answer | 64 KiB | 65,552 |

Sized for hybrid keys, as in key-identity: the offer holds a tagged ML-DSA-87 hybrid key (about
3.5 KB) several times over; the answer holds a 16 KiB certificate payload escaped in JSON (at
most twice its size) plus a hybrid signature (about 6.3 KB for ML-DSA-87).

### Plaintext rules

The plaintext is UTF-8 JSON with `"v"` (the code version, `1`) and `"type"`. The rules of
key-identity statements apply: unknown fields, duplicate fields and a wrong `type` are refused
(a browser's `JSON.parse` cannot see duplicates; that is acceptable there, since the AEAD already
proves the writer knew the secret). A plaintext that fails these rules after it opened is treated
exactly like one that failed to open.

**Offer** (`type` `link-offer`), sealed by the new device:

```json
{"v":1,"type":"link-offer","deviceId":"<uuid>","deviceKey":"ed25519:…","name":"Laptop","platform":"desktop","statementVersion":2}
```

| Field | Rule |
|---|---|
| `deviceId` | A UUID, lowercase with hyphens, chosen by the new device (v7 recommended) |
| `deviceKey` | The new device key in tagged (version 2) form |
| `name` | The certificate name rules: trimmed, 1 to 64 characters, no control characters |
| `platform` | `desktop` or `web`. Self-reported, so a display hint only |
| `statementVersion` | The highest key-identity statement version the new device reads; at least 2 |

There is no signature by the device key in the offer. The first `loginWithKey` proves the new
device holds it, and a certificate for a key nobody holds signs nothing in.

**Answer** (`type` `link-answer`), sealed by the old device:

```json
{"v":1,"type":"link-answer","certificate":{"payload":"{\"v\":2,\"type\":\"device\",…}","signature":"ed25519:…"}}
```

`certificate` is the device certificate exactly as sent to `addDeviceKey`. The old device writes
it in statement version min(its own write version, the offer's `statementVersion`): 2 today.

### Fingerprint

```
fingerprint = base32(first 5 bytes of SHA-256("rim.device-link.v1 fingerprint\0" || offer plaintext bytes))
```

RFC 4648 base32, uppercase, no padding: 8 characters, shown as `XXXX-XXXX`. The new device
computes it over the bytes it sealed and the old device over the bytes it opened, so it binds the
whole offer (id, key, name, platform). Both screens show it; the person compares them.

## Server rules

### Calls

| Call | Caller | Transition | Repeats |
|---|---|---|---|
| `startDeviceLink` | Session; account has a key identity | → `open` | Each call is a new link; the user's previous live link is cancelled |
| `sendDeviceLinkOffer` | Anyone with the link id | `open` → `offered` | Same bytes while `offered`: success. Other bytes: refused |
| `deviceLink` | Session of the link's user | none | Returns state, expiry, and the offer while `offered` or `answered` |
| `sendDeviceLinkAnswer` | Session of the link's user | `offered` → `answered`; expiry set to now + 10 min | Same bytes while `answered`: success |
| `deviceLinkAnswer` | Anyone with the link id | none | `null` while `open` or `offered`; the answer while `answered` |
| `completeDeviceLink` | Session of the link's user | `answered` → `consumed` | `true` again while `consumed` |
| `cancelDeviceLink` | Anyone with the link id | `open`, `offered`, `answered` → `cancelled` | See errors |

"Live" means `open`, `offered` or `answered` and not expired. Any session of the account may start
a link; only a device holding the identity key can finish one, which the server cannot check and
does not need to. `completeDeviceLink` is called by the new device right after `loginWithKey`, so
"the link's user" is the same account. `consumed` and `cancelled` set `ended_at` and clear the
sealed columns.

Every transition is one conditional `UPDATE … WHERE link_hash = $1 AND state = <from> AND
expires_at > now() RETURNING …` (plus `AND user_id = $2` for session calls), so two racing calls
cannot both succeed and a call that arrives after expiry loses. A zero-row update is then
classified (same bytes repeated, wrong state, expired, unknown) by one read.

### Errors

| Call | Situation | Code |
|---|---|---|
| Any | Over a rate limit | `RATE_LIMITED` |
| `sendDeviceLinkOffer`, `sendDeviceLinkAnswer` | `nonce` not 12 bytes, `ciphertext` under 17 bytes or over its cap, not strict base64url (checked before the link is looked up) | `VALIDATION` |
| Anonymous calls | Link id malformed or unknown; link expired, cancelled or consumed; offer slot holds other bytes | `UNAUTHENTICATED`, no detail |
| Session calls | No session | `UNAUTHENTICATED` |
| Session calls | Link id malformed or unknown, or the link belongs to another user | `NOT_FOUND` |
| `startDeviceLink` | Account has no key identity | `VALIDATION` (`this account has no key identity yet`) |
| `sendDeviceLinkAnswer`, `completeDeviceLink` | Wrong state, expired, or other bytes already stored | `CONFLICT` |
| `cancelDeviceLink` by the link's user | Already `consumed` | `CONFLICT` |

`cancelDeviceLink` with a session of the link's user follows the session rules, and answers
`true` for a link already cancelled or expired; any other caller follows the anonymous rules.
`deviceLink` reports an expired live link as `EXPIRED` until the row is cleaned up, then
`NOT_FOUND`.

### Link ids

The server generates 16 bytes from the OS random generator (128 bits, unguessable, so the uniform
`UNAUTHENTICATED` is about not leaking state, not about stopping guessing). It stores
`link_hash = SHA-256(the 16 bytes)`, like session tokens and unlike the challenge nonce, because
the link id alone is a bearer capability (read the answer, post an offer, cancel) and a database
read must not hand those out. Rows have their own UUID v7 `id`; audit logs carry that and the user
id, never the link id or sealed bytes (events `device_link_started`, `_offered`, `_answered`,
`_completed`, `_cancelled`).

### Sealed bytes

The server checks only the encoding and the sizes above, stores the decoded bytes, and returns
them re-encoded (byte-identical, since decoding was strict). It never decrypts, parses or
inspects them, and has no key that could.

### Limits

| What | Limit | Key |
|---|---|---|
| `startDeviceLink` | 10 per hour | user |
| Live links | 1; starting a new one cancels the previous | user |
| `sendDeviceLinkOffer` + anonymous `cancelDeviceLink` | 30 per 15 minutes | IP |
| `deviceLinkAnswer` | 300 per 5 minutes (a poll every 2 s is 150) | IP |
| Anonymous refusals (`UNAUTHENTICATED`) | 20 per 15 minutes, a slot reserved up front and refunded on success, as for key login | IP |

The refusal limiter is separate from key login's, so a botched link does not lock the same
network out of `loginWithKey`. Limiters are the in-memory `RegistrationRateLimiter`, per node;
correctness never depends on them.

### Expiry and cleanup

- A link expires 10 minutes after `startDeviceLink`. `sendDeviceLinkAnswer` resets expiry to
  10 minutes after the answer, so a slow new device still has time to fetch it.
- Expired rows are never returned with sealed bytes. Every `startDeviceLink` deletes, best
  effort, rows whose `expires_at` is more than one hour past; the hour lets the old device read
  `CONSUMED`, `CANCELLED` or `EXPIRED` after the fact.

## Storage

A table, behind `DeviceLinkRepository` (rim_core), not an in-memory store. The two devices'
requests may reach different nodes, and the roadmap guardrail forbids features that depend on
single-node semantics; `key_challenge` is a table for the same reason. The trait is the seam if a
deployment ever wants another store. Migration `add_device_link` (generated with
`sea migrate generate add_device_link`):

```sql
CREATE TABLE device_link (
    id uuid PRIMARY KEY,
    link_hash bytea NOT NULL,
    user_id uuid NOT NULL,
    state text NOT NULL DEFAULT 'open',
    offer_nonce bytea NULL,
    offer_ciphertext bytea NULL,
    answer_nonce bytea NULL,
    answer_ciphertext bytea NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    expires_at timestamptz NOT NULL,
    offered_at timestamptz NULL,
    answered_at timestamptz NULL,
    ended_at timestamptz NULL,
    CONSTRAINT uq_device_link_link_hash UNIQUE (link_hash),
    CONSTRAINT chk_device_link_link_hash CHECK (octet_length(link_hash) = 32),
    CONSTRAINT chk_device_link_state
        CHECK (state IN ('open', 'offered', 'answered', 'consumed', 'cancelled')),
    CONSTRAINT chk_device_link_offer CHECK ((offer_nonce IS NULL) = (offer_ciphertext IS NULL)),
    CONSTRAINT chk_device_link_answer CHECK ((answer_nonce IS NULL) = (answer_ciphertext IS NULL)),
    CONSTRAINT fk_device_link_user FOREIGN KEY (user_id)
        REFERENCES "user" (id) ON DELETE CASCADE ON UPDATE CASCADE
);
CREATE INDEX idx_device_link_expires_at ON device_link (expires_at);
CREATE UNIQUE INDEX uq_device_link_live_user ON device_link (user_id)
    WHERE state IN ('open', 'offered', 'answered');
```

`startDeviceLink` locks the user row, sets every live-state row of the user to `cancelled` (expired
ones included, so the partial index never blocks), and inserts the new row in one transaction.

**Rollback.** `down` runs `DROP INDEX uq_device_link_live_user`, `DROP INDEX
idx_device_link_expires_at`, `DROP TABLE device_link`. Nothing references the table, and what it
loses is links at most 20 minutes old: a person mid-link starts again. Devices already added stay
in `device_key`.

Code: a `device_link` module in `rim_core`, `rim_services`, `rim_infra_services` and
`rim_infra_db`. `DeviceLinkService` does not depend on `KeyAuthService`; it reads only
`user.principal_id`.

## GraphQL

```graphql
input SealedMessageInput { nonce: String!  ciphertext: String! }
type SealedMessage { nonce: String!  ciphertext: String! }

enum DeviceLinkState { OPEN OFFERED ANSWERED CONSUMED CANCELLED EXPIRED }
type StartedDeviceLink { linkId: String!  expiresAt: DateTime! }
type DeviceLink { state: DeviceLinkState!  expiresAt: DateTime!  offer: SealedMessage }

type Query {
  deviceLink(linkId: String!): DeviceLink!               # session; the link's user
  deviceLinkAnswer(linkId: String!): SealedMessage       # no session; null until answered
}
type Mutation {
  startDeviceLink: StartedDeviceLink!                                              # session
  sendDeviceLinkOffer(linkId: String!, offer: SealedMessageInput!): Boolean!       # no session
  sendDeviceLinkAnswer(linkId: String!, answer: SealedMessageInput!): DeviceLink!  # session
  completeDeviceLink(linkId: String!): Boolean!                                    # session
  cancelDeviceLink(linkId: String!): Boolean!                                      # either side
}
```

- **CSRF.** `sendDeviceLinkOffer` joins `CSRF_EXEMPT_MUTATIONS`: it is used without a session
  and acts only on a capability a cross-site page does not have. `cancelDeviceLink` joins
  `CSRF_EXEMPT_WITHOUT_SESSION`: with a session cookie it may act as the link's user, so it needs
  the header. The other mutations need it as usual.
- **SDL order.** The resolvers are new `DeviceLinkQueries` / `DeviceLinkMutations` objects
  appended to the last query and mutation groups.
- **Polling.** Clients poll every 2 seconds for the first minute, then every 5 seconds. A later
  `deviceLinkChanged(linkId)` subscription over the existing graphql-ws socket can replace both
  polls; the anonymous side needs per-connection limits on that socket first.

## Client rules

### Old device (desktop)

1. Linking is offered only when this device holds the head's identity key
   (`LogStatus.holdsIdentityKey`). It generates the secret, calls `startDeviceLink`, and shows the
   QR code, the text code with a copy button, and the time left. The secret stays in memory.
2. When `deviceLink` says `OFFERED`, it opens the offer. A failure to open or parse means someone
   other than the intended device used the code: it calls `cancelDeviceLink` and says so.
3. Before asking the person, it fetches `myKeyIdentity`, replays the log, and refuses (and
   cancels) an offer whose `deviceKey` is malformed, weak, of an algorithm it cannot verify, equal
   to the identity or recovery key, or used by any device of the account (revoked included); whose
   `deviceId` is not a UUID or is used by any device of the account; or whose `name` breaks the
   certificate rules.
4. It shows the name (editable before signing), the platform, and the fingerprint, and asks the
   person to compare the fingerprint with the new device's screen. For `web` it says that keys in
   a browser are weaker than on a desktop. Declining cancels the link.
5. On confirmation it writes a **pending link** record to the keyring (link id, secret, device
   id, certificate), signs the certificate at the head `keySeq`, and calls `addDeviceKey`. A
   `CONFLICT` whose device list shows this device id with this key (a retry after a lost answer)
   continues with the stored certificate; any other refusal cancels the link.
6. It seals the answer, calls `sendDeviceLinkAnswer`, and only then republishes the profile card
   (best effort, as after enrollment), so a card failure never holds up the new device.
7. It polls until `CONSUMED`, says the device is linked, and deletes the pending record. If the
   link ends `CANCELLED` or `EXPIRED` after the device was added and the device's `lastUsedAt` is
   null, it revokes the device with the identity key, so no certified key is left that nobody
   used.
8. On start-up with a pending record: `OFFERED` sends the answer again (adding the device first
   if the device list lacks it), `ANSWERED` keeps polling, anything else applies rule 7.

### New device (desktop)

1. Reads the pasted code (desktops do not scan QR codes in v1) and refuses an endpoint that is
   not the app's configured one (compared as the key store does: trimmed, trailing `/` ignored).
2. Creates the device key and a UUID v7 device id **in memory** and seals the offer. It writes
   nothing to the keyring until the answer verifies, so an abandoned link leaves no keys behind.
3. Sends the offer and shows the fingerprint with "Confirm this code on your other device".
   `UNAUTHENTICATED` means the code expired, was cancelled, or another device used it first; the
   app says so and suggests cancelling on the other device and starting again.
4. Polls `deviceLinkAnswer`. `UNAUTHENTICATED` means the link ended: the keys are dropped.
5. Opens the answer and checks that the certificate is a `device` statement naming exactly the
   offer's `deviceId` and `deviceKey`. It fetches `keyLog(principalId)` from the endpoint, replays
   it, and verifies the certificate against the identity key at its `keySeq`
   (`certified_device`). Any failure cancels the link and drops the keys.
6. Stores the device key, id and the certificate's name for the principal (no identity key,
   account link with user id unknown, as after a recovery), signs in with
   `keyLoginChallenge` + `loginWithKey` (audience from the endpoint), settles the keys, and calls
   `completeDeviceLink`. A failed `completeDeviceLink` is not retried beyond once: the device
   works, and the old device keeps it because `lastUsedAt` is set.
7. A `loginWithKey` refusal of `FORBIDDEN` (the old device revoked it meanwhile) removes the keys;
   other failures keep them for the next sign-in.

### Web client

Same as the new desktop device, except:

- It only takes a pasted code, and only for its own GraphQL endpoint (cookies and CORS would fail
  for another).
- The device key is a WebCrypto Ed25519 key generated with `extractable: false` and kept as a
  `CryptoKey` in IndexedDB; the public key is exported raw and tagged `ed25519:`. A browser
  without Ed25519 in WebCrypto cannot be linked; there is no fallback to extractable keys.
- HKDF and AES-GCM run in SubtleCrypto: `importKey("raw", secret, "HKDF")`,
  `deriveKey({ name: "HKDF", hash: "SHA-256", salt, info }, …, { name: "AES-GCM", length: 256 },
  false, ["encrypt", "decrypt"])`, then `encrypt({ name: "AES-GCM", iv, additionalData,
  tagLength: 128 })`, whose output is already ciphertext followed by tag.
- `platform` is `web`. The secret lives only in the page; a reload abandons the attempt.
- The web client is never the old device in v1: it holds no identity key.

## Failure and retry

| Situation | What happens |
|---|---|
| Answer to `sendDeviceLinkOffer` lost | The new device resends the same bytes: success |
| Answer to `deviceLinkAnswer` lost | It polls again; the answer stays for 10 minutes after it was sent |
| Old device crashes after `addDeviceKey`, before the answer | On restart, its pending record resumes the link (old device rule 8). If the link expired first, the new device sees `UNAUTHENTICATED`, drops its keys, and the person starts a new link, for which the new device makes **a new device id and key**: the old pair is used on the account for good. The old device revokes the unused one (rule 7) |
| New device crashes or the page reloads | Its keys were only in memory. The person starts again; the old device revokes the unused device |
| Wrong secret (corrupted code) | The offer fails to open; the old device cancels; the new device's poll gets `UNAUTHENTICATED`; the person starts again |
| Bogus offer from someone without the secret (the server, a log reader) | Fails to open; the link is cancelled; nothing is signed |
| Someone who saw the code posts first | The real device's offer is refused and it says so; the old device shows a name and fingerprint that match nothing on the person's new device, and the person declines. If they confirm anyway, that party gets a device of the account, as with any confirmed link |
| Someone who saw the code opens the answer | They get the certificate, which is public anyway (`myKeyIdentity`, the profile card). Without the device key it signs nothing in |
| Malicious server | Can drop, delay, replay or cancel. Cannot open or forge sealed messages, move them between links or directions, or forge a key log, so a substituted certificate fails the `keyLog` check |
| Person declines on the old device | `cancelDeviceLink`; the new device's poll ends with `UNAUTHENTICATED` |

## Deferred

- **The archive key in the answer.** It would let the new device read the drive archive without
  the passphrase, but the answer key is known to anyone who saw the code, so a secret in the
  answer must be encrypted to a key only the new device holds: an ephemeral key-agreement key in
  the offer, which the post-quantum decision makes hybrid (X25519 + ML-KEM-768). The archive
  format already reserves the mechanism (a new `keyWraps` kind, `history-archive-v1.md`). A later
  code version adds both; until then the new device unlocks the archive with the passphrase, and
  it needs the folder or drive sign-in in any case.
- **Moving the identity key** off the enrolling device, or holding it offline.
- **Short typed codes** with a PAKE (CPace, SPAKE2), so a code can be read aloud.
- **QR scanning**, with the mobile PWA; the code format is fixed now so the QR works then.
- **A subscription** in place of polling (see GraphQL).
- **Linking across servers** (a code whose endpoint is not the new device's server): Stage 5.
- Rate limits shared across nodes.

## Test plan

Server (this PR):

- `rim_identity::device_link`, the reference implementation of this document (code parse and
  format, key derivation, seal, open, fingerprint, offer and answer types). The service never
  calls it; tests and clients do. Its tests check appendix A byte for byte, that an offer does
  not open as an answer or under another link id, that a flipped AAD, nonce or tag byte fails,
  and the code parser's edge cases (whitespace, uppercase scheme, version 2, 335 characters,
  padded or 21-character link id, `?`, `#` or `@` in the endpoint, colons in the endpoint).
- Unit tests of `DefaultDeviceLinkService` with a mock repository: every row of the calls and
  errors tables, idempotent repeats, expiry at the boundary, answer resetting expiry, a second
  `startDeviceLink` cancelling the first, the per-user and per-IP limiters (injected small, like
  `with_rate_limiters`), refunds on success.
- `rim_infra_db` (`db-tests`): two concurrent offers on one link, exactly one wins; transitions
  refuse expired rows; cleanup deletes only rows past one hour; terminal states clear sealed
  columns; the partial unique index; cascade on user delete; migration up and down.
- `rim_api/tests/device_link_e2e.rs` (`e2e-tests`), with real sealed messages from
  `rim_identity::device_link`: the whole flow from `startDeviceLink` to `loginWithKey` with the
  linked device and `CONSUMED` on the old side; anonymous calls on unknown, malformed, expired,
  cancelled and consumed links all `UNAUTHENTICATED` with identical bodies; another user's link
  `NOT_FOUND`; answer before offer `CONFLICT`; random ciphertext stored and returned byte-exact;
  size and encoding `VALIDATION`; `startDeviceLink` without a key identity.
- `graphql_support`: `sendDeviceLinkOffer` exempt from CSRF; `cancelDeviceLink` exempt only
  without a session cookie; the other mutations need the header.

Clients (rim-ui):

- Appendix A in the Tauri module and, for the web client, in Vitest through Node's WebCrypto.
- Old device: each refusal in rule 3; confirmation declined cancels; a lost `addDeviceKey` answer
  continues with the stored certificate; restart with a pending record in each state; an
  unused device revoked after `EXPIRED`.
- New device: a certificate for another key, another device id, or one that does not verify
  against the served log is refused and the link cancelled; nothing is in the keyring before the
  answer verifies; `FORBIDDEN` at sign-in removes the keys.

## Appendix A: known-answer vectors

Computed with Node's `crypto` and cross-checked in Python (HKDF and SHA-256 with `hmac` and
`hashlib`, AES-GCM with `cryptography`).
The new device is appendix B's device of `key-identity-v1.md` (seed 20, device id
`…0001`, name `Laptop`), and the answer carries appendix B's certificate unchanged. Never use
these values for real links.

| Item | Value |
|---|---|
| Link id (bytes `00`..`0f`) | `AAECAwQFBgcICQoLDA0ODw` |
| Secret (bytes `20`..`3f`) | `ICEiIyQlJicoKSorLC0uLzAxMjM0NTY3ODk6Ozw9Pj8` |
| Code (endpoint `http://localhost:8080/api`) | `rim-link:1:AAECAwQFBgcICQoLDA0ODw:ICEiIyQlJicoKSorLC0uLzAxMjM0NTY3ODk6Ozw9Pj8:http://localhost:8080/api` |
| `K_offer` (hex) | `6dff67bf777c3cf073ebc0f8810dae7535d3bdb59c9178183441db3768212fba` |
| `K_answer` (hex) | `d6ec24d3f4380cb2ca50865f95a37f3e6b0ab9c2d74ee31d605768abe6bf56e1` |
| Offer AAD (hex) | `72696d2e6465766963652d6c696e6b2e7631006f666665720041414543417751464267634943516f4c4441304f4477` |
| Answer AAD (hex) | `72696d2e6465766963652d6c696e6b2e763100616e737765720041414543417751464267634943516f4c4441304f4477` |
| Offer nonce (bytes `40`..`4b`) | `QEFCQ0RFRkdISUpL` |
| Answer nonce (bytes `50`..`5b`) | `UFFSU1RVVldYWVpb` |
| Fingerprint | `QSFX-VHMA` (SHA-256 `848b7a9d80a01b87adceab1630b57a452359598c53e4326382d7a236e3ca8629`) |

Offer plaintext (201 bytes) and its `ciphertext` (217 bytes, tag `4e654172510e4140794b15c8b0ed70b1`):

```
{"v":1,"type":"link-offer","deviceId":"00000000-0000-0000-0000-000000000001","deviceKey":"ed25519:IIKL9cW9ystoSGMzbCAvtVmdpIvlWWYVdCFwcFvsqfc","name":"Laptop","platform":"desktop","statementVersion":2}
J8-HezTgXKj9OyT7HMBA1zjjB4ZTJEJkqjh4skQfSya0Sxe6uk9qWLSzUqUAQ7QrC4ZIEYlO_dsskNkfhQ_iEVXyyUkGmH2j1gqSNpQTetjmsgmQEXUbEsSs8ciJPmD_o_T8zR_R7k-rjthHmMQYHVfwslbojzQhR7wXQiUJfZtIDm5bfx9wYmPPjvmcA9RYQnV6_3GRGoFbsLE9fGt4mRlidnXDiOweblWqdi0dARTNJHJL7DDAMwmqGHiEIcAVTVu7B4SiHFPrTmVBclEOQUB5SxXIsO1wsQ
```

Answer plaintext (460 bytes) and its `ciphertext` (476 bytes, tag `54f5a552761c925f6e4ac7ac5d45abb7`):

```
{"v":1,"type":"link-answer","certificate":{"payload":"{\"v\":2,\"type\":\"device\",\"principalId\":\"rimp1_txDLbnipi5i3kCJtJwBOUixzv_MY8OnU0mcy--Tyje0\",\"deviceId\":\"00000000-0000-0000-0000-000000000001\",\"deviceKey\":\"ed25519:IIKL9cW9ystoSGMzbCAvtVmdpIvlWWYVdCFwcFvsqfc\",\"name\":\"Laptop\",\"keySeq\":0,\"issuedAt\":\"2026-10-09T00:00:00Z\"}","signature":"ed25519:0HjRzn6gQ2q-vCHSkdOPK2ykGGBUpW2fYdDuEdTzf6OxsWzjXeNNpRSVgzPxOEfbYzZ9o9DWqp4NxcTEDhOaDA"}}
vkDrDIh3nkBRDHroeH_tmxnHXtoMzfo4grqtAiBG46z7xMsAlJDtvvExm51M7w7G7oglt9YXamu3_qjKYYDPA7b_xGvbPvWGkw98Hc91oL2qQvVYz0MSwIWu5R_FyFwN0SIVdph0xJxLBdXq3E3-iKFd4fdfLRdZvRi7qMMTY7wW8DMw2ItevB1tV2xfMF2_WpZNSUvPuJUF9l8VIdtiIHO1V94Sx-ombzLAvfndFHJfxKhsDwxPB9YSaKoaIvKPZrI_6Z0Mzwdn7RAyepsgjrOz4wDnoEWv5eFOO1zU4RpZvRwOmlhJ2NKwLsG87fEtw16kB3g2qMqvwGLWrQ4VuqygY3JucPE0Gu2H-dkxLEYrEuPMbGxMzD2ROIAlZFKljdgj8zGLrYbkLml1Jh-rJijDswN0ktaw-dCCqd0-Pbt4nXvBKGTV5ZuDJ1W-nIMYs9Py81klr5gBOypP8A3In4OFi7ePsu2FOnZE-LRYfaLeYv2PCPKCFaZnCjJS0asVa-UFXHhmz2sWxVwEi1OnENi11C4bweM6a_9ydUHi4w4QMBkZpfHCyondEdgCtxyq8xZXPsEP6_JX007Y9jm9gGQ8JExrsviYRMpeWFT1pVJ2HJJfbkrHrF1Fq7c
```

The plaintexts are written with the fields in the order shown and no spaces, which is what makes
these bytes reproducible; readers do not depend on field order.
