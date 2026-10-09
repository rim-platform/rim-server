> **Status:** Accepted 2026-10-09 by Jakub. Implementation follows Stage 5 in `ROADMAP.md`.
> **Do not edit** once implementation lands — historical record. For changes, write a new ADR.

# Workspace traffic goes through the user's relay, an untrusted courier

**Date:** 2026-10-09
**Decided by:** Jakub
**Amends:** `decentralization-2026-10-08.md`, the confirmed answer "What goes through relays?"
(apps connect to each workspace host directly; relays never carry channel messages) and the
open question "Does a workspace host accept any relay, or only approved ones?".
**Builds on:** `post-quantum-hybrid-2026-10-09.md`, `docs/specs/key-identity-v1.md`.

## Context

The 2026-10-08 decision had the app open one GraphQL connection and one WebSocket per
workspace host, and use its relay only for DMs, the request inbox, profile cards and MLS key
packages. That makes multi-server connections a Stage 5 client feature: rim-ui today talks to
exactly one endpoint. Jakub proposed (2026-10-09) that the app keep one endpoint, its own
relay, and that the relay subscribe to each workspace host on the user's behalf. The relay
already queues DMs for the user's offline devices, so doing the same for workspace events
gives one place for push, catch-up and multi-device fan-out.

This is not the Matrix model rejected on 2026-10-08: the relay forwards and caches, it does
not hold a replica or vote on state. The workspace host stays the single authority on
members, roles, bans, private channels and message order.

## Decision

**The app keeps one GraphQL endpoint and one WebSocket, to its own relay. The relay
subscribes to every workspace host the user belongs to and forwards traffic both ways. The
relay is an untrusted courier, never an authority. A direct connection from the app to a
workspace host stays as the fallback when the relay is unreachable.**

### Joining a workspace

1. The workspace admin (Bob) creates an invite. The link or QR code carries the host's URL,
   a fingerprint of the host's server key, and an invite token signed by the host.
2. Alice's app opens it and checks the token's signature against that fingerprint and the
   host's `GET /.well-known/rim/server` document (fetched through her relay; the fingerprint
   makes substitution detectable). The app pins the host key.
3. The host issues a one-time challenge. Alice's device signs it with the host named as the
   audience, as rim-server #30 already does for key login, so the proof cannot be replayed to
   another server. Her relay carries the challenge and the answer but never vouches for her:
   the host verifies her device signature against her key log.
4. Alice's device signs a **delegation** naming her device key, her relay's server key, the
   one workspace and an expiry. The relay presents it to subscribe. The host authorizes the
   relay per person, per workspace and revocably.
5. The relay subscribes over the signed server-to-server API and forwards host events to
   Alice's devices over her single WebSocket.

### Posting and reading

- Every write is signed by Alice's device and forwarded by the relay unchanged. The host
  verifies the device signature against her key log before accepting it.
- The host signs the events it emits with the key in its well-known document. The app
  verifies that signature itself, against the pinned host key, not only the relay's.
- The host is the sequencer: it orders channel messages and, for MLS groups, accepts one
  commit per epoch. Relays only deliver.

### Security rules

The relay is assumed hostile, as MLS assumes of its delivery service.

| A bad relay tries to | Stopped by | What is left |
|---|---|---|
| Forge messages as Alice | Every write carries Alice's device signature, which the host verifies against her key log. The relay's server signature means only "this came through relay R", never "Alice said this". Keeping the two meanings apart is the most important rule | Nothing while her device keys are safe |
| Read content | MLS (Stage 4) for DMs and private groups: the relay sees ciphertext | Plaintext channels, which stay plaintext by the 2026-10-08 decision, are readable by the relay as they are by the host (see Consequences) |
| Feed Alice a fake workspace state (members, roles) | The host signs its events with its well-known key; the app checks that signature against the key pinned from the invite | A relay can withhold events, see the next row |
| Replay old messages | Per-sender sequence numbers and single-use ids checked by the host, plus MLS epochs | Nothing |
| Drop, delay or reorder messages | Cannot be prevented, only detected: host sequence numbers per channel, and MLS gap counters, let the app show "messages missing" | Alice switches relay or falls back to a direct connection; the same residual risk Signal users accept with Signal's server |
| Abuse the delegation | It names Alice's device, the relay's key, one workspace and an expiry; it dies when Alice revokes that device or signs a new delegation | Misuse within its scope until it expires or is revoked |
| Replay Alice's invite acceptance elsewhere | Alice signs the host's challenge with the host named as audience | Nothing |
| Flood a host with subscriptions from fake relays | Rate limits per relay key and per delegation; a host may keep an allow-list of relays | A host that turns the allow-list on refuses users of unlisted relays |

### Answers to the open questions this touches

- **Does a host accept any relay, or only approved ones?** Any relay by default; a host admin
  may turn on an allow-list.
- **Do clients connect to several hosts?** No, not in the normal path. The direct connection
  is a fallback only.

## Consequences

- **The client stays single-endpoint.** "A client that talks to several hosts" leaves Stage 5.
  The relay's GraphQL API serves workspace queries, mutations and subscriptions for remote
  workspaces by forwarding them to the host; GraphQL stays the client contract and the
  server-to-server API stays a signed HTTP API.
- **Two hops per channel message**, app to relay to host and back.
- **Metadata shifts rather than worsens.** The relay learns which workspaces Alice is in and
  when she is active. The host no longer sees her IP address.
- **Availability.** Alice's relay must be up for her to read channels. The direct connection
  fallback and switching relays (a profile card update) cover a relay outage.
- **Ordering with Stage 4.** Stage 5 already ships after Stage 4, so DMs and private groups
  are ciphertext on the relay from the start. Plaintext channels are plaintext on the relay's
  queue too: more exposure than the 2026-10-08 model, where only the host stored them. The
  relay keeps them only until every device of the user has fetched them; whether relays keep
  queued messages after the drive has them stays an open question.
- **New signed statements.** Invite tokens, delegations and host event signatures use the
  algorithm-agile Stage 3 encodings and move to hybrid Ed25519 + ML-DSA-44 signatures with the
  rest of the server-to-server API.
- **Wire formats are not fixed here.** The invite token, delegation and event-signature
  formats are specified in the Stage 5 server-to-server spec.

## Rejected alternatives

- **Direct connection to every host as the default** (the 2026-10-08 answer): the client must
  manage one connection, session and catch-up per host, and multi-device push has no single
  place to live. Kept as the fallback.
- **The relay authenticates on the user's behalf with its own credential:** a compromised
  relay could then act as every user it serves. Device signatures on every write and a
  per-workspace delegation keep a relay's power to delivery.
