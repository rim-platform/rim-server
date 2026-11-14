# Bots and incoming webhooks, v1

**Status: draft.** This is the contract for a bot account in a workspace and for an incoming webhook
bound to one. `docs/vision.md` ("Working with agents") says why. `docs/specs/agent-data-flow-v1.md`
covers an agent that runs on a person's machine and acts as them; a bot is the other case, an
identity of its own that the server verifies.

The key words MUST, MUST NOT, SHOULD and MAY are used as in RFC 2119.

## What RIM controls

The server enforces a bot's identity, its tokens, which channels it can reach and what it can do
there. RIM does not control what runs behind a bot or where what it reads goes. Whoever runs it
chooses that, with their own model and key. RIM runs no model service for bots.

The disclosure below is what the person who added the bot says. RIM cannot check it, and the app
MUST say so where it shows it.

## A bot

- A bot belongs to one workspace, on the server that hosts it. An Owner or a Moderator of that
  workspace creates it and can change or deactivate it. Deactivating is final, and its messages stay. It cannot be created in the direct
  space. A workspace holds at most 25 active bots.
- A bot is a user with the kind `bot`. It has a name, and no email, key, password, device or session,
  so it cannot sign in.
- A bot that was kicked or banned keeps its row and counts against the limit until it's deactivated.
- A person cannot start a direct message or a group with a bot.
- A bot sees members joining its workspace only if an Owner or Moderator turned that on for it with
  `seesMemberJoins`. It is off by default, and turning it on or off is recorded in the audit log. Who
  joins a workspace isn't channel content, and a guest cannot otherwise see the member directory.
  A bot sees joins only from when it was allowed to. Turning it on starts from that moment, turning it
  on again changes nothing, and turning it off and on starts again from the second time.
- A pipeline's Post as bot action posts as a bot when someone joins (see "A pipeline posting as a
  bot").
- Bots are switched off for a whole instance with the `bots` feature in `RIM_FEATURES_DISABLED`.
  While it is off, nothing can be created, and bot tokens and webhooks are refused.
  With the feature off, a bot can't be added to a channel, and it can still be removed.

## Who runs it

- A bot MUST have a `runBy` (who runs it) and a `readsGoTo` (where what it reads goes), each 1 to 200
  characters of plain text, set by the person who creates it and changeable by an Owner or Moderator.
- Everyone who can see the bot can see both, and who added it. Clients MUST show them next to the bot
  in a channel's members and from any message it sends, marked as what that person says.
- A bot's messages MUST be marked as a bot's in every client, so nobody takes one for a person.

## Tokens

- A token is `rimbot_` followed by 43 base64url characters. It is shown once, when created, and only
  its SHA-256 is stored. A bot has at most 5 active tokens, and an Owner or Moderator can revoke any
  of them. Tokens do not expire on their own. The last time each was used is shown.
- A bot token works only on the bot API below. It MUST NOT be accepted by `/api`, `/api/ws`, file
  routes or anything else, and a session cookie MUST NOT be accepted on the bot API. So a bot has no
  way to reach any other query, mutation or command.
- A token that is unknown, revoked or belongs to a deactivated bot or one no longer in its workspace
  gets the same `401`.

## What a bot can reach

- A bot is a workspace member with the role `GUEST`, which reads and writes only in conversations it
  was explicitly added to. That role does not change.
- Only an Owner or Moderator can add a bot to a channel. A bot never joins by itself, and joining a
  workspace adds it to no channel. Adding a bot to a private channel is how it gets to read it.
- An Owner or Moderator adds it to a public channel without joining it, and to a private channel only
  if they are in it, since they can't give it what they can't read.
- A bot's grants are the channels it was added to, and in them only: read messages, search, post a
  message, see its mentions and poll for new messages. Every other command is out of its reach, because the API offers
  nothing else.
- A bot's actions need no confirmation. A post and a search run as commands with the initiator
  `BOT`, under the same capability, role, timeout, rate limit and audit gates as anyone else. A read
  a command can't answer runs on the service behind it, keeps the token and channel checks, and
  writes its own audit entry. The dispatcher gates apply to commands. A moderation timeout, slow
  mode, a ban or an archived channel applies to it as to a person.
