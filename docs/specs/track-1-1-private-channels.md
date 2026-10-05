# Track 1.1 — Private channels, channel browse/join/leave, archive

_Spec written 2026-10-05 by the coordinator. Implements `ROADMAP.md` Track 1.1 ("Enough chat").
Status: ready to implement. Server first, client second._

## Today

- `ConversationType` is `Direct | Group | Channel`. Channels have no membership: every
  workspace member can read every channel (`ensure_conversation_access` short-circuits on
  `Channel`), `list_channels_for_user` ignores the user, `list_accessible_conversation_ids`
  returns all channels, and `user_can_see_event` in `rim_api/src/subscription.rs` treats
  every channel event as visible to all workspace members.
- There is no GraphQL mutation to add or remove conversation members or to leave; the service
  methods (`add_member`, `remove_member`, `leave`) exist on `ConversationService`.
- `createConversation` takes `participants` and `title`; no privacy flag, no archive.

## Goal

1. **Private channels**: readable, listable, searchable and subscribable only by their members.
   A non-member must not be able to tell a private channel exists (`NOT_FOUND`, matching the
   DM convention), and must receive none of its events.
2. **Membership for channels**: public channels can be joined and left by any workspace member;
   private channels gain members only by invitation from an existing member; a member can leave
   any channel; Owner/Moderator can remove anyone. Joined public channels are what the sidebar
   shows; all public channels appear in a browse view.
3. **Archive**: Owner/Moderator can archive and unarchive a channel. Archived channels are
   read-only (no new messages, reactions, pins, topic edits), hidden from default listings,
   still readable and searchable by whoever could read them before.
4. **Commands** for all of the above so the palette, keybinds, macros and the agent get them.

## Non-goals

- Per-channel permission overrides or roles beyond Owner/Moderator/Guest (Track 1.2).
- Channel categories/ordering (later in Track 1.1 if cheap, otherwise 1.5).
- Converting a channel between public and private after creation (explicitly out; document it).
- Migrating DMs/groups: their behaviour is unchanged.

## Server (rim-server)

### Schema

