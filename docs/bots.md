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

`seesMemberJoins`, on either, says whether the bot is told about members joining the workspace (see
"Polling for events"). It is off unless someone turns it on, `Bot.seesMemberJoins` shows it to any member,
and a change to it is in the `bot.update` audit entry.

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
| `GET /me` | `{ untrusted, id, name, workspaceId, runBy, readsGoTo, seesMemberJoins }` |
| `GET /channels` | `{ untrusted, channels: [{ id, name, private }] }`: the channels it was added to, archived ones left out |
| `GET /channels/{id}/messages?before=&limit=` | `{ untrusted, messages, cursor }`: a page, oldest first. `limit` is 1 to 100, 50 when left out. To go further back, pass `cursor` as `before`; it is null on the oldest page |
| `GET /channels/{id}/search?q=&limit=` | `{ untrusted, messages }`, newest first. `q` is 1 to 500 characters, and takes the operators of `search.md` but for `in:`, which answers `400`. `limit` is 1 to 100, and a search answers at most 50 even when `limit` is higher. No cursor |
| `POST /channels/{id}/messages` | Body `{"text": "..."}`. Answers `201` and `{ id, channelId }` |
| `GET /mentions?after=&limit=` | `{ untrusted, mentions, cursor }`: oldest first, `limit` as above |
| `GET /events?after=&limit=` | `{ untrusted, events, cursor }`: oldest first, `limit` as above |

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

**Polling for events.** `/events` tells a program what happened, so it can react without being mentioned.
Without `after` it starts from now, with an empty page and a cursor, so a new bot doesn't welcome everyone who
joined yesterday. Each answer has a `cursor` for the next call, the same one when nothing new came. Keep it
between runs: a restart with no cursor skips whatever happened while it was down, and an old cursor pages
forward from where it stood.

```sh
cursor=""
while true; do
  page=$(curl -sS -H "$AUTH" -G --data-urlencode "after=$cursor" "$RIM/events") || { sleep 30; continue; }
  echo "$page" | jq -c '.events[] // empty'
  cursor=$(echo "$page" | jq -r '.cursor // ""')
  sleep 5
done
```

```json
{ "untrusted": true,
  "events": [
    { "type": "message", "channelId": "...", "mentioned": false, "message": { "id": "...", "text": "..." } },
    { "type": "member_joined", "at": "2026-11-07T10:00:00+00:00",
      "user": { "id": "...", "name": "Ada", "isBot": false } }
  ],
  "cursor": "..." }
```

Ignore a `type` you don't know: new ones can be added.

- **`message`** is a message as `/channels/{id}/messages` has it, so a reply in a thread and a deleted message
  are left out. A mention in a thread is on `/mentions` and not here. `mentioned` says whether it mentions the
  bot. It comes from channels the bot is in at the time of the poll, not archived, and from since it was last
  added to each: being added doesn't hand over the channel's history, which `/channels/{id}/messages` reads. The
  bot's own messages are left out, so it can't react to itself. Other bots' messages are not, so check
  `authorIsBot` before answering one.
- **`member_joined`** comes only while an Owner or Moderator has `seesMemberJoins` on for the bot, and `/me` says
  whether they do. It holds the person's id and name, whether they are a bot and when they joined: no email, key,
  device or how they joined. A bot joining counts, and so does a member from another server. Someone who left and
  came back is a new event, and someone already gone by the time of the poll isn't one. A bot sees joins only
  from when it was allowed to, whatever the cursor: turning it on starts from that moment, turning it on again
  changes nothing, and turning it off stops them at once. Off and on again starts from the second time, so the
  joins between never show. It never lists the bot itself.
- **Not yet:** edits, deletes, reactions and presence.

A row is handed over once it is 2 seconds old, by the database's clock, so a message reaches a bot at least that
late. A row is stamped when its transaction starts and can be seen only when that commits, so anything younger
could appear behind a cursor already past it. A transaction running longer than that can be missed, so treat this
as what happened and not as a delivery guarantee. Polling every few seconds is fine, inside the limit below.

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
That member's messages and their joining the workspace reach `/events` like anyone's.

