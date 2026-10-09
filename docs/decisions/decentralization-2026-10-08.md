> **Status:** Accepted 2026-10-08 by Jakub. Implementation follows Stages 1 to 5 in `ROADMAP.md`.
> **Do not edit** once implementation lands — historical record. For changes, write a new ADR.
> **Amended** 2026-10-09 by `relay-proxied-workspaces-2026-10-09.md`: workspace traffic now goes through the user's relay; hosts accept any relay.

# Decentralization: key identity, interchangeable relays, hosted workspaces

**Date:** 2026-10-08
**Decided by:** Jakub
**Supersedes:** ROADMAP Track 7.1/7.4 "decide the model after a research spike" and the
2026-10-04 lean toward server-to-server federation with portable identities.

## Context

RIM's second pillar is decentralization. Jakub's hard requirements (2026-10-08):

- A user can run RIM on another device and get their chat history back.
- History is saved on the user's own drive (Google Drive, Dropbox, a folder, S3).
- No central RIM server anyone depends on.
- No domain or home server required per user.

Today identity is a username and password per server, `Principal` is an in-memory struct with
no table, and the client keeps no history on disk.

## Decision

**A person is a key pair; servers are interchangeable relays running the same rim-server; each
workspace is hosted on one server that enforces its roles, bans and private channels.**

- **Identity.** The app creates an inception key and device keys with a signed rotation log.
  A signed profile card lists the person's relay URLs. Devices link by QR code; a 12-word
  recovery phrase and the recovery key in the rotation log restore identity. The identity key
  stays off day-to-day devices, which hold only device keys it signed.
- **History.** An encrypted local store on each device plus an encrypted archive on the
  user's own drive, written as per-device segments under one archive key.
- **Relays.** Any rim-server can run in relay mode and is swappable with no data loss.
- **Workspace hosts.** One server hosts each workspace and is the authority for its
  membership, roles, bans and private channels, and its plugin registry.
- **Encryption.** MLS (OpenMLS, on the Tauri side) for DMs and private groups. Channels stay
  plaintext so server search, pipelines and the agent keep working there.
- **Contracts.** GraphQL stays the client contract. Server-to-server traffic is a small signed
  HTTP API (HTTP Message Signatures with replay protection and rotatable server keys), a
  non-GraphQL transport following the `file_handlers.rs` pattern, not GraphQL.

## Confirmed answers

| Question | Answer |
|---|---|
| Does the project run a public relay? | Yes: `relay.rim.chat`, stock rim-server in relay mode, with a runbook and abuse policy; registering a key needs proof-of-work or an invite |
| DMs from strangers? | Go to a request inbox first. Workspace members and invited contacts are never strangers |
| What is a user id? | A stable user id (`principalId`, opaque to plugins) with a signed key log, stable across key changes; it is the hash of an inception key |
| How does the web app get keys? | It is a linked device of the desktop app (non-extractable WebCrypto keys in IndexedDB; the UI says this is weaker than desktop) |
| What goes through relays? | Apps connect to each workspace host directly for workspace traffic. Relays carry only DMs, the request inbox, profile cards and MLS key packages, never channel messages |

## Rejected alternatives

- **Stay single-server:** no federation and export only as an admin tool, so history is not the user's and a server going away takes the community with it.
- **Domain federation (`name@domain`, home servers):** identity is tied to a domain, so only people who own one are fully independent; it may return later as an optional alias on top of a key.
- **Fully replicated federation (Matrix):** every server keeps the whole room and votes on its state, at the price of state resolution, heavy storage and complex moderation.
- **Pure relay network (Nostr):** identity is fully portable, but roles, bans and private channels are weak because no server is authoritative. Its key identity and relays are kept.
- **Peer-to-peer:** delivery needs both sides online, NAT pushes traffic through a relay anyway, and search, roles and history for new members become research problems.

## Consequences

- The plugin protocol v1 identity shape is `{ principalId, displayName }` before the SDK
  freezes (Track 7.0, Stage 1). Ids minted before Stage 3 are provisional and migrate via
  `rim.principal.migrated`.
- Keys arrive as nullable columns on the existing `user` row with a kind of local or remote,
  so the dozens of tables that reference the user id do not move. Bots, ingest tokens and the
  CLI keep host-local credentials and host-scoped ids.
- Track 6.1 (TOTP, passkeys) folds into key login (Track 7.2).
- Drive providers are built-in Rust modules behind one trait in the Tauri app, not SDK plugins.
- The server needs a message change feed (Stage 2) so edits and deletes reach client caches.
- Every server feature that reads message text needs a client replacement in encrypted
  conversations; the Stage 2 local index is the first.

## Open decisions

Proceeding on the recommended answer until Jakub says otherwise.

| Question | Recommended |
|---|---|
| Readable names for keys in Stage 5: contact lists, alias or directory? | Contact lists |
| May plugins read decrypted DMs with a per-plugin grant? | Grant |
| First drive providers after the local folder: Google Drive, Dropbox or both? | Google Drive |
| Mobile: PWA after Stage 3, or native later? | PWA |

Not blocking before Stage 5:

- Do relays delete messages once the drive has them?
- Does a workspace host accept any relay, or only approved ones?
- Matrix interop (bridge or none)?
- Is losing the phrase, the drive and every device together an acceptable total loss? (The plan assumes yes.)

The full list of open product decisions is in `ROADMAP.md` under **Open Decisions**.
