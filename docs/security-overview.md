# Security overview

RIM is a chat server with end-to-end encrypted DMs and groups, identities that are key pairs, and
federation between servers. This page says what it protects, from whom, and where the limits
are, for someone deciding whether to run a RIM server or to join one. Each section links the spec
or document that is the contract; where this page and a spec differ, the spec is right.

## What RIM protects and what it does not

| Asset | Stance |
|---|---|
| DM and group messages | End-to-end encrypted with MLS. Servers store, order and deliver ciphertext and never read it. |
| Channel messages | Plaintext on the server on purpose, private channels included. The server that hosts the workspace can read them. |
| Metadata | Not hidden. Servers see members, devices, senders, times, sizes, mentions, reactions, pins and threads. |
| Identities | A person is a key pair. Servers check and store what a person signs and cannot sign for them. |
| Key material | Private keys stay on devices. A server holds only its own server key and franking keys, in files outside the database. |
| Audit log | Who ran which command or moderation action, and the outcome. Owners and Moderators read their workspace's rows; everyone reads their own. |
| Plugin reach | A sandboxed plugin reaches only what it was granted. A plugin that runs on the computer runs as the user. |

## Identity

A person is a principal: an id `rimp1_…` that hashes the first statement of their key log, which
names an identity key and a recovery key. The log grows only by signed rotations, at most 256
entries. Every statement carries a context for its purpose, is verified as exact text, and is
checked with strict Ed25519 ([key identity](specs/key-identity-v1.md)).

- **The keys.** The identity key signs the log, device certificates, revocations and the profile
  card. The recovery key comes from a 12-word recovery phrase (128 random bits) and is never
  stored. Each device has its own device key, certified by the identity key; only device keys
  sign in.
- **Challenge login.** The server issues a single-use challenge valid for 120 seconds. The device
  signs a proof naming the challenge, the principal and this server's audience, so a proof works
  once and only on the server it was made for. A challenge a host issues for joining a workspace
  is bound to the relay that asked for it and never signs anyone in, so a relay cannot turn a
  join proof into a session at the host. A server bound to a public address refuses to
  start without an audience. Sessions are cookies whose tokens the server stores hashed, and a
  mutation made with a session needs a CSRF token.
- **One kind of account.** Every local account signs up with a key and an account password;
  there is no username and password sign-in and no reset by email.
- **The account password.** A local account also has a password (a PIN or a passphrase),
  checked with OPAQUE ([key identity](specs/key-identity-v1.md), "Account password"): the
  server stores a record that checks the password without revealing it, and a sign-in needs the
  device key's proof and the password together. The server counts every login start as a wrong
  try until a login finishes: three are free, the 4th to the 9th make the next wait from one
  minute to twelve hours, and the 10th locks the password until a reset signed by the recovery
  key. A finished login returns the account's server share, sealed under the login's session
  key; the client needs it with the password to open its local data. The records need the
  password key file `<RIM_DATA_DIR>/password-key.json` (`RIM_PASSWORD_KEY_FILE`), which also
  seals the shares, which `/readyz` checks and which the backups hold; without it every account
  resets its password with its recovery phrase. An account of this server without a password
  does not sign in. People from other servers keep their password at their own server.
- **Keep me signed in.** A key sign-in chooses it. A kept session has persistent cookies; for
  a web client the server also holds a keep key for it, sealed under the password key file,
  which opens the client's data key at start. An unkept session has browser-session cookies and
  a session secret (32 random bytes, stored hashed) that the client holds only in memory and
  that every request carries in a header and the WebSocket in its `connection_init`, so a
  cookie left behind in a closed browser or a copied profile signs nobody in. Plain `GET`
  downloads of files, avatars and attachments are the exception: an `<img>` tag cannot send a
  header, so they answer to the cookie alone, as do `logout` and the reload below. A reloaded
  tab takes back its sealed data with a reload key the server holds, once, and only within 60
  seconds of the session's WebSocket closing and while none is open; after that the account
  is locked on that device until the password is entered again.
- **Linking a device.** The old device shows a code holding a 32-byte secret the server never
  sees. The new device sends its key sealed under that secret; the person compares an
  8-character fingerprint and confirms on the old device, which then signs the certificate
  ([device linking](specs/device-linking-v1.md)).
