# Outside cryptographic review: brief for Stages 3 and 4

## Status as of 2026-10-14

This brief predates several fixes, so its numbered items in section 6 are not all open. Review
them against the release commits, not the commits in 1.1. These are fixed:

| Item | Fix | Where |
|---|---|---|
| 1. Client encryption pinning | A device pins a conversation it holds group state for as encrypted and refuses every plaintext write to it | rim-ui PR #47 |
| 2. Remembered revocations | Each client keeps the devices it has seen revoked, so a revocation the server later omits no longer gets through | rim-ui PR #47 |
| 7. Server franking key handling | The franking keys live in a key file, not the database, and a report is checked against the row's stored commitment | rim-server PR #74 |
| 11. Key-log head pinning | Each client keeps the head of every principal's longest verified key log and refuses a shorter or forked one | rim-ui PR #47 |
| 14. Login validation order | The challenge and audience are checked before a revoked device is answered with `FORBIDDEN` | rim-server PR #74 |
| 15. Public-bind audience checks | The server refuses to start on a public bind without a key login audience | rim-server PR #74 |

The same two pull requests also change code behind other items (for example 3 to 6, 9, 10, 20
and 22); check each against the code before treating it as open.

_Written 2026-10-09 for the outside review that `0.1.0` and any production-ready claim wait for
(`ROADMAP.md`, "Where it stands").
It covers key identity, device linking, recovery, the desktop secret vault and the browser
device key (Stage 3), and end-to-end encryption of DMs and groups with MLS (Stage 4). It points
into the specs instead of restating them; where the code and a spec differ, this brief says so
and names the line._

## 1. Scope and goals

### 1.1 What was reviewed, at which commit

Every line number in this brief refers to these commits. Both are the `master` of their
repository on the day this brief was written.

| Repository | Commit | Contents |
|---|---|---|
| `github.com/rim-platform/rim-server` | `efdd71d3ce0ac22d1118d8f0a1d09109f2809b9c` | Server, the shared verifier crate `rim_identity`, the specs and decisions |
| `github.com/rim-platform/rim-ui` | `c4aadbf9933c2c1a94ccaffa25601ae8985f2d16` | Desktop app (Tauri, Rust), the `rim_mls` crate and its wasm build, the web app (TypeScript) |

Paths below are relative to the repository named in the row or section. "server" means
rim-server, "ui" means rim-ui, and "ui/tauri" means `rim-ui/src-tauri`.

### 1.2 In scope

| Area | Specs and decisions (server `docs/`) | Code |
|---|---|---|
| Signed statements, key log, certificates, profile cards, login proofs | `specs/key-identity-v1.md` | server `rim_identity/src/{statement,encoding,key_log,device,login,profile_card}.rs`, `rim_infra_services/src/key_auth.rs`; ui/tauri `src/key_identity/{statement,mod,store,commands}.rs`; ui `src/features/key-identity/browser/statements.ts` |
| Plugin manifest signatures | `specs/workspace-plugins-v1.md`, `specs/web-plugins-v1.md` | server `rim_identity/src/plugin_manifest.rs`; ui/tauri `src/key_identity/plugin_manifest.rs`; ui `src/features/key-identity/browser/pluginManifest.ts` |
| Device linking | `specs/device-linking-v1.md` | server `rim_identity/src/device_link.rs` (reference implementation), `rim_infra_services/src/device_link.rs` (courier); ui/tauri `src/key_identity/device_link.rs`; ui `src/features/key-identity/browser/{channel,browserKeyBackend,keyStore}.ts` |
| Recovery phrase, identity-key backup, archive key wraps | `specs/key-identity-v1.md` ("Recovery kit", "Identity key backup"); ui `docs/specs/history-archive-v1.md` ("Keys", "Sealed files", "Identity key backup") | ui/tauri `src/key_identity/{recovery_kit,backup}.rs`, `src/identity_backup/`, `src/history_archive/{crypto,identity}.rs` |
| Desktop secret vault | ui `docs/secret-vault.md` | ui/tauri `src/secret_store.rs`, `src/message_cache/key.rs` |
| Browser device key | ui `docs/web-linked-device.md` | ui `src/features/key-identity/browser/`, `src/core/runtime/ed25519.ts` |
| MLS end to end encryption | `specs/e2ee-mls-v1.md`, `decisions/mls-library-2026-10-09.md`, `decisions/post-quantum-hybrid-2026-10-09.md` | server `rim_identity/src/mls.rs`, `rim_infra_services/src/e2ee/{mod,aad,franking,openmls_wire}.rs`, `rim_infra_services/src/message.rs` (encrypted send and edit), `rim_api/src/mutation/e2ee.rs`, `rim_services/src/e2ee.rs`; ui/tauri `crates/rim_mls/`, `crates/rim_mls_wasm/`, `src/e2ee/`; ui `src/features/e2ee/{runtime,web,hooks}/` |
| Plugins' access to decrypted content | `specs/e2ee-mls-v1.md` ("Plugins and the client message service") | ui/tauri `src/message_cache/plugin_view.rs`, `src/plugin_host/broker.rs`; ui `src/features/plugins/chat/`, `src/features/e2ee/web/pluginView.ts` |
| Supporting primitives on the server | — | server `rim_auth/src/{session_token,csrf,password}.rs` |