Migration `add_channel_privacy_and_archive` (generate with `sea migrate generate`):
- `conversation.is_private boolean not null default false`
- `conversation.archived_at timestamptz null`
- `conversation.created_by uuid null` FK user ON DELETE SET NULL (needed for "creator joins
  automatically" and later for moderation history; back-fill is not possible, leave null)
- Index `(server_id, conversation_type, is_private, archived_at)` for the listing queries.
- Data step: for every existing channel, insert a `conversation_member` row (`id` a fresh
  UUIDv7, `joined_at = now()`) for every current workspace member who has no row yet, so "joined
  channels" does not start empty for existing users. Check the unique index on
  `(conversation_id, user_id)` in the conversation_member migrations and use `ON CONFLICT DO
  NOTHING` against it; generating ids row by row from Rust in the migration is acceptable.

Entity merge by hand per CLAUDE.md; mappers updated; `Conversation` domain model gains
`is_private: bool`, `archived_at: Option<DateTime<Utc>>`, `created_by: Option<UserId>`.

### Repository changes (`rim_core` traits, `rim_infra_db` impls, tests under `db-tests`)

- `ConversationRepository`:
  - `create_conversation*` take the new fields (`NewConversation { is_private, created_by }`).
  - `list_channels_for_user(user_id, server_id, filter: ChannelListFilter { joined_only: bool,
    include_archived: bool }, limit, dir, cursor)` returns public channels plus private channels
    the user is a member of, with a per-row `is_member: bool` (a join against
    `conversation_member`), excluding archived unless asked. Keep the `(created_at, id)`
    cursor and the `BEFORE` fix pattern from PR #14 (nearest-first fetch and flip).
  - `list_accessible_conversation_ids`: public non-archived-or-archived channels OR member
    rows. Private channels only via membership.
  - `set_archived(conversation_id, archived_at: Option<...>)`, `exists_channel_by_title`
    unchanged (titles stay unique per workspace across public and private).
- `ConversationMemberRepository`: `count_members(conversation_id)`, and a batch
  `member_counts(&[ConversationId])` for list rendering.

### Service layer (`rim_services` contracts, `rim_infra_services` impls)

- Access gate (`util.rs`): `ensure_conversation_access` returns `Ok` only for **public**
  channels; private channels and archived-private channels require membership; the error for a
  private channel non-member is `NotFound` (same as DMs), not `PermissionDenied`. Add
  `ensure_conversation_writable` that additionally rejects archived conversations with a new
  `ServiceError::ConversationArchived` (GraphQL code `CONVERSATION_ARCHIVED`), and call it from
  every write path: send/reply/edit/delete message, reactions, pins, topic update, attachments
  finalize, typing start, mentions, incident start on that conversation.
- `ConversationService`:
  - `create_conversation`: `CreateConversationCmd` gains `is_private: bool` (rejected with
    `Validation` if set on a Direct or Group; `ConversationParticipants::Channel` stays as is).
    The creator is always added as a member of a channel (public or private). Initial members of
    a private channel are added by the client with `add_conversation_members` right after
    creation; no new participant shape.
  - New `join_channel(JoinChannelCmd { actor, server_id, conversation_id })`: public,
    non-archived channel only; idempotent.
  - New `leave_conversation(LeaveConversationCmd)`: channels and groups; not DMs. Leaving a
    private channel as its last member is allowed (Owner/Moderator can still archive it; add
    `list_channels_for_user` visibility for Owner/Moderator of private channels they are not in?
    **No**: keep private channels invisible to non-members including owners; document that an
    orphaned private channel can only be archived by a moderator via its id, which they can get
    from the audit log. Simplest honest rule for now.)
  - New `add_conversation_members(AddConversationMembersCmd { actor, server_id,
    conversation_id, user_ids })`: for private channels and groups, actor must be a member;
    targets must be workspace members; emits `MemberAddedToConversation` per user. For public
    channels this is "invite" and behaves the same (adds membership).
  - New `remove_conversation_member(RemoveConversationMemberCmd)`: Owner/Moderator, or the
    actor removing themselves (equivalent to leave). Emits `MemberRemovedFromConversation`.
  - New `set_conversation_archived(ArchiveConversationCmd { actor, server_id, conversation_id,
    archived: bool })`: Owner/Moderator; channels only; emits `ConversationUpdated`.
  - `list_channels` takes the filter and returns `is_member`, `member_count`, `is_private`,
    `is_archived` on `ConversationDto`.
- Events: add `#[serde(skip_serializing)] is_private: bool` next to the existing
  `member_user_ids` on every event variant that carries `conversation_type` (the wire shape is a
  frozen cross-repo contract; skipped fields do not change it). `user_can_see_event`: a channel
  event is visible to all workspace members only when `conversation_type == "Channel" &&
  !is_private`; otherwise membership. Unit tests for both branches on every variant.
- Search: `search_messages` must scope by `list_accessible_conversation_ids`; verify and add a
  test that a private channel's messages do not appear for a non-member.
- Mentions, pins, read state, lenses, incidents: all go through the access gate already; add a
  test per path for the private case where cheap (pins and read state at least).

### Commands (`rim_infra_services/src/command/registry` and `executor`)

- `chat.create_channel`: new optional boolean arg `is_private` (default false) and optional
  `member_ids`.
- New builtins, all `Backend`, `Mutating`, palette and keybind visible, `ai_callable: true` with
  `nl_description`/`intent_samples` (the agent operates chat too, Track 4.1 starts here):
  `chat.join_channel`, `chat.leave_conversation`, `chat.invite_to_conversation`,
  `chat.archive_channel` (Owner/Moderator; the executor delegates authorization to the service),
  `chat.unarchive_channel`. `chat.remove_conversation_member` is `Mutating`, `dangerous: true`
  (confirmation), `ai_callable: false`.
- Update `docs/command_model.md` builtin table and the AI-callable inventory test if it pins
  the list.

### GraphQL (`rim_api`, now split by domain under `mutation/` and `query/` after PR for the
resolver split; put the new resolvers in the conversation modules)