**Encrypted conversations.** `/events` has channels alone, and they are plaintext. A direct message, a group or an
encrypted conversation never reaches it.

## Incoming webhooks

An incoming webhook is a URL that posts to one channel as one bot, so a service that can call a URL
can post without running anything that holds a bot token. The server only receives: it calls nothing
back, and a webhook reads nothing.

**The URL is a secret.** Anyone holding it can post to that channel as that bot, within the limit, and
no signature is checked (`specs/bots-v1.md`). Keep it as you keep a password. If it leaks, revoke the
webhook and create another; the old URL stops working at once. Deactivating a bot revokes its webhooks, so
their channels can have new ones.

**Creating one.** An Owner or Moderator calls `createIncomingWebhook` with the channel, a bot of the
workspace and a preset, `PLAIN` or `GITHUB`, or `PLAIN` and a format (see "Webhook formats"). A channel
has one webhook that is not revoked; a second is `CONFLICT`, so revoke the first. Creating it adds the bot to the channel if it was not there, under the
rule for adding a bot. An Owner or Moderator creates one on a private channel only if they are in it,
since they can't post where they can't read, even when the bot is there already. Only a plaintext channel
that is not archived can have one.

The answer holds `token`, `rimhook_` and 43 base64url characters, shown once; only its SHA-256 is
stored. The URL is `POST /api/hooks/<token>` on the address of your server.

`incomingWebhooks` lists a workspace's webhooks with when each was created, last used and revoked and
its format, for an Owner or Moderator. The last use is kept to the minute. `revokeIncomingWebhook` ends
one. A caller who does not moderate its workspace gets `NOT_FOUND`, the same as for an unknown id.

**Plain.** The body is JSON with a string `text`, at most 4,000 characters after trimming. Markdown in
it stays as you wrote it.

```sh
curl -H 'Content-Type: application/json' -d '{"text":"The deploy is **done**."}' \
  "https://rim.example/api/hooks/$RIM_WEBHOOK_TOKEN"
```

**GitHub.** A webhook with the preset `GITHUB` takes what a repository or organization sends, through a
format RIM ships. In the repository's **Settings**, **Webhooks**, **Add webhook**:

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
under every format. People whose notification level is `ALL` hear of the post as of any other message.

**Answers.**

| Status | Body | When |
|---|---|---|
| `200` | `{"ok":true}` | It was posted. |
| `202` | `{"ok":true,"ignored":true}` | Nothing to post. |
| `400` | A refusal, as in "The API" | A body the webhook's format cannot read, or text a command refuses. |
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

## Webhook formats

A format says how what a service sends becomes a message, so a service RIM knows nothing about can post
without any code. An Owner or Moderator gives one to `createIncomingWebhook` as `template`, with the
preset `PLAIN`, or changes a webhook's with `updateIncomingWebhookFormat`. The URL stays the same. The
contract is `specs/bots-v1.md`.

**A format doesn't say who sent something.** Anyone holding the URL can send any header and any body, and
no signature is checked, so the URL is still the only thing to protect. A condition chooses what to post,
and it keeps nobody out. Don't put a secret in a format: every Owner and Moderator of the workspace can
read it.

**A small one.** A backup script that posts when it finishes:

```json
{
  "version": 1,
  "rules": [
    {
      "when": [{ "field": "status", "equals": "failed" }],
      "post": "**Backup failed** on {host}: {reason}"
    },
    {
      "when": [{ "field": "status", "equals": "ok" }],
      "post": "Backup finished on {host}."
    }
  ]
}
```

```sh
curl -H 'Content-Type: application/json' -d '{"status":"failed","host":"db-1","reason":"disk full"}' \
  "https://rim.example/api/hooks/$RIM_WEBHOOK_TOKEN"
```