- **Where the identity key lives.** On the desktop that created it, in the
  [secret vault](../../rim-ui/docs/secret-vault.md). The web app can also create an identity, as
  a non-extractable WebCrypto key in the browser's IndexedDB.
- **Identity-key backup.** With the history archive set up, a desktop keeps the identity key in
  the archive, sealed under a key only the recovery phrase gives. Another desktop of the account
  takes the identity over with the phrase
  ([identity-key backup](../../rim-ui/docs/identity-key-backup.md)). A browser-held identity is
  never backed up.
- **What the server can do.** It cannot sign as the person, add a device of its own to their
  card, or forge a rotation. It can refuse service: refuse logins, withhold data or serve old
  data, and delete what it stores. Each client pins the head of every key log it verified and
  refuses a shorter or forked log afterwards, and refuses device certificates signed before a
  recovery.
- **Recovery.** With every device lost, the phrase signs a recovery rotation naming a new identity
  key and a new device. The server revokes every other device and ends every session. Encrypted
  history comes back only from the history archive.
- **Losing everything.** With every device and the phrase gone, nothing can sign for that
  principal again. The identity cannot be recovered, and the person starts a new one.

## Encryption

DMs and groups are encrypted with MLS (RFC 9420), one group per conversation, with devices as
members ([end-to-end encryption](specs/e2ee-mls-v1.md)). The cipher suite is 0x004F: X-Wing
(X25519 with ML-KEM-768) for a hybrid post-quantum key agreement, AES-128-GCM, and Ed25519
signatures. Signatures and authentication are classical. Attachments are encrypted with
AES-256-GCM under a secret per file; the server stores the blob and checks its SHA-256.

- **Which conversations.** A new DM or group is encrypted when its creator has a device set up for
  it; members who are not ready are invited. A member's desktop upgrades an existing DM or group
  once, behind a visible marker, and older plaintext stays as it was. After that the server
  refuses plaintext writes there.
- **Plaintext on purpose.** Channels, private ones included, stay plaintext: server search,
  pipelines, bots and the agent work there, and later members read the history. In encrypted
  conversations those features run on the device or see metadata only.
- **Who holds which keys.** Each device holds its MLS signature key, its key-package keys and the
  group secrets. Its device key certifies the MLS key, chained to the key log, and every client
  verifies every member's leaf against that chain. The server is the sequencer: it accepts one
  Commit per epoch, owns membership, and validates key packages, but holds no key that could add
  a device of its own.
- **What clients hold the server to.** A conversation a device knows is encrypted stays encrypted
  there, whatever the server answers later. A DM stays pinned to its two people, so the server
  cannot add a third. A message shows its MLS sender, not the row's author. A device refuses
  replayed Welcomes and Commits that lie about membership, and remembers revocations
  ([desktop](../../rim-ui/docs/e2ee-desktop.md), [web](../../rim-ui/docs/e2ee-web.md)).
- **Franked reports.** Each message commits to its content with an HMAC under a key that travels
  inside the encrypted content, and the server adds a receipt under its franking key. A report
  reveals one message and its key to the workspace's moderators. The server checks the receipt
  and that the commitment matches one recorded when the message was sent or edited, so even a
  stolen franking key cannot get text the sender never sent verified. A report proves one
  message, not its context, and its content stays on the server until 90 days after the report
  is resolved.
- **History on a device.** The desktop keeps decrypted messages and MLS state in a SQLCipher
  cache whose key is in the vault. The browser keeps MLS keys and decrypted messages unencrypted
  in IndexedDB; the app says these conversations are only as safe as the browser profile.
- **History in the archive.** The optional archive, in a folder or a cloud drive, is encrypted
  under keys the drive provider never sees
  ([history archive](../../rim-ui/docs/specs/history-archive-v1.md)). Group state and MLS keys
  are never archived. A new device reads older messages only from the archive. The provider can
  delete, withhold or roll back files, but cannot read or forge them.

## Servers and federation

Every server has an Ed25519 server key in a signed key log of its own. Its id `rims1_…` hashes
the log's first statement, and a descriptor signed by the current key is published at
`/.well-known/rim-server`. The key is a file the server refuses to use when others can read it;
the recovery key that replaces a stolen one is kept offline
([server identity](server-identity.md), [server to server](specs/server-to-server-v1.md)).
Requests between servers carry HTTP Message Signatures with a five-minute window and a nonce
store, responses are signed too, and a server pins each peer's key log on first contact. A peer
whose log forks is refused until an operator checks it out of band.

