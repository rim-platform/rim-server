# Nested resolver access

Every field of a non-root GraphQL type that loads another object, what it loads and what stops
a viewer from reading something the matching root would refuse them. "Parent" means the field
returns data about the conversation or workspace the root that produced the parent already
checked. A field that crosses into another conversation, workspace or user needs its own check.

The member visibility rule referred to below: a guest sees only the members who share a joined
conversation with them (`ServerMemberService::visible_members`); everyone else sees every member.

## Fields resolved with I/O

| Field | Loads | Guard | Verdict |
|---|---|---|---|
| `Message.author` | the author's `User` through `UserLoader` | parent: the author of a message the viewer may read | guarded |
| `Message.mentions` | the mentioned users through `MentionLoader` and `UserLoader` | parent; mentions are stored only for users who can read the conversation (plaintext) or are its members (encrypted) | guarded |
| `Message.mentionedUserIds` | the same ids through `MentionLoader` | as `mentions` | guarded |
| `Message.reactions` | reaction groups with reactor ids through `ReactionLoader` | parent; reacting needs write access to the same conversation | guarded |
| `Message.isPinned` | the pin flag through `PinLoader` | parent | guarded |
| `Message.linkPreviews` | previews of the message's own URLs through `LinkPreviewLoader` | parent | guarded |
| `Message.forwardedFrom` | the source message, in any conversation | resolver: `ResolveExt::load_readable_conversation` on the source; null when unreadable | guarded |
| `SearchResultMessage.author` | the author's `User` | parent: `searchMessages` returns only readable conversations (joined ones for a guest) | guarded |
| `Notification.conversation` | the conversation | resolver: `load_readable_conversation`; null when unreadable | guarded |
| `Notification.message` | the message | resolver: `load_readable_conversation` first | guarded |
| `Conversation.isMuted`, `.notificationLevel`, `.effectiveNotificationLevel` | the viewer's own preferences | keyed by the viewer | guarded |
| `Conversation.mlsGroup` | the group and its devices | resolver: null for a viewer in `pendingMemberIds`; service: `E2eeService::group` checks read access to the conversation | guarded |
| `Conversation.predecessor` | the host and id this conversation succeeds | parent: same conversation | guarded |
| `Server.viewerRole` | the viewer's own membership | keyed by the viewer | guarded |
| `Server.owner` | the owner's `User` through `UserLoader` | service: `ServerMemberService::get_owner` answers members only; the owner is workspace metadata every member sees, guests included | guarded |
| `ServerBan.user` | the banned user's `User` | root: `serverBans` and `banMember` are for moderators, who see every member | guarded |
| `WorkspacePlugin.approvedBy` | the approver's `User` | resolver: the member visibility rule; null for a guest who shares no conversation with the approver | guarded |

## Fields filled by the root

| Field | Holds | Guard | Verdict |
|---|---|---|---|
| `Message.attachments`, `.encryption` | the message's own attachments and envelope | parent | guarded |
| `MessageChange.message` | a changed message | root: `messageChanges` checks each conversation and answers `NO_ACCESS` without changes | guarded |
| `Conversation.memberIds` | the conversation's members | root: a guest lists only joined conversations | guarded |
| `Conversation.pendingMemberIds` | the users invited to an encrypted conversation | root: as `memberIds`; an invitee gets the conversation from `inboxConversations` alone, and every root that reads its content (`conversation`, `messages`, `unreadCounts`, sends) treats them as a non-member | guarded |
| `ServerMember` profile fields | a member's profile | root: `serverMembers` applies the member visibility rule | guarded |
| `Thread.rootAuthorId`, `.rootText` | the thread root | root: thread queries check the conversation | guarded |
| `MessageReport.message*`, `.franked*` | the reported message | root: moderators only | guarded |
| `MlsGroup.*` | devices, pending adds and removes, re-add requests | `Conversation.mlsGroup` | guarded |
| `WorkspacePlugin.statement`, `.source`, `.publisherKeyLog` | the signed manifest and the publisher's key log | key logs and manifests are signed public statements (`keyLog` needs no session) | public by design |
| `KeyIdentity.*`, `DeviceKey.certificate` | the viewer's own key log, devices and card | `myKeyIdentity` | guarded |
| `RemoteIdentity.*` | a principal's key log, devices and card | the same document the S2S identity endpoint serves to any peer | public by design |
| `PrincipalDevices.devices` | certified devices | root: `principalDevices` answers only principals who share a workspace with the viewer | guarded |
| `DmRequest.card`, `.heldItems`, `RelayItem.envelope` | the viewer's own requests and queue | keyed by the viewer or the session device | guarded |
| `InviteTokenPreview.workspace` | workspace name, inviter name and owner name | the token holder; meant to be read before joining | public by design |

## Dataloaders

| Loader | Key | Note |
|---|---|---|
| `UserLoader` | user id | No actor. Any call site must pass an id the viewer may see: today a message author, a mentioned user, a banned user (for a moderator), an approver after the member visibility check and a workspace's owner (for a member). |
| `ReactionLoader` | message id and viewer | The viewer only sets `reactedByMe`; access comes from the parent message. |
| `MentionLoader`, `PinLoader`, `LinkPreviewLoader` | message id | No actor; access comes from the parent message. |

A new nested field that takes an id from outside its parent's conversation or workspace goes
through the matching root's check, the way `forwardedFrom` and `approvedBy` do.