That posts `**Backup failed** on db-1: disk full`. A `status` of anything else matches no rule, so it
answers `202` and posts nothing.

**GitHub's, in the same language.** The preset `GITHUB` reads a format RIM ships, and `webhookFormats`
shows it. This is its rule for a merged pull request. It comes before the rule for a closed one, because
the first rule that holds decides:

```json
{
  "when": [
    { "header": "X-GitHub-Event", "equals": "pull_request" },
    { "field": "action", "equals": "closed" },
    { "field": "pull_request.merged", "equals": "true" }
  ],
  "post": "{repository.full_name}: {sender.login} merged pull request #{pull_request.number}: {pull_request.title}",
  "link": { "value": "{pull_request.html_url}", "origin": "https://github.com/" }
}
```

**Rules.** The first rule whose conditions all hold decides, and the rest aren't read. A rule with no
conditions always holds.

**Conditions.** Each is a `header` or a `field` with `equals`, always a string, so `"true"` and not `true`.
There is no regular expression, "contains" or "not". For more than this, run a bot on the events feed.

- A field compares as text: a string exactly, a number in decimal, a boolean as `true` or `false`. An
  object, an array, `null` or a missing field doesn't hold.
- A header's name matches in any case and its value exactly. Only names starting `x-` work, and none
  containing `token`, `secret`, `signature`, `auth`, `cookie` or `key`, which are refused by name when you
  save. Those are the usual places for a shared secret. RIM can't recognise one anywhere else.

**Paths.** Keys with dots between them, and a number for a place in an array: `alerts.0.labels.alertname`.
Each part is letters, digits, `_` and `-`. A key with a dot in it can't be reached.

**The text.** `post` is your text with `{path}` placeholders. It keeps its Markdown and line breaks.

- `{release.name|release.tag_name}` takes the first that has a value. `{{` and `}}` are literal braces, and
  anything else in braces is refused when you save.
- A value is put on one line: whitespace and control characters become single spaces, and format characters
  are removed, so a bidi override can't reorder the message and a joined emoji can come apart. Its Markdown is
  escaped. It is never expanded again, so a title of `{sender.login}` shows as that text. A header can't be a
  placeholder, so a format can't print one.
- `<@id>`, `@channel` and `@here` are made plain text in the whole post, your own text included, and the
  post is cut to 1,000 characters, counting the link.

**The link.** `link` is optional: a `value` written like `post` and an `origin` such as
`https://github.com/`. It's added after the text only when it starts with the origin and holds nothing odd
(`<`, `>`, brackets, quotes, a backtick, a backslash, `*`, `|`), up to 512 characters. Otherwise the message
posts without it.

**What it answers.**

| What happened | Answer |
|---|---|
| A rule held and all its placeholders had values | `200`, posted |
| No rule held | `202`, nothing posted |
| The rule that held has a placeholder with no value: missing, blank, `null`, an object or an array | `202`, nothing posted, and later rules aren't tried |
| The link has no value, or fails its check | `200`, posted without the link |
| The body had to be read and isn't JSON | `400` "The body must be JSON." |

Headers are checked first, and the body is read only when a rule that could still hold needs a field or a
placeholder. So an event nobody handles answers `202` whatever its body. The rest of "Answers" above applies
as to any webhook.

**Limits,** refused when you save, with the rule's number:

| What | At most |
|---|---|
| Rules | 20 |
| Conditions in a rule | 5 |
| `post` as written | 1,000 characters |
| Placeholders in a rule | 30 |
| A path | 256 characters |
| A header name | 64 characters |
| The whole format as JSON | 16 KiB |

A key RIM doesn't know is refused too, so a typo like `equal` can't quietly make a rule always hold. When a
delivery comes in, a header value over 64 characters counts as missing, and at most 50 `x-` headers are read.