- **A relay** is the server a person's app talks to. It holds their account, devices and profile
  card, and queues host-signed events for them. It can read the plaintext channel events and the
  DM metadata it queues, and drop or delay them. It cannot forge a write, which needs a device
  signature, alter a host's events, which clients check against the pinned host key, or read a
  DM. A queued item goes once every device of the person acknowledged it, and after 30 days by
  default regardless; over the byte quota per person the oldest go first.
- **A workspace host** is the authority for a workspace: membership, roles, kicks, bans,
  timeouts, slow mode and moderator deletes ([messaging](messaging.md)). It learns the
  principals, devices and activity of members from other servers in its workspaces. Those
  members cannot change their account through it.
- **One malicious server against people of another.** A hostile peer can send signed junk up to
  its limits, claim last-resort key packages and send DM requests as a stranger. It cannot
  impersonate another server, replay a request, or push events for spaces it does not host. A
  hostile host cannot sign as a device, read DMs homed elsewhere, or see a member's IP address
  unless that member connects to it directly.
- **Revocation** fans out from the relay to every host, each answering with a signed
  acknowledgement, and the relay stops forwarding what the revoked device signs at once. The app
  goes to a silent host directly after two minutes and names any host that has not confirmed:
  the device keeps access there until its delegation expires, 24 hours at most.
- **A stolen server key** signs as that server until the operator recovers with the recovery key.
  It never signs as a person or reads their DMs. A database dump holds no private key.

## The client

- **The secret vault.** The desktop keeps every secret in one file, each record sealed with
  XChaCha20-Poly1305 under a key in the operating system's credential store, with the record's
  name bound in, so a record moved under another name does not open
  ([secret vault](../../rim-ui/docs/secret-vault.md)). A program running as the user can read
  the file, and the key too while the keyring is unlocked.
- **Sandboxed plugins** run in an iframe with `allow-scripts` only and an opaque origin. They
  cannot reach the RIM session, the CSRF token, device keys, files or other plugins, and a
  per-plugin policy lets them fetch only from the sites their manifest lists and the user allowed
  ([web plugins](../../rim-ui/docs/web-plugins.md), [spec](specs/web-plugins-v1.md)). The shell
  removes WebRTC before any plugin code runs. Small one-way channels remain, such as navigation
  and DNS prefetch, so whatever a plugin is shown could still reach its author.
- **Plugins that run on the computer** run as the user, with the user's access to files, the
  network and local credentials. Their permissions govern what the app does for them, not what
  the program can do. Install steps such as `npm install` run only after a dialog lists each
  command and warns that it runs the author's code ([plugins](../../rim-ui/docs/plugins.md)).