- A `<@id>` mention reaches a bot when it is in that channel, and not otherwise.
- A bot gets no notifications, digests or pushes. It reads its mentions from `/mentions`, and what
  happened from `/events`.

## Encrypted conversations

Bots never join an encrypted conversation. A bot can only be a member of a channel. Adding one to a
direct message, a group or any encrypted conversation MUST be refused, and channels stay plaintext
(`docs/specs/e2ee-mls-v1.md`). The server has no keys for one, so this is a rule it can enforce. A webhook
can only be on a channel, so neither it nor its format reaches an encrypted conversation. An unbound test
session posts nowhere, and a bound one copies what reaches a plaintext channel's webhook.

## The bot API

Under `/api/bot/v1`, authorized with `Authorization: Bearer rimbot_...`. JSON both ways.

| Call | What |
|---|---|
| `GET /me` | The bot's id, name, workspace, `runBy`, `readsGoTo` and `seesMemberJoins` |
| `GET /channels` | The channels it was added to: id, name, private |
| `GET /channels/{id}/messages?before=&limit=` | Messages, oldest first in the page, at most 100 |
| `GET /channels/{id}/search?q=&limit=` | Messages matching `q`, at most 100, no cursor |
| `POST /channels/{id}/messages` | Body `{"text": "..."}`. Posts it, answers its id |
| `GET /mentions?after=&limit=` | Where it was mentioned since the cursor, oldest first, at most 100 |
| `GET /events?after=&limit=` | What happened since the cursor, oldest first, at most 100 |

- A channel it was not added to and one that does not exist get the same `404`.
- A message is its id, channel, thread, author's name, whether the author is a bot, text and times.
  Text somebody else wrote is data: every response holding any carries `"untrusted": true`.
- `/mentions` answers the messages and a `cursor` for the next call. Without `after` it starts from
  the last 24 hours. A bot learns about a mention by polling: it runs where its owner chooses, often
  behind a router, and the server makes no outbound call to it. A channel it has since left is left
  out.
- `/events` answers `events` and a `cursor` for the next call, the one it was given when nothing new
  came. Events of every type sort together by time, then id, and a cursor is one position among them.
  Without `after` the page is empty and the cursor is now, so nothing from before the first call is
  replayed. A cursor that is old still pages forward.
  - A client MUST ignore an event whose `type` it does not know. New types can be added.
  - `{"type": "message", "channelId": "...", "mentioned": bool, "message": {...}}` is a message as
    `/channels/{id}/messages` has it, so a thread reply and a deleted message are left out. `mentioned`
    comes from the same record as `/mentions`, so a mention in a thread reply is on `/mentions` and not
    here. It is in a channel the bot is in at the time of the poll, not archived, sent since the bot
    was last added to it. The bot's own messages are left out, so it can't react to itself.
  - `{"type": "member_joined", "at": "...", "user": {"id": "...", "name": "...", "isBot": bool}}` is
    someone becoming a member of its workspace, and only while `seesMemberJoins` is on. It holds nothing
    more of them: no email, key, device or how they joined. A bot joining counts, and a member from
    another server does. The bot itself doesn't, and neither does anyone who joined before it was
    allowed to see them, whatever the cursor. Someone who left and came back is a new event. Someone
    who has left by the time of the poll is not one.
  - A row is handed over once it is at least 2 seconds old, by the database's clock. A row is stamped
    when its transaction starts and can be seen only when that commits, so a younger one could appear
    behind a cursor already past it. A transaction running longer than that can still be missed: this
    feed tells a program what happened and isn't a delivery guarantee.
- A refusal is `{"refusal": "DENIED" | "FAILED" | "EXPIRED" | "INVALID", "message": "...", "retrySafe":
  bool}`, the same kinds as `docs/command_model.md`, "Refusals". Status: `400` invalid, `401` token,
  `403` denied, `404` not found, `429` with `Retry-After`, `5xx` failed.
- Each token has its own limit of 120 calls a minute. The per-address limit of `/api` and the message
  budget of the author also apply.