**Ready-made formats.** `webhookFormats` lists them with a setup note and a sample: GitHub and Grafana. A
GitHub webhook left as it is gets the fixes of each release. Anything else is a copy that belongs to its
webhook, so it stays as it is when RIM ships a newer one. That includes Grafana's, and GitHub's once you've
edited it. To share a format, copy its JSON. `updateIncomingWebhookFormat` with `GITHUB` or `PLAIN` and no
`template` goes back to that preset.

**Trying one.** `previewWebhookFormat` takes a format, sample headers and a sample body of at most 256 KiB, and
says which rule would decide and what it would post, or why nothing would. It runs what a delivery runs, and it
posts and stores nothing.

**An older app** shows a webhook with a format as Plain, and revoking it still works.

**Rolling the server back** to a release without formats leaves a webhook with one acting as plain: a body with
a string `text` posts that text, and any other body answers `400`. It still lists and can be revoked, and the
next upgrade brings its format back. Migrating down drops the formats (`operations/upgrade.md`).

**Encrypted conversations and other servers.** A webhook is only ever on a plaintext channel, so a format never
reaches an encrypted conversation. A format lives with its webhook on the server hosting the workspace, and runs
there. An Owner or Moderator from another server changes it through the same GraphQL on that host.

## Seeing what a service sends

A format is easiest to write against a real delivery. A test session holds what arrives, in memory, for a short
time, and only the person who started it can read it. The contract is `specs/bots-v1.md`, "Webhook test
sessions".

**For a new webhook.** `startWebhookTestSession(serverId)` answers an id, a token and an expiry. Paste
`/api/hooks/<token>` on your server's address into the service. A delivery to it posts nothing and answers
`{"ok":true,"test":true}`. When you save, pass the session's id as `testSessionId` to
`createIncomingWebhook`, and the webhook keeps that token: the URL in the service starts posting, with nothing
to change. The answer then has `keptTestUrl` true and an empty `token`, since RIM holds only the token's hash and
you already have the URL. If the session ran out first, `keptTestUrl` is false and `token` is a new one, shown
once: put that URL in the service instead.

**For an existing webhook.** `startWebhookTestSession(serverId, webhookId)` has no URL of its own. The webhook
keeps posting under its saved format, and the session gets a copy of each delivery while it is open.

**Reading it.** `webhookTestDeliveries(sessionId, after)` answers what arrived after `after`, oldest first. Each
delivery has when it arrived, its `Content-Type`, its `x-` headers and its body as text. A form-encoded
`Content-Type` is the usual reason a body isn't JSON: choose `application/json` in the service's settings. To see
what a delivery would post, give it to `previewWebhookFormat` with the format you're writing.

**What isn't kept.** No address, `Authorization`, cookie or header but the `x-` ones, and none of those whose name
a format refuses, such as `x-gitlab-token`. A body over 256 KiB is held as too large without its bytes, and one
that isn't UTF-8 as not text. Nothing a session holds reaches the database, a log line, an audit entry or a
metric. The audit log has `bot.webhook.test_session.start` and `bot.webhook.test_session.end`, with ids alone.

**Limits.**

| What | At most |
|---|---|
| Deliveries held | 20 a session, the oldest dropped |
| What a session holds | 1 MiB, the oldest dropped |
| Open sessions | 3 a workspace, 20 on the server. Past that, starting one is `RATE_LIMITED` |
| Deliveries to a new webhook's test URL | 30 a minute, then `429` |
| How long it lasts | 15 minutes from the start or the last read, never more than an hour |

`endWebhookTestSession` ends one at once, saving ends an unbound one, and revoking a webhook ends its sessions. A
restart of the server ends them all. A bound session takes nothing from the webhook's own limit.

**Who.** Starting one takes what creating a webhook takes, and for an existing webhook what changing its format
takes, including being in its private channel. Nobody else can read it, and its owner can't either once they lose
that standing.

**Encrypted conversations and other servers.** A test URL posts nowhere, and a webhook is only ever on a plaintext
channel. A session lives on the server hosting the workspace, in its memory.

## Welcoming people from a pipeline