Shipped work, by PR (from the plans' "Shipped" tables):

| Repository | PRs | What |
|---|---|---|
| rim-server | #30, #33, #34 | `rim_identity`, key tables, enrollment, rotation, recovery, cards, challenge login; statement version 2; public `keyLog`; 256-entry cap |
| rim-server | #36, #39 | Device linking spec and server |
| rim-server | #37, #41 | Key-only signup; `logoutOtherSessions` |
| rim-server | #45, #46, #48 | Web and workspace plugin specs; plugin registry with signed manifests |
| rim-server | #50 to #53, #56, #57, #63 | Stage 4 plan, E2EE spec and library decision, the server side, client spec notes |
| rim-ui | #24, #26, #28, #29, #30, #31, #32, #33, #35 | Keyring, desktop key client, recovery phrase, device linking, key-only signup, web linked device, identity-key backup |
| rim-ui | #36, #37 | Web plugins; registry client |
| rim-ui | #40, #42, #44, #45 | `rim_mls`; desktop E2EE; plugins and the agent in encrypted conversations; E2EE in the web app |

**Not on `master` at these commits:** the client side of franked reports and the moderator's
view (`e2ee-mls-v1.md` "Franking" step 5, Stage 4 plan row S4-5). The server verifies reports
(`reportEncryptedMessage`), but no query exposes `message_report.franked_content`, there is no
report-scoped attachment route, and the rim-ui report UI lives on an unmerged branch. Review
step 5 against the spec only.

### 1.3 Out of scope

- TLS, HTTP framing, cookies and CORS (the review may assume a correct TLS channel to the
  server, and should say where a design depends on more than that).
- PostgreSQL itself, the database's access control, backups and disk encryption.
- The plaintext product: channels, search, pipelines, the agent's server side, email, alerts.
- OpenMLS, libcrux and RustCrypto internals as libraries. Their use is in scope: configuration,
  suite choice, storage, which checks we skip, and what we assume they enforce.
- Stage 5 (relays and server-to-server, `specs/server-to-server-v1.md`), except where Stage 4
  defers a risk to it.

### 1.4 What we want answered

1. Can a malicious server (full database and code) read, inject into or silently downgrade an
   encrypted DM or group, or make clients trust a device it controls? Section 6 lists the paths
   we found; we want the ones we missed.
2. Are the signed statement formats unambiguous and non-malleable: contexts, exact-text
   payloads, version 1 and 2 spellings, duplicate and unknown fields, key and signature
   canonicality, the 256-entry log?
3. Is the device-linking channel sound as specified (a 256-bit symmetric secret, HKDF per
   direction, AES-256-GCM, AAD, one message per key, a 40-bit fingerprint the person compares)?
4. Are the key derivations separated (HKDF salts and infos, one secret per purpose), and is any
   seed used in two roles in a way that matters?
5. Is our use of OpenMLS correct: suite 0x004F, pure-ciphertext policy, the credential chain,
   the authenticated-data cross-checks, the decrypt-once transaction, Welcome handling with
   `skip_lifetime_validation` and `replace_old_group`, last-resort key packages, re-adds and
   incarnations?
6. Is franking binding enough for a moderator to rely on, and who can forge a "verified" report?
7. What does local state rollback (backups, a second tab, a restored browser profile) do to MLS,
   the vault and the archive?
8. What is agile for the planned hybrid post-quantum signatures, and what would have to change?

### 1.5 Security goals as the specs state them

| Goal | Claimed in | Enforced in |
|---|---|---|
| A principal id is fixed by the inception and commits to both its identity and recovery keys | `key-identity-v1.md` "Key log and principal id" | server `rim_identity/src/encoding.rs:257` `principal_id_for_inception`; `key_log.rs:78` `KeyState::from_inception`; ui/tauri `src/key_identity/statement.rs:525` `replay` |
| A signature made for one purpose never verifies as another | `key-identity-v1.md` "Signed statements" | server `rim_identity/src/statement.rs:68-93` `Purpose::context`, `:125` `verify`; ui/tauri `statement.rs:187`; ui `browser/statements.ts:137` |
| Only the head identity key certifies a new device; a server cannot add a key to the card | `key-identity-v1.md` "Device certificate", "Profile card" | server `rim_identity/src/device.rs:46` `verify_device_cert` (head only); client `statement.rs:645` `certified_device` (any `keySeq`, see section 6) |
| A login proof is single use, short-lived and cannot be relayed to another server | `key-identity-v1.md` "Login proof", "Challenge login" | server `key_auth.rs:410` `verify_login`, `:287` `audience_accepted`, `rim_infra_db/src/sea_orm_impl/key_identity.rs:287` `consume` |
| Enrolling or signing up needs proof of possession of the device key | `key-identity-v1.md` "Enrollment" rule 2, "Key-only signup" | server `key_auth.rs:317` `verify_presented`, `:374` `consume_proof_challenge` |
| The recovery key replaces a lost identity key; recovery revokes every device and session | `key-identity-v1.md` "Rotation and recovery" | server `key_log.rs:106` `KeyState::apply`; `key_auth.rs:807` `rotate` |
| The server never sees the link secret and cannot read, forge, move or substitute a sealed message | `device-linking-v1.md` "What the channel protects" | server `rim_identity/src/device_link.rs:304` `derive_key`, `:313` `aad`, `:599` `seal`, `:627` `open`; ui/tauri `src/key_identity/device_link.rs:227-329`; ui `browser/channel.ts:103-199` |
| The recovery key and the backup key both come from the phrase and neither yields the other | ui `docs/specs/history-archive-v1.md` "Backup key" | ui/tauri `src/key_identity/recovery_kit.rs:71` `backup_key`, `:99` `phrase_key` |
| Desktop secrets at rest open only with the key in the OS credential store; a record moved under another name does not open | ui `docs/secret-vault.md` | ui/tauri `src/secret_store.rs:455` `aad`, `:459` `seal_record`, `:479` `open_record` |
| The browser device key cannot be exported | ui `docs/web-linked-device.md` | ui `browser/browserKeyBackend.ts:241` (`generateKey(…, false, …)`) |
| A server operator with full database access cannot read a DM or a private group | `stage-4-2026-10-09.md` "What Stage 4 must deliver"; `e2ee-mls-v1.md` "Threat model" | Server decodes framing only: server `e2ee/openmls_wire.rs:37-89`; content encrypted in ui/tauri `crates/rim_mls/src/group.rs:716-792` |
| The server cannot add a device of its own to a group | `e2ee-mls-v1.md` "Threat model" | Upload check server `e2ee/mod.rs:1061` `upload_key_packages`; leaf checks ui/tauri `crates/rim_mls/src/identity.rs:266` `verify_leaf`, `src/e2ee/directory.rs:64` `Directory::new` |
| One Commit per epoch; racing committers cannot fork a group | `e2ee-mls-v1.md` "One Commit per epoch" | server `e2ee/mod.rs:608` `accept_commit` (row lock), unique index `uq_mls_commit_epoch` |
| A revoked device reads nothing after its removal; senders commit removals first | `e2ee-mls-v1.md` "Threat model", "What the server checks" | server `message.rs:569` `check_envelope_against_group` (`MLS_COMMIT_REQUIRED`) |
| A message is decrypted once, in one transaction with storing its plaintext | `e2ee-mls-v1.md` "Clients: decrypt once" | ui/tauri `crates/rim_mls/src/storage/sqlite.rs:32`, `src/e2ee/feed.rs`; ui `src/features/e2ee/web/store.ts:178` |
| A ciphertext cannot move to another row or conversation | `e2ee-mls-v1.md` "Replay and gaps" | server `message.rs:542` `decode_envelope`; ui/tauri `crates/rim_mls/src/group.rs:236` `parse_private`, `:849-853` |
| Every message a member sees is reportable; a report proves one message's content | `e2ee-mls-v1.md` "Franking" | ui/tauri `crates/rim_mls/src/group.rs:865-868`; server `e2ee/franking.rs:35-77`, `e2ee/mod.rs:1795` |
| One blob decrypts to one file for every member and a moderator | `e2ee-mls-v1.md` "Attachments" | ui/tauri `crates/rim_mls/src/attachment.rs:83`; server `rim_server/src/file_handlers.rs:442` |
| Plugins see decrypted text only through the message service, never keys or secrets | `e2ee-mls-v1.md` "Plugins and the client message service" | ui/tauri `src/message_cache/plugin_view.rs:153`, `src/plugin_host/broker.rs:737` |
| Key agreement is hybrid post-quantum | `post-quantum-hybrid-2026-10-09.md`; `mls-library-2026-10-09.md` §3 | ui/tauri `crates/rim_mls/src/lib.rs:74` `CIPHER_SUITE`; server `rim_services/src/e2ee.rs:11` `ACCEPTED_CIPHER_SUITES` |

## 2. System overview for a reviewer

### 2.1 Parties

| Party | Holds | Trusted for |
|---|---|---|
| A person | A recovery phrase on paper | Choosing to confirm a link, a recovery, a plugin grant |
| Desktop app (Tauri) | Identity key (on devices that enrolled, recovered or took it over), device key, MLS state, plaintext cache, vault | Everything this brief calls "the client". Rust holds every secret; the webview drives it |
| Desktop webview | The session cookie, the CSRF token, server answers; it never holds a private key | Making every server call and handing answers to the Rust side. It sees the recovery kit text and the link code (see section 6) |
| Browser (web app) | A non-extractable Ed25519 device key, MLS state and plaintext in IndexedDB | The same as the desktop, with weaker storage; never holds an identity key |
| rim-server | Key logs, certificates, revocations, cards, session tokens (hashed), link mailboxes (sealed), key packages, Commits, Welcomes, ciphertext, franking receipts and its franking key | Sequencing, membership and roles, routing. Not trusted for confidentiality of encrypted conversations or for the authenticity of anything signed |
| Drive provider (local folder, Google Drive, Dropbox) | The history archive, identity-key backups, archive key wraps | Availability only |
| Plugins | What their grants give: sandboxed web plugins in an opaque-origin iframe; local-process plugins run as the user | Nothing (see area l) |

### 2.2 Every key

| Key | Holder and storage | Derivation | Lifetime and rotation | Signs or protects |
|---|---|---|---|---|
| Identity key (Ed25519) | Desktop that enrolled, recovered or took over; vault record `key-identity-v1:<account>` | OS random seed | Until an identity rotation (`signer: identity`) or a recovery | Key log rotations, device certificates, identity revocations, profile cards, plugin manifests (`rim.plugin-manifest.v1\0`) |
| Recovery key (Ed25519) | Never stored; re-derived from the phrase when needed | `HKDF-SHA256(ikm = 16 phrase bytes, salt "rim.recovery-phrase.v1", info "recovery key")`, 32-byte seed; version 1 kits carry the seed (`RK1-…`) | Changes only in a recovery rotation (clients keep it) | Recovery rotations; committed in the principal id |
| Phrase entropy | Paper only; in memory while shown or typed (it crosses the webview) | 16 OS random bytes, BIP39 English | Never rotates | Root of the recovery key and the backup key |
| Device key, desktop (Ed25519) | Vault record of the account | OS random seed | Per device; replaced only by linking a new device | Login proofs, self-revocations, `mls-key` statements, re-add requests |
| Device key, browser (Ed25519) | IndexedDB `rim-key-identity`, a non-extractable `CryptoKey` | WebCrypto `generateKey` | Per browser profile | As the desktop device key |
| MLS signature key (Ed25519) | `mls_storage` row `rim.device` (desktop SQLCipher cache); IndexedDB `mls` store as raw bytes (web) | Generated by `rim_mls` | One per device; replaced only after lost state | Every MLS message; certified by the device key in an `mls-key` statement |
| HPKE init keys (X-Wing: X25519 + ML-KEM-768) | MLS storage, per key package | OpenMLS | One-time packages: deleted after use; last-resort: until replaced (expiry within 7 days or more than 5 claims), private keys pruned 7 days after expiry or replacement | Welcome secrets |
| MLS epoch and secret-tree keys | MLS storage | RFC 9420 key schedule | Per epoch; `max_past_epochs(2)`; deleted after use | AES-128-GCM content and Commits |
| Franking key `fk` | Inside the application data; kept with the plaintext and in the archive | 32 OS random bytes per message and edit | Per message revision | `c = HMAC-SHA256(fk, "rim.e2ee.franking.v1\0" ‖ content)` |
| Server franking key `K` | `franking_key` table, row id 1, plaintext | 32 OS random bytes, created on first use | Never rotated by code | Receipt `r = HMAC-SHA256(K, "rim.e2ee.franking-receipt.v1\0" ‖ ids ‖ sent_at_us ‖ c)` |
| Attachment secret | Inside the message content | 32 OS random bytes per file | One file | `key, nonce = HKDF-SHA256(secret, no salt, info "rim.e2ee.attachment.v1 key"/"… nonce")`, AES-256-GCM, AAD `rim.e2ee.attachment.v1` |
| Link secret | Old device memory (and its pending-link vault record); the code text; the new device's memory | 32 OS random bytes | One link, at most 20 minutes | `K_offer`, `K_answer = HKDF-SHA256(secret, salt = link id, info "rim.device-link.v1 offer"/"… answer")` |
| Link id | Server stores SHA-256 only | 16 OS random bytes | One link | Bearer capability for the anonymous mailbox calls; HKDF salt; AAD |
| Vault key | OS credential store, `com.rim.rim-ui` / `vault-key-v1` | 32 OS random bytes | Never rotated; regenerated only when lost (the vault is then set aside) | XChaCha20-Poly1305 of each vault record, AAD `rim-secret-vault v1\0<name>` |
| Message cache key | Vault record `message-cache-key-v1` | 32 OS random bytes per installation | Never rotated | SQLCipher database of every account's cache, including MLS state |
| Archive key (+ file key, name key) | Vault `history-archive-key-v1:<archive id>`; wrapped in the manifest and per-device `key-wraps.json` | 32 OS random bytes; file and name keys by HKDF (`rim-history-archive v1 file key` / `name key`) | Never rotated | Archive segments, heads (XChaCha20-Poly1305) |
| Passphrase key | Not stored | Argon2id (64 MiB, 3 passes, 1 lane, 16-byte salt) | Per wrap | Archive key wrap in the manifest |
| Backup key | Vault record of a device holding the identity key | `HKDF-SHA256(phrase entropy or v1 seed, salt "rim.recovery-phrase.v1", info "identity backup key")` | As the phrase | `identity-backup.rimx` |
| Identity wrap key | Derived when needed | `HKDF-SHA256(identity seed, salt "rim.history-archive.v1", info "archive key wrap")` | Changes with the identity key | `identity-v1` archive key wraps |
| Session token, CSRF token, challenge nonce | Server stores SHA-256 of the session token; the nonce is stored as is | 32 OS random bytes each | Sessions per TTL; challenges 120 s, single use | Authentication; `CSRF = HMAC-SHA256(session token, nonce)` |

### 2.3 Trust boundaries

1. **Client and server.** Everything the server returns is data. Signed statements are verified
   by the client against key logs it replays itself. Membership, revocation status, the
   conversation's `encryptedAt`, the author of a row, pending adds and removes, Welcomes and
   Commits are server assertions; section 6 lists where clients act on them unchecked.
2. **Rust and the webview (desktop).** Private keys never cross into the webview, which asks the
   Tauri side for signed statements (`key_identity/commands.rs`, `e2ee/commands.rs`). The webview
   chooses what to ask for and supplies the endpoint, logs and device lists the Rust side checks.
3. **The page and IndexedDB (web).** Any script in the page can use the device key handle and
   read the MLS keys and plaintext.
4. **Devices and the drive.** The archive is encrypted under keys the drive provider never sees;
   it can delete, withhold or roll back files.
5. **The app and plugins.** Sandboxed web plugins see only host messages; local-process plugins
   run as the user.

### 2.4 Statement formats

Every statement travels as `{ payload, signature }`; the payload is the exact signed text and is
never re-serialized; the signature covers `context ‖ payload`; unknown and duplicate fields are
refused (`key-identity-v1.md` "Signed statements"). Version 2 spells keys and signatures
`<algorithm>:<base64url>`; version 1 spells bare Ed25519. A key log never goes from version 2
back to 1; other statements may be either version (`mls-key`, `mls-readd` and `plugin-manifest`
exist only in version 2).

| Context or domain string | Used for | Defined in |
|---|---|---|
| `rim.keylog.v1\0` | Inception and rotations | server `rim_identity/src/statement.rs:70-80` |
| `rim.device.v1\0`, `rim.device-revoke.v1\0` | Certificates, revocations | same |
| `rim.profile-card.v1\0`, `rim.login.v1\0` | Cards, login and enrollment proofs | same |
| `rim.plugin-manifest.v1\0` | Publisher signatures | same |
| `rim.mls-key.v1\0`, `rim.mls-readd.v1\0` | MLS credential certification, re-add requests | same; ui/tauri `crates/rim_mls/src/identity.rs:17`, `readd.rs:12` |
| `rim.principal.v1\0` | Principal id hash | server `encoding.rs:15` |
| `rim.recovery-kit.v1\0` | `RK1-` check bytes | ui/tauri `recovery_kit.rs:42` |
| `rim.device-link.v1 offer`, `… answer`, `rim.device-link.v1\0<dir>\0<link id>`, `rim.device-link.v1 fingerprint\0` | HKDF infos, AAD, fingerprint | `device-linking-v1.md` "Channel cryptography" |
| `rim.e2ee.franking.v1\0`, `rim.e2ee.franking-receipt.v1\0` | Commitment, receipt | server `e2ee/franking.rs:15-16` |
| `rim.e2ee.attachment.v1 key`, `… nonce`, `rim.e2ee.attachment.v1` | HKDF infos, AAD | ui/tauri `crates/rim_mls/src/attachment.rs:12-14` |
| `rim-secret-vault v1\0<name>` | Vault AAD | ui/tauri `secret_store.rs:50` |
| `rim-history-archive v1 …`, `rim.history-archive.v1` | Archive HKDF infos, wrap AAD, wrap salt | ui `docs/specs/history-archive-v1.md` "Keys" |

Payload caps: 16 KiB, profile cards 128 KiB, plugin manifests 96 KiB (`statement.rs:89`).
Every payload has `"v"` and `"type"`; timestamps are informational and never checked.

### 2.5 Key log and certificate chain

```
principalId = "rimp1_" + b64url(SHA-256("rim.principal.v1\0" ‖ inception payload))
inception (seq 0, signed by identityKey_0, names identityKey_0 and recoveryKey_0)
  └ rotate (seq n, prev = b64url(SHA-256(payload n-1)), signed by identityKey_{n-1} [signer identity]
                                                  or recoveryKey_{n-1} [signer recovery])
device certificate (principalId, deviceId, deviceKey, keySeq) signed by identityKey_keySeq
  └ mls-key (principalId, deviceId, mlsKey) signed by deviceKey
       └ MLS leaf: basic credential identity = {"payload","signature"} of mls-key; signature_key = mlsKey
login proof (challengeId, nonce, audience, principalId, deviceId) signed by deviceKey
```

The server verifies a certificate only against the head (`device.rs:46`), once, when a device is
added; clients verify certificates later against the identity key at the certificate's own
`keySeq` (ui/tauri `statement.rs:645`), because certificates outlive identity rotations.

### 2.6 How the server is kept honest

| The server sequences or decides | The server never sees or holds |
|---|---|
| Account, membership and role state; which devices are revoked | Any private key; the link secret; the recovery phrase |
| One Commit per `(conversation, incarnation, epoch)` | MLS secrets, message content, attachment plaintext, franking keys |
| Which key package each claim gets (oldest one-time, else last resort) | What a sealed link message says |
| Welcome delivery and Commit retention (90 days) | The archive |
| Which cipher suite is accepted (`[0x004F]`) | |
| Franking receipts under its own key | |

What the server reads from MLS: the outer `MLSMessage` framing, `group_id`, `epoch`,
`content_type` and the cleartext `authenticated_data`; key packages (validated with
`openmls_rust_crypto`); Welcome `new_member` refs (`e2ee/openmls_wire.rs`). It cross-checks the
authenticated data against its own state (`e2ee/aad.rs`, `e2ee/mod.rs:608`) and clients
cross-check it against the decrypted content and staged Commits.

## 3. Review guide per area

Each area gives the claim, the design in brief, the code, the vectors and tests that pin it,
our questions, and what we suspect or deferred. Section 6 collects the findings across areas.

### (a) Signed statements and key logs

**Claim.** One meaning per statement; a principal id commits to its inception; only the current
identity key extends a log, except a recovery key replacing a lost one; nothing is
re-serialized.

**Design.** Exact-text payloads; per-purpose context bytes; strict unpadded base64url
(canonical trailing bits); Ed25519 `verify_strict`; weak and non-canonical public keys refused
on the server; `deny_unknown_fields` plus serde's duplicate-field refusal; version 2 tags with an
algorithm name, a signature's algorithm must be its key's; logs up to 256 entries at enrollment;
`prev` chaining by SHA-256 of the previous payload; plugin manifest statements stay valid across
identity rotations and fail after a later recovery rotation.

**Code.**

| Function | Location |
|---|---|
| `Purpose::context`, `max_payload_len`, `sign`, `verify`, `parse`, `check_header` | server `rim_identity/src/statement.rs:68`, `:89`, `:109`, `:125`, `:137`, `:146` |
| `PublicKey::parse_ed25519` (weak and non-canonical refused), `Signature::decode_for` | server `rim_identity/src/encoding.rs:141`, `:222` |
| `KeyState::from_inception`, `apply`, `replay` | server `rim_identity/src/key_log.rs:78`, `:106`, `:165` |
| `verify_device_cert`, `UnverifiedRevocation::verify` | server `rim_identity/src/device.rs:46`, `:135` |
| `verify_profile_card` | server `rim_identity/src/profile_card.rs:83` |
| `KeyHistory::recovery_after`, `verify_plugin_manifest` | server `rim_identity/src/plugin_manifest.rs:86`, `:349` |
| Server use: `enroll`, `signup_with_key`, `add_device`, `revoke_device`, `rotate`, `publish_profile_card`, `get_key_log` | server `rim_infra_services/src/key_auth.rs:578`, `:632`, `:743`, `:761`, `:807`, `:907`, `:945` |
| Desktop: `verify`, `canonical_key`, `usable_device_key`, `replay`, `certified_device` | ui/tauri `src/key_identity/statement.rs:187`, `:179`, `:218`, `:525`, `:645` |
| Browser: `verify` (small-order, non-canonical, scalar checks before WebCrypto), `replay`, `certifiedDevice` | ui `src/features/key-identity/browser/statements.ts:137`, `:328`, `:462` |

**Vectors.** `key-identity-v1.md` appendices A and B; `workspace-plugins-v1.md` appendix A.
Tests in appendix A of this brief.

**Questions.**

1. Is there any way two different payloads verify as the same statement, or one payload as two
   types, given exact-text signing and per-purpose contexts?
2. The server parses a payload before verifying it to learn the signer. Is any parser behaviour
   reachable before the signature check a risk (serde errors are returned as text)?
3. Clients replay logs with less strict parsing than the server: no `deny_unknown_fields` in
   `LogEntry` (ui/tauri `statement.rs:504-515`), no `identityKey != recoveryKey` check, no
   non-canonical point check. Is a log that the server would refuse but a client accepts ever
   useful to an attacker?
4. A rotation must change the identity key from the head's, but may return to an older one
   (A → B → A). Does anything depend on identity keys being unique across the log?
5. Device certificates are checked against the identity key at their `keySeq` with no regard for
   a later recovery rotation (section 6, item 3). Should they follow the plugin manifest rule?

**Suspected or deferred.** Key-log freshness (a server can withhold later entries) and head
pinning are deferred (`post-quantum-hybrid-2026-10-09.md` "Consequences";
`workspace-plugins-v1.md` "Deferred"). A stolen old identity key can claim the principal on a
server that has not seen the real log; the recovery key takes it back.

### (b) Login proofs and audiences

**Claim.** A proof is bound to one fresh 120-second challenge and to the server's `host[:port]`,
so it cannot be replayed or relayed.

**Design.** `keyLoginChallenge` returns 32 random bytes, stored as is and consumed with one
conditional `UPDATE` (unused, unexpired, same nonce). The audience is compared ASCII
case-insensitively against `RIM_KEY_LOGIN_AUDIENCES`, else the host of `RIM_PUBLIC_BASE_URL`,
else `localhost:PORT` and `127.0.0.1:PORT`. The user row is locked and the device re-read before
the session is created. Enrollment and key-only signup use the same proof over a fresh
challenge, consumed inside the transaction.

**Code.** server `rim_infra_services/src/key_auth.rs:287` `audience_accepted`, `:294`
`reserve_failure`, `:410` `verify_login`, `:472` `issue_challenge`, `:502` `login_with_key`;
`rim_auth/src/session_token.rs:42` `generate_challenge_nonce`;
`rim_infra_db/src/sea_orm_impl/key_identity.rs:287` `consume`; `rim_server/src/config.rs:370`
`key_login_audiences`; CSRF exemptions `rim_server/src/graphql_support.rs:52-78`. Desktop
signing ui/tauri `src/key_identity/mod.rs:662` `sign_login`; browser
`browser/browserKeyBackend.ts:77` `audience`.

**Vectors.** Login proofs in `key-identity-v1.md` appendices A and B.

**Questions.**

1. A valid signature from a revoked device answers `FORBIDDEN` before the audience and the
   challenge are checked (`key_auth.rs:441-453`). Any old proof of that device, from any
   server, therefore tells whether the device is revoked here. Acceptable?
2. The desktop computes the audience from an endpoint the webview passes in
   (`key_identity_sign_login`, ui/tauri `src/key_identity/commands.rs:137`). A compromised
   webview can obtain proofs for any audience. Should the Rust side pin the endpoint per
   account?
3. The default audience when neither variable is set is `localhost:PORT`. Two misconfigured
   servers on the same port accept each other's proofs. Should production refuse to start
   without an explicit audience?
4. Challenges are not bound to the principal or the requesting IP; is that fine given single
   use and 32 random bytes?

### (c) Device linking

**Claim.** The server is a courier: it cannot read, forge, move or substitute an offer or an
answer. Someone who only saw the code gets nothing unless the person confirms their offer.

**Design.** `rim-link:1:<link id>:<secret>:<endpoint>`; `K_dir = HKDF-SHA256(secret,
salt = link id, info = "rim.device-link.v1 <dir>")`; AES-256-GCM, random 96-bit nonce, AAD names
version, direction and link id; each side seals exactly one plaintext per link and resends the
same bytes; the offer carries the new device's key unsigned; the answer carries the certificate;
the new device verifies the certificate against `keyLog` before storing any key; both screens
show `base32(SHA-256("rim.device-link.v1 fingerprint\0" ‖ offer bytes)[0..5])`, 40 bits; the
link id is stored hashed and its anonymous calls answer uniformly.

**Code.** Reference: server `rim_identity/src/device_link.rs:304` `derive_key`, `:313` `aad`,
`:361` `Fingerprint::of_offer`, `:464` `Offer::parse`, `:538` `Answer::verify`, `:599` `seal`,
`:627` `open`. Courier: server `rim_infra_services/src/device_link.rs:88` `link_hash`, `:302`
`start`, `:367` `send_offer`, `:408` `send_answer`, `:440` `get_answer`. Desktop: ui/tauri
`src/key_identity/device_link.rs:227` `channel_key`, `:236` `aad`, `:288` `seal`, `:296`
`open`, `:329` `fingerprint`. Browser: ui `browser/channel.ts:69` `parseLinkCode`, `:103`
`hkdfParams`, `:136` `sealWithNonce`, `:164` `open`, `:199` `fingerprint`; key generation
`browser/browserKeyBackend.ts:241`.

**Vectors.** `device-linking-v1.md` appendix A (keys, AADs, both ciphertexts and tags, the
fingerprint), checked in Rust on both sides and in Vitest through Node's WebCrypto.

**Questions.**

1. Is a 40-bit fingerprint enough when the attacker who saw the code races the real device?
   The real device's offer is then refused and it says so; the old device shows the attacker's
   fingerprint. What should the UI force?
2. The answer is readable by anyone who saw the code. Today it carries only the (public)
   certificate. The deferred archive key in the answer needs a hybrid ephemeral key in the
   offer; is the planned shape (`device-linking-v1.md` "Deferred") sound?
3. The desktop shows the full code, secret included, in the webview (QR and copy). Should the
   QR be rendered on the Rust side?
4. A browser's `JSON.parse` keeps the last duplicate field (`web-linked-device.md`, "Known
   gaps"). The answer is authenticated, and its certificate verifies; anything left?

**Deferred.** PAKE short codes, QR scanning, moving the identity key, the archive key in the
answer, linking across servers (`device-linking-v1.md` "Deferred").

### (d) Recovery phrase and identity-key backup on the drive

**Claim.** 128 bits of phrase entropy give both the recovery key and the backup key, neither
derivable from the other; the identity key in the archive opens only with the phrase; a takeover
must match the head of the server's log.

**Design.** BIP39 English encoding of 16 random bytes (the `bip39` crate), not BIP39's PBKDF2
seed; HKDF-SHA256 with salt `rim.recovery-phrase.v1` and infos `recovery key` and `identity
backup key`. The device keeps the backup key, not the entropy. Backups are `.rimx` files of kind
3 (XChaCha20-Poly1305, AAD = header ‖ archive id ‖ account tag ‖ device id ‖ file name). The
archive key is also wrapped under `HKDF(identity seed, salt "rim.history-archive.v1", info
"archive key wrap")`. Version 1 kits (`RK1-`) remain readable.

**Code.** ui/tauri `src/key_identity/recovery_kit.rs:71` `backup_key`, `:80`
`phrase_backup_key`, `:99` `phrase_key`, `:110` `phrase_words`, `:147`
`parse_recovery_phrase`; `src/key_identity/backup.rs:48` `archive_wrap_key`, `:165`
`begin_takeover`, `:186` `finish_takeover`; `src/history_archive/identity.rs:40`
`identity_wrap_aad`, `:154` `write_identity`, `:225` `unlock_with_identity`, `:284`
`write_wrap`, `:327` `write_backup`; `src/history_archive/crypto.rs:89` `seal`, `:117` `open`,
`:177` `passphrase_key`; `src/identity_backup/mod.rs:155` `take_over`.

**Vectors.** `key-identity-v1.md` "Recovery kit" (entropy, phrase, recovery key); ui
`docs/specs/history-archive-v1.md` "Known-answer vectors" (backup keys, wrap key, a sealed
backup, a wrap).

**Questions.**

1. The identity seed is an Ed25519 signing seed and also HKDF input key material for the
   archive wrap key (`backup.rs:48`). Is this cross-use acceptable?
2. The archive key never rotates, and an identity rotation only replaces this device's wrap
   entry. A drive provider that kept an old `key-wraps.json`, plus a leaked retired identity
   key, opens the whole archive. Should a rotation re-key the archive?
3. The phrase is shown in, and typed into, the webview (enrollment, recovery, takeover). What
   is the minimum we should do about that?
4. Nothing tells the previous holder that the identity key was taken over. Is a signal needed?

### (e) Desktop secret vault and the OS credential store

**Claim.** Secrets at rest open only with the key in the OS credential store; a record moved
under another name does not open.

**Design.** One 32-byte key in Keychain, Credential Manager or Secret Service. One JSON file,
each record XChaCha20-Poly1305 with a new 24-byte random nonce on every write, AAD
`rim-secret-vault v1\0<name>`, whole-file atomic replace, `0600`. A file whose any record fails
to open is moved aside and every secret in it is lost. The message cache (and MLS state) is a
SQLCipher database keyed from the vault.

**Code.** ui/tauri `src/secret_store.rs:217` `read`, `:253` `write`, `:290` `load`, `:448`
`verify`, `:455` `aad`, `:459` `seal_record`, `:479` `open_record`, `:509` `write_new`;
`src/message_cache/key.rs` (cache key), `src/message_cache/store.rs:446` (`PRAGMA key`).

**Tests.** `secret_store.rs` unit tests (`secrets_round_trip` and others), Secret Service error
tests.

**Questions.**

1. No anti-rollback: an attacker with write access to the file can restore an older vault (older
   device or identity keys, a pending rotation, an older cache key). With the cache database it
   also rolls back MLS state (section 6, item 8). Worth a counter in the credential store?
2. One altered record moves the whole vault aside (`secret_store.rs:448`), losing device and
   identity keys; a `v` above 1 locks every call. Should a bad record fail alone?
3. Any process of the user can read an unlocked Secret Service item and the file. Is this model
   stated well enough to users, given local-process plugins (area l)?
4. `message_cache/key.rs:4` says the cache key is re-wrapped under the device key in Stage 3. It
   is not; it stays a vault record.

### (f) The browser device key and the weaker-storage model

**Claim.** The device key cannot be exported; the person is told that browser keys are weaker.

**Design.** WebCrypto Ed25519 `extractable: false`, stored by structured clone in IndexedDB.
Statements are verified in front of WebCrypto against small-order points, non-canonical
encodings and `S ≥ L`. The MLS signature key and HPKE private keys are raw bytes in IndexedDB,
because OpenMLS signs and decapsulates synchronously. The web app never holds an identity key.

**Code.** ui `src/features/key-identity/browser/browserKeyBackend.ts:241`; `statements.ts:72`
`smallOrder`, `:79` `nonCanonicalPoint`, `:85` `canonicalScalar`, `:137` `verify`; `keyStore.ts`;
`src/core/runtime/ed25519.ts`; web E2EE storage `src/features/e2ee/web/db.ts`.

**Questions.**

1. Any script in the page can sign with the handle and read every MLS key and plaintext. Is
   "only as safe as this browser profile" an honest summary, and is the CSP
   (`'wasm-unsafe-eval'`) acceptable?
2. Should the IndexedDB MLS state be wrapped under a non-extractable AES key, to stop at least
   disk-level copies of the profile?

### (g) MLS

**Claim.** A server with full access cannot read content, add a device, forge a message or force
a last-resort re-add; one Commit per epoch; post-compromise security from update paths at least
weekly; key agreement is hybrid.

**Design summary.** Suite 0x004F (X-Wing KEM 0x647a, AES-128-GCM, SHA-256, Ed25519), OpenMLS
0.9.1 with `openmls_libcrux_crypto` 0.4.0 on clients and `openmls_rust_crypto` 0.6.0 on the
server. Group id `rim1 ‖ conversation UUID ‖ u32 incarnation`. Pure-ciphertext wire format
(Commits are PrivateMessages), ratchet tree in the Welcome, padding 256, out-of-order tolerance
32, forward distance 10,000, `max_past_epochs(2)`, Welcome leaf lifetimes not validated. Members
are devices. Leaves carry an `mls-key` statement; clients verify each leaf against the key log,
the certificate, revocation and membership. The server validates every key package against the
uploading session's own device key. Twenty one-time packages plus one last-resort package per
device, 90-day lifetime, never handed out within 14 days of expiry. The server sequences one
Commit per epoch under a row lock and a unique index (`STALE_EPOCH` otherwise) and refuses
application messages while removals are pending (`MLS_COMMIT_REQUIRED`). Re-add requests are
signed by the device and served only from one-time packages uploaded after the request.
Incarnations restart the group for suite migration, recovery, or when no device is left.

**Code.**

| What | Location |
|---|---|
| `mls-key` and `mls-readd` statements, credential identity | server `rim_identity/src/mls.rs:88`, `:124`, `:140`, `:151`, `:208`, `:247`; ui/tauri `crates/rim_mls/src/identity.rs:74`, `:97`, `readd.rs:78` |
| Server framing decode and key package validation | server `rim_infra_services/src/e2ee/openmls_wire.rs:37` `read_key_package`, `:61` `read_private_message`, `:77` `read_welcome` |
| Upload, claim, commit, re-add, devices | server `e2ee/mod.rs:1061` `upload_key_packages`, `:1215` `claim_key_packages`, `:1400` `submit_commit`, `:608` `accept_commit`, `:562` `check_key_package`, `:494` `incarnation_ground`, `:535` `may_create_group`, `:1597` `request_readd`, `:1733` `principal_devices` |
| Group state on the server | server `e2ee/mod.rs:228` `GroupState`, `:279` `pending_removes`, `:286` `readd_targets` |
| Application messages | server `message.rs:542` `decode_envelope`, `:569` `check_envelope_against_group`, `:1771` `send_encrypted_message`, `:1963` `edit_encrypted_message` |
| Client engine | ui/tauri `crates/rim_mls/src/group.rs:181` `join_config`, `:400` `create`, `:448` `commit`, `:519` `process_commit`, `:617` `check_summary`, `:677` `join`, `:716` `encrypt`, `:794` `decrypt`; `device.rs:224` `key_package_lifetime`, `:233` `generate_key_packages`, `:330` `prune_key_packages` |
| Leaf verification | ui/tauri `crates/rim_mls/src/identity.rs:266` `verify_leaf`; `src/e2ee/directory.rs:64` `Directory::new`; ui `src/features/e2ee/web/directory.ts:35` `answers` |
| Client loop and decrypt-once | ui/tauri `src/e2ee/feed.rs:336-360`; `crates/rim_mls/src/storage/sqlite.rs:32`; ui `src/features/e2ee/web/store.ts:172` `run`, `:178` `runNow` |
| Joins, re-adds, membership | ui/tauri `src/e2ee/group.rs:457` `join`, `:683` `may_honor_readd`; ui `src/features/e2ee/runtime/{groups,membership}.ts` |
| Signing for MLS on the desktop | ui/tauri `src/key_identity/mod.rs:701` `sign_mls` (only `mls-key` and `mls-readd`, only for this principal and device) |

**Vectors and tests.** The library decision's vector table (RFC 8032, RFC 7748, RFC 9180 A.1,
RFC 5869, RFC 4231, Wycheproof AES-GCM, NIST ACVP ML-KEM-768, X-Wing draft vectors) in
`crates/rim_mls/src/tests/vectors.rs`; `e2ee-mls-v1.md` appendix A in `tests/formats.rs`;
cross-provider tests `tests/providers.rs`; multi-device simulations against an in-process
sequencer `tests/simulation.rs`, `tests/sequencer.rs`; tampering `tests/tampering.rs`; server
races in `rim_infra_db` `db-tests` and the e2e `rim_api/tests/e2ee_e2e.rs`. The IETF
`mls-implementations` vectors named in the decision for suite 0x0001 are not re-run in RIM's
tree (`tests/vectors.rs` covers primitives only).