- **Plugin settings** stay on the device, scoped to the plugin, the account and the device, and
  never reach the server or another device. A setting the plugin marks secret is kept in the
  desktop's secret vault and never shown again once saved; the web app has no encrypted store
  for them and offers none. The plugin that declares a secret receives it in plain form, so it
  is kept from other plugins and web pages, not from that plugin or its author
  ([protocol](specs/plugin-protocol-v1.md#15-settings)).
- **The permission prompt** lists each capability on its own line. Encrypted DMs and groups need
  `chat.read.encrypted` or `chat.write.encrypted` on top of the plain permission, and a plugin
  never gets keys, franking keys, attachment secrets or ciphertext. Revoking a permission stops
  the plugin. A desktop plugin whose files change does not start until the user trusts the new
  files.
- **Signed registry plugins.** A workspace's registry pins each plugin version with a statement
  its publisher signed with their identity key. Every client verifies it against the publisher's
  key log and checks the bundle's hash, so the server is not trusted for what the plugin is.
  Plugins installed from a folder, git or a link are labelled unsigned.
- **The AI agent** acts through the command registry with a model and key the user brings. The
  server asks for a confirmation before it runs a mutating command the agent started, and before
  any dangerous command whoever started it; the agent can never confirm one itself. The client
  declares the agent when it calls, so the gate holds the agent to the person, not a modified
  client to the server ([command model](command_model.md)). In encrypted conversations the agent
  is off until the person turns it on, with a warning that the text goes to the model provider,
  and every post there is confirmed.
- **Updates.** The desktop installs an update only when its minisign signature verifies against
  the key built into the app, and only after the user confirms the restart
  ([updater](../../rim-ui/docs/updater.md)). Release checksums are signed with cosign keyless,
  every file carries a build attestation, and the server image is signed with cosign keyless.

## Abuse and limits on a public server

- **Signups** are invite-only by default ([self-hosting](self-hosting.md)). Open registration is
  limited per address, and a proof-of-work can be asked of every signup without an invite; the
  public relay asks 24 bits ([public relay](operations/relay-public.md)). Probation keeps new
  accounts from creating workspaces or messaging strangers for a set number of hours.
- **Password tries.** Every OPAQUE login start counts against the account (the schedule in
  "Identity") and against the address, sharing the key sign-in's 20 failures per address per 15
  minutes. A principal without a password here gets an answer of the same shape, so the start
  does not tell whether an account or a password exists.
- **Rate and instance limits.** Request, mutation, message and connection limits per address,
  session or user, and caps on workspaces, members, devices, DM requests, invites and message
  size, are each off until the operator sets them. Pending invitations to encrypted
  conversations are capped at 50 per person by default.
- **Public join links.** A workspace's permanent link is a 128-bit random token behind the
  prefix `rim_pjl_` (which secret scanners can match), never a name,
  and admits only accounts that already exist: it is never a signup token, so it opens no way
  around proof-of-work, the per-address signup limit or an invite-only server. Every invalid
  link answers `NOT_FOUND` with one message on one path (a lookup by the token's SHA-256), so a
  guesser learns nothing about which workspaces exist or have a link, and 20 misses from an
  address in ten minutes cut it off for an hour. The token is stored as its hash plus a copy
  sealed (XChaCha20-Poly1305, bound to the workspace) under the link key file
  `<RIM_DATA_DIR>/link-key.json`, which `/readyz` checks; a database dump alone yields no
  working link. The Owner can issue a new token or turn the link off at any time, and switch to
  join requests that a moderator decides ([messaging](messaging.md)).
- **DM policy.** The operator sets floors for DMs from strangers and to other servers, a
  workspace Owner sets one for members, and each person chooses within both. By default a
  stranger's first message is a request the recipient accepts, ignores, blocks or reports.
- **Names.** Reserved usernames and display names are refused at signup and on profile edits.
  The `r_` prefix of members from other servers is refused to local signups.
- **Blocking and reports.** Operators block addresses and ranges, peer servers, and accounts by
  key identity, optionally with every account their invites created. Moderators kick, ban, time
  out and delete, and review reports of plaintext messages, franked reports of encrypted ones,
  and reported DM requests.
- **The audit log** records every command run, with secret-looking arguments redacted, and
  every moderation action with ids and flags only, never reasons or message text. Owners and
  Moderators of a workspace read all of its rows; members, guests and everyone else read only
  their own. A request for another user's rows needs a workspace and returns the whole workspace
  log, so it cannot reveal whether that user ever acted there
  ([access matrix](security/audit-access-matrix.md)). Rows are kept forever unless the operator
  sets a retention ([audit retention](operations/audit-retention.md)).

## Threat model

Joined from the spec tables and the review brief's summary.

| Attacker | Can | Stopped by | Out of scope |
|---|---|---|---|
| Compromised device | Read what its cache, archive and MLS state hold; send as that device; on the desktop, read the vault | Revocation, then removal from every group; later epochs use new keys | What it already decrypted; anything else the user's account on that computer reaches |
| Malicious own server | See metadata; withhold, delay, reorder; refuse Commits; control membership; read and change channels; mark any report verified | Pinned key-log heads; leaf checks; DM and encryption pins; MLS sender checks | Availability; channel confidentiality; metadata privacy; the verdict on reports, which moderators take from the server |
| Database or key-file thief | Read channels, metadata, ciphertext and session token hashes; with the franking key file, mint receipts; with the link key file, read the public join links; with the password key file and the database, run offline guesses against a password record and open the server shares | No private key in the database; reports checked against recorded commitments; franking and server key rotation and recovery; OPAQUE records without the key file allow no guessing, and each guess costs an Argon2id run (64 MiB, 3 passes) | Channel text in the dump; a weak PIN once both the database and the key file are taken |
| Another person using the same browser profile or OS account | Open the app on a device where the account was used | The account password at every sign-in, checked by the server with the counted, slowed and locked tries; the server share only after the password; the device key alone signs in no account of this server that has a password | What the device holds unencrypted until the clients seal it under the data key; a kept session left open; an unkept one's file downloads while its cookie lives |
| Another server | Signed junk within limits; DM requests and last-resort key package claims as a stranger | Self-certifying server ids and pinned keys; signed requests with nonces; the request inbox; peer blocks | What a person's own relay or a workspace's host sees |
| Network attacker | See TLS metadata | TLS; signed requests and responses between servers; pinned server ids | Broken TLS between an app and its server; a forged certificate for a server not yet pinned |
| Malicious plugin author | What the granted permissions allow; for a plugin on the computer, anything the user can do | Sandbox and per-plugin policy; the permission prompt; the signed registry; bundle fingerprints | Leaks through channels the sandbox cannot close; a local plugin the user chose to trust |
| Malicious co-member | Keep what it read; commit and send as itself; lie in its own metadata | Receivers refuse messages and Commits that disagree with their content; removal ends reading | What it saw before removal, and whom it shows it to |

## Known gaps and review status

- **Outside review open.** The outside cryptographic review of key identity, device linking,
  recovery, the secret vault, the browser device key, the account password (OPAQUE with
  `opaque-ke` 4.0.1, which has no public audit of its own) and end-to-end encryption is still
  open. The
  0.0.x releases come out before it and are pre-review; `0.1.0` and any 1.0 or production-ready
  claim wait for its sign-off ([review brief](security/crypto-review-brief-2026-10.md),
  [roadmap](../ROADMAP.md)).
- **Fixed, per the brief's status of 2026-10-14.** Clients pin encrypted conversations and refuse
  plaintext there, remember revocations, and pin key-log heads. The franking keys live in a file
  and reports are checked against stored commitments. A login checks challenge and audience
  before it reveals that a device was revoked. A public bind needs a login audience. The brief
  asks that its other items be checked against the code.
- **Also in the code as of 2026-10-18.** DM pinning against added members, the MLS sender check,
  refusal of replayed Welcomes and lying Commits, refusal of certificates older than a recovery,
  strict key checks on clients, freshness and nonces for re-add requests, a clock margin for key
  packages, and a vault that sets aside only the records that fail to open.
- **Still open.** Restoring an older desktop vault and cache, or a browser profile, can replay
  MLS secrets, and nothing detects the rollback. The desktop webview sees the recovery phrase and
  link codes and chooses what the identity key signs. A plugin on the computer can read every
  secret. The archive key and the MLS signature key never rotate, and the identity key's seed
  also derives the archive's wrap key. Last-resort key packages are shared between claims for up
  to seven days. The link fingerprint is 40 bits. Section 6 of the brief lists every item it
  raised, the fixed ones included.
- **Not built.** Hybrid post-quantum signatures, safety numbers or key transparency, message
  deniability, encrypted reactions and pins, encrypted private channels, and an admin policy that
  restricts dangerous plugin capabilities.
- **Distribution.** Windows installers are not code-signed, so SmartScreen warns. There is no
  macOS download: the app is built from source and runs on the Mac that built it
  ([install](../../rim-ui/docs/install.md)). The web app's WebAssembly module is committed and
  checked against a hash of its sources, not rebuilt reproducibly.

## Reporting a vulnerability

Report privately through GitHub's vulnerability reporting, never in a public issue, pull request
or chat room. [`SECURITY.md`](../SECURITY.md) says what to include, which repository a problem
belongs to, and when to expect an answer. A report that matches a gap listed in the review brief
is still welcome, especially with an attack the brief does not describe.

### Plugin link downloads

Authenticated browser installs use the server's `plugin_link_fetch` endpoint. The fetcher
requires a SHA-256 pin and public HTTPS targets, validates every redirect and DNS result,
disables proxies and referrers, and enforces size, time, concurrency and account rate limits.
It verifies both the hash and bundle structure before returning an attachment with `no-store`
and `nosniff`. Audit records include the account and original hostname, without URL paths,
query strings or bundle contents. This route stores no workspace bundle.