A pipeline on the Member Joined trigger can post as a bot with no program behind it. An Owner or Moderator
adds a Post as bot action to `createPipeline` or `updatePipeline`, in `PipelineActionInput`:

```json
{ "commandId": "chat.post_message",
  "postAsBot": { "botUserId": "...", "conversationId": "...", "text": "Welcome, {name}!" } }
```

- **The bot posts without asking anyone,** each time someone joins, so only an Owner or Moderator can add or
  change this, and only on a pipeline created by one. Anyone who may change the pipeline can keep it unchanged.
- **The bot and channel.** An active bot of the workspace, and a plaintext channel that isn't archived with the
  bot already in it: adding a bot to a channel stays its own step. For a private channel, the person saving and
  the pipeline's creator must be in it.
- **The text** is 1 to 10,000 characters and takes `{username}` and `{name}`. Anything else in braces is refused
  by name, so a typo doesn't get posted to everyone. `{username}` is the person's username, at their home server
  if they are from another one. `{name}` is their display name, or their username without one.
- **A name can't do anything.** A display name is written by whoever's joining, so control and format characters
  are removed from it and its Markdown is escaped. `<@id>`, `@channel` and `@here` are made plain text in the
  whole post, your own text included, so a welcome mentions nobody. People whose notification level is `ALL`
  hear of the post as of any other message.
- **It's checked again each time it fires:** the creator is still an Owner or Moderator, the bot is active and in
  the channel, and the channel is still plaintext and not archived. If not, it posts nothing and the server logs a
  warning with the pipeline and the reason. A bot joining doesn't fire it, so bots can't welcome each other. A
  pipeline with other actions runs those for a bot too.
- **It's the bot's post.** Its badge, message budget, slow mode, moderation and audit entry all apply, and the entry
  holds no text.
- **Past a limit, a welcome is skipped, not queued.** That is the pipeline's cooldown or the bot's budget. As for
  any pipeline, more than 10 firings in a minute switches it off and tells its creator, so a rush of people joining
  can stop your welcomes until someone turns it back on.
- **Every member can see it,** as they see every pipeline: `PipelineActionGql.postAsBot` has the text, the bot and
  the channel's id. Don't put anything in the text you wouldn't show them. Saying which channel is the client's
  job, and it doesn't name one the member can't see.
- **An older app** shows the action as the command `chat.post_message` with its `args`, and warns it would be skipped,
  which isn't so. Saving the pipeline from there keeps the action. Changing those `args` is refused, and removing
  the action works.
- **Rolling the server back** to a release without this action can't read a pipeline that has one, and the workspace's
  pipelines stop firing and listing. Remove these actions first.
- **Encrypted conversations and other servers.** It never posts into an encrypted conversation. A member from
  another server joining fires it on the server hosting the workspace, like anyone joining.

## Audit

Each management call writes an entry to the workspace's audit log: `bot.create`, `bot.update`,
`bot.deactivate`, `bot.token.create`, `bot.token.revoke`, `bot.webhook.create`, `bot.webhook.update`,
`bot.webhook.revoke`, `bot.webhook.test_session.start` and `bot.webhook.test_session.end`. An entry holds the bot's id and, for a token, its id and label, and for a webhook,
its id and channel. It never holds a token or its hash, or the text of a format.

Each call of the bot API writes one too, with the initiator `bot`: `bot.me`, `chat.list_channels`,
`chat.get_recent_messages`, `chat.search_messages`, `chat.post_message`, `bot.mentions` and `bot.events`. A
call refused for its token, its limit or a channel the bot cannot see runs nothing and writes no entry. A
poll for mentions or events that returned nothing writes none either, since nothing was handed over. A
`bot.events` entry holds how many messages and joins it handed over, and no text or name.

## Switching it off

`RIM_FEATURES_DISABLED=bots` switches bots off for the whole instance (`operations/features.md`).
Nothing can be created or managed, and tokens and webhooks are refused. A bot can't be added to a
channel, and it can still be removed. Bots that exist stay members, still marked as bots with their
disclosure.