**Questions.**

1. **Credential chain.** Is `mls-key` (device key certifies an Ed25519 MLS key) a sound binding,
   given that the MLS key never rotates and `issuedAt` is not checked?
2. **Leaf verification inputs.** Revocation status and membership are what the server says
   (section 6, items 2 and 4). What must a client verify itself?
3. **Commit summary.** A Commit whose authenticated summary does not match its proposals is
   still merged (to avoid a fork) and only flagged (`group.rs:617`). Proposals other than Add and
   Remove (for example GroupContextExtensions) only make the check fail. Is merging first
   acceptable, and which proposal types should a client refuse outright?
4. **Welcomes.** `join` uses `skip_lifetime_validation()` and `replace_old_group()`
   (`group.rs:691-692`); the app refuses only an older incarnation (`src/e2ee/group.rs:466`).
   What can a replayed Welcome of the same incarnation do (section 6, item 6)?
5. **Last resort.** One last-resort package can be claimed many times until the device replaces
   it (more than 5 claims, or expiry within 7 days). What is the forward secrecy cost, and
   should the server stop handing it out sooner?
6. **Re-adds and incarnations.** What can a server force with replayed re-add requests and the
   10-minute recovery rule (section 6, item 9)?
7. **Decrypt-once.** The desktop runs decrypt and store in one SQLCipher transaction. The web
   journals OpenMLS writes in memory and commits them with the plaintext in one IndexedDB
   transaction, rolling back on failure. Are there OpenMLS writes outside the storage trait
   (caches, in-memory group fields) that a rollback would miss?
