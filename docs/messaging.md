# Messaging Contract

This document describes the backend contract for conversations, messages, threads, and attachments.

## Product Terms

- `workspace` is the product-level term.
- `Conversation` is the messaging container used for channels, DMs, and group chats.
- `MessageThread` is an optional thread opened from a timeline message.

## Conversation Types

- `Channel`: stable shared conversation inside a server/workspace, public or private.
- `Direct`: 1:1 conversation between two users.
- `Group`: multi-user private conversation.

## Channel Visibility And Membership

- **Public channels** (`isPrivate = false`) are readable by every workspace member, joined or not
  (except guests, see Roles And Moderation below).
  Joining (`joinChannel`) adds a `conversation_member` row; joined channels are what the sidebar
  lists (`channels(filter: { joinedOnly: true })`), all public channels appear in browse
  (`channels` with the default filter). Posting does not require joining.
- **Private channels** (`isPrivate = true`) are readable, listable, searchable and subscribable by
  their members only. A workspace member outside one gets `NOT_FOUND` on every read and write —
  never `FORBIDDEN` — so the response does not reveal that the channel exists. This holds for
  workspace owners and moderators too.
- Privacy is chosen at creation (`createConversation(input: { isChannel: true, isPrivate: true })`)
  and **cannot be changed later** in either direction. `isPrivate` on a DM or group is rejected
  with `VALIDATION`.
- The creator of any channel is its first member. A private channel's initial members are added
  by the client with `addConversationMembers` right after creation.
- `addConversationMembers` (invite): the caller must be a member of the channel or group (for a
  public channel: join first, `FORBIDDEN` otherwise); targets must be workspace members;
  existing members are skipped. Not valid for DMs. In the server's default workspace, which
  every account joins, only an Owner or Moderator, or the creator of a private channel, adds
  people to a channel (`FORBIDDEN` otherwise), and one call names at most 50 people
  (`VALIDATION`). A group holds at most 100 members, plaintext or encrypted (`VALIDATION`).
- A channel's topic (`updateConversationTopic`) is changed by an Owner or Moderator, or by the
  creator of a private channel; anyone else gets `FORBIDDEN`. In a DM or group any member
  changes it. Anyone who may write pins a message; a message someone else pinned in a channel is
  unpinned by an Owner, a Moderator or a private channel's creator (`FORBIDDEN` otherwise).
- `leaveConversation`: channels and groups, not DMs. Leaving a public channel you never joined
  is a no-op. Leaving a private channel as its last member is allowed; it then stays invisible to
  everyone, and only an Owner/Moderator can archive it, by id (from the audit log).
- `removeConversationMember`: Owner/Moderator may remove anyone (also from a private channel
  they are not in); everyone else only themselves.
- DMs and groups are unchanged: non-members get `FORBIDDEN`.
- Mentions (`<@user>`) are stored and notified (`UserMentioned`) only for users who can read the
  conversation: any workspace member but a guest for a public channel, members otherwise. A
  message mentions at most 50 distinct users; later `<@user>` tokens stay plain text, are not
  stored and notify nobody. An edit replaces the message's mentions.
- Realtime: events of a private channel, DM or group reach only its members; public-channel
  events reach the whole workspace. The visibility fields on `WorkspaceEvent` (`is_private`,
  `member_user_ids`, and `conversation_type` where it was not already serialized) are
  server-side only and never serialized, so the WebSocket JSON is unchanged.
- Existing workspace members became members of every existing channel when the
  `m20261005_005743_add_channel_privacy_and_archive` migration ran, so the joined list did not start empty. A user who joins the workspace later
  (invite acceptance, join by invite, owner adding them, server creation — all through
  `util::add_workspace_member`) is joined to every public, unarchived channel at that moment;
  private and archived channels are not joined.
- Unread counts (`unreadCounts`) and `markAllAsRead` cover only conversations the user is a
  member of: joined public channels, private channels, DMs and groups. A public channel the user
  can read but has not joined carries no unread badge; after joining, messages count from the
  join time.

## Who May Start A DM

A first DM from someone arrives directly, as a request the recipient accepts, ignores, blocks or
reports (`dmRequests`, see "The request inbox" in `docs/relay.md`), or not at all. Three layers
decide, and each may only tighten the one above it:

| Layer | Set by | People outside the recipient's workspaces | Fellow members of a workspace |
|---|---|---|---|
| Server | The operator: `RIM_DM_INBOUND`, `RIM_DM_OUTBOUND`, `RIM_DM_PARTNERS` (read-only in `serverInfo.dmInbound`, `dmOutbound`, `dmPartners`) | Inbound `OPEN`, `REQUESTS` (default), `PARTNERS` (people on the listed servers and on this one) or `NONE`; outbound `ALLOWED` (default), `PARTNERS` or `NONE` | Not the server's matter |
| Workspace | Its Owner: `setWorkspaceDmPolicy` (`Server.dmOutsideFloor`, `Server.dmMemberDefault`) | A floor for its members: `NONE` (default, no floor), `REQUESTS` or `CLOSED` | A default: `DIRECT` (default) or `REQUESTS` |
| Person | Each user: `setMyDmSettings`, `setMyWorkspaceDmSetting` (read in `myDmSettings`) | `REQUESTS` (default) or `NONE`, within the floors | Per workspace `DIRECT` or `REQUESTS`, either way |

- **A fellow member** shares a workspace hosted here with the recipient. Their DM is delivered
  when they are an Owner or Moderator of one shared workspace, when they are a contact, or when
  the recipient's choice (else the workspace's default) for one shared workspace is `DIRECT`;
  otherwise it is a request.
- **Anyone else.** From another server, `RIM_DM_INBOUND` must admit their server or the DM is
  refused, contacts included: `PARTNERS` admits the listed servers, `NONE` none. Then a contact's
  DM is delivered. Otherwise the strictest of the operator's floor (`NONE` refuses strangers on
  this server too), the floors of the recipient's workspaces and the recipient's choice decides
  between a request and a refusal. `OPEN` sets no floor; with the choices a person has today it
  works as `REQUESTS`.
