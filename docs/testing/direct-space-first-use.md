# Direct space first use

The roots a client sends with the direct space's id, called by an account that has joined
nothing and has not read `relayInfo` with its session. Each row is a test in
`rim_api/tests/direct_space_e2e.rs`. "Guest" means the call made the caller a guest of the
direct space and answered as it does for any guest.

The membership gates (`ensure_server_member`, `member_standing`) make a local user a guest when
the space is a direct space and they are not a member yet. Ordinary workspaces and remote users
are refused as before.

| Root | Before | After |
|---|---|---|
| `unreadCounts` | FORBIDDEN | guest, `[]` |
| `myNotifications` | FORBIDDEN | guest, no items |
| `myUnreadNotificationCount` | FORBIDDEN | guest, `0` |
| `myNotificationSettings` | FORBIDDEN | guest, defaults |
| `mutedConversations` | FORBIDDEN | guest, `[]` |
| `workspacePresence` | FORBIDDEN | guest, `[]` |
| `heartbeat` | FORBIDDEN | guest, `true` |
| `server` | FORBIDDEN | guest, `viewerRole: GUEST` |
| `serverMembers` | FORBIDDEN | guest, only the caller |
| `channels`, browsed and joined only | FORBIDDEN | guest, no items |
| `resolvedKeybinds` | FORBIDDEN | guest, no direct-space keybinds |
| `customEmojis` | FORBIDDEN | guest, `[]` |
| `workspacePlugins` | FORBIDDEN | guest, `[]` |
| `searchMessages` | FORBIDDEN | guest, no items |
| `messageChanges` | FORBIDDEN | guest, `[]` |
| `workspaceEvents` (subscription) | FORBIDDEN | guest, subscribed |
| `createConversation` | guest, both members | unchanged |
| `inboxConversations` | guest | unchanged, now through the shared gate |
| `commands` | answers, joins nobody | guest, the commands a guest runs there |
| `myMlsKeyPackageStatus`, `myMlsWelcomes` | answer, join nobody | unchanged |

Not tested here: roots that take a conversation id (`conversation`, `messages`, `channelLenses`,
`markAsRead`, `claimMlsKeyPackages`, `submitMlsCommit`, `mlsCommits`). A newcomer belongs to no
conversation, so these still refuse, as they do in any workspace.

## Decided (2026-10-12)

1. **Reads join too.** Kept: a read such as `unreadCounts` makes the caller a guest, as a
   signed-in `relayInfo` already does for every local user.
2. **`commands` filters by membership and role.** A caller who is not a member of the space in
   `commandContext` sees no commands; a member sees the commands their role may run, and
   `commandDiscover` filters by capability the same way. The test that pins the unfiltered list
   changes with it.
3. **Workspace-only features are refused in the direct space.** Custom emojis, plugins,
   keybinds, pipelines, workspace state and channels answer `VALIDATION` there instead of empty,
   once the client no longer sends them.
4. **Client.** The client subscribes to the direct space's unread counts, notifications and
   events at sign-in, so DM badges show without opening Direct messages, and stops sending
   workspace-only queries for the direct space. This ships before 3.
