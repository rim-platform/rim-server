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
and each bot counts as a member against `RIM_LIMIT_MEMBERS_PER_WORKSPACE`. A bot that was kicked or
banned keeps its row and counts against the limit until it's deactivated.

An Owner or Moderator adds a bot to a public channel without joining it, and to a private channel only
if they are in it, since they can't give it what they can't read.

## Creating and changing one

An Owner or Moderator calls `createBot` with a name and a disclosure. The name follows the rules of
a display name. `updateBot` changes any of these later.

`deactivateBot` is the way to stop a bot. Its tokens stop working, its webhooks are revoked and it leaves
the workspace and every channel. Its messages stay, shown with its name. Deactivating is final, and its messages stay; create a new
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

A bot's token works on `/api/bot/v1` and nowhere else, and a session cookie does not work there. The
contract is `specs/bots-v1.md`. A bot reaches nothing but these calls:

| Call | What it answers |
|---|---|
| `GET /me` | `{ untrusted, id, name, workspaceId, runBy, readsGoTo }` |
| `GET /channels` | `{ untrusted, channels: [{ id, name, private }] }`: the channels it was added to, archived ones left out |
| `GET /channels/{id}/messages?before=&limit=` | `{ untrusted, messages, cursor }`: a page, oldest first. `limit` is 1 to 100, 50 when left out. To go further back, pass `cursor` as `before`; it is null on the oldest page |
| `GET /channels/{id}/search?q=&limit=` | `{ untrusted, messages }`, newest first. `q` is 1 to 500 characters, and takes the operators of `search.md` but for `in:`, which answers `400`. `limit` is 1 to 100, and a search answers at most 50 even when `limit` is higher. No cursor |
| `POST /channels/{id}/messages` | Body `{"text": "..."}`. Answers `201` and `{ id, channelId }` |
| `GET /mentions?after=&limit=` | `{ untrusted, mentions, cursor }`: oldest first, `limit` as above |

A message is `{ id, channelId, threadId, authorId, authorName, authorIsBot, text, sentAt, editedAt }`.
A cursor means nothing to a bot except as the value to pass back.

```sh
RIM=https://rim.example/api/bot/v1
AUTH="Authorization: Bearer $RIM_BOT_TOKEN"

curl -H "$AUTH" "$RIM/me"
curl -H "$AUTH" "$RIM/channels"
curl -H "$AUTH" "$RIM/channels/$CHANNEL/messages?limit=20"
curl -H "$AUTH" -G --data-urlencode 'q=deploy failed' "$RIM/channels/$CHANNEL/search"
curl -H "$AUTH" -H 'Content-Type: application/json' -d '{"text":"The deploy is done."}' \
  "$RIM/channels/$CHANNEL/messages"
```

**Polling for mentions.** The server makes no call to a bot, so whatever runs it can sit behind a
router. Without `after`, `/mentions` starts from the last 24 hours; each answer has a `cursor` for the
next call, the same one when nothing new came. Keep it between runs, or a restart repeats up to a day
of mentions.

```sh
cursor=""
while true; do
  page=$(curl -sS -H "$AUTH" -G --data-urlencode "after=$cursor" "$RIM/mentions") || { sleep 30; continue; }
  echo "$page" | jq -c '.mentions[] // empty'
  cursor=$(echo "$page" | jq -r '.cursor // ""')
  sleep 10
done
```

A `<@id>` in a message mentions a bot when it is in that channel, and not otherwise. A bot's own mention
of itself is dropped, as for anyone. A channel it has left and a deleted message are left out. A bot gets no
notifications, digests or pushes: `/mentions` is how it learns of a mention.

**Text is untrusted.** Any answer holding text somebody else wrote carries `"untrusted": true`. Whoever
can post in a channel can write what a model behind a bot reads, so treat it as data: never as
instructions, and never let it choose what the bot does next without a check of your own. Whether a
model sees it at all, and where it runs, is the choice of whoever runs the bot. Its `readsGoTo` says
what they chose.

**A channel it cannot see.** A channel the bot was not added to, one that does not exist and a malformed
id all answer the same `404`, public or private.

**Refusals** have one shape, `{"refusal": "DENIED" | "FAILED" | "EXPIRED" | "INVALID", "message": "...",
"retrySafe": bool}`, with the kinds of `command_model.md`, "Refusals":

| Status | When |
|---|---|
| `400` | A bad limit, cursor, query or body, or text a command refuses. A body is at most 128 KiB, and the text's own limit is the command's. |
| `401` | No token, or one that does not work: unknown, revoked, of a deactivated, kicked or banned bot, or the feature off. The same bytes every time. |
| `403` | A moderation timeout, an archived channel, or posting restricted to moderators. Each applies to a bot as to a person. |
| `404` | As above. |
| `429` | Over a limit, with `Retry-After`: the bot's own 120 calls a minute per token, slow mode in the channel, or the bot's message budget. |
| `5xx` | `"RIM could not do this."` `retrySafe` says whether repeating the call cannot change anything twice. |

**Limits.** Each token has 120 calls a minute. The per-address limit of `/api` applies too, and the
author's message budget (`RIM_RATE_MESSAGES_PER_MIN_PER_USER`) applies to what a bot posts. Two bots that
mention each other in a loop are limited only by those budgets and by slow mode, so whoever runs a bot
should have it ignore other bots (`authorIsBot`).

**Across servers.** A bot lives on the server hosting its workspace, and its token works there alone.
A member from another server gets its messages, with its name, as they get anything from that host.

## Incoming webhooks