Reads are recorded in the audit log with the initiator `BOT`, as the gates above record any command.
A poll for mentions or events that returned nothing isn't recorded, since nothing was handed over. A
poll for events records how many of each kind it handed over, and no text or name.

## Incoming webhooks

- An Owner or Moderator creates one for a channel and binds it to a bot of that workspace. A channel
  has at most one active webhook. Creating it adds the bot to the channel if it was not there. It
  cannot be created for a conversation that is not a channel. An Owner or Moderator creates one on a
  private channel only if they are in it, since they can't post where they can't read.
- The URL is `POST /api/hooks/{token}`, with the token `rimhook_` followed by 43 base64url
  characters, shown once. Only its SHA-256 is stored. It is revoked by an Owner or Moderator, and it
  stops working with its bot. Deactivating a bot revokes its webhooks. The last time it was used is
  shown.
- Anyone holding the URL can post to that one channel as that bot, within the limit. A proxy's access
  log can hold the URL, so `docs/self-hosting.md` says to keep such logs private. RIM itself MUST NOT
  log the token. A leaked URL is revoked and a new one made.
- A webhook has its own limit of 30 posts a minute, and a body limit of 1 MiB.
- A post runs `chat.post_message` as its bot with the initiator `BOT`, so the gates and the audit log
  apply. A moderation action on the bot stops its webhook too.
- A webhook's text MUST NOT mention anyone: `<@id>`, `@channel` and `@here` in what was received are
  made plain text.
- A webhook's format decides how the body becomes text (see "Webhook formats"). It is one of three.
  - The preset `plain` and no `template`: `{"text": "..."}`, at most 4,000 characters.
  - The preset `github` and no `template`: the format RIM ships for GitHub, where the `X-GitHub-Event`
    header decides. A published release, a pull request opened, closed or merged and an issue opened or
    closed become a line with the repository, who did it, the title and the link, with Markdown in what
    GitHub sent escaped. A `ping` and every other event answer `202` and post nothing.
  - The preset `plain` and a `template`: that format.
- Signatures are not checked in this version, because checking `X-Hub-Signature-256` needs a secret kept
  readable on the server. So the URL alone is enough to post, and it has to be kept like a password. A
  format changes none of this.

## Webhook formats

A format says how what a service sends becomes a message, with no code and no new URL. An Owner or
Moderator writes one or starts from a ready-made one. The GitHub behaviour above is a format RIM ships,
written in this same language.

```json
{
  "version": 1,
  "rules": [
    {
      "when": [
        { "header": "X-GitHub-Event", "equals": "pull_request" },
        { "field": "action", "equals": "closed" },
        { "field": "pull_request.merged", "equals": "true" }
      ],
      "post": "{repository.full_name}: {sender.login} merged pull request #{pull_request.number}: {pull_request.title}",
      "link": { "value": "{pull_request.html_url}", "origin": "https://github.com/" }
    }
  ]
}
```

**Rules.** Rules are tried in order. The first whose conditions all hold decides, and later rules MUST
NOT be read. A rule with no conditions always holds. When no rule holds, the answer is `202` and nothing
is posted.

**Conditions.** A condition is `{"header": name, "equals": text}` or `{"field": path, "equals": text}`,
and `equals` is the only test.

- A field compares as text: a string exactly, a number in decimal, a boolean as `true` or `false`. An
  object, an array, `null` or a missing field MUST NOT hold.
- A header's name matches in any case and its value exactly. A name MUST start with `x-` and MUST NOT
  contain `token`, `secret`, `signature`, `auth`, `cookie` or `key`, in any case. Saving another is
  refused, and the refusal names the header.
- A delivery's headers are read only when their names start with `x-`. A value over 64 characters counts
  as missing. At most 50 such headers are read, so a delivery with more can lose some.

**Paths.** A path is keys with dots between them, each part `[A-Za-z0-9_-]+`, such as
`alerts.0.labels.alertname`. A part reads that key of an object, and a number also reads that index of an
array. A key holding a dot can't be reached.

**`post`.** Literal text with `{path}` placeholders.

- `{a|b}` takes the first path that has a value. `{{` and `}}` are literal braces. Anything else in braces
  and a lone brace MUST be refused when saved, and so MUST a `post` that is blank.