- **A contact** is someone the recipient added (`addContact`), accepted, or messaged first:
  creating a DM records the other person as the creator's contact, so their DM back is never a
  request. A user holds at most 1,000 contacts: `addContact` for someone new past that answers
  `VALIDATION` ("a user holds at most 1000 contacts; remove one first"), and a DM records no
  further contact for its creator.
- **Who the sender knows.** The sender's own limits on DMs to strangers, the daily invites to
  people on other servers and a new account's probation (below), skip only people the sender
  knows: a contact who lists the sender as a contact too (only people on this server have a list
  here), or someone either of the two accepted a DM request from on this server (an invite
  delivered directly from a contact is recorded as accepted). A contact the sender added on their
  own, or that a DM they started recorded, counts as a stranger. These limits are the sender's;
  the recipient's request inbox still applies its own rules.
- **Outbound.** A DM with someone on another server needs `RIM_DM_OUTBOUND` to admit their
  relay: `createConversation` with `remoteMembers` answers `FORBIDDEN` ("this server starts no
  direct messages with that server") otherwise, before their relay is asked anything, and so do
  key package claims for their devices. With `NONE` nothing of the direct space reaches another
  server; with `PARTNERS` it goes to the listed servers only. A conversation that invites someone
  on another server the sender does not know counts against the sender's daily invites
  (`RIM_RELAY_DM_REQUESTS_PER_DAY`, 20): past them it answers `RATE_LIMITED` with
  `retryAfterSeconds`.
- **Profile card.** An account with a principal id and no profile card starts no conversation in
  the direct space, and no workspace DM, with a user here who has a principal id:
  `createConversation` and `addConversationMembers` answer `VALIDATION` ("publish a profile card
  before starting a conversation with someone here"). The invite that takes such a conversation
  through the recipient's DM settings, blocks and request limits carries the creator's card. The
  app publishes the card right after signup and, when that fails, from Settings. Reopening an
  existing DM is not refused.
- **Conversations per day.** A user starts at most 50 new conversations a day in the direct
  space, DMs and groups alike: past them `createConversation` answers `VALIDATION` ("you can start
  at most 50 new conversations a day; try again in N hours"). Reopening an existing DM is not
  counted. The count is kept in memory on each node.
- **Probation.** During `RIM_PROBATION_HOURS` a new account starts no DM with a stranger, in the
  direct space or in a workspace: someone it does not know (above) who is in none of its
  workspaces here, the account without a key included: `VALIDATION` saying when it can
  (`docs/api.md`). The server's default workspace does not count, since every new account joins
  it, so a DM in it meets probation like one in the direct space; any other workspace does. The
  instance operator is never on probation.
- **Refused here.** A DM in the direct space that a recipient here refuses answers `FORBIDDEN`
  when it is started (`createConversation`, `addConversationMembers`), and so does one whose
  recipient blocked the sender or whose sender the operator blocked. An invite to a group
  someone was added to comes from whoever added them, so their blocks and settings judge that
  person. A refused DM from another
  server is answered as for a block, so its server stops addressing the recipient.
- **Request limits.** A conversation that would file a DM request with someone here meets the
  request inbox's limits when it is started (`createConversation`, `addConversationMembers`, in
  the direct space, for a workspace's DMs and for the default workspace's groups): `FORBIDDEN`
  ("their request inbox is full") while the recipient holds 100 pending requests, and
  `RATE_LIMITED` with `retryAfterSeconds` (until the sender's oldest request of the last 24 hours
  leaves the window) once the sender would file more than `RIM_RELAY_DM_REQUESTS_PER_DAY` (20) requests in 24 hours on this server; a group
  counts each request it files. A DM delivered directly (from a contact, an Owner or Moderator,
  or a member the recipient takes DMs from directly) files no request and meets no limit, and
  reopening an existing DM files none. A conversation the request inbox still refuses for a
  recipient here is kept from them as an ignored request is, with no notification.
- **Workspace DMs.** In a workspace hosted here, a DM a member whose relay is this server takes
  as a request is held in their request inbox like a DM from outside: until they accept it, it stays out of their
  `inboxConversations` and `workspaceEvents`, and its notifications and unread counts out of
  theirs; meanwhile they may read it but post, react and type nothing in it (`docs/relay.md`,
  "The request inbox"). Like the request inbox it needs server-to-server traffic to be on; without it workspace
  DMs arrive directly.
- **The default workspace's groups.** In the server's default workspace, which every account
  joins, a group is started and grown like a DM with each newcomer: `createConversation` and
  `addConversationMembers` (GraphQL or the `chat.create_group` and `chat.invite_to_conversation`
  commands) are refused as that DM would be, by a newcomer's DM settings, the request limits and
  probation, and a newcomer who takes it as a request holds it in their request inbox until they
  accept it. One call names at most 50 people and a plaintext group holds at most 50 members
  (`VALIDATION`); an encrypted group keeps its own cap. Groups of other workspaces are
  unchanged: their members are added by someone already in them.
- **The sender's view.** `Conversation.dmRequestPending` is true for a DM whose other member,
  on this server, has not yet accepted it as a request; false once they did or when it was no
  request, null for groups and channels.
- **The settings.** `setMyDmSettings(outside: REQUESTS)` under a floor that refuses DMs from
  outside answers `VALIDATION` naming it; `myDmSettings.effectiveOutside` says what applies and
  `outsideFloorFrom` where a refusing floor comes from (`OPERATOR`, or `WORKSPACE` with its id and
  name). `setWorkspaceDmPolicy` is the Owner's alone (`FORBIDDEN` for anyone else);
  `setMyWorkspaceDmSetting` needs membership of the workspace (`NOT_FOUND`). `Server.dmOutsideFloor`
  and `Server.dmMemberDefault` are null in the direct space. A person whose relay is another server
  sets their choices there; both personal mutations are refused through the forward path.

## Archived Channels

- Owner/Moderator archive and unarchive channels (`setConversationArchived`); other roles get
  `FORBIDDEN`. Only channels can be archived (`VALIDATION` otherwise). An archived channel does
  not count against `RIM_LIMIT_CHANNELS_PER_WORKSPACE`; unarchiving one past the cap is
  `VALIDATION`. A workspace holds at most five times that cap in channels, archived ones
  included, and creating one past it is `VALIDATION`; channels cannot be deleted, so archiving
  frees no room toward it.
- An archived conversation is read-only: sending, replying, editing, deleting and forwarding into
  it, reactions, pins, topic edits, threads, attachments, typing, joining and inviting fail with
  `CONVERSATION_ARCHIVED`. Leaving and removing members still work.
- Archived channels are hidden from `channels` unless `includeArchived: true`, and stay readable
  and searchable by whoever could read them before.

## Roles And Moderation

### Roles

| Role | Meaning |
|---|---|
| `OWNER` | Everything. One per workspace; the role cannot be granted (`setMemberRole` with `OWNER` is `VALIDATION`). |
| `MODERATOR` | Moderation (kick, ban, timeout, delete any message, slow mode, posting permissions, reports) and channel management (archive, remove members). Changes roles only between `MEMBER` and `GUEST`. Posts in every channel they can write in, whatever its posting policy. |
| `MEMBER` | The default. Reads and writes wherever they are a member (public channels without joining) where the channel's posting policy lets them, creates channels and groups, joins public channels, invites people into conversations. In the server's default workspace only the Owner and Moderators create channels (`FORBIDDEN`). |
| `GUEST` | Restricted. Reads and writes only in conversations they were explicitly added to. |

- **Migration:** before the member role existed every new member was created as `guest`, so "guest" meant an
  ordinary member. The `m20261005_024730_add_member_role` migration adds `member` to the
  `server_member_role` enum and turns **every existing `guest` into `member`**; the column
  default is now `member`.
- New members get `MEMBER` on every join path (invite acceptance, join by invite, owner-added).
  Invites carry a role (`CreateInviteInput.role`, `MEMBER` by default, `GUEST` for a guest link;
  any other value is `VALIDATION`). Workspace invites (`createInvite`) stay owner-only.
- `ServerMember.memberRole` is the enum `ServerMemberRoleGql` (it was a string); the field name is
  unchanged. `ServerMember.timeoutUntil` is set while the member is timed out.
- `setMemberRole(serverId, userId, role)`: the Owner may set `MODERATOR`, `MEMBER` or `GUEST` on
  anyone but themselves; a Moderator may move a `MEMBER` to `GUEST` and back; everyone else gets
  `FORBIDDEN`. Changing your own role is `VALIDATION`. Emits `MemberRoleChanged`.

### Workspace invites

- `createInvite` is owner-only (`FORBIDDEN` for everyone else). `expiresAt` defaults to seven
  days from now and must be in the future and at most 30 days away, else `VALIDATION`; the
  same bounds as instance invites. `maxUses` defaults to 1 and is capped by
  `RIM_LIMIT_INVITE_MAX_USES` when the operator sets it. The token is returned once and never shown again.
- `recipientEmail` emails the link to that address, and nothing more: the invite is not bound to
  it, and whoever holds the link can use it. One user creates at most
  `RIM_LIMIT_INVITES_PER_USER_PER_DAY` invites (default 50, 0 turns it off) and sends at most as
  many invite emails in 24 hours; past either a `createInvite` is `VALIDATION` and creates
  nothing.
- `workspaceInvites(serverId, dir, cursor, limit)` lists the workspace's invites for its Owner
  (`FORBIDDEN` for everyone else): every invite, revoked, expired and used-up ones included,
  ordered `createdAt DESC, id DESC` with the standard cursor rules (`dir` defaults to `AFTER`,
  towards older invites; `limit` defaults to 50, at most 100). Each `WorkspaceInvite` has its
  role, `useCount`, `maxUses`, `expiresAt`, `createdAt`, `createdBy`, `revokedAt` and `state`:
  `REVOKED` when revoked, else `EXPIRED` once `expiresAt` has passed, else `USED_UP` once
  `useCount` reached `maxUses`, else `ACTIVE`. Neither the token nor its hash is listed.
- `revokeInvite(inviteId)` ends an invite: it admits nobody from then on, and members who joined
  with it stay. Owner-only: another member of the invite's workspace gets `FORBIDDEN`; an
  unknown invite, or one of a workspace the caller is not a member of, is `NOT_FOUND`. Revoking
  a revoked invite answers it unchanged. The revocation is audit-logged
  (`workspace_invite_revoked`), as the creation is (`workspace_invite_created`).
- An invite stored without an expiry, by a server from before invites had to expire, keeps
  admitting until it is used up or revoked.
- Redeeming an invite (`joinServerByInvite`, `signupWithKey`, a join through another server) checks the revocation, the expiry and the uses in the same update that counts
  the use. A revoked, expired or used-up invite is `FORBIDDEN` for `joinServerByInvite`; a signup
  gets `INVALID_TOKEN` for a revoked or used-up invite and `EXPIRED_TOKEN` for an expired one.

### Public join link and join requests

A workspace can have one permanent public link, `<server>/join/<token>`, to put on a website, a
README or a social profile. It lets **existing accounts** join the workspace, or ask to join it:
accounts of this server, and, unless the Owner turns that off, accounts of other servers through
their own server. It is a different thing from a workspace invite:

| | Workspace invite | Public link |
|---|---|---|
| Expiry, uses | at most 30 days, `RIM_LIMIT_INVITE_MAX_USES` | none; it works until turned off or replaced |
| Creates an account | yes: it stands in for the proof-of-work and the per-address signup limit | never, and no setting changes that |
| Token shown | once, by `createInvite` | to the Owner at any time (`publicJoinLink`) |
| Join requests | no | `REQUESTS` mode |

**Why it never creates accounts.** A workspace invite is a signup token: it skips the
proof-of-work and the per-address signup limit, and on an invite-only server it is what opens
registration. A permanent, unlimited link posted on a public page would let anyone, scripts
included, create accounts on the server for free. So the link only admits accounts that already
exist; someone without one signs up the server's normal way (proof-of-work, an instance invite,
open registration) and then opens the link again. `signupWithKey` refuses a link token as
`INVALID_TOKEN`, as it does any unknown token.
`joinByPublicLink` is for accounts this server holds; an account of another server joins through
its own server, which presents it here (below), and nothing about that creates an account
either: the host holds the person as a remote user, as for any member from elsewhere.

**The link.** A token `rim_pjl_` followed by 16 random bytes in base64url (22 characters).
`/join/<token>` carries invite tokens too; the prefix tells the kinds apart (`rim_pjl_` a public
link, `rim_inv_` a workspace invite, `rim_iinv_` an instance invite) and lets secret scanners
recognise a link. It adds no guessable structure to the random part. Lookups go by the SHA-256
of the whole string; a copy of the token is sealed under the server's link key (`RIM_LINK_KEY_FILE`, default
`<RIM_DATA_DIR>/link-key.json`), so the Owner can read the link again and a database dump alone
yields no working link. Neither the direct space nor the server's default workspace has a link:
everyone joins the default workspace anyway.

