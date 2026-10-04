# Fix — channel pin/unpin events never reach subscribers (latent bug)

**Author:** iron-man · **Implementer:** hulk · **Found by:** hulk (during B2 review) · **Confirmed by:** iron-man
**Priority:** medium — a realtime-correctness bug, not a security hole. Pre-existing (not introduced by B2).

## The bug (confirmed)
`WorkspaceEvent::MessagePinned` / `MessageUnpinned` (`rim_services/src/events/types.rs:93-107`)
carry `member_user_ids` but **no** `conversation_type`. The subscription visibility filter
(`rim_api/src/subscription.rs:116-121`) gates both purely on membership:
```rust
WorkspaceEvent::MessagePinned { member_user_ids, .. }
| WorkspaceEvent::MessageUnpinned { member_user_ids, .. }
    => member_user_ids.iter().any(|id| id.to_string() == user_id),
```
The pin service populates `member_user_ids` from
`conversation_member_repo.list_member_ids(conversation_id)` (`rim_infra_services/src/pin.rs:112-113`,
155-156). **Channels have no `conversation_member` rows** (channel access = server membership), so
for a message pinned in a channel `member_user_ids` is empty → `.any()` is always false → **no
subscriber ever receives the pin/unpin event**. Users only see channel pin changes on refetch or
reconnect, not in realtime.

This is the exact pattern every other channel-capable event already handles: `NewMessage`,
`ConversationCreated`, `ThreadCreated`, `TypingStarted`, and (as of B2) `IncidentDeclared`/
`IncidentUpdated` all carry `conversation_type` and the filter treats `"Channel"` as visible to all
workspace members. Pins were simply missed.

## Fix
Mirror the established pattern:

1. **Event structs** (`rim_services/src/events/types.rs`): add
   `#[serde(skip_serializing)] conversation_type: String` to both `MessagePinned` and
   `MessageUnpinned`. Use `skip_serializing` (like `TypingStarted` does) so the **client wire shape
   is unchanged** — `conversation_type` is only needed server-side for the visibility filter, and
   the FE pin handlers already shipped without it. This keeps the fix non-breaking for the frontend.

2. **Emit sites** (`rim_infra_services/src/pin.rs`): `pin_message` and `unpin_message` already load
   the conversation via `ensure_conversation_member_access` (which returns the `Conversation`).
   Pass `conversation.conversation_type.as_str().to_string()` into the event. (If the returned
   conversation isn't currently bound, bind it — it's already fetched by the access gate.)

3. **Subscription filter** (`rim_api/src/subscription.rs`): change the pin/unpin arm to the
   channel-bypass pattern used by messages:
   ```rust
   WorkspaceEvent::MessagePinned { conversation_type, member_user_ids, .. }
   | WorkspaceEvent::MessageUnpinned { conversation_type, member_user_ids, .. } => {
       if conversation_type == "Channel" { true }
       else { member_user_ids.iter().any(|id| id.to_string() == user_id) }
   }
   ```

The `workspace_id()` / `member_user_ids()` exhaustive matches in `types.rs` may need the added
field in their patterns (compiler will tell you). `member_user_ids` stays populated for DM/group
pins as today.

## Tests
- A subscription-filter unit test: a `MessagePinned` with `conversation_type == "Channel"` and
  **empty** `member_user_ids` is visible to an arbitrary workspace member (the regression guard —
  this fails before the fix). Add the symmetric DM case: a `Group` pin with a non-member is not
  visible. Mirror the existing message-event filter tests.
- If pin.rs has emit tests, assert the emitted event carries the conversation type.

## Docs
Doc comment on the two event variants noting why they carry `conversation_type` (same rationale as
`NewMessage`), matching the note B2 added to the incident variants.

## Rollback
Additive, `skip_serializing` field — no schema, no wire change. Straight code revert.