8. **Epoch window.** A message more than two epochs behind becomes a placeholder; the server
   accepts application messages only in the current epoch. Any interaction between
   `max_past_epochs` and Commit retention we missed?
9. **0x004F.** A code point OpenMLS chose for draft TBD1. Are we right that only our clients need
   to agree, and is the incarnation path a sound migration once IANA assigns one?

**Deferred.** Hybrid MLS credentials, safety numbers and key transparency, sender keys beyond
512 devices, encrypted private channels, deniability, encrypted reactions and pins
(`e2ee-mls-v1.md` "Deferred").

### (h) Authenticated data and cross-checks

**Claim.** The only cleartext the server reads is the authenticated data; receivers refuse a
message whose authenticated data and content disagree.

**Design.** Application AAD `{v, type, messageId, franking, mentions, mentionAll}` (8 KiB);
Commit AAD `{v, type, adds[{principalId, deviceId, keyPackageRef}], removes[…]}` (256 KiB).
Server checks the message id against the request, mentions syntax and distinctness, and the
Commit summary against pending adds and removes, claims and the Welcome's refs. Receivers check
message id, thread, author, mentions, `mentionAll` and the franking commitment after decrypting,
and the staged Commit's Add refs and removed devices against the summary.

**Code.** server `e2ee/aad.rs:73` `parse_application`, `:146` `parse_commit`; ui/tauri
`crates/rim_mls/src/aad.rs:155` `AuthenticatedData::parse`, `group.rs:794-877` (decrypt checks),
`:617` `check_summary`; `content.rs:219` `MessageContent::parse`, `:285` `check`.

