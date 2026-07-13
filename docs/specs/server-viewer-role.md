# Expose `Server.viewerRole` — server-role signal for client affordance-gating

**Author:** iron-man · **Implementer:** hulk · **Requested by:** vision (approved by nick-fury)
**Priority:** small, high-leverage. **Can ship ahead of C1c** — it's independent of the on-call work and
unblocks vision, so land it early (parallel with C1a is fine).

## Goal
Give the client a single authoritative signal for the viewer's role in a server, so affordance-gating
(showing/hiding edit controls) stops being best-effort guesswork. Add `Server.viewerRole:
ServerMemberRole!` — the requesting actor's effective role in that server.

**Display/affordance ONLY. Not an authorization boundary.** Every mutation still enforces its own
server-side role check (defense in depth). This field just lets the UI avoid rendering controls the user
can't use (and stop faking it — see "Retro-fixes" below).

## Design
- **Reuse the existing enum** `ServerMemberRole { Owner, Moderator, Guest }` (`rim_core/src/server_member/model.rs`).
  Do NOT introduce a new enum. It's already exposed in the GraphQL schema; if it isn't yet a
  `#[derive(Enum)]` GraphQL type, add that derive (serializes as `OWNER`/`MODERATOR`/`GUEST`).
- Add resolver field `viewerRole` on the GraphQL `Server` type (`rim_api/src/types.rs` Server object).
  It's a `ComplexObject` field (needs the actor + the member lookup), returning `ServerMemberRole!`
  (non-null — every viewer of a server they can query is a member; if a non-member somehow reaches it,
  return an error consistent with existing server-visibility checks rather than a bogus role).
- Resolution: from the request `Actor` + the server id, look up the membership via
  `ServerMemberService`/`server_member_repo.get_member(server_id, user_id)` → `member_role`. Reuse the
  existing member-fetch the server resolvers already use; add a DataLoader only if an N+1 shows up in a
  server-list query (a `(ServerId, UserId)`-keyed loader mirrors the existing `ReactionLoader` pattern —
  do it if `servers` list resolves viewerRole per-row, otherwise a direct lookup is fine).
- **Non-null contract:** `viewerRole` is `!`. A user querying a server they belong to always has a role.
  Do not invent a "None"/"NonMember" variant — that's what query-level visibility already guards.

## Authorization semantics this field advertises (and the server enforces)
- **Editing on-call schedules / escalation policies (Track C1):** allowed for **Owner + Moderator**.
  Per-channel moderator roles do NOT apply — schedules/policies are server-scoped objects, so it's the
  server role that matters. (This answers vision's open sub-question.) The C1 spec's CRUD gates use
  `ensure_server_owner_or_moderator` accordingly (§ update below).
- Guests: read-only for on-call/escalation surfaces.
- The FE gates edit affordances on `viewerRole in (OWNER, MODERATOR)`; the backend independently enforces
  the same via the service-layer role check on every mutation.

## Retro-fixes (call these out in the PR)
Landing `viewerRole` lets two existing best-effort/fake gates become real, single-sourced:
1. **B2 Integrations tab** ingest-token management gate (currently client-side best-effort).
2. **EmojiManagement's fake `isAdmin`** — replace the fabricated flag with `viewerRole === 'OWNER'`
   (custom-emoji management is owner-gated server-side today; confirm and match).
Coordinate the FE swap with vision — but the backend deliverable here is just the `viewerRole` field.

## Tests
- Resolver test: an Owner querying `server.viewerRole` gets `OWNER`; a Moderator gets `MODERATOR`; a
  Guest gets `GUEST`. (Use the e2e/test harness that builds a schema with a seeded membership.)
- Confirm non-null: a member always resolves a role; add a case documenting the non-member path matches
  existing server-visibility behavior (error, not a fake role).

## SDL delta (for vision, via nick-fury)
`type Server { … viewerRole: ServerMemberRole! }` and (if not already present) `enum ServerMemberRole {
OWNER MODERATOR GUEST }`. Additive, non-breaking. Regenerate `schema.graphql`.

## Docs
Doc comment on the resolver stating it is a display/affordance signal, not an auth boundary, and that
every mutation re-checks the role server-side.

## Rollback
Pure additive GraphQL field. Revert is a code revert; no schema/DB change.