**The Owner's settings** (only the Owner; `FORBIDDEN` for everyone else, Moderators included;
`VALIDATION` for the direct space and the server's default workspace):

- `publicJoinLink(serverId)`: `enabled` (off by default), `mode` (`DIRECT` or `REQUESTS`), `role`
  (`MEMBER`, the default, or `GUEST`), `allowRemote` (whether accounts of other servers may join
  or ask to join through the link; on by default), `enabledAt`, `issuedAt`, `issuedBy`, the
  `token` and `url`, `joinsLastDay` and `joinsLastWeek` (joins through the link, directly or by
  an accepted request) and `openRequests`. A workspace that never had a link answers with
  `enabled` false and a null token.
- `updatePublicJoinLink(input)` changes any of `enabled`, `mode`, `role`, `allowRemote`; a role
  other than `MEMBER` or `GUEST` is `VALIDATION`. Turning the link on the first time issues its
  token. Turning it off makes every lookup answer `NOT_FOUND`; turning it on again brings the
  same link back.
- `reissuePublicJoinLink(serverId)` issues a new token; the old one stops working at once. The
  settings stay.

**Looking a link up.** `publicJoinLinkPreview(token)` needs no sign-in. A valid, switched-on link
answers the workspace's `serverId`, `name` and `mode` and, as `viewer`, where the caller stands:
`SIGNED_OUT`, `NOT_MEMBER`, `MEMBER`, `BANNED` or `REQUEST_OPEN` (with `joinRequestId`). Every
other token (unknown, switched off, replaced, its workspace deleted) answers `NOT_FOUND` with the
message "not found", on the same path: a lookup by the token's hash each time. Workspaces have no
icon, so the preview carries none.