An incoming webhook is a URL that posts to one channel as one bot, so a service that can call a URL
can post without running anything that holds a bot token. The server only receives: it calls nothing
back, and a webhook reads nothing.

**The URL is a secret.** Anyone holding it can post to that channel as that bot, within the limit, and
no signature is checked (`specs/bots-v1.md`). Keep it as you keep a password. If it leaks, revoke the
webhook and create another; the old URL stops working at once. Deactivating a bot revokes its webhooks, so
their channels can have new ones.

**Creating one.** An Owner or Moderator calls `createIncomingWebhook` with the channel, a bot of the
workspace and a preset, `PLAIN` or `GITHUB`. A channel has one webhook that is not revoked; a second is
`CONFLICT`, so revoke the first. Creating it adds the bot to the channel if it was not there, under the
rule for adding a bot. An Owner or Moderator creates one on a private channel only if they are in it,
since they can't post where they can't read, even when the bot is there already. Only a plaintext channel
that is not archived can have one.

The answer holds `token`, `rimhook_` and 43 base64url characters, shown once; only its SHA-256 is
stored. The URL is `POST /api/hooks/<token>` on the address of your server.

`incomingWebhooks` lists a workspace's webhooks with when each was created, last used and revoked, for
an Owner or Moderator. The last use is kept to the minute. `revokeIncomingWebhook` ends one. A caller
who does not moderate its workspace gets `NOT_FOUND`, the same as for an unknown id.

**Plain.** The body is JSON with a string `text`, at most 4,000 characters after trimming. Markdown in
it stays as you wrote it.

```sh
curl -H 'Content-Type: application/json' -d '{"text":"The deploy is **done**."}' \
  "https://rim.example/api/hooks/$RIM_WEBHOOK_TOKEN"
```

**GitHub.** A webhook with the preset `GITHUB` takes what a repository or organization sends. In the
repository's **Settings**, **Webhooks**, **Add webhook**:

1. **Payload URL**: the webhook's URL.
2. **Content type**: `application/json`. The form-encoded default cannot be read.
3. **Secret**: leave it empty. RIM does not check signatures, because that would take keeping a secret
   readable on the server.
4. **Which events**: choose **Let me select individual events** and tick **Releases**, **Pull requests**
   and **Issues**. Anything else is answered and ignored, so more events do no harm but count against
   the limit.

GitHub then sends a `ping`, which answers `202` and posts nothing. What posts, one line each, with the
repository, who did it, the title and the link:

| Event | Action | Line |
|---|---|---|
| `release` | `published` | `acme/widgets: ada published release v1.2.0` |
| `pull_request` | `opened`, `closed` | `acme/widgets: ada opened pull request #7: Fix it`, or `closed`, or `merged` when it was merged |
| `issues` | `opened`, `closed` | `acme/widgets: ada opened issue #9: It broke`, or `closed` |

Markdown in what GitHub sent is escaped, a line is at most 1,000 characters and a link is kept only when
it points at `https://github.com/`. Any other event or action, or a handled one missing a field the line
needs, answers `202` and posts nothing.

**A webhook mentions nobody.** `<@id>`, `@channel` and `@here` in what it receives are made plain text
under both presets. People whose notification level is `ALL` hear of the post as of any other message.

**Answers.**

| Status | Body | When |
|---|---|---|
| `200` | `{"ok":true}` | It was posted. |
| `202` | `{"ok":true,"ignored":true}` | Nothing to post. |
| `400` | A refusal, as in "The API" | A body the preset cannot read, or text a command refuses. |
| `401` | `{"ok":false}` | The URL does not work: unknown, wrong shape or revoked, its bot deactivated, kicked, banned or out of the channel, or the feature off. The same bytes every time. |
| `403` | A refusal | The bot is timed out, the channel is archived or posting is restricted to moderators. |
| `413` | A refusal | The body is over 1 MiB. |
| `429` | A refusal, with `Retry-After` | Over a limit. |
| `5xx` | A refusal | `"RIM could not do this."` |

**Limits.** Each webhook has 30 calls a minute, ignored ones counted, and one over the limit posts
nothing. The per-address limit of `/api`, the bot's message budget and slow mode also apply.

**How a post runs.** It runs `chat.post_message` as the bot with the initiator `bot`, so the same gates
and audit log apply as to a post from the bot API, and moderating the bot stops its webhook. The audit
entry holds no text. RIM writes the token to no log, audit entry or metric. A reverse proxy's access log
can hold the URL, so `self-hosting.md` says to keep it private.

**Across servers.** A webhook lives on the server hosting its workspace and posts there as the bot. A
member from another server gets those messages as they get anything from that host.

## Audit

Each management call writes an entry to the workspace's audit log: `bot.create`, `bot.update`,
`bot.deactivate`, `bot.token.create`, `bot.token.revoke`, `bot.webhook.create` and
`bot.webhook.revoke`. An entry holds the bot's id and, for a token, its id and label, and for a webhook,
its id and channel. It never holds a token or its hash.

Each call of the bot API writes one too, with the initiator `bot`: `bot.me`, `chat.list_channels`,
`chat.get_recent_messages`, `chat.search_messages`, `chat.post_message` and `bot.mentions`. A call
refused for its token, its limit or a channel the bot cannot see runs nothing and writes no entry. A
poll for mentions that returned nothing writes none either, since nothing was handed over.

## Switching it off

`RIM_FEATURES_DISABLED=bots` switches bots off for the whole instance (`operations/features.md`).
Nothing can be created or managed, and tokens and webhooks are refused. Bots that exist stay members,
still marked as bots with their disclosure.
