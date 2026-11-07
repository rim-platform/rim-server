# Home layout

The app's Home is made of boxes, each half or full width, in an order the person chooses. The
layout is saved on the person's home server, one per account, so every device shows the same
Home. This page is the server's contract for it: what is stored, what the server checks, how two
devices' edits meet, and who can read it. What each box shows is the client's.

## What is stored

One layout per account:

```json
{
  "version": 1,
  "boxes": [
    { "id": "b1", "kind": "waiting", "width": "half" },
    { "id": "b2", "kind": "channel", "width": "full",
      "settings": { "workspaceId": "…", "conversationId": "…", "count": 5 } },
    { "id": "b3", "kind": "quick", "width": "half",
      "settings": { "actions": [ { "commandId": "workspace.open_home" } ] } }
  ]
}
```

- **`version`**: the layout's format, 1.
- **`boxes`**: the boxes in the order Home shows them.
  - **`id`**: names the box within the layout.
  - **`kind`**: what the box shows.
  - **`width`**: `half` or `full`.
  - **`settings`**: the kind's settings, when it has any.

With the layout the server keeps `editedAt`, when the person made the edit, as their client said
(below), and `updatedAt`, when the server stored it.

Nothing is stored until the person saves a layout. Without one, a client shows its default Home.

## What the server checks

`saveHomeLayout` refuses with `VALIDATION`, naming what failed, a layout that is not:

- a JSON object of at most 16 KiB (16,384 bytes) written as compact JSON;
- of `version` 1;
- holding at most 30 boxes in `boxes`.

And each box:

- an `id` of 1 to 64 characters, unique in the layout;
- a `kind` of 1 to 64 characters;
- a `width` of `half` or `full`;
- `settings`, when present, a JSON object.

The server checks nothing more. It doesn't know the kinds or read the settings, and stores the
layout as sent, so a box of a kind a newer client added survives a save from an older one.

## Kinds

The kinds the RIM app knows. The server doesn't check them; a client does.

| Kind | Shows | Settings |
|---|---|---|
| `waiting` | What waits for the person | none |
| `mentions` | Mentions and replies | none |
| `unread` | Unread conversations | none |
| `agentRuns` | The person's agents' runs | none |
| `direct` | The latest direct messages | `count`: 3, 5 or 10 |
| `channel` | One channel's latest messages | `workspaceId`, `conversationId`, `count`: 3, 5 or 10 |
| `quick` | Buttons that run commands | `actions`: 1 to 6, each a `commandId` and optional `presetArgs` |

A quick action follows the rules of a personal keybind (`keybind_model.md`): any keybind-capable
command, with preset arguments of at most 4096 bytes. Pressing it runs the command as the person's
own click, so a command that asks for confirmation still asks.

A client keeps a box of a kind it doesn't know in its place, as one line saying it needs a newer
version of RIM, and saves it back unchanged.

## Newest edit wins

Each save carries `editedAt`, the time the person made the edit on their device. A device that
was offline sends its edit later with the time it was made.

`saveHomeLayout(layout, editedAt)`:

1. checks the layout as above;
2. takes `editedAt`, or the server's own clock when `editedAt` is later than it;
3. when the stored layout's `editedAt` is later than that, keeps the stored layout and answers
   `kept: false` with it;
4. otherwise stores the new layout with that `editedAt` and answers `kept: true` with it.

The comparison and the write are one step, so of two saves arriving together the one with the
later `editedAt` is kept. A client that gets `kept: false` shows the layout it was given and tells
the person their layout changed on another device. A client that fetches a layout newer than its
own copy does the same.

`resetHomeLayout` deletes the stored layout, so every device goes back to the default. With
nothing stored, the next save is kept whatever its `editedAt`.

## Who can read it

Only its owner, signed in at their home server. No other person, workspace Owner or Moderator
reads or changes it through the API. The server's operator can read it in the database, as every
other setting.

A person from another server signed in here, whose home server is elsewhere, gets `FORBIDDEN`
from `homeLayout`, `saveHomeLayout` and `resetHomeLayout`: their layout lives at their own server.

Saving and resetting are ordinary mutations: they need the CSRF header and count against the
per-session mutation limit (`api.md`, "Limits"), as keybind changes do. They write no audit
entry, since the layout is the person's own preference, as their keybinds are. The server's logs
never contain a layout.

**What it holds.** Workspace and conversation ids, command ids and their preset arguments.
No message, file or name. A box for a channel on another server stores that workspace's and
channel's ids at the person's home server.

**Account deletion.** The layout stays while the deletion waits out its grace period. When the
purge runs it is removed with the account's other settings (`api.md`, "Deleting an account").
Leaving a workspace, or a channel being deleted or archived, doesn't change the layout: the box
says so on the device and offers to remove it.

## Encrypted channels

A `channel` box names an encrypted channel by its ids only. The device decrypts what the box
shows, as it does for the channel's timeline, and nothing of it goes into the layout, so the
server never sees those messages through it.

## Across servers

The layout lives on the person's home server only. Each box's content comes from its workspace's
own host, the way the app reaches that workspace anyway, and a host never sees the layout. When
the home server can't be reached, the app shows its copy of the layout and what it last loaded,
and boxes whose workspace is on another server keep updating.

## What a client keeps

A client keeps, per account on each device, a copy of the last layout it fetched and an edit not
yet sent with its `editedAt`, which it sends once the server answers. It also keeps a list of
boxes hidden on that device only, which it never sends: a box hidden there stays in the layout
for every other device.

No command changes a layout, so keybinds, macros, pipelines and agents, which run commands, can't
either. Opening Home stays the command `workspace.open_home`; editing it is the app's own screen.

## Older servers

A server that doesn't list `home_layout` in `serverInfo.capabilities` has no layout. A client
shows its default Home there and offers no way to edit it. After a rollback to such a server the
stored layouts stay in the database and come back with the next upgrade
(`operations/upgrade.md`).