- `Conversation` type gains `isPrivate: Boolean!`, `isArchived: Boolean!`, `archivedAt`,
  `isMember: Boolean!`, `memberCount: Int!`, `createdBy: ID`.
- `CreateConversationInput` gains `isPrivate: Boolean` (default false).
- `channels(serverId, dir, limit, cursor, filter: ChannelListFilter)` where
  `input ChannelListFilter { joinedOnly: Boolean = false, includeArchived: Boolean = false }`.
- Mutations: `joinChannel(serverId, conversationId): Conversation!`,
  `leaveConversation(serverId, conversationId): Boolean!`,
  `addConversationMembers(serverId, conversationId, userIds: [ID!]!): Conversation!`,
  `removeConversationMember(serverId, conversationId, userId): Boolean!`,
  `setConversationArchived(serverId, conversationId, archived: Boolean!): Conversation!`.
- Error codes: `NOT_FOUND` for private non-member access, `CONVERSATION_ARCHIVED` for writes
  to archived conversations, `FORBIDDEN` for role failures, `VALIDATION` for DM misuse.
- CSRF: all mutations as usual. SDL changes → client codegen.

### Tests (server)

- Unit: access gate matrix (public/private × member/non-member × archived), each new service
  method's authorization, event visibility matrix.
- `db-tests`: listing filter and `is_member`, accessible ids, migration data step (existing
  workspace members become members of existing channels).
- `e2e-tests`: a private channel created by alice with bob; carol (workspace member) gets
  `NOT_FOUND` on `conversation`, `messages`, `pinnedMessages`, does not see it in `channels`
  or `searchMessages`, cannot `joinChannel` it; bob leaves and loses access; alice invites
  carol and she gains access; a public channel is joinable by carol and appears with
  `isMember` true afterwards; archive makes `sendMessage` fail with `CONVERSATION_ARCHIVED`
  and hides the channel from the default list but not from `includeArchived`; a subscription
  unit test proving the private channel's `NewMessage` is not delivered to carol.
- Update `docs/messaging.md`, `docs/context_model.md` (conversation visibility), and
  `docs/architecture.md` if it lists conversation rules.

## Client (rim-ui)

- Codegen first.
- **Sidebar**: channels section shows joined channels (`joinedOnly: true`); a lock icon for
  private, an archived section collapsed by default (or hidden with a toggle); "Browse
  channels" entry opening a dialog that lists all public channels with member counts and a
  Join button; joining adds it to the sidebar.
- **Create channel dialog**: a "Private" toggle with copy "Only invited members can see this
  channel. This cannot be changed later." and an optional initial member picker (reuse the
  group member picker from `NewMessagePicker`).
- **Conversation header**: lock icon and member count; a members panel (tool panel) listing
  members with Invite (any member for private/group; any member for public), Remove
  (Owner/Moderator), Leave; Archive/Unarchive in the header menu for Owner/Moderator; archived
  banner with the composer disabled.
- **Reading a public channel without joining**: allowed; show a "Join channel" bar above the
  composer instead of the composer.
- **Errors**: `CONVERSATION_ARCHIVED` and `NOT_FOUND` surfaced with server messages through
  the existing `graphqlErrorMessage` helper.
- **Realtime**: handle `MemberRemovedFromConversation` for self (drop the conversation from
  the sidebar and navigate away if open) and `ConversationUpdated` archive changes.
- **Commands/keybinds**: nothing hard-coded; the palette gets the new builtins from the server.
- Tests: sidebar sections and browse/join flow, create-channel private toggle variables,
  members panel actions and role gating, archived composer state, removal event handling.

## Definition of done

Server: `cargo fmt`, both clippy lines, `cargo test --workspace`, `cargo test -p migration`,
`db-tests`, `infra-db-tests`, `e2e-tests` single-threaded. Client: lint (0 errors), format,
tsc, vitest, build. Two PRs, server first.