**Misses are rate limited per address.** Each failed lookup or join by an invalid link counts
against the client address; at 20 within ten minutes, every link lookup and join from that address
answers `RATE_LIMITED` (with `retryAfterSeconds`) for an hour, valid links included. Successful
lookups, and refusals of a valid link (banned, full, already a member), do not count. Each miss is
logged with the address in the normal log, not the audit trail. The counts are in memory, per
server process.

**Joining.** `joinByPublicLink(token, message)`, signed in. In one transaction it checks that the
link is valid, the caller is not a member (`CONFLICT`), not banned (`FORBIDDEN`, "you are banned
from this workspace") and that the workspace has room under `RIM_LIMIT_MEMBERS_PER_WORKSPACE`
(`VALIDATION`). `RIM_LIMIT_WORKSPACES_PER_USER` counts workspaces a person owns, and a link never
makes anyone an owner, so it does not refuse a join, as it does not refuse an invite.

- `DIRECT`: the caller joins with the link's role and, as a Member, every public channel, as an
  invite join does. `MemberJoined` carries `"source": "public_link"`. The answer is
  `{outcome: JOINED, serverId}`; `message` is ignored.
- `REQUESTS`: the caller files a join request; the answer is `{outcome: REQUESTED, serverId,
  joinRequest}`.

**Accounts from other servers.** Someone whose account is on another server pastes the link into
their own app ("Add workspace"), which reads this server from the URL and joins through the
remote-join path of `docs/specs/server-to-server-v1.md` ("Joining"): their server, the relay,
asks this one for a join challenge with the link token (`remoteJoinChallenge(…,
publicLinkToken)`), the app signs a login proof and a delegation, and `joinRemoteWorkspace` with
`publicLinkToken` presents them. This server runs the same checks as `joinByPublicLink` through
the same code, and the same modes, with these differences:

- `allowRemote` off answers `FORBIDDEN` ("this workspace's public link does not admit accounts
  from other servers"), at the challenge and at the join; it is a refusal of a valid link and
  does not count as a miss. Accounts of this server still join.
- Misses count against the relay's server id rather than an IP address: the relay's signature
  proves its id, and its address is shared by all its users. The 20 misses in ten minutes cut
  that relay off from link lookups here for an hour; client addresses are not affected.
- A server the operator blocked (`rim-server peer block <serverId>`, `docs/relay.md`, "Blocking a
  peer") gets `403 peer_not_allowed` for every request, the challenge included, before the link
  is looked up: its people do not join through any link here, whatever `allowRemote` says.
- `DIRECT`: the person joins with the link's role and public channels, with `MemberJoined`
  `"source": "public_link"`, the join count and the audit row, and their relay subscribes as
  for any remote join.
- `REQUESTS`: a join request is filed for the person as for a local one (the same message,
  limits, expiry, decline wait, ban cancellation), recording the relay that filed it. The
  moderators see it in `joinRequests`; its `user` is the person as held here (a remote user,
  `r_…`). Accepting adds them with the link's role, as an invite join does. The decision
  (`JoinRequestDecided` with `accepted`, `declined` or `cancelled`) travels to that relay as a host
  event on the person's `u:` stream; their app, told `accepted`, completes the join as a current
  member (`joinRemoteWorkspace` without a token) to subscribe. The person cannot withdraw the
  request from their own server; it expires after 30 days.
- `myJoinRequests` lists the requests filed on the server answering it, so a person's requests
  at other servers are not in their own server's list; their app knows them from the
  `REQUESTED` answer and learns the decision from the relay item.
- An account of this server presented by a relay elsewhere is refused (`VALIDATION`): it joins
  through the link here directly.

**Join requests.**

- One open request per person and workspace (`CONFLICT` for a second); a person holds at most 20
  open requests across workspaces (`VALIDATION`). The message is trimmed, at most 500 characters
  (`VALIDATION`); an empty one is null.
- A request expires 30 days after it was filed. Expiry is read, not scheduled: an open request
  past `expiresAt` is no longer listed or counted, cannot be accepted, declined or withdrawn
  (`CONFLICT`), reads as `EXPIRED`, and is marked expired when the person asks again.
- `joinRequests(serverId, dir, cursor, limit)` lists the open requests for the Owner and the
  Moderators (`FORBIDDEN` for everyone else), newest first (`createdAt DESC, id DESC`, standard
  cursors, `limit` 50 by default and at most 100). `myJoinRequests` lists the caller's own open
  requests.
- `acceptJoinRequest(serverId, requestId)` (Owner or Moderator) runs the checks of a direct join
  and adds the person with the link's current role; the request need not be valid through the
  link any more (it may have been turned off or replaced since). A person who joined another way
  since asking (an invite, say) is left as they are: the request is closed as accepted, with
  `JoinRequestDecided` but no second `MemberJoined`, join count or join audit row.
  `declineJoinRequest` closes it;
  the person may ask the same workspace again 24 hours after the decline (`VALIDATION` naming the
  hours left before that). A request of another workspace is `NOT_FOUND`; one no longer open is
  `CONFLICT`.
- `withdrawJoinRequest(requestId)`: the person takes their open request back (`NOT_FOUND` for
  anyone else's); they may ask again at once.
- A ban cancels the person's open requests in that workspace, and the purge of a deleted account
  deletes its requests. A banned person cannot file one.
- `JoinRequest`: `id`, `serverId`, `workspaceName`, `userId`, `user`, `message`, `status` (`OPEN`,
  `ACCEPTED`, `DECLINED`, `WITHDRAWN`, `CANCELLED`, `EXPIRED`), `createdAt`, `expiresAt`,
  `decidedAt`, `decidedById`.

### Guest restrictions (enforced in the services)

- Creating a channel or a group, joining a channel, inviting into a conversation
  (`addConversationMembers`) and creating workspace invites: `FORBIDDEN`. Guests may open DMs.
- A public channel the guest has not been added to behaves like a private channel: `NOT_FOUND`
  on every read and write, absent from `channels` (a guest's channel list is always the joined
  list, whatever `joinedOnly` says), and from `searchMessages`, and its realtime
  events do not reach the guest.
- `serverMembers` for a guest lists only themselves and the members of conversations they share;
  `ServerMember` lookups of anyone else are `NOT_FOUND`. The directory events `MemberJoined`,
  `MemberLeft` and `MemberRoleChanged` reach a guest only when they are about the guest.
- Guests are not auto-joined to public channels when they join the workspace.
- Inside a conversation they belong to, a guest can do everything a member can.

### Kick, ban, timeout

Who may target whom (kick, ban, timeout, clear timeout): the actor must be Owner or Moderator;
nobody can target the Owner; a Moderator cannot target another Moderator; nobody targets
themselves (`VALIDATION`). Otherwise `FORBIDDEN`.

- `kickMember`: removes the member from the workspace and from every conversation of it, deletes
  their macros and pipelines there, emits `MemberLeft`. They can re-join with a new invite.
- `banMember(reason, expiresAt)`: records a ban (`server_ban`), then removes the member like a
  kick. A user who is not a member can be banned pre-emptively. While a ban is active (no
  `expiresAt`, or `expiresAt` in the future) accepting an invite, joining by invite and being
  added by the owner fail with `FORBIDDEN`; the refused join consumes no invite use.
  `unbanMember` deletes the ban; `serverBans` lists active bans (Owner/Moderator).
  In the current default workspace, an unban also restores a local account as Member with
  public channels, in the same transaction; it does not restore private channels, workspace
  DMs, macros or pipelines. In other workspaces a new invite is needed. After a temporary
  ban expires, or for an older account, `joinDefaultWorkspace` lets a signed-in local account
  rejoin the default workspace without an invite; an active ban returns `FORBIDDEN` with
  "you are banned from this workspace". Already joined accounts keep their role.
  Message authors expose `banned` and `bannedUntil` in that message's workspace to its
  members. `banReason` is null unless the viewer moderates that workspace. Expired and lifted
  bans report false with null end and reason.
- `timeoutMember(until, reason)`: `until` must be in the future and at most 30 days ahead. While
  it lasts, every write fails with `TIMED_OUT` (extension `until`, RFC 3339): sending, replying,
  editing, forwarding, reactions, pins, topic, attachments (REST upload: 403 with `until`),
  joining, inviting, creating conversations, and `executeCommand` of
  any `Mutating` (or stricter) command in that workspace. Reads continue, and a timed-out author
  may still delete their own messages over GraphQL. Expired timeouts are ignored;
  `clearTimeout` ends one early.

### Slow mode

- `setSlowMode(serverId, conversationId, seconds)`: Owner/Moderator, channels only, `0..=21600`
  (`0` turns it off), not on archived channels. Shown as `Conversation.slowModeSeconds`; changes
  emit `ConversationUpdated`.
- While it is on, a member's `sendMessage`, `replyToMessage` or forward into the channel fails
  with `RATE_LIMITED` (extension `retryAfterSeconds`, rounded up) if their previous message there
  is younger than the interval. Owner and Moderators are exempt. The check reads the author's
  last `sent_at` in the conversation; no in-memory state.

### Posting permissions

- `setChannelPosting(serverId, conversationId, policy, allowsThreads)`: Owner/Moderator, channels
  only (a DM or group is `VALIDATION`), not on archived channels (`CONVERSATION_ARCHIVED`).
  `policy` is `EVERYONE` (the default) or `MODERATORS` (the workspace's Owner and Moderators);
  `allowsThreads` (default `true` for a new channel) says whether, under `MODERATORS`, everyone
  else may still react and reply in threads, and is ignored under `EVERYONE`. Shown as
  `Conversation.postingPolicy` and `Conversation.postingAllowsThreads` (DMs and groups are
  always `EVERYONE`); changes emit `ConversationUpdated`. Setting what is already set writes
  nothing. Also the command `chat.set_channel_posting`.
- Under `MODERATORS`, the Owner and Moderators do everything as before. Everyone else (members
  and guests alike) is refused with `FORBIDDEN` ("only the owner and moderators can post in
  this channel") as follows:

  | Action | `allowsThreads: true` | `allowsThreads: false` |
  |---|---|---|
  | `sendMessage` (timeline replies with `parentMessageId` included) | refused | refused |
  | `forwardMessage` into the channel | refused | refused |
  | `replyToMessage` (thread reply) | allowed | refused |
  | adding a reaction | allowed | refused |
  | removing one's own reaction | allowed | allowed |
  | `pinMessage` / `unpinMessage` | refused | refused |
  | editing or deleting one's own messages | allowed | allowed |
  | reading, joining, leaving | allowed | allowed |

- The check runs after the archive and timeout gates and before slow mode, in the services, so
  commands, macros, pipelines and requests forwarded from other servers meet it too. Uploading
  an attachment is not checked; the message that carries it is.
- The default workspace's definition file sets it per channel (`docs/self-hosting.md`).

### Deleting messages

`deleteMessage`: the author, or an Owner/Moderator of the workspace who can read the
conversation (a moderator outside a private channel gets `NOT_FOUND`, like any read). A
moderator delete writes an `admin.delete_message` audit row with the message id and author (no
text). `MessageDeleted` is unchanged.

### Reports

- `reportMessage(serverId, conversationId, messageId, reason)`: any member who can read the
  message, once per message (`CONFLICT` on a second report); deleted messages cannot be
  reported. Allowed while timed out.
- `messageReports(serverId, status, dir, cursor, limit)`: Owner/Moderator, ordered newest first
  (`(createdAt, id)` descending; `AFTER` walks toward older reports) with the standard cursor
  contract (`docs/pagination.md`). Moderators
  see every report of the workspace, but the reported message's current text and author
  (`messageText`, `messageAuthorId`) are included only when the moderator can read the
  conversation (`messageReadable`); for a private channel they are not in, only the ids and the
  reporter's reason are shown.
- `resolveReport(serverId, reportId, outcome: RESOLVED | DISMISSED)`: Owner/Moderator; a report
  that is no longer open is `CONFLICT`.
- The direct space has no Owner or Moderator: everyone there is a guest. Its reports (of DMs and
  groups) go to the instance operators instead (`docs/api.md`, "Instance moderation"):
  `messageReports`, `messageReport` and `resolveReport` with the direct space's id
  (`relayInfo.directSpaceId`) answer an instance operator and refuse everyone else with
  `FORBIDDEN`, an app or bot acting for an operator too. An operator is no member of the
  conversations there, yet for a report of an unencrypted message sees that one message's
  `messageText`, `messageAuthorId` and `messageSentAt` (never the rest of the conversation;
  `messageReadable` stays false), and for any report whose author is shown the author's
  principal id (`messageAuthorPrincipalId`, for `instanceBlockPrincipal`). An encrypted message
  shows its author only through a verified report, as in a workspace. `deleteMessage` lets the
  operator delete a reported unencrypted message, or an encrypted one with a verified report,
  and no other message there. An operator gets nothing else there: no kick, ban, timeout, role
  change or slow mode.

### Audit

Every moderation action writes one `command_audit_log` row (initiator `moderation`) through one
shared helper, whether it came from GraphQL or a command: `admin.set_member_role`, `admin.kick`,
`admin.ban`, `admin.unban`, `admin.timeout`, `admin.clear_timeout`, `chat.set_slow_mode`,
`chat.set_channel_posting`, `chat.report_message`, `admin.resolve_report`,
`admin.delete_message`. Arguments hold ids and
flags only, never reasons or message text. An action run as a command also gets the generic
command-execution row. Refused actions are not audited by the helper.

The public link writes rows with initiator `public_link`: `public_link.enabled`,
`public_link.disabled`, `public_link.reissued` and `public_link.settings_changed` (the Owner's
changes, with the mode, role and `allow_remote`), `join_request.accepted` and
`join_request.declined` (the moderator's decisions, with the request and the person), and
`workspace.member_joined` with `"source": "public_link"` for every join through the link (the
joiner's row, with the role and the accepted request's id). Never the token.

### Realtime events

Two new `WorkspaceEvent` variants (snake_case fields, like every other variant):

- `{"type": "MemberLeft", "workspace_id": "…", "user_id": "…"}` — a member was kicked or banned.
  Delivered to the workspace (guests: only about themselves), including the removed user, whose
  subscription then ends. Optional `cause` is `kick`, `ban`, `workspace_deleted` or
  `account_deleted`; a temporary ban includes `until` (RFC 3339). For a kick or ban, only the
  removed user's own delivery includes the moderator's optional `reason`. Other members
  receive no reason. The removed member may keep this explanation until they dismiss it.
- `{"type": "MemberRoleChanged", "workspace_id": "…", "user_id": "…", "role": "member"}` — `role`
  is `owner`, `moderator`, `member` or `guest`. Same visibility as `MemberLeft`. A subscriber whose
  own role changes to or from `guest` gets the matching visibility from the next event on.

Join requests:

- `{"type": "MemberJoined", "workspace_id", "user_id", "username", "source": "public_link"}` —
  `source` is present only for a join through the public link, directly or by an accepted
  request.
- `{"type": "JoinRequestCreated", "workspace_id": "…", "request_id": "…", "user_id": "…"}` — a
  person filed a request.
- `{"type": "JoinRequestDecided", "workspace_id": "…", "request_id": "…", "user_id": "…",
  "outcome": "accepted"}` — `outcome` is `accepted`, `declined`, `withdrawn` or `cancelled`
  (a ban).

Both reach the workspace's Owner and Moderators on their `workspaceEvents(workspace)`
subscription, and the person, who is not a member, on their subscription to their server's
direct space (`workspaceEvents(relayInfo.directSpaceId)`, which a signed-in client keeps open):
`workspace_id` is still the workspace asked to join. On acceptance the person gets
`JoinRequestDecided` there and then subscribes to the workspace; `MemberJoined` goes to its
members as for any join.

A subscription that falls behind loses events, possibly any of these. Before it delivers another
event it reads the subscriber's membership and role again: it ends when they are no longer a
member (or the read fails), and otherwise goes on with the visibility of their current role.

## Reactions

`toggleReaction(serverId, conversationId, messageId, emoji)` adds the caller's reaction with
that emoji, or removes it when they already have it, and answers the message's reaction groups.
It needs write access to the conversation and a message that is not deleted.

- `emoji`, trimmed, is 1 to 32 characters and is either a Unicode emoji (pictographs with the
  components that join, modify or tag them: zero-width joiner, variation selectors, skin tones,
  hair, keycaps, regional indicators, tags; digits, `#` and `*` alone are not one) or a custom
  emoji's `:name:`, the name made of letters, digits and underscores. A `:name:` that starts a
  new emoji on the message must name a custom emoji of the workspace while the `custom_emoji`
  feature is enabled; with the feature off every new `:name:` is refused. Adding anything else
  answers `VALIDATION`. Removing checks only the length: a reaction the caller holds comes off
  whatever its text, so one stored before these rules stays removable.
- A message carries at most 20 different emoji, and one user puts at most 10 reactions on one
  message. Adding past either answers `VALIDATION`; joining an emoji the message already has
  (under the per-user cap) and removing a reaction always work. Toggles on one message run one
  at a time (the message row is locked), so concurrent toggles cannot pass the caps.
- Every toggle sends `ReactionsUpdated` to the conversation's members with all of the
  message's reaction groups, which the caps bound.

## Notification Rules

The server decides, once per message, who is told about it, and tells a message's author when
someone reacts to it.
For every recipient it stores a notification (the inbox, the bell badge and the email digest) and
sends that user a private `NotificationDue` event; clients only render what the server decided.
Sound and preview stay device-local client settings.

### Settings

- **Per-conversation level** (`setConversationNotificationLevel`, `Conversation.notificationLevel`):
  `ALL`, `MENTIONS` or `NONE`; null inherits the workspace default.
  `Conversation.effectiveNotificationLevel` is what applies now: `NONE` while a timed mute is in
  effect, else the conversation level, else the workspace default.
- **Workspace settings** (`myNotificationSettings`, `updateNotificationSettings`), per user and
  workspace: `defaultLevel` (default `ALL`; in the server's default workspace, which every
  account joins, `MENTIONS`, so a plain message there stores no notification per member unless
  they chose `ALL`, here or for the conversation), `keywords` (at most 20, each 2 to 40 characters after
  trimming, stored lower-cased and deduplicated), a daily do-not-disturb window `dnd`
  (`start`/`end` as `"HH:MM"` in an IANA `timezone`; crosses midnight when `end < start`; `start`
  must differ from `end`), `digest` (`OFF`, `HOURLY`, `DAILY`; `docs/email.md`), and
  `reactions` (default `true`: reactions to the user's messages notify them; null in
  `updateNotificationSettings` keeps the stored value).
- **Do-not-disturb pause** (`setDoNotDisturb`, command `notifications.dnd`): on until turned off,
  or until an instant (`dndPaused`, `dndPausedUntil`). It is separate from the daily window and
  `updateNotificationSettings` leaves it alone.
- **Mutes** keep working: `muteConversation` with `mutedUntil` is a timed `NONE` (it clears a
  `NONE` level so the mute really ends; another level stays); without `mutedUntil` it sets level
  `NONE`. `unmuteConversation` clears `mutedUntil` and a `NONE` level. `isMuted` is true for a
  timed mute in effect or level `NONE`; `mutedConversations` lists rows with level `NONE` or any
  `mutedUntil`.

### The decision, per member except the author

1. Candidates are the conversation's members (the list its events go to): a private channel
   never notifies a non-member, and a public-channel reader who has not joined is not notified.
   A timeout does not affect receiving.
2. A timed mute in effect yields nothing.
3. Effective level = conversation level, else workspace default. `NONE` yields nothing, direct
   messages and mentions included.
4. Reason, strongest first: `direct` (a Direct conversation), `mention` (`<@user>` naming the
   member, or `@channel` / `@here` as a whole word; in the server's default workspace these two
   notify only from its Owner or a Moderator and are plain text from anyone else), `keyword` (a keyword as a whole word,
   case-insensitive, outside `<@…>` tokens), `thread_reply` (a reply in a thread whose root the
   member wrote or where they replied), else `all`.
5. `MENTIONS` admits every reason but `all`; `ALL` admits everything.
6. Inside the do-not-disturb window (in the user's timezone) or pause the notification is still
   stored, but the event carries `quiet: true` so clients neither toast nor play a sound.

### Reactions

A reaction added to a message (`toggleReaction` that adds, not one that removes) tells its author,
with reason `reaction`:

1. Never about their own reaction or a deleted message, and only while the author is still a
   member of the conversation.
2. A timed mute in effect, effective level `NONE`, or the `reactions` setting off yields nothing.
   `MENTIONS` and `ALL` admit it.
3. Inside do-not-disturb it is stored and the event is `quiet`, as for messages.
4. The author has at most one reaction notification per message. The first reaction inserts it;
   each later one replaces its emoji and reactor, marks it unread and moves its `createdAt` to
   now, so the entry surfaces again. `NotificationDue` is sent when the entry is new, was read,
   or was last surfaced more than 10 minutes ago; a reaction replacing an unread entry from the
   last 10 minutes updates it silently, so reacting and un-reacting in a loop cannot make the
   author's client toast. A notification the message already has for another reason (a
   mention, say) is left as it is, and no event is sent.
5. Best-effort, after the reaction commits: a failure is logged and never fails the reaction.

Digest emails list messages only: reaction notifications never trigger, nor appear in, a
digest. Reading the conversation (`markAsRead`) marks them read like any other notification of
its messages.

One message yields at most one notification per user (unique `(user, message)`). Notifications
are created after the message commits (`sendMessage`, `replyToMessage`, and forwards into the
target conversation) and best-effort: a failure is logged and never fails the send. Edits do not
notify again. Deleting a message deletes its unread notifications (read ones stay).
`UserMentioned` is still emitted for the mentions view.

### Inbox

- `myNotifications(serverId, unreadOnly, dir, cursor, limit)`: newest first (`createdAt DESC,
  id DESC`, standard cursors; `AFTER` goes to older ones). Each item has `reason`, `createdAt`,
  `readAt` and nested `conversation` / `message`, which are null once the viewer can no longer
  read the conversation. A `REACTION` item also has `reactionEmoji` and `reactedBy` (the latest
  reaction); both are null for every other reason.
- `myUnreadNotificationCount(serverId)`; `markNotificationsRead(serverId, ids | allBefore)`
  (exactly one; returns how many changed; ids of other users are ignored).
- `markAsRead` on a conversation also marks its notifications for messages sent up to `readAt`
  read, in the same transaction; `markAllAsRead` marks every notification of the workspace read.
- Notifications older than 90 days are deleted hourly.

### `NotificationDue` event

```json
{"type": "NotificationDue", "workspace_id": "…", "user_id": "…", "notification_id": "…",
 "conversation_id": "…", "message_id": "…", "reason": "mention", "quiet": false}
```

`reason` is `direct`, `mention`, `keyword`, `thread_reply`, `all` or `reaction`. A `reaction`
event adds `reaction_emoji` and `reaction_by` (the reactor's user id); the two keys are absent
from every other event:

```json
{"type": "NotificationDue", "workspace_id": "…", "user_id": "…", "notification_id": "…",
 "conversation_id": "…", "message_id": "…", "reason": "reaction", "quiet": false,
 "reaction_emoji": "👍", "reaction_by": "…"}
```

The event is delivered only to `user_id` (the server-side scope list is `[user_id]`), guests
included.

## Titles

- `Channel` and `Group` conversations may have an optional `title`.
- Empty or whitespace-only titles are normalized to `None`.
- `Direct` conversations cannot have titles.

## Timeline And Threads

- Every message belongs to a `conversation_id`.
- A timeline message has `thread_id = None`.
- A thread reply has `thread_id = Some(thread_id)`.
- Creating a thread from a timeline message is optional.
- The root message remains in the main timeline after a thread is created.
- Replies inside the thread live only in the thread view; they do not appear in the main timeline listing.

## Reply Semantics

- Timeline replies use `parent_message_id` and still stay in the main timeline.
- Thread replies may point to:
  - the root timeline message for the thread, or
  - another message already inside the same thread.
- A message from another conversation or another thread is rejected as a reply parent.

## `thread_id` vs `root_thread_id`

- `thread_id`
  - `None` for main timeline messages
  - `Some(thread_id)` for thread replies
- `root_thread_id`
  - `Some(thread_id)` only for a timeline message that is the root of an existing thread
  - `None` for ordinary timeline messages
  - `None` for thread replies

This lets clients render the timeline normally while also knowing whether a timeline message can open an existing thread.

## Attachments

- Sending a message takes no attachments; the author adds them to a message they already sent.
- `createAttachment(serverId, conversationId, messageId, filename, contentType, sizeBytes)`:
  only the message's author, at most 25 MB. The declared size is reserved against the
  workspace quota (`RIM_MAX_WORKSPACE_STORAGE_BYTES`), the attachment is stored as pending
  upload, the message's `has_attachments` is set, and the answer is an upload ticket: the
  attachment id, an upload token, the size limit and an expiry.
- `POST /api/files/upload` (multipart: `upload_token`, `attachment_id` and one file) stores the
  bytes and marks the attachment uploaded; a file larger than declared is refused.
- `finalizeAttachment` marks an uploaded attachment ready. `deleteAttachment` removes it and
  refreshes `has_attachments`.
- An upload still pending 15 minutes after it started is marked failed by the stale-attachment
  cleanup job (`docs/operations/background-jobs.md`).
- Reading an attachment (`GET /api/attachments/{id}/download`) needs workspace membership and read
  access to the conversation of its message.
- In an encrypted conversation the client uploads the encrypted blob: `createAttachment` then
  requires `blobSha256`, a fixed file name and type, and the upload is checked against the hash
  (`docs/specs/e2ee-mls-v1.md`, "Attachments").
- The `attachments` instance feature switches all of this off (`docs/operations/features.md`).

## Pagination

- Timeline messages: `ORDER BY sent_at DESC, id DESC`
- Thread messages: `ORDER BY sent_at DESC, id DESC`
- Inbox conversations: `ORDER BY last_activity_at DESC, id DESC`
- Channels: `ORDER BY created_at ASC, id ASC` (filtered by `ChannelListFilter`: `joinedOnly`,
  `includeArchived`; both default to `false`)

Cursor pagination uses overfetch (`limit + 1`) so `next` and `prev` reflect actual additional data, not page-size guesses.

## Permalinks

- A timeline message's permalink is
  `/workspaces/{server}/conversations/{conversation}/messages/{message}`.
- A thread reply's permalink is
  `/workspaces/{server}/conversations/{conversation}/threads/{thread}/messages/{message}`.
- The email digest links each message by its permalink.

### Profile changes

`MemberProfileUpdated {workspace_id, user_id}` announces a successful local profile update.
It carries no profile fields. Workspace members refetch their member directory; guests see
only their own event. In the local direct space the account's own sessions receive it and
refetch `me`. A client showing the changed account's own profile invalidates `me` too. It
uses the directory stream rules (`w` for members and the subject's `u:` stream for guests).
Profiles published by another server are unchanged by this event.

### Ban changes

`MemberBanChanged {workspace_id, user_id}` follows every successful ban and unban, including
pre-emptive bans. It carries no reason. It follows directory visibility and stream rules;
clients refetch workspace message-author ban status. Ban expiry is evaluated from `until`
and the next query, without a scheduled event.
