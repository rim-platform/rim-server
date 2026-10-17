# A closed company server

A company that runs a RIM server for its own people can close it to the rest of the network for
direct messages, or open it to named partner servers only. Inside, everyone shares the company's
workspace and talks directly. Outside, nobody can start a conversation with the company's people,
and a leaked list of their names or principal ids reaches nobody: other servers get no identity,
no device and no key package of anyone here. "Who May Start A DM" in `docs/messaging.md` is the
full rule; this page is the recipe.

## The settings

In `deploy/.env`, then `dc up -d`:

```sh
# Nobody outside a shared workspace may start a DM with people here, and other servers read no
# identity of theirs. With partners, only people on the listed servers may, as a request.
RIM_DM_INBOUND=none
# People here start no DM with anyone on another server. With partners, only with the listed
# servers' people.
RIM_DM_OUTBOUND=none
# With partners in either setting: the partner servers' ids, comma-separated.
# RIM_DM_PARTNERS=rims1_...,rims1_...
```

Leave `RIM_OPEN_REGISTRATION` and `RIM_REGISTRATION_POW_BITS` unset: signups are then by invite
only, and the operator invites each person (`rim-server invite create`, "The first account" in
`docs/self-hosting.md`). The company's workspace keeps its default for members, `DIRECT`, so
colleagues reach each other at once; its Owner can still set `REQUESTS` for a large workspace
(`setWorkspaceDmPolicy`).

Name the company's workspace as the server's default workspace, so every person the operator
invites joins it, with its public channels, the moment their account exists ("A default
workspace" in `docs/self-hosting.md`):

```sh
# The id of the company's workspace on this server.
RIM_DEFAULT_WORKSPACE=0199...
```

A partner's server id is what `rim-server server-key show` prints on that server as its server
id, or the `serverId` of its `/.well-known/rim-server`. An id that is not one (`rims1_` and 43 characters)
stops the server at start, as do values these settings do not take.

## What people see

- `serverInfo.dmInbound`, `dmOutbound` and `dmPartners` show the settings; `myDmSettings` reports
  `effectiveOutside: NONE` with `outsideFloorFrom: { kind: OPERATOR }` under `none`, so the app
  greys the choice and says why.
- Starting a DM with someone on another server answers `FORBIDDEN` ("this server starts no direct
  messages with that server"); with `partners` it works for the partners' people only.
- Someone on another server who tries to start a DM with a person here learns nothing: their
  server gets `404` for the person's identity, as for a name that does not exist. With `partners`,
  a partner's people reach the person's request inbox.
- A person here who shares no workspace with another person here cannot start a DM with them
  under `none` (`FORBIDDEN`): the company's workspace is where its people meet.
- A person who joined a workspace hosted on another server keeps working in it: that server is
  answered for the person's identity and key packages, since the person holds a subscription
  there.

## Opening up later

Changing a setting takes a restart (`dc up -d`). Conversations decided before keep their
decision: a DM accepted before `none` was set stays accepted. Under `RIM_DM_OUTBOUND=none` the
server also sends no event of its direct space to another server, so a DM with a former outside contact goes quiet
until outbound DMs are allowed again.