- A string that isn't blank, a number and a boolean are values. When a placeholder has none (missing,
  blank, `null`, an object or an array), the rule MUST post nothing and answer `202`, without trying
  later rules.
- A value MUST be put on one line, with whitespace and control characters turned into single spaces and
  format characters removed, so a bidi override can't reorder the message, and have its Markdown escaped,
  including the `:` of a `://` and the `.` of a `www.`, so the value never becomes a link. Only a link a
  format's link field names, under the origin the format allows, is one. A joined emoji in a value can come apart. A value left blank by this has none. It MUST NOT be expanded
  again: a title of `{sender.login}` shows as that text.
- The literal text is what the Owner or Moderator wrote, so its Markdown and line breaks stay.
- Every mention in the result, the literal text's own included, MUST be made plain text, as for any
  webhook. The result MUST then be cut to 1,000 characters, counting the link.

**`link`,** optional, is a `value` written like `post` and an `origin`, which MUST be `https://`, a host,
an optional port and a single `/`, and nothing else. The finished value is added after the text only if
it starts with the origin, has something after it, is at most 512 characters and is printable ASCII
without `<`, `>`, `(`, `)`, `[`, `]`, `\`, a backtick, a quote, `*` or `|`. Otherwise, and when a
placeholder in it has no value, the message MUST still post, without a link. Each `@` that begins
`@channel` or `@here`, in any case, MUST be written `%40` in the link, and the 512 characters count
after that.

**When the body is read.** Header conditions are checked first, and the body MUST be parsed only when a
rule that could still hold needs a field or a placeholder. So a `ping`, or an event no rule handles,
answers `202` whatever its body. A body that must be read and isn't JSON answers `400` "The body must be
JSON.", with nothing of the body in the answer.

**Limits.** Each of these, an unknown key anywhere and a `version` other than `1` MUST be refused when
saved, naming the rule's number where there is one, so a typo like `equal` can't make a rule always hold:
20 rules; 5 conditions a rule; 1,000 characters of `post` as written; 30 placeholders a rule; 256
characters a path; 64 a header name; 16 KiB the whole format as JSON.

**What a format doesn't check.** Anyone holding the URL can send any header and any body, so a condition
says which deliveries post and not who sent them. A format can't print a header, because `post` and `link`
read the body alone. Refusing the names above keeps the usual headers for a shared secret out of what is
stored. RIM can't recognise a secret under any other name or in an `equals` value.

**Where it lives.** The column `template` of the webhook holds the format.

| Webhook | `preset` | `template` |
|---|---|---|
| Plain | `plain` | null |
| GitHub, unedited | `github` | null: it reads the format the running release ships |
| Any other format | `plain` | the format, a copy that belongs to this webhook |

- With a `template`, the `preset` MUST be `plain`. A copy doesn't change when RIM ships a newer format.
- Only an Owner or Moderator can read a webhook's format, as for the list of webhooks.
- Changing a format takes the same standing as creating a webhook, including being in a private channel,
  and it MUST NOT change the URL. A revoked webhook MUST be refused. The audit entry holds ids and no
  text of the format.
- RIM ships ready-made formats, GitHub and Grafana, and lists them for any signed-in client. Only GitHub
  is read live. Choosing another stores a copy.
- There is no command for creating or changing a format, as there is none for creating a webhook, so
  nothing that runs commands can change one.

**Trying one.** A preview takes a format and a sample of headers and a body, at most 256 KiB, and says
which rule would decide and what it would post, why nothing would post, or why the format would be
refused. It MUST run the same code as a delivery, and it MUST NOT post or store anything. It is for an
Owner or Moderator of that workspace.

**Older clients and servers.**

- A client MUST send `template` and call the new roots only when `serverInfo.capabilities` lists
  `webhook_formats`, because an older server refuses the whole call. The preset values don't change, so a
  client without formats reads every webhook, showing one with a `template` as `plain`, and can still revoke it.
- A server without formats ignores `template`, so a webhook with one acts as `plain`: a body with a string
  `text` posts that text, and any other body answers `400` and posts nothing. No listing breaks, and
  upgrading again brings the format back. Migrating down drops the column and every format in it.

## Webhook test sessions

A test session lets an Owner or Moderator see what a service really sends, so a format can be written
against it. It is held in memory for a short time and seen only by the person who started it.

**Two kinds.**

- **Unbound,** for a webhook not made yet: `startWebhookTestSession(serverId)` answers an id, a token and an
  expiry. The URL is `POST /api/hooks/{token}`, the same shape as a webhook's. A delivery to it MUST NOT post
  anything, and it answers `200` `{"ok":true,"test":true}`.
- **Bound** to an existing webhook: `startWebhookTestSession(serverId, webhookId)` has no URL of its own.
  Deliveries to the webhook's URL go on as before, under its saved format, its answers and its limits, and
  the session keeps a copy of each one that passes the webhook's own rate limit. Without a session, nothing
  of a delivery is kept.

**Who.** Starting one takes the standing of creating a webhook in that workspace, and for a bound one the
standing of changing that webhook's format, including being in its private channel. A session belongs to the
person who started it. Reading it MUST be refused with `NOT_FOUND` for anyone else, for one that ended, and for
its owner once they no longer have the standing to start it.

**Saving promotes an unbound session.** `createIncomingWebhook` takes `testSessionId`. When the session is the
caller's, for the channel's workspace and still open, the new webhook keeps that session's token, so a URL
already pasted into a service starts posting with nothing to change, and the session ends. The answer's
`keptTestUrl` is then true and its `token` is the empty string: the server holds only the token's hash, and
the client already has the URL. When the session has expired or ended, `keptTestUrl` is false and `token` is
a new one, shown once, as without a session. A client MUST read `keptTestUrl` to tell the person whether the
URL changed. Someone else's session, or one of another workspace, MUST be refused. A session can be promoted
once. A client that never sends `testSessionId` always gets a real token.

**What a delivery holds.** When it arrived, its `Content-Type`, the `x-` headers as a format reads them, and the
body as text. The header names a format may not use (see "Webhook formats") MUST be dropped, by the same list,
so a header such as `x-gitlab-token` is never kept. No address, `Authorization`, cookie or other header is kept: the headers a proxy adds with the sender's address
(`Forwarded`, `X-Forwarded-*`, `X-Real-IP`, `X-Client-IP`) are dropped, and a format can't test them.

- A body over 256 KiB is held as "too large" without its bytes, and an unbound session answers `413`.
- A body that isn't UTF-8 is held as "not text" without its bytes.
- A body is held exactly as it came. A client MUST show it as plain text, with invisible characters shown, and
  never as Markdown.
- What it would post isn't worked out when a delivery arrives. A client asks a preview, under whatever format
  it is showing.

**Limits.** This is an unauthenticated route that fills memory, so each of these is enforced:

- 20 deliveries and 1 MiB a session. Past either, the oldest are dropped.
- 3 open sessions a workspace, 3 an account across all its workspaces, and 20 an instance. Starting one past
  any of them answers `RATE_LIMITED`.
- 30 deliveries a minute to an unbound session, then `429`. The per-address limit of `/api` applies too. A bound
  session takes nothing from the webhook's own limit.
- A session lasts 15 minutes from its start or its last read, and never more than an hour from its start. Expired
  sessions are dropped by a sweep, and `endWebhookTestSession` drops one at once. Revoking a webhook ends its
  bound sessions.

**Reading it.** `webhookTestDeliveries(sessionId, after)` answers the deliveries after `after`, oldest first, and
the session's expiry. Each read extends the session as above.

**Where it is held.** Sessions are held in the memory of the server process, as confirmations and rate limits
are. A restart drops them. A server is one process: nothing supports running several on one database yet.

**What it records.** What a session holds MUST NOT reach the database, a log line, an audit entry or a metric.
Starting and ending one write audit entries holding ids alone, and the sweep writes none. A request to a
session's URL counts in the HTTP metrics as a request to any webhook does. The token MUST NOT be logged. It is
held in memory as its SHA-256, and only promotion stores that hash, as for any webhook.

- There is no command for any of this, so nothing that runs commands reaches it. Bots being switched off stops it.
- A client MUST call these roots and send `testSessionId` only when `serverInfo.capabilities` lists
  `webhook_test_sessions`.

## A pipeline posting as a bot

A pipeline with the Member Joined trigger can have a Post as bot action: a bot, a channel and a text.
Nobody confirms each post, because an Owner or Moderator chose this one.

- **Member Joined only, in this version.** A bot post triggered by new messages could make bots set each
  other off. A bot joining doesn't fire it, so bots can't welcome each other.
- **It needs authority both when it is saved and when it fires.**
- When it is saved, adding or changing one takes an Owner or Moderator, and a pipeline whose creator is one.
  The bot MUST be active and in this workspace. The channel MUST be a plaintext channel of the workspace
  that is not archived, with the bot already in it, and it is never added for you. If the channel is
  private, the person saving and the pipeline's creator MUST be in it, since they can't post where they
  can't read. Anyone who may change the pipeline may keep an action unchanged.
- When it fires, the server MUST check again that the pipeline's creator is an Owner or Moderator, the bot
  is active and in the channel, and the channel is still plaintext and not archived, with the creator in
  it if it is private. If any fails it posts nothing and logs a warning naming the pipeline and the
  reason. An encrypted conversation is never posted into. Bots being switched off stops it too.
- **The author is the bot.** It runs `chat.post_message` with the initiator `BOT` and the bot as actor, so
  its badge, budget, slow mode, moderation and audit all apply. The audit entry holds no text. This doesn't
  weaken the rule that a pipeline's `ExecuteCommand` actions never confirm themselves.
- **The text** is 1 to 10,000 characters, with the placeholders `{username}` and `{name}` and nothing else in
  braces: any other `{...}` is refused by name. `{username}` is someone's username, at their home server if
  they are from another one, and `{name}` is their display name, or their username without one.
- **A joining person's name MUST NOT be able to do anything.** Control and format characters are removed from
  what fills a placeholder and its Markdown is escaped. Every mention in the result, `<@id>`, `@channel` and
  `@here`, is made plain text, the text's own included.
- **Every member can see it,** as they see every pipeline of the workspace: the text, the bot and the channel's
  id. A client MUST NOT show the name of a channel the member can't see.
- **A burst of joins is limited, not queued.** The pipeline's cooldown and the bot's budget skip a welcome past
  their limit, and it never comes later. As for any pipeline, more than 10 firings in a minute switches it
  off and tells its creator.
- A client that doesn't know the action reads it as the command `chat.post_message` with the bot, channel and
  text in `args`. When it saves the pipeline and sends those back unchanged, the action is kept. Different
  `args` are refused, so it never turns into a command run as the creator.

## Across servers

- A bot lives on the server that hosts its workspace. A member from another server sees its messages
  and its disclosure as they see anything from that host.
- A bot is never a remote user, is not offered to other servers and has no home relay. Its token works
  on that one server. It takes no part in the direct space and cannot message anyone outside the
  workspace.
- A member from another server joining fires a Post as bot action like anyone, on the host, and its
  `{username}` is their username at home.
- A webhook's format lives with the webhook, on the host. A delivery and a preview run there, and an Owner
  or Moderator from another server changes a format through the same GraphQL on that host.
- A test session and its URL live on the host too, and its URL is built from the host's address as a webhook's
  is.
- The public relay runs no agent behind a bot. A workspace on it can have bots run by whoever owns
  them, elsewhere, using the same API.

## What this does not cover

- Replies in a thread from the bot API, editing, reactions, attachments and any other command.
- Outbound webhooks and a mention pushed to the bot. It polls.
- Edits, deletes, reactions and presence in `/events`: it has new messages and members joining.
- A bot run by the server itself. `docs/vision.md` says how an owner may run one next to the server;
  it uses this same API and needs nothing more from this contract.
- Signed webhooks.
- In a format: conditions other than `equals`, loops over arrays, or several messages from one delivery.
- A permanent log of what a service sent: a test session keeps it for at most an hour. A shared library of
  formats: a format belongs to its webhook.
- A Post as bot action on any trigger but Member Joined, a mention in its text, or a welcome queued past a
  limit.
