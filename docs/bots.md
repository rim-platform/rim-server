# Bots

A bot is an account of one workspace that software runs instead of a person. An Owner or a Moderator
creates it, and it reads and writes only in the channels they add it to. The contract is
`specs/bots-v1.md`; this page says how the accounts work.

RIM runs nothing behind a bot and cannot see what does. Whoever runs one chooses that, with their
own model and key.

## What a bot is

- A `user` row of the kind `bot`, with a name and no email, key, password, device or session. It cannot
  sign in.
- A `GUEST` of its workspace, and that role cannot change. Creating it adds it to no channel.
- Only in channels. A direct message, a group or an encrypted conversation with a bot is refused, and
  channels are plaintext.
- Marked as a bot wherever it shows: `User.isBot` and `ServerMember.isBot`.
- On the server that hosts its workspace, and nowhere else. It has no handle and no home relay, and
  it is not offered to other servers. A member from another server sees it as they see anything
  from that host.

A workspace holds at most 25 bots that are not deactivated. It cannot have one in the direct space,
and each bot counts as a member against `RIM_LIMIT_MEMBERS_PER_WORKSPACE`.

## Creating and changing one

An Owner or Moderator calls `createBot` with a name and a disclosure. The name follows the rules of
a display name. `updateBot` changes any of these later.

`deactivateBot` is the way to stop a bot. Its tokens stop working and it leaves the workspace and every
channel. Its messages stay, shown with its name. There is no way back in this version; create a new
bot instead.

## Disclosure

Every bot has two statements, each 1 to 200 characters of plain text:

- `runBy`: who runs it.
- `readsGoTo`: where what it reads goes.

Everyone who can see the bot can see both, and who added it: `User.bot` and `ServerMember.bot`, for
every member, including from another server. They are what the person who added the bot says. RIM
cannot check them, and a client says so where it shows them.

`bots` lists a workspace's bots for any of its members.

## Tokens

A token is `rimbot_` and 43 base64url characters. It is shown once, when it is created, and only its
SHA-256 is stored, so a lost token cannot be shown again: revoke it and make another.

`createBot` makes the first one, labelled `default`. `createBotToken` makes more, up to 5 that are not
revoked. `botTokens` lists them with when each was created, last used and revoked, for an Owner or
Moderator. The last use is kept to the minute. Tokens do not expire on their own.

`revokeBotToken` ends one. A caller who does not moderate the bot's workspace gets `NOT_FOUND`, the
same as for an unknown id.

A token stops working when it is revoked, when its bot is deactivated, kicked or banned, when its
workspace is deleted, and while the `bots` feature is off.

## The API

See `specs/bots-v1.md`.

## Audit

Each management call writes an entry to the workspace's audit log: `bot.create`, `bot.update`,
`bot.deactivate`, `bot.token.create` and `bot.token.revoke`. An entry holds the bot's id and, for a
token, its id and label. It never holds a token or its hash.

## Switching it off

`RIM_FEATURES_DISABLED=bots` switches bots off for the whole instance (`operations/features.md`).
Nothing can be created or managed and tokens are refused. Bots that exist stay members, still marked
as bots with their disclosure.