**Vectors.** `e2ee-mls-v1.md` A.4 (content, application AAD, a Commit AAD, a recorded
PrivateMessage).

**Questions.**

1. The author check runs only when the server supplies the author's principal id
   (`group.rs:854-861`; section 6, item 5). Should a missing id make the message invalid?
2. Mentions leak who was mentioned (accepted in the spec). Anything else in the AAD a server
   could exploit, such as `mentionAll` for notification floods?
3. The server's `senderDeviceId` on a row is never compared with the MLS sender's device id.
   Does anything rely on it (the moderator view will)?

### (i) Franking

**Claim.** A report proves one message's exact content to moderators without the server having
read it; every visible message is reportable.

**Design.** `c = HMAC-SHA256(fk, "rim.e2ee.franking.v1\0" ‖ content)` in the AAD, `fk` (exactly 32
bytes) in the application data. Server receipt `r = HMAC-SHA256(K, context ‖ message id ‖
conversation id ‖ sender user id ‖ sender device id ‖ sent_at_us ‖ c)`. A report sends content,
`fk`, `r`, the key id, sender ids and `sentAtUs`; the server recomputes `c` and `r` and compares
in constant time (`verify_slice`). The content is then stored in `message_report` for 90 days
after resolution.

**Code.** server `e2ee/franking.rs:35` `commitment`, `:44` `receipt_input`, `:57` `receipt`,
`:68` `verify_receipt`; `message.rs:626` `frank`; `e2ee/mod.rs:1795`
`report_encrypted_message`; `rim_infra_db/src/sea_orm_impl/e2ee.rs:1297`
`current_franking_key`; ui/tauri `crates/rim_mls/src/franking.rs`, `group.rs:865-868`.

**Vectors.** `e2ee-mls-v1.md` A.4 (commitment) and A.5 (receipt input and receipt), checked by
`franking.rs::tests::known_answer_commitment_and_receipt` (server) and
`tests/formats.rs::content_franking_and_authenticated_data_vectors`,
`franking_receipt_vector` (client).

**Questions.**

1. `K` is one database row, created on first use, never rotated, in plaintext next to the
   messages. A database reader can mint receipts and therefore "verified" reports of any content
   attributed to any member (section 6, item 7). Should `K` live outside the database, rotate,
   and should a report also be checked against the row's stored commitment?
2. An edit is franked with the original `sent_at` (`message.rs:2025`), so every revision a
   member decrypted stays reportable under the same message id. The moderator view should show
   `rev`; is anything else needed?
3. The spec says a report "needs no message row"; the code requires one (`e2ee/mod.rs:1814-1820`).
   Soft-deleted rows remain, so deletion by the sender does not stop a report, but a deleted
   conversation does.
4. Is HMAC-SHA256 with a fixed 32-byte key a sufficient commitment here (we rely on the fixed key
   length to avoid HMAC key-padding collisions)?

### (j) Attachments

**Claim.** One blob decrypts to one file for every member and a moderator.

**Design.** Per-file 32-byte secret; key and nonce by HKDF with distinct infos; AES-256-GCM with
a constant AAD; one encryption per secret; the content entry commits to the blob's SHA-256, the
secret, name, type and size; the server refuses uploads whose SHA-256 or size differ; the
client checks the hash, decrypts, checks the size.

**Code.** ui/tauri `crates/rim_mls/src/attachment.rs:17` `attachment_key_and_nonce`, `:36`
`encrypt_attachment`, `:47` `encrypt_attachment_with`, `:83` `decrypt_attachment`;
`src/e2ee/attachment.rs`; ui `src/features/e2ee/web/attachments.ts:49`, `:67`, `:97`; server
`rim_server/src/file_handlers.rs:442`.

**Vectors.** `e2ee-mls-v1.md` A.3, in `tests/formats.rs::attachment_vector`,
`src/e2ee/tests.rs::appendix_a3_attachment_round_trip`, `web/attachments.test.ts`, and the
server e2e `encrypted_attachments_are_checked_by_hash`.

**Questions.**

1. The nonce is a function of the secret, so the design is safe only while a secret encrypts one
   plaintext. `encrypt_attachment_with` is public and takes a caller's secret. Should it be
   private to the crate's tests?
2. Is binding the blob hash in the franked content enough to make the non-committing AES-GCM
   irrelevant here?

### (k) The wasm build and the browser store

**Claim.** The web app runs the same `rim_mls`; no ciphertext is sent and no plaintext shown
before the state that produced it is stored; one tab per account runs encryption.

**Design.** `rim_mls` built for `wasm32-unknown-unknown` with libcrux's portable code; a
journaled `MemoryStore` written through to IndexedDB in one transaction per unit of work; a
trap stops the store for the page; the Web Lock `rim-e2ee:<endpoint>:<principalId>` keeps a
second tab out. The compiled module is committed, and a test fails if its sources changed
(`src/features/e2ee/web/wasmArtifact.test.ts`).

**Code.** ui/tauri `crates/rim_mls_wasm/src/lib.rs`, `crates/rim_mls/src/storage/memory.rs:45`
`commit`, `:53` `pending`, `:67` `rollback`; ui `src/features/e2ee/web/store.ts:172-197`,
`webSession.ts:21` `holdTabLock`, `mlsModule.ts`, `scripts/build-wasm.mjs`.

**Tests.** `npm run test:wasm` (the whole `rim_mls` suite in headless Chromium); Vitest
`src/features/e2ee` (simulated server, `fake-indexeddb`, failed writes rolled back, reloads).

**Questions.**

1. Without the Web Locks API the page "assumes it is alone" (`webSession.ts:24-25`). Web Locks
   and `crypto.subtle` both need a secure context, so this path should be unreachable in
   practice; should it refuse instead?
2. Is a committed binary (`rim_mls_wasm_bg.wasm`, checked only by a source hash) acceptable
   supply-chain hygiene, or should the build be reproducible and verified in CI?
3. getrandom on wasm uses WebCrypto (`wasm_js`, `js`). Any concern with OpenMLS's clock through
   `web-time` for lifetimes?

### (l) Plugins' access to decrypted content

**Claim.** Plugins see decrypted text only through the client message service under
`chat.read.encrypted` (on top of `chat.read`), never franking keys, attachment secrets, blob
hashes, MLS state, ciphertext or keys.

**Design.** The service reads the local cache; placeholders, invalid rows, tombstones and unsent
rows are left out; writes go through the normal encrypted send path; grants and daily counts
are audited locally. Sandboxed web plugins run in an `allow-scripts` iframe with an opaque
origin and a per-plugin CSP. Local-process plugins are not sandboxed.

**Code.** ui/tauri `src/message_cache/plugin_view.rs:115` `conversation_encrypted`, `:136`
`check_readable`, `:153` `plugin_messages`, `:315` `project`; `src/plugin_host/broker.rs:737`;
ui `src/features/plugins/chat/appMessageService.ts:127`, `src/features/e2ee/web/pluginView.ts`.

**Tests.** `plugin_view.rs::tests::nothing_secret_or_unverified_reaches_a_plugin`,
`an_encrypted_conversation_needs_the_encrypted_grant`; `protocol/plugin-chat-vectors.json`.

**Questions.**

1. A local-process plugin runs as the user and can read the credential store entry and the vault
   file directly. The consent prompt says it is not sandboxed; is the encrypted grant then
   meaningful only for web plugins and honest local ones, and should the UI say so?
2. The web app's message service decides "encrypted" from the server's `encryptedAt`
   (`appMessageService.ts:127`), the desktop also from local knowledge (`plugin_view.rs:618`
   test). Should both use local state only?

### (m) The post-quantum plan and what is agile today

