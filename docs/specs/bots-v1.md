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
(`docs/specs/e2ee-mls-v1.md`). The server has no keys for one, so this is a rule it can enforce.

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
- The preset decides how the body becomes text.
  - `plain`: `{"text": "..."}`, at most 4,000 characters.
  - `github`: the `X-GitHub-Event` header decides. A published release, a pull request opened, closed
    or merged and an issue opened or closed become a line with the repository, who did it, the title
    and the link, with Markdown in what GitHub sent escaped. A `ping` and every other event answer
    `202` and post nothing.
- Signatures are not checked in this version, because checking `X-Hub-Signature-256` needs a secret kept
  readable on the server. So the URL alone is enough to post, and it has to be kept like a password.

## Across servers

- A bot lives on the server that hosts its workspace. A member from another server sees its messages
  and its disclosure as they see anything from that host.
- A bot is never a remote user, is not offered to other servers and has no home relay. Its token works
  on that one server. It takes no part in the direct space and cannot message anyone outside the
  workspace.
- The public relay runs no agent behind a bot. A workspace on it can have bots run by whoever owns
  them, elsewhere, using the same API.

## What this does not cover

- Replies in a thread from the bot API, editing, reactions, attachments and any other command.
- Outbound webhooks and a mention pushed to the bot. It polls.
- Edits, deletes, reactions and presence in `/events`: it has new messages and members joining.
- A bot run by the server itself. `docs/vision.md` says how an owner may run one next to the server;
  it uses this same API and needs nothing more from this contract.
- Signed webhooks and presets other than GitHub.
