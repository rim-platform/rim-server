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
| `commands` | answers, joins nobody | unchanged |
| `myMlsKeyPackageStatus`, `myMlsWelcomes` | answer, join nobody | unchanged |

Not tested here: roots that take a conversation id (`conversation`, `messages`, `channelLenses`,
`markAsRead`, `claimMlsKeyPackages`, `submitMlsCommit`, `mlsCommits`). A newcomer belongs to no
conversation, so these still refuse, as they do in any workspace.