| Mechanism | Today | Agile how | What changes for hybrid |
|---|---|---|---|
| MLS key agreement | X25519 + ML-KEM-768 (X-Wing) in 0x004F | New suite by a new incarnation; the server lists accepted and preferred suites | Already hybrid; migration when IANA assigns TBD1 |
| MLS signatures | Ed25519 | Suite-bound | Needs a composite suite; none is drafted |
| Identity, recovery, device keys and every statement | Ed25519, version 2 tags | `<algorithm>:` tags; caps sized for ML-DSA-87; `key_hash` index; `#[non_exhaustive] Algorithm` | Add `ed25519-mldsa44` in `rim_identity/src/encoding.rs` and each client's verifier; rotate each log with an Ed25519-signed rotation |
| Principal id, `prev`, key hashes | SHA-256 | Not tagged | None planned |
| Device-link channel | Symmetric only | Code version in the code | Ephemeral hybrid KEM when the answer carries a secret |
| Franking, attachments, vault, archive | HMAC-SHA256, AES-256-GCM, XChaCha20-Poly1305, Argon2id | Version strings in contexts and file formats | None needed for quantum |

**Questions.** Is the planned transition (an existing Ed25519 log rotates to a hybrid key with a
rotation signed by the old Ed25519 key, then a cutoff refuses Ed25519-only rotations) safe
without head pinning, which is not implemented (section 6, item 11)? Is a version 1 statement
still a downgrade path after hybrid keys exist (we believe not: version 1 can only spell and
verify Ed25519, `encoding.rs:97-106`, `:228`)?

## 4. Threat model summary

The specs carry their own tables: `key-identity-v1.md` (implicit in "Server rules"),
`device-linking-v1.md` "What the channel protects" and "Failure and retry",
`e2ee-mls-v1.md` "Threat model", `web-plugins-v1.md` "Security",
`relay-proxied-workspaces-2026-10-09.md`, ui `docs/specs/history-archive-v1.md` "What a drive
provider learns". This table joins them and points to the items in section 6 that probe each
row.

| Adversary | Can (by design) | Cannot (claimed) | Probed by section 6 items |
|---|---|---|---|
| Malicious server (database and code) | See metadata, mentions, sizes, timing; withhold, delay, reorder; refuse Commits; control membership; serve old logs | Read content; add its own device; forge messages; move ciphertext; force last-resort re-adds | 1, 2, 3, 4, 5, 6, 9, 11, 14, 19 |
| Database reader only (a leaked backup) | Read hashed tokens, sealed link bytes, ciphertext, franking key `K`, franked content | Read content or sealed messages | 7 |
| Someone who saw a link code | Post an offer first; read the answer | Get a certificate unless the person confirms | 24 |
| Thief of an old identity key | Sign statements at its own `keySeq` | Survive a recovery (servers revoke every device) | 3, 11, 17 |
| Compromised desktop (code execution as the user) | Read the vault, cache, MLS state until removed | Read after removal | 8, 12, 13, 18 |
| Compromised webview (XSS in the desktop app) | Ask Rust for signatures and decryptions | Read private keys | 12 |
| Compromised browser page or profile | Sign with the device key; read MLS keys and plaintext | Export the device key | 8, 12, 22 |
| Malicious group member device | Send as itself; commit; lie in its AAD and summaries | Make others accept a lying message; read after removal | 5, 20, 25 |
| Malicious plugin | What its grants allow; anything as the user if local-process | Read keys through the service | 13 |
| Drive provider | Delete, withhold, roll back archive files | Read them or forge backups | 17 |
| Former member | Keep what it decrypted | Read after removal | 2, 6 |
| Quantum adversary recording traffic | Break Ed25519 later | Decrypt recorded MLS traffic (hybrid KEM) | 11, 26 |
| Local attacker with file access | Roll back or corrupt the vault and cache | Read without the OS store key | 8, 18 |

## 5. How to run everything

CI on GitHub Actions is over its quota since 2026-10-09; every result in the PRs comes from local
runs of the commands below. Rust 2024 toolchain as pinned in `rust-toolchain.toml` (server);
rim-ui's `src-tauri` workspace builds with its own `Cargo.lock`.

### 5.1 Server (rim-server)

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings

cargo test -p rim_identity            # statements, logs, linking, mls-key, readd, manifests, all vectors
cargo test -p rim_services group_id   # group id vector A.1
cargo test -p rim_infra_services key_auth device_link e2ee franking encrypted_message

