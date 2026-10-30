# Instance features

An operator can switch off optional modules of rim-server with `RIM_FEATURES_DISABLED`. Unset,
nothing changes: every feature works as before, except that email reports `unavailable` when
SMTP is not configured, which is how it already behaved.

## Catalogue

| Name | What it covers | Why it can be switched off |
|---|---|---|
| `attachments` | `POST /api/files/upload`, `GET /api/attachments/{id}/download` (and both when another relay forwards them) and the `createAttachment`, `finalizeAttachment` and `deleteAttachment` mutations | Stored files cost disk and backups, and some instances must not hold files at all. Messages work without them. |
| `link_previews` | Fetching previews for links in new messages (no fetch is started while off, so the server makes no outbound request for them), and `Message.linkPreviews` (empty while off) | The server fetches URLs users post, which leaks the server's address to those sites and costs outbound traffic. |
| `email` | Invite mail and notification digests | Some instances must not send mail at all. |
| `custom_emoji` | The `customEmojis` query, the `createCustomEmoji` and `deleteCustomEmoji` mutations, and `GET /api/emoji/{id}` (also when another relay forwards it) | Uploaded images are content the operator moderates and stores. |
| `workspace_plugins` | The `workspacePlugins` query, the `addWorkspacePlugin`, `updateWorkspacePlugin` and `removeWorkspacePlugin` mutations, and `POST`/`GET /api/plugin-bundles/…` (and the forwarded download) | Hosting plugin bundles distributes code to members' clients. Plugins a user installs locally on their client are not affected. |
| `ai_commands` | `commandDiscover`, `commands(surface: AI)`, `executeCommand` and `executeMacro` with the `AGENT` initiator, and the agent-facing fields of `Command` (`nlDescription` is null, `intentSamples` empty and `aiCallable` false while off) | An instance may not want an AI agent driving its workspaces. |
| `avatars` | `POST /api/files/avatar`, `GET /api/files/{path}` (the stored avatars; both also when another relay forwards them), `updateMyProfile` with an `avatarUrl`, and `User.avatarUrl` and `ServerMember.avatarUrl` (null while off) | Uploaded profile pictures are content the operator moderates and stores. |
| `text_art_check` | Scoring the readable workspace messages that look like text art with the image check service, and flagging and reporting those that score high (`docs/messaging.md`, "Flagged messages"); while off no message is scored or flagged, existing flags stay and `clearMessageFlag` still clears them | The text of those messages goes to the image check service, and an instance may not want its messages scored. |
| `avatar_check` | Scoring uploaded profile pictures with the image check service, refusing those at or above `RIM_AVATAR_CHECK_BLOCK` and reporting those at or above `RIM_AVATAR_CHECK_REPORT` (`docs/self-hosting.md`, "Content check"); while off pictures are stored unchecked | Pictures go to the image check service, and an instance may not want them scored. |

These are platform core and have no switch: end-to-end encrypted conversations, key identity,
the relay and host roles (`RIM_ROLE`), registration (its own modes: `RIM_OPEN_REGISTRATION`,
`RIM_REGISTRATION_POW_BITS`), moderation, the audit log, commands, keybinds, macros and
pipelines.

## States

Each feature is in one of three states:

| State | Meaning |
|---|---|
| `enabled` | The feature works. |
| `disabled` | The operator listed it in `RIM_FEATURES_DISABLED`. |
| `unavailable` | The configuration lacks something it needs; a reason says what. |

Unavailable today:

| Feature | When | What happens |
|---|---|---|
| `email` | `RIM_SMTP_HOST` or `RIM_SMTP_FROM` is not set | Mail is logged (recipient and subject) instead of sent, as before. |
| `text_art_check` | `RIM_AVATAR_CHECK_URL` is not set | No message is scored or flagged. |
| `avatar_check` | `RIM_AVATAR_CHECK_URL` is not set | Pictures are stored unchecked. |

A feature both disabled and unavailable is reported as `disabled`. Storage is always the local
data directory, so `attachments`, `custom_emoji`, `workspace_plugins` and `avatars` are never
unavailable.

## Setting it

A comma-separated list of names; spaces around names are ignored:

```sh
RIM_FEATURES_DISABLED=attachments,custom_emoji
```

An unknown name stops the server at boot, naming it, so a typo never leaves a feature on:

```text
RIM_FEATURES_DISABLED: unknown feature: atachments (known: attachments, link_previews, email, custom_emoji, workspace_plugins, ai_commands, avatars, plugin_link_fetch, text_art_check, avatar_check)
```

The switches are read at boot; change the variable and restart. At boot the server logs the
disabled and unavailable features:

```json
{"level":"INFO","fields":{"message":"instance features (RIM_FEATURES_DISABLED)","disabled":"[\"attachments\", \"custom_emoji\"]","unavailable":"[\"email: no mail server is set up\"]"},"target":"rim_server::app"}
```

Switching a feature off deletes nothing. Stored attachments, emoji, plugin entries, bundles,
link previews and avatars stay in the database and on disk, and come back when the feature is
enabled again. While `attachments` is off, messages still list their attachments, but downloads
are refused. While `avatars` is off, every avatar URL reads as null, so clients show their
placeholder; `updateMyProfile` without an `avatarUrl` still works.

With `email` disabled, invite mail is not sent (the invite link is still returned to whoever
created it), and the notification digest job does not run.

## Link preview limits

While `link_previews` is on, the server fetches each page under these limits:

| Limit | Value |
|---|---|
| Links previewed per message | 5 |
| Time per fetch, redirects and body included | 5 seconds |
| Redirects followed | 5 |
| Body read | 1 MiB; a longer body is cut off there and the link gets no preview |
| Fetches at once, per process | `RIM_LINK_PREVIEW_CONCURRENCY`, 1 to 64, default 8 |

A fetch that finds every slot busy for five seconds is dropped with a log line and its link gets
no preview. The server connects only to public addresses: a link, a redirect or a name
resolving to a loopback, private, link-local, shared, reserved or other special-use address is
not fetched, IPv6 forms that embed an IPv4 address (mapped, NAT64, 6to4) included. Outgoing
requests ignore proxy environment variables.

## What clients see

- **`serverInfo.features`**, readable without a session: one entry per feature, in a fixed
  order, `{ key state reason }`, where `state` is `ENABLED`, `DISABLED` or `UNAVAILABLE` and
  `reason` is set only for an unavailable feature. A client ignores a `key` it does not know.
- **The discovery document** `GET /.well-known/rim-server` has a `features` object beside the
  signed descriptor: `{"attachments":{"state":"disabled"},"email":{"state":"unavailable","reason":"…"},…}`.
  It is unsigned and advisory; see the server-to-server spec.
- **Errors.** A GraphQL resolver of a feature that is not enabled answers with the error code
  `FEATURE_DISABLED` and a `feature` extension naming it. The file and plugin bundle routes
  answer `403` with `{"error":"…","code":"FEATURE_DISABLED","feature":"attachments"}`.

A client reads `serverInfo.features` after connecting and hides the surfaces of disabled and
unavailable features: the attach button, emoji management, the workspace plugin settings, the
AI agent, "forgot password", the profile picture upload. The server enforces the switches either way.

`ai_commands` refuses agent runs by the initiator the client declares. It switches the
AI surface off for well-behaved clients; the capability checks, confirmation and audit that
apply to every command are the security boundary, not this switch.