# Need PostgreSQL 17 and DATABASE_URL; these panic without it, by design.
export DATABASE_URL=postgres://postgres:postgres@localhost:5432/rim_test
sea migrate up
cargo test -p rim_infra_db --features db-tests -- --test-threads=1
cargo test -p rim_api --features e2e-tests -- --test-threads=1
#   key_identity_e2e, device_link_e2e, e2ee_e2e, sessions_e2e, workspace_plugin_e2e
cargo test -p rim_infra_services --features infra-db-tests -- --test-threads=1
```

### 5.2 Clients (rim-ui)

```sh
cd src-tauri
cargo test -p rim_mls                    # vectors, formats, providers, simulation, tampering, sizes
cargo test -p rim_mls vectors -- --nocapture
cargo test -p rim_mls_wasm
cargo test key_identity                  # statements, recovery kit, device linking, manifests
cargo test identity_backup history_archive secret_store
cargo test e2ee plugin_view              # app side against rim_mls and real cache databases
cd ..
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.114 --locked
npm ci
npm run test:wasm                        # rim_mls and rim_mls_wasm in headless Chromium (chromedriver on PATH)
npx vitest run src/features/key-identity src/features/e2ee src/features/plugins/chat
npm run build:wasm                       # rebuilds the committed module; wasmArtifact.test.ts checks its sources
```

### 5.3 A server and two desktops by hand

`docs/testing/manual-test-plan-2026-10-10.md` in rim-server, section 0, sets up the server
(`RIM_KEY_LOGIN_AUDIENCES=localhost:8080,127.0.0.1:8080`), two desktop instances as two OS users
(each needs its own vault and credential store entry) and the web app. Sections 1 and 3 walk
key-only signup, linking, recovery, the identity-key backup, encrypted DMs and groups,
attachments, revocation, lost state and re-adds, and the web client. Nothing in it has run on two
real devices yet.

### 5.4 Contact

The project is run by the maintainers of the `rim-platform` organisation on GitHub
(repositories above). The reporting channel and contact details are in the engagement letter.
Please report anything you would rate high or critical before the final write-up.

## 6. Known gaps and open questions

Numbered for reference from sections 3 and 4. Each names where it lives at the commits in 1.1.
"Ask" is what we want the reviewer's view on. The first eleven are what we would fix first.

### Server substitution and client trust in server-provided data

1. **The client takes "encrypted" from the server on every render.** `encrypted:
   Boolean(encryptedAt)` from the server's `Conversation.encryptedAt` (ui
   `src/features/e2ee/hooks/useE2eeConversation.ts:183`) decides whether the composer uses the
   encrypted path (ui `src/features/messages/components/MessageTimeline.tsx:295`,
   `Composer.tsx:233`). A server that answers `null` for a conversation this device holds group
   state for gets the next messages through `sendMessage` in plaintext. The spec's "never
   cleared" is a server promise only. Ask: confirm, and whether a local pin (any conversation
   with local group state or an `e2ee_conversation` row is encrypted for good) is enough.
2. **Revocation status is the server's word.** Leaf verification treats a device as revoked only
   if `principalDevices` lists `revokedAt` or a `revocation` (ui/tauri
   `src/e2ee/directory.rs:94`; ui `src/features/e2ee/web/directory.ts:61`). A server that omits
   them keeps a revoked (stolen) device a verified leaf; it also stops listing it as a pending
   remove, so no client commits its removal. The signed revocation is never verified either
   (presence alone counts). Ask: is a client-side revocation list or a signed device-set
   statement (the profile card already lists active devices) the right fix?
3. **Certificates from before a recovery stay valid on clients.** `certified_device` (ui/tauri
   `src/key_identity/statement.rs:645`; ui `browser/statements.ts:462`) accepts a certificate
   signed by the identity key at its `keySeq`, with no check that a recovery rotation followed
   that entry, unlike plugin manifests (server `rim_identity/src/plugin_manifest.rs:86`
   `recovery_after`). A thief of the pre-recovery identity key, with a server that lists the new
   device, gets a device every member's client verifies and adds. A retired key after an
   ordinary identity rotation has the same power. The server's own `addDeviceKey` refuses such
   certificates (`device.rs:52`), so this needs a malicious or compromised server. Ask: should
   clients refuse certificates whose `keySeq` precedes a recovery, and should certificates carry
   something that stops a retired key from minting new ones?
4. **Membership is server-owned; clients add whatever the server lists.** Pending adds come from
   `Conversation.mlsGroup` and are committed after the rank delay (ui
   `src/features/e2ee/runtime/groups.ts:287`, `membership.ts:67`); `is_member` comes from the
   server's member list (ui/tauri `src/e2ee/directory.rs:147`). A malicious server can add a
   member it controls (an account with its own key identity) to a DM or group and every client
   commits its devices. Nothing pins a DM to two principals or asks the person. Ask: what is the
   minimum (DM pinning, a visible "X was added by Y" that a member signs, refusing adds not
   initiated by a member device)?
5. **Sender binding depends on a server field.** The author check in `decrypt` runs only when
   `DecryptExpectations.author_principal_id` is `Some` (ui/tauri `crates/rim_mls/src/group.rs:144`,
   `:854-861`), and the app fills it from the server's `author.principalId` (ui/tauri
   `src/e2ee/feed.rs:339`; ui `src/features/e2ee/web/feed.ts:529`). A server that returns no
   principal id for the author can show member A's message as member B's: the row's `author_id`
   is what the timeline shows. The server's `senderDeviceId` is never compared with the
   decrypted sender device. Ask: make a missing principal id invalid, and show the MLS sender
   rather than the row's author?
6. **A replayed Welcome can roll a device's group state back.** `join` builds with
   `replace_old_group()` and `skip_lifetime_validation()` (ui/tauri
   `crates/rim_mls/src/group.rs:691-692`); the app refuses only an older incarnation, not an
   older epoch of the same incarnation (ui/tauri `src/e2ee/group.rs:466`). The server keeps
   Welcomes for 30 days; a Welcome to a last-resort key package whose private key the device
   still holds can be delivered again through `myMlsWelcomes`, replacing the current state with
   the old epoch. Effects we see: messages of that epoch decrypt again (breaking decrypt-once),
   the device falls out of step and asks for a re-add. Ask: refuse a Welcome whose epoch is not
   above the held state's, and are there worse effects?

### Franking

7. **Whoever reads the database can forge "verified" reports.** The receipt key `K` is one
   `franking_key` row created on first use, stored in plaintext, never rotated (server
   `rim_infra_db/src/sea_orm_impl/e2ee.rs:1297`). Verification checks only the receipt over a
   reporter-supplied commitment; it does not compare with the row's stored
   `franking_commitment` (server `rim_infra_services/src/e2ee/mod.rs:1832-1851`). With `K` from a
   backup, a member can report any text against any real message id of any sender and the server
   marks it verified. The moderator trusts the server's verdict by design. Ask: move `K` out of
   the database (configuration or KMS), rotate it with key ids, and require the reported
   commitment to equal the row's?

### Rollback and reuse

8. **State rollback replays MLS secrets.** Restoring the desktop's SQLCipher cache (Time Machine,
   a disk image) together with the vault, restoring a browser profile, or two tabs running
   without the Web Lock (ui `src/features/e2ee/web/webSession.ts:24-25`) restart the sender
   ratchet at generations already used. Only OpenMLS's random 4-byte reuse guard then separates
   two encryptions under one AES-128-GCM key from nonce reuse. Neither the vault (ui/tauri
   `src/secret_store.rs`) nor the cache has rollback detection. Ask: how bad is this in practice,
   and should a device self-update (or request a re-add) when it detects it restarted from an
   older state (for example a monotonic counter in the OS credential store)?
9. **Re-add requests have no freshness.** The server checks only the signature, the
   incarnation and `lastEpoch ≤ epoch` (server `e2ee/mod.rs:1597-1690`); committers check only the
   signature and their own 24-hour limit (ui/tauri `crates/rim_mls/src/readd.rs:78`, `:119`). The
   nonce is never checked for reuse and `issuedAt` is ignored. A server can store a genuine
   request and replay it later in the same incarnation, forcing a Remove and Add of that device
   once a day and, after 10 minutes without a Commit, opening the recovery path to a new
   incarnation (server `e2ee/mod.rs:494` `incarnation_ground`). Ask: bind requests to the epoch they were made in
   (`lastEpoch` must equal the device's last known epoch) and record nonces?

### Clock skew

10. **Any client clock ahead of the server stops key package uploads.** The client sets
    `not_after = client_now + 90 days` exactly (ui/tauri `crates/rim_mls/src/device.rs:224`); the
    server refuses `not_after > server_now + 90 days` (server `e2ee/mod.rs:1113`). A device whose
    clock runs ahead by more than the upload latency never becomes MLS-active. Receivers also
    validate an added key package's lifetime with their own clock against a one-hour
    `not_before` slack; a receiver more than an hour behind cannot process the Commit and asks
    for a re-add. Server-side windows (challenge expiry, claim validity of one hour, 14-day claim
    margin, 60-day activity) use the application node's clock while the database stores the
    times; nodes with skewed clocks disagree. Ask: margins, and which checks should use database
    time.
11. **No pinning of key-log heads.** Clients replay whatever log the server serves. A server can
    serve an older prefix (hiding a rotation or a recovery), or, with a stolen old identity key,
    a fork for a principal it has never seen. The post-quantum decision names head pinning as the
    planned mitigation; nothing implements it (no store of seen heads in ui/tauri
    `src/e2ee/directory.rs` or `src/key_identity/`). Ask: is pinning per contact on first use
    enough until Stage 5's multi-relay cards?

### The desktop's webview and local plugins

12. **The webview drives signing and sees two secrets.** The desktop webview sees the recovery
    kit text at enrollment and signup, the phrase typed at recovery and takeover, and the full
    link code with its secret. It chooses the endpoint the Rust side derives a login audience
    from (ui/tauri `src/key_identity/mod.rs:662` `sign_login`, called from `commands.rs:137`) and
    supplies the inputs to revocations, profile cards and plugin manifests signed with the
    identity key (`mod.rs:778`, `:840`, `:876`). An XSS in the desktop app (DOMPurify is the
    backstop) gets login proofs for any audience and identity-key signatures on attacker-chosen
    manifests. Ask: which commands should require a Rust-side confirmation or pin their inputs?
13. **Local-process plugins can read every secret.** They run as the user; the credential store
    hands an unlocked item to any process of the session and the vault file is readable by the
    user. `chat.read.encrypted` constrains sandboxed web plugins and honest local ones only (ui
    `docs/plugin-chat-methods.md`, `web-plugins-v1.md` "Security"). Plugin secret settings,
    specified on 2026-10-18 after the commits in 1.1 (`plugin-protocol-v1.md`, section 15), are
    kept in the same vault and handed to the plugin that declares them in plain form, so they are
    kept from other plugins and web pages, not from that plugin. Ask: is the consent wording
    enough, or should encrypted grants be refused to local-process plugins?

### Login and errors

14. **Revocation is disclosed before the challenge is checked.** `verify_login` answers
    `FORBIDDEN` for a valid signature from a revoked device before the audience and challenge
    checks (server `rim_infra_services/src/key_auth.rs:441-453`). The spec says "a valid proof".
    Any proof from that device, however old or for whatever server, reveals revocation here.
15. **The audience defaults to localhost.** With neither `RIM_KEY_LOGIN_AUDIENCES` nor
    `RIM_PUBLIC_BASE_URL` set, a server accepts `localhost:PORT` and `127.0.0.1:PORT` (server
    `rim_server/src/config.rs:370`) and logs the list. A deployment behind a proxy that forgets
    both accepts proofs meant for any other misconfigured server on the same port.

### Key separation and lifetimes

16. **The identity seed doubles as HKDF input.** The Ed25519 identity seed derives the archive
    wrap key (ui/tauri `src/key_identity/backup.rs:48`, salt `rim.history-archive.v1`, info
    `archive key wrap`). The HKDF salt and info separate the outputs, but the seed now has two
    roles. Ask: acceptable, or derive both the signing seed and the wrap key from a root?
17. **The archive key never rotates.** An identity rotation replaces this device's
    `identity-v1` wrap entry, but a drive provider can keep the old `key-wraps.json`; with a
    retired identity key the archive key opens. Likewise a passphrase change cannot revoke old
    manifest copies. Ask: should a rotation or recovery re-key the archive?
18. **One bad vault record discards the vault.** `load` verifies every record and moves the file
    aside if any fails (ui/tauri `src/secret_store.rs:290-331`, `:448`), losing device and
    identity keys on that computer; `"v": 2` makes every call fail. Ask: fail per record?
19. **The MLS signature key never rotates.** One per device; `issuedAt` in `mls-key` is not
    checked anywhere. A leaked MLS key signs as that device until a lost-state re-add. Ask:
    rotate with self-updates?

### MLS details

20. **Lying Commits are merged before they are flagged.** `check_summary` (ui/tauri
    `crates/rim_mls/src/group.rs:617`) marks a mismatch, then `process_commit` merges anyway
    (`:573`). Proposals other than Add and Remove (GroupContextExtensions, PSK, ReInit) inside a
    Commit only make the summary fail. Ask: which should cause a refusal and a re-add instead?
21. **Last-resort key packages are shared across claims.** A last-resort package is handed out
    whenever a device has no one-time package left (server `e2ee/mod.rs:1215`), up to more than
    5 claims or 7 days before expiry before the device replaces it, and its private key stays up
    to 7 days after. Ask: the forward-secrecy cost, given that item 6 also relies on it.
22. **Verifiers disagree on what a valid Ed25519 key is.** The server refuses non-canonical point
    encodings (server `rim_identity/src/encoding.rs:144`); the desktop's `verify` and
    `usable_device_key` (ui/tauri `src/key_identity/statement.rs:187`, `:218`) and `rim_mls`'s
    `parse_tagged_key` (`crates/rim_mls/src/identity.rs:114`) check only `is_weak`; the browser
    checks canonicality itself (`statements.ts:79`). The desktop's log replay also skips the
    `identityKey != recoveryKey` rule and accepts unknown fields. Ask: does any split matter?
23. **Zeroization gaps.** `rim_identity` has no `zeroize`: `LinkSecret`, derived link keys and
    test signing keys stay in memory (server `rim_identity/src/device_link.rs:140-170`, `:304`);
    the server's franking key is a plain `Vec` (`message.rs:631`). The webview's strings (phrase,
    code) cannot be wiped. The web keeps MLS keys and plaintext unencrypted in IndexedDB
    (`e2ee-web.md` "Limits"). `DecryptedMessage.content_bytes` is a plain `Vec` (ui/tauri
    `crates/rim_mls/src/group.rs:168`). The desktop's Rust side otherwise wraps secrets in
    `Zeroizing`. Ask: which of these matter given the threat model?
24. **The link fingerprint is 40 bits and the race is UX-dependent.** See area (c), question 1.
25. **Edits share one receipt time.** See area (i), question 2.

### Spec against code, and dependencies

26. **What the specs promise and the code does not do yet.**
    - The franking moderator view and the report-scoped attachment download
      (`e2ee-mls-v1.md` "Franking" step 5) are not on `master` in either repository; no GraphQL
      field exposes `franked_content`.
    - A report "needs no message row" (`e2ee-mls-v1.md` "Franking" step 4); the code requires the
      row (server `e2ee/mod.rs:1814-1820`).
    - "A valid proof from a revoked device" answers `FORBIDDEN` (`key-identity-v1.md` "Challenge
      login" rule 5); the code does not check the challenge or audience first (item 14).
    - `encrypted_at` is "never cleared" (`e2ee-mls-v1.md` "Scope"); clients do not hold the
      server to it (item 1).
    - The cache key "is re-wrapped under the device key in Stage 3" (ui/tauri
      `src/message_cache/key.rs:4`); it is not.
    - The library decision lists the IETF `mls-implementations` vectors for suite 0x0001 as
      re-run in RIM's CI; `crates/rim_mls/src/tests/vectors.rs` runs primitive vectors only.
    - The decision names `libcrux-ml-kem` 0.0.11 (2026-10-07) among the sources; the lock files
      link 0.0.10, which the ACVP test pins (`=0.0.10` in `crates/rim_mls/Cargo.toml`).
27. **Errors that disclose.** Anonymous `rotateIdentityKey` refusals other than "unknown
    principal" carry the validation reason (documented: they show a principal is enrolled);
    serde parser messages reach clients as `VALIDATION` text; `principalDevices` answers empty
    for principals outside the caller's workspaces. Ask: anything here worth closing?

## Appendix A: known-answer vectors and the tests that check them

Server paths are under rim-server, client paths under rim-ui.

| Vector | Spec | Checked by |
|---|---|---|
| Version 1 keys, inception, principal id, `prev`, certificate, login proof | `key-identity-v1.md` appendix A | server `rim_identity/src/vectors.rs::known_answer_vectors`; ui/tauri `src/key_identity/statement.rs::tests::known_answer_vectors_match_the_spec`; ui `browser/statements.test.ts` "appendix A: a version 1 log, certificate and login proof verify" |
| Version 2 inception, certificate, login proof | `key-identity-v1.md` appendix B | server `rim_identity/src/vectors.rs::known_answer_vectors_v2`; ui/tauri `statement.rs::tests::known_answer_vectors_match_the_spec`; ui `statements.test.ts` "appendix B: the inception replays…", "appendix B: this module signs the login proof byte for byte" |
| Mixed log (version 2 rotation of a version 1 log) | `key-identity-v1.md` appendix B | server `rim_identity/src/vectors.rs::known_answer_mixed_log`; ui `statements.test.ts` "appendix B: the mixed log replays and keeps appendix A's principal id" |
| Recovery phrase (entropy, phrase, recovery key) | `key-identity-v1.md` "Recovery kit" | ui/tauri `src/key_identity/recovery_kit.rs::tests::the_phrase_matches_its_known_answer_vector` |
| Link code, `K_offer`, `K_answer`, AADs, offer and answer ciphertexts, fingerprint | `device-linking-v1.md` appendix A | server `rim_identity/src/device_link.rs::tests::known_answer_code_keys_and_aad`, `known_answer_offer`, `known_answer_answer`; ui/tauri `src/key_identity/device_link.rs::tests::appendix_a_code_keys_and_aad`, `appendix_a_offer_seals_opens_and_fingerprints`, `appendix_a_answer_seals_and_opens`; ui `browser/channel.test.ts` "the code, the channel keys and the AADs", "the offer seals, opens and fingerprints byte for byte", "the answer seals and opens byte for byte" |
| Plugin manifest statement | `workspace-plugins-v1.md` appendix A | server `rim_identity/src/plugin_manifest.rs::tests::known_answer_statement_verifies_against_the_known_inception`, `known_answer_bundle_matches_the_statement`; ui/tauri `src/key_identity/plugin_manifest.rs::tests::known_answer_statement_verifies`, `signing_writes_the_known_payload_byte_for_byte`; ui `browser/pluginManifest.test.ts` "appendix A verifies against appendix B's inception" |
| Backup keys (phrase and v1 seed), identity wrap key | ui `docs/specs/history-archive-v1.md` "Known-answer vectors" | ui/tauri `src/key_identity/backup.rs::tests::the_backup_and_wrap_keys_match_their_known_answer_vectors` |
| Sealed identity backup, `identity-v1` wrap | same | ui/tauri `src/history_archive/identity.rs::tests::a_sealed_backup_matches_its_known_answer_vector`, `an_identity_wrap_matches_its_known_answer_vector` |
| A.1 group id | `e2ee-mls-v1.md` | server `rim_services/src/e2ee.rs::tests::group_id_known_answer`; ui/tauri `crates/rim_mls/src/tests/formats.rs::group_id_vector` |
| A.2 `mls-key` statement, credential identity hash | `e2ee-mls-v1.md` | server `rim_identity/src/mls.rs::tests::known_answer_mls_key_statement_and_credential`; ui/tauri `formats.rs::mls_key_statement_and_credential_identity_vectors` |
| A.2 recorded key package and `KeyPackageRef` (fixture) | `e2ee-mls-v1.md` | ui/tauri `formats.rs::the_recorded_key_package_is_the_vector_device_s` (fixture `crates/rim_mls/src/tests/fixtures/recorded.json`) |
| A.3 attachment key, nonce, blob, hash | `e2ee-mls-v1.md` | ui/tauri `formats.rs::attachment_vector`, `src/e2ee/tests.rs::appendix_a3_attachment_round_trip`; ui `src/features/e2ee/web/attachments.test.ts` "match appendix A.3 byte for byte"; server `rim_api/tests/e2ee_e2e.rs::encrypted_attachments_are_checked_by_hash` (hash only) |
| A.4 content, commitment, application data, application and Commit AAD | `e2ee-mls-v1.md` | ui/tauri `formats.rs::content_franking_and_authenticated_data_vectors`; server `e2ee/franking.rs::tests::known_answer_commitment_and_receipt` (commitment), `e2ee/aad.rs` tests (AAD parsing) |
| A.4 recorded PrivateMessage (fixture, decrypts once) | `e2ee-mls-v1.md` | ui/tauri `formats.rs::the_recorded_private_message_decrypts_to_the_vector_application_data` |
| A.5 franking receipt input and receipt | `e2ee-mls-v1.md` | server `e2ee/franking.rs::tests::known_answer_commitment_and_receipt`; ui/tauri `formats.rs::franking_receipt_vector` |
| A.6 re-add request | `e2ee-mls-v1.md` | server `rim_identity/src/mls.rs::tests::known_answer_readd_request`; ui/tauri `formats.rs::readd_request_vector` |
| Ed25519 RFC 8032 §7.1 tests 1 to 3 | `mls-library-2026-10-09.md` §5 | ui/tauri `crates/rim_mls/src/tests/vectors.rs::ed25519_rfc8032_tests_1_to_3` (both providers) |
| X25519 RFC 7748 §5.2, §6.1 | same | `vectors.rs::x25519_rfc7748_sections_5_2_and_6_1` |
| HPKE RFC 9180 A.1 base mode | same | `vectors.rs::hpke_rfc9180_a1_base_mode` |
| HKDF-SHA256 RFC 5869 A.1 to A.3 | same | `vectors.rs::hkdf_sha256_rfc5869_a1_to_a3` |
| HMAC-SHA256 RFC 4231 | same | `vectors.rs::hmac_sha256_rfc4231` |
| AES-GCM Wycheproof (96-bit nonces) | same | `vectors.rs::aes_gcm_wycheproof_96_bit_nonces` (file `tests/vectors/aes_gcm_wycheproof.json`) |
| X-Wing draft vectors | same | `vectors.rs::xwing_draft_vectors_through_both_hpke_backends` (file `tests/vectors/xwing.json`; encapsulation checked as a cross-provider property) |
| ML-KEM-768 NIST ACVP keyGen, encapDecap | same | `vectors.rs::ml_kem_768_acvp_through_libcrux`, `ml_kem_768_acvp_through_rust_crypto` (file `tests/vectors/ml_kem_768_acvp.json`) |
| 0x004F key packages validate on the server | — | server `rim_infra_services/src/e2ee/openmls_wire.rs::tests::suite_0x004f_key_packages_validate_and_decode`, `commits_welcomes_and_messages_expose_their_framing` |

Negative tests worth reading first: server `rim_identity/src/device_link.rs::tests::a_flipped_aad_nonce_tag_or_ciphertext_byte_fails`,
`an_offer_never_opens_as_an_answer_or_under_another_link`; server
`e2ee/franking.rs::tests::any_changed_input_fails_the_receipt`; ui/tauri
`crates/rim_mls/src/tests/tampering.rs::every_leaf_verification_failure_is_reported`,
`application_messages_whose_authenticated_data_lies_are_invalid`,
`commits_whose_summary_lies_are_flagged`, `the_server_refuses_a_summary_it_cannot_match`.

## Appendix B: cryptographic dependencies as locked

From `Cargo.lock` at the commits in 1.1 (rim-server root; rim-ui `src-tauri/Cargo.lock`). Where
two versions of a crate are locked, both appear; the second is pulled transitively. The web app
uses the browser's WebCrypto for Ed25519, HKDF, AES-GCM and SHA-256 and has no JavaScript crypto
library.

| Crate | rim-server | rim-ui | Used for |
|---|---|---|---|
| `openmls` | 0.9.1 | 0.9.1 (`=0.9.1`) | MLS |
| `openmls_traits` | 0.6.0 | 0.6.0, feature `draft-ietf-mls-pq-ciphersuites` | Provider and storage traits |
| `openmls_rust_crypto` | 0.6.0 (server provider) | 0.6.0 (dev: fallback and cross-provider tests) | Server key package validation |
| `openmls_libcrux_crypto` | — | 0.4.0, feature `draft-ietf-mls-pq-ciphersuites` | Client provider |
| `openmls_memory_storage` | 0.6.0 | 0.6.0 | Pulled by OpenMLS |
| `hpke-rs`, `hpke-rs-crypto`, `hpke-rs-libcrux`, `hpke-rs-rust-crypto` | 0.7.0 | 0.7.0 | HPKE in both providers |
| `libcrux-ml-kem` | 0.0.10 | 0.0.10 | ML-KEM-768 (libcrux path) |
| `libcrux-kem`, `libcrux-aead`, `libcrux-aes`, `libcrux-chacha20poly1305` | 0.0.9 | 0.0.9 | libcrux primitives |
| `libcrux-curve25519`, `libcrux-ecdh`, `libcrux-hkdf`, `libcrux-hmac`, `libcrux-sha2`, `libcrux-p256`, `libcrux-traits` | 0.0.8 | 0.0.8 | libcrux primitives |
| `libcrux-ed25519` | — | 0.0.9 | libcrux signatures |
| `libcrux-sha3` | 0.0.10 | 0.0.10 | ML-KEM hashing |
| `libcrux-hacl-rs`, `libcrux-poly1305`, `libcrux-secrets`, `libcrux-hmac-drbg` | 0.0.5, 0.0.6, 0.0.6, — | 0.0.5, 0.0.6, 0.0.6, 0.0.1 | libcrux support |
| `x-wing` | 0.1.1 | 0.1.1 | X-Wing (RustCrypto path) |
| `ml-kem` | 0.3.2 | 0.3.2 | ML-KEM-768 (RustCrypto path) |
| `ed25519-dalek` | 2.2.0 | 2.2.0 | Statements, `verify_strict`, MLS key on the desktop side |
| `curve25519-dalek` | 4.1.3, 5.0.0 | 4.1.3, 5.0.0 | Ed25519, X25519 |
| `x25519-dalek` | 2.0.1, 3.0.0 | 2.0.1, 3.0.0 | X25519 (RustCrypto path) |
| `aes-gcm`, `aes` | 0.10.3, 0.8.4 | 0.10.3, 0.8.4 | Device link, attachments |
| `chacha20poly1305` | 0.10.1 | 0.10.1 | Vault, archive |
| `hkdf` | 0.12.4, 0.13.0 | 0.12.4, 0.13.0 | Link keys, recovery derivations, attachments, archive |
| `hmac` | 0.12.1, 0.13.0 | 0.12.1, 0.13.0 | Franking, CSRF |
| `sha2` | 0.10.9, 0.11.0 | 0.10.9, 0.11.0 | Hashes |
| `argon2` | 0.5.3 | 0.5.3 | Passwords (server), archive passphrase (client) |
| `bip39` | — | 2.2.2, feature `zeroize` | Recovery phrase |
| `subtle` | 2.6.1 | 2.6.1 | Constant-time comparisons (through `hmac` and others) |
| `zeroize` | 1.9.0 | 1.9.1 | Secret wiping (client) |
| `getrandom` | 0.2.17, 0.3.4, 0.4.3 | 0.1.16, 0.2.17, 0.3.4, 0.4.3 | OS randomness; WebCrypto on wasm |
| `rand_core` | 0.6.4, 0.9.5, 0.10.1 | 0.5.1, 0.6.4, 0.9.5, 0.10.1 | RNG traits |
| `tls_codec` | 0.5.0 | 0.5.0 | MLS encoding |
| `keyring` | — | 3.6.3 (`apple-native`, `windows-native`, `sync-secret-service`, `crypto-rust`) | OS credential store |
| `rusqlite`, `libsqlite3-sys` | —, 0.30.1 (server tooling) | 0.40.2, 0.38.2 (`bundled-sqlcipher`) | SQLCipher message cache and MLS state |
| `wasm-bindgen` | 0.2.126 (transitive) | 0.2.114 (`=0.2.114` in `rim_mls_wasm`) | wasm facade |
