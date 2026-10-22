# Public relay runbook

The public relay is the server anyone's app can use as its relay: accounts, devices, profile
cards, the queue of what other servers send, the request inbox, and the forward path to the
servers that host people's workspaces. It runs the stock image from `deploy/compose.yml` with the
`deploy/compose.relay.yml` override (`deploy/README.md` brings it up), with `RIM_ROLE=relay`, or
`both` to host workspaces too. The project's relay is `relay.rim-platform.org`; read it as "the
relay's domain" wherever this page says it. It runs `both`: it also hosts the RIM community
workspace, the server's default workspace, which every new account joins, and serves the web
client on `chat.rim-platform.org` (`RIM_WEB_DOMAIN`).

Everything marked **FILL** is a decision or a fact the operator records here before the relay
opens to the public.

The commands below assume, on the host, in the repository checkout:

```sh
export COMPOSE_FILE=deploy/compose.yml:deploy/compose.relay.yml
alias dc='sudo -E docker compose'
alias rim-relay='sudo -E docker compose exec rim /app/rim-server'
alias rim-sql='sudo -E docker compose exec -T postgres psql -U rim -d rim'
```

`COMPOSE_FILE` points docker compose and the backup scripts at the relay's two files. The
operator's account is in no group that reaches Docker, so every command runs through `sudo`;
`-E` keeps `COMPOSE_FILE`. The backup and restore scripts call Docker too: run them as
`sudo -E deploy/backup.sh` and so on.

On `relay.rim-platform.org` the files of `deploy/` sit flat in `/srv/rim-server` ("Deploys"),
and its `.env` sets `COMPOSE_FILE=compose.yml:compose.relay.yml`: there, `cd /srv/rim-server`
and read `deploy/backup.sh` as `./backup.sh` and `deploy/.env` as `.env`.

## Run your own server first

This relay is for people who cannot run a server yet. A server of your own, or one your team,
project or community runs, is the better home: you choose its limits, its terms and who may
join, and it does not depend on someone else's budget. `docs/self-hosting.md` sets one up with
the same image and compose file as this relay. An account here can move to such a server later
and keep its account id ("If the relay shuts down" says how).

## A first test run

The public relay is a test run of the project, provided as is: there is no guarantee that it
stays up, and none against data loss. Accounts and what the relay holds may be reset or deleted
at any time, and the relay may end. Its terms (`deploy/terms.md`) say the same to everyone who
signs up. Questions go to the project's GitHub issues
(`https://github.com/rim-platform/rim-server/issues`), which are public; reports about content
or a person go through the app's report function, which reaches a workspace's moderators, or the
relay's instance operators for direct messages and groups.

## If the relay shuts down

When the relay is going to close and there is time, it is announced on the project site and in
`RIM_INSTANCE_NOTICE`, which the app shows where people sign in, and `RIM_LIMIT_ACCOUNTS` is set
at or below the number of accounts there are, which closes signups and says so in the app. A
test run promises no notice period.

People then move to another relay, a server of their own or one someone else runs, with the
desktop app's Settings > Security > "Switch relays" (`docs/relay-client.md` in rim-ui; the
contract is "Switching relays" in `docs/specs/server-to-server-v1.md`). The account id stays the
same, so contacts and workspaces keep knowing them, and nothing they had is lost:

- their keys and their message history are on their devices, not on the relay;
- their workspaces stay on the servers that host them, which deliver to the new relay once the
  switch has delegated each workspace to it;
- what this relay queued for them is drained into the app before the switch leaves it;
- their DMs and groups homed in this relay's direct space get successors on the new relay, the
  history before stays on the devices.

What stays here and goes with the relay: the account row, the contact list (the switch copies
the names the app knows) and DM requests nobody answered yet.

A switch needs this relay to answer: it drains the queue here and ends with `leaveRelay` here.
If the relay stops, or its accounts are reset, before someone has switched, what was queued for
them and not yet fetched is lost, and the conversations homed here take no new messages; their
keys, the history on their devices and their workspaces on other servers stay. Switch while the
relay still answers.

## What lives where

| What | Where | Backed up |
|---|---|---|
| Configuration | `deploy/.env` on the host (secrets), `deploy/compose.yml`, `deploy/compose.relay.yml` and the Caddyfile `RIM_CADDYFILE` names | `.env` in every backup; the compose files and Caddyfiles are in Git |
| Default workspace | `deploy/default-workspace.json`, mounted at `/config/default-workspace.json`; until claimed, the owner claim link in `/data/owner-claim.txt` | the file is in Git; the link is in a backup taken before the claim, so a restart, which revokes it, follows a leaked backup |
| Database | volume `rim-relay_postgres_data` | nightly backup, "Backups" |
| Server key | `/data/server-key.json` in volume `rim-relay_rim_data` | with every dump |
| Server recovery key | offline only, "The server recovery key" | its own copies |
| Franking keys | `/data/franking-key.json` | with every dump |
| Workspace content: channels, their messages, members | the database | nightly backup, "Backups" |
| Uploads (avatars, attachments in channels) | `/data/uploads` in volume `rim-relay_rim_data` | with every dump |
| Audit files | `/data/audit.<YYYY-MM-DD>.log`, one per day, and every audit event on stdout too (`docs/operations/audit-retention.md`) | with every dump |
| Audit exports | `/data/audit-exports/`, command audit rows past `RIM_AUDIT_RETENTION_DAYS`, deleted after `RIM_AUDIT_EXPORT_RETENTION_DAYS` | with every dump |
| TLS certificates | volume `rim-relay_caddy_data` | not needed: Caddy gets new ones |

### How long personal data and logs are kept

The terms tell people that the addresses they connect from appear in the server's logs, which
are kept only as long as needed to run the service and fight abuse. These are the places that
hold addresses or logs, how long each keeps them and what deletes them. Nothing here sends logs
off the host: Alloy sends metrics only.

| What | IP addresses | Kept | Deleted by |
|---|---|---|---|
| Container logs (`dc logs`) | the server's audit events for signups, logins and refusals carry `remote_ip`; the web client's nginx logs each request with the address Caddy forwards; Caddy logs the request, address included, of a request it could not serve | about 100 MB per service (five files of 20 MB), and never longer than the container | Docker, when the newest file fills (the `logging` options in the compose files); a deploy that replaces a container's image starts its log afresh |
| Audit files, `/data/audit.<YYYY-MM-DD>.log` | the same audit events as on stdout | 90 days (`RIM_AUDIT_LOG_RETENTION_DAYS`, default 90) | the server, daily |
| Command audit rows, the `command_audit_log` table | none | 90 days (`RIM_AUDIT_RETENTION_DAYS`, 90 in `deploy/compose.relay.yml`) | the server, daily, after it exported them to `/data/audit-exports/` |
| Audit exports, `/data/audit-exports/` | none: user and workspace ids, redacted arguments | 90 days after the export (`RIM_AUDIT_EXPORT_RETENTION_DAYS`, 90 in `deploy/compose.relay.yml`), so a command audit row is gone completely after about 180 days | the server, daily |
| Sessions, the `session` table | none: its `ip` column is never filled | until revoked (sign-out, a revoked device, a block) or expired, 30 days after sign-in (`RIM_SESSION_TTL_DAYS`); use does not extend it | the server, within an hour |
| Rate limits and the known sign-in addresses | the addresses themselves, in memory | minutes for the rate windows; a known sign-in address counts for 30 days; none outlives a restart | the server; every restart clears them |
| IP blocks, the `ip_block` table | the blocked ranges | until the block's expiry, or until `rim-relay ip unblock` for one without | the server, within 30 seconds of the expiry |
| Caddy access log | every request's address | off; switched on only during an incident ("Monitoring"), and then part of Caddy's container log above | the operator, by removing `log` again |
| Backups | those in the audit files of the backed-up `/data` | 30 days (`RIM_BACKUP_KEEP_DAYS=30` in `/srv/rim-server/.env`) | `deploy/backup.sh`, after each backup that succeeded; there is no other copy |
| The host's own logs (journald: SSH, fail2ban, the firewall) | addresses that connect to the host itself | at most 500 MB and 30 days (`SystemMaxUse=500M`, `MaxRetentionSec=30day` in `/etc/systemd/journald.conf.d/rim.conf`) | journald |

## Keys

### The server recovery key

The recovery key replaces a lost or stolen server key. Whoever holds it can make any key this
server's, so it never stays on the host: the first start writes it to
`/data/server-recovery-key.txt` and warns at every start until it is gone (`deploy/README.md`,
"Start").

| | |
|---|---|
| Holders | the maintainer (@dvorakj31) alone for now; a second holder, so one absence never blocks a recovery, is to be named |
| Copy 1 | **FILL**: e.g. a password manager vault, its name and who can open it |
| Copy 2 | **FILL**: e.g. a printed copy in a safe, its location |
| Last checked that both copies read back | **FILL**: date; check every quarter with the restore drill |

The file is one line, `rim-server-recovery:1:<base64url>`. Check a copy without using it: the
public half it names is `recovery key` in `rim-relay server-key show`.

### Yearly rotation

Rotate the server key once a year, in October, and when someone with host access leaves.

```sh
rim-relay server-key rotate
rim-relay server-key show          # key seq went up by one
```

The running server switches within a minute. Peers accept the previous key for 24 hours after
they see the rotation, so nothing in flight fails. Take a backup right after: an older backup
of `/data` with a newer database (or the reverse) does not start, "Restore".

### Compromise: recover

When the server key may be stolen (host broken into, `/data` or a backup of it leaked), recover
with the recovery key. The new key is signed by the recovery key, and peers stop accepting the
old one as soon as they see it.

```sh
dc exec -T rim sh -c 'umask 077; cat > /tmp/recovery.txt' < ./recovery.txt   # in memory, briefly
rim-relay server-key recover --recovery-key-file /tmp/recovery.txt --new-recovery-key
dc exec rim rm /tmp/recovery.txt
dc exec rim cat /data/server-recovery-key.txt     # the new recovery key: store it as above
dc exec rim rm /data/server-recovery-key.txt
rim-relay franking-key rotate                     # /data leaked with the key
```

Replace the old recovery key's copies with the new one. If the recovery key itself may be
known to someone else, `--new-recovery-key` is what takes it away; do it before they do.

### The franking key file

`/data/franking-key.json` signs the receipts that let users report encrypted messages. Rotation
adds a key and keeps every older one, since old receipts must still verify:

```sh
rim-relay franking-key show
rim-relay franking-key rotate
```

Rotate when `/data` may have leaked. Losing the file loses the ability to verify reports of
messages sent before: back it up with everything else in `/data`.

## The default workspace

With `RIM_DEFAULT_WORKSPACE_FILE=/config/default-workspace.json` in `.env` (`deploy/README.md`, "A
relay that hosts a workspace"), `deploy/default-workspace.json` is the server's default
workspace: every new account joins it as a Member with its public channels. Its owner claims it once, right after the
first start:

```sh
dc exec rim cat /data/owner-claim.txt
```

Open the link and sign up with a key. The signup creates the workspace with the file's name,
channels, topics and DM policy, makes the account its Owner and, with `claimGrantsOperator`, an
instance operator, and deletes the file. A lost or leaked link before the claim is a restart
(`dc restart rim`): each start revokes the previous link and writes a new one. After the claim,
add the owner's name to `RIM_RESERVED_NAMES`.

Its DM policy takes a fellow member's first DM or group as a request (`"memberDefault":
"REQUESTS"`): every account on the relay is in this workspace, so a stranger's first message
waits in the recipient's request inbox, under its limits, until they accept it. A member who
wants DMs from everyone here directly sets that for themself in the app; the Owner and
Moderators' DMs arrive directly.

The file changes only through a release: each start applies it to the claimed workspace (new
channels, topics, the name, the DM policy) and deletes nothing. A channel is renamed in the app
first and then in the file. Ownership moves in the app.

## Backups

`deploy/backup.sh` writes one archive of the database dump, the `/data` volume and `deploy/.env`;
`docs/self-hosting.md`, "Backups", says what is in it. It never includes the recovery key file.

Nightly at 03:15 host time from root's cron (`/etc/cron.d/rim-backup`, below), before every
update and right after every key rotation; encrypted with `age` to the public key of the
maintainer's backup identity (`RIM_BACKUP_AGE_RECIPIENT`), whose private half is held by the
maintainer and never on the host; written off the VPS disk to the provider's NAS share mounted
at `/mnt/backups` (`RIM_BACKUP_DIR=/mnt/backups/rim-backups`); kept for 30 days, with
`RIM_BACKUP_KEEP_DAYS=30` in `/srv/rim-server/.env` so each backup deletes the older ones. The
share is with the same provider as the VPS; a copy elsewhere is not set up yet.

```sh
RIM_BACKUP_AGE_RECIPIENT=age1… deploy/backup.sh       # or set it in deploy/.env
# with RIM_BACKUP_DIR on the NAS share, the archive is already off the VPS disk
```

Cron, `/etc/cron.d/rim-backup` on the host, with `COMPOSE_FILE`, `RIM_BACKUP_DIR`,
`RIM_BACKUP_AGE_RECIPIENT` and `RIM_BACKUP_KEEP_DAYS` in `/srv/rim-server/.env`:

```cron
15 3 * * * root cd /srv/rim-server && ./backup.sh >>/var/log/rim-backup.log 2>&1
```

The dump and the data archive are one set: the key file must match the head of the key log in
the database. The scripts restore both from one archive; never mix sets.

### Restore

On the new host, with the repository checked out, `deploy/.env` taken from the archive's `env`
file, and the image pulled (`dc pull rim`):

```sh
dc stop rim                                            # restore.sh refuses while it runs
RIM_BACKUP_AGE_IDENTITY=/path/to/identity deploy/restore.sh rim-backup-rim-relay-<time>.tar.age
rim-relay server-key show
```

It drops and recreates the database from the dump, empties `/data` and refills it, starts the
stack and waits for `/readyz`. The server refuses to start when the key file and the log head
disagree: that is a mixed set, or a rotation after the backup. Use a matching set, or
`server-key recover` if the key file of the newest log is gone.

### Restore drill

Every quarter, in January, April, July and October (record each drill's date and result
below), restore the latest set:

```sh
RIM_BACKUP_AGE_IDENTITY=/path/to/identity deploy/restore-drill.sh rim-backup-rim-relay-<time>.tar.age
```

The drill runs in its own project, `rim-drill`, with server-to-server traffic and mail off and no
port published, so the copy never speaks for the relay; it removes the project and its volumes
when it ends. Check its output:

- `server-key show` names the production server id and key seq;
- the row counts (`user`, `session`, `device_key`, `relay_item`, …) are close to production's;
- the recovery key copies read back (see above).

For `abuse-reports list` and `principal list` against the copy, keep it running with
`RIM_DRILL_KEEP=1`, run `docker compose -p rim-drill exec rim /app/rim-server principal list`,
and remove it with `docker compose -p rim-drill down -v`: two running servers with one key are
one server to every peer.

| Date | Set restored | Result | By |
|---|---|---|---|
| **FILL** | | | |

## Deploys

The relay runs published images only, from `/srv/rim-server` on the host, and
`rim-deploy.timer` installs every rim-server release there within five minutes of its
publication (`deploy/README.md`, "Unattended upgrades from releases"). Approving the release in
the `release` environment is the deployment: approve it in a quiet hour, after doing what the
release's **Upgrade:** lines say in `/srv/rim-server/.env`. Tag rim-ui first: until `rim-web` of
the version is published, each check waits and tries again.

- **The log**: `journalctl -u rim-deploy`, with `-f` during a deploy: the file copy, the
  signature check, the backup, the pull, the migrations, the start, `/readyz` and the version.
  `systemctl list-timers rim-deploy.timer` shows the next check.
- **Publish without deploying**: mark the release a **pre-release** on GitHub.
- **Stop all deploys**: `sudo touch /srv/rim-server/HOLD`; `sudo rm /srv/rim-server/HOLD` lets
  the timer deploy again.
- **A failed deploy** rolls back by itself and writes the version to `/srv/rim-server/FAILED`;
  the timer skips that version until the file is removed, and deploys the next release.
- **Roll back by hand**: `sudo touch /srv/rim-server/HOLD`, then
  `sudo rim-deploy <previous version>` (or mark the bad release a pre-release instead of the
  `HOLD`). Remove `HOLD` once the fix is released. Anything further back than the release before
  is a restore from the backup the deploy took ("Restore").
- **When the GitHub API is down**: `sudo rim-deploy <version>` deploys without it.

Files in `/srv/rim-server` other than `.env` come from the release tag; an edit on the host is
overwritten by the next deploy, so a change to them is a rim-server pull request.

## Quotas

Per local user, set in `deploy/.env`; the compose file has the public relay's values.

| Variable | Public relay | When it is reached |
|---|---|---|
| `RIM_RELAY_QUEUE_BYTES_PER_USER` | 67108864 (64 MiB) | The user's oldest queued items are dropped; their apps online get `QueueOverflow` |
| `RIM_RELAY_CARD_MAX_BYTES` | 131072 | `publishProfileCard` fails `VALIDATION` (128 KiB is also the most a card can be) |
| `RIM_RELAY_KEY_PACKAGES_PER_DEVICE` | 100 | `uploadMlsKeyPackages` fails `VALIDATION` past 100 unclaimed per device and suite |
| `RIM_RELAY_DEVICES_PER_USER` | 32 | `addDeviceKey` fails `VALIDATION`; revoked devices do not count |
| `RIM_S2S_DEVICE_RATE_PER_MIN` | 120 | `/api/remote/…` answers `429` per device per minute (as a host, this server accepts as many forwarded requests per device) |
| `RIM_RELAY_FORWARDS_PER_USER_PER_DAY` | 50000 | `/api/remote/…` answers `429` |
| `RIM_RELAY_DM_REQUESTS_PER_DAY` | 20 | A sender principal's further DM invites to people here are refused (`limited`); a local user's further invites to people on other servers they do not know (a contact who lists them too, or someone either accepted a DM request from) fail `RATE_LIMITED` |
| `RIM_RELAY_RETENTION_DAYS` | 30 | Queued items and DM requests are deleted |
| `RIM_REGISTRATION_POW_BITS` | 24 | Signups without a registration token need a proof-of-work of that many leading zero bits (about 2^24 hashes); the open-signup limit of 20 per IP per 10 minutes applies too |

### The account cap

`RIM_LIMIT_ACCOUNTS` in `deploy/.env`. The public relay sets none for now and takes anyone who
does the proof-of-work; the operator sets a cap at any time with `dc up -d`.

It counts the people who signed up here, those waiting out their deletion grace period included,
since they can still cancel; it does not count the `r_` rows of people from other servers or the
tombstones of purged accounts, so each purge frees a place. At the cap every signup is refused
with `REGISTRATION_FULL`, "this server is full and takes no new accounts; run your own server or
sign up on another one": a proof-of-work, a signup link from `invite create` and a person from
another server signing up here with their key alike. The cap is the most the operator chose to
carry, so an invite does not get past it; raise the cap to let someone in. The app reads
`serverInfo.registrationFull` and the discovery document says `"registration":{"full":true}`, so
people see it before they do the work. Signups count one at a time under a database lock, so
two at once never pass the cap. Each refusal is audited as `signup_refused_registration_full`.

```sh
rim-sql -c "SELECT count(*) FROM \"user\" WHERE kind = 'local' AND deleted_at IS NULL"
```

To change it, edit `deploy/.env` and run `dc up -d`. A cap below the accounts there are refuses
new signups only; nobody loses an account.

The public relay keeps the DM defaults, `RIM_DM_INBOUND=requests` and
`RIM_DM_OUTBOUND=allowed`: a stranger's first message is a request, from anywhere ("Who May Start
A DM" in `docs/messaging.md`).

The public relay serves `deploy/terms.md` as its terms of use (`RIM_TERMS_FILE=/config/terms.md`),
publishes where to reach its operators
(`RIM_ABUSE_CONTACT=https://github.com/rim-platform/rim-server/issues`, public; reports about
content or a person go through the app's report function), both set in `deploy/.env`. It
asks for no minimum age: `RIM_MINIMUM_AGE` stays unset. After editing `deploy/terms.md`,
`dc restart rim` serves the new text; its URL changes with it, so new signups accept the new
version.

To change one, edit `deploy/.env` and run `dc up -d`: the server restarts in seconds and apps
reconnect. The per-minute and per-day counters are in memory, so a restart starts them afresh.
Each extra bit of proof-of-work doubles a signup's cost; the descriptor tells apps the new value
at once.

To let people in without the proof-of-work, while the relay is under its account cap, give them
a signup link:
`rim-relay invite create --max-uses <1-50> --expires-in-days <1-30> --public-url https://<domain>`
prints it on its first line (the token is shown once); `rim-relay invite list` shows each live
link's use count and `rim-relay invite revoke <id>` ends one. For one person,
`rim-relay invite create --email <their address> --public-url https://<domain>` gives a
single-use link bound to that address: the account it signs up gets that address. The binding
does not keep others out, since a key signup gives no address: whoever holds the link signs up
with it.

## Monitoring

Logs are JSON on stdout (`dc logs rim`); audit events (`"target":"audit"`) also go to the
daily files `/data/audit.<YYYY-MM-DD>.log`, kept 90 days. The database answers what logs do not.
Alert thresholds are **FILL** until a month of normal traffic shows the baseline.

| Watch | How | Alert at |
|---|---|---|
| Queue bytes | `rim-sql -c 'SELECT sum(size_bytes) AS bytes, count(DISTINCT user_id) AS users FROM relay_item'`; per user: `… GROUP BY user_id ORDER BY 2 DESC LIMIT 10`; log `relay queue over its cap; dropped the oldest items` | **FILL** |
| Outbox depth per peer | `rim_outbox_peer_max_pending_rows`, `rim_outbox_dead_peers`, `rim_outbox_not_queued_rows_total`; per peer the query below; log `host events for a relay are not queued` | **FILL** rows or oldest over **FILL** minutes |
| Deliver failures | logs `delivery to a relay failed`, `a relay refused a delivery`, `outbox delivery to a relay failed`; `s2s_peer_delivery.failures` | **FILL** |
| `401` from peers | log `refused a server-to-server signature` (with `reason` and `peer_server_id`) | **FILL** per hour |
| `403` to peers | log `refused a peer that is not allowed` | any from a peer not known to be blocked |
| `409` replays | log `refused a replayed server-to-server request` | **FILL** per hour |
| `429` | logs `rate-limited a peer's server-to-server requests`, `a user's forwarded requests are over their quota`, `a user's invites to other servers are over their daily quota`; audit `signup_rate_limited` | **FILL** |
| Proof-of-work signups | audit `relay.registration_pow` per hour; log `refused a signup's proof-of-work` | **FILL** above the weekly hourly mean |
| Blocked principals knocking | audit `relay.blocked_principal_refused` and `blocked_email_refused` | **FILL** |
| Open abuse reports | `rim-relay abuse-reports list --open`; reports of DMs and groups on the app's instance moderation page | any older than the response time below |
| Accounts against the cap | the query in "The account cap"; audit `signup_refused_registration_full` | 90% of `RIM_LIMIT_ACCOUNTS`, once one is set |

```sh
dc logs --since 1h rim | grep -c '"event":"relay.registration_pow"'
dc logs --since 1h rim | grep 'refused a server-to-server signature' | grep -o '"reason":"[a-z_]*"' | sort | uniq -c
rim-sql <<'SQL'
SELECT o.peer_server_id, count(*) AS rows, min(o.created_at) AS oldest,
       sum(o.size_bytes) AS bytes, d.failures, d.failing_since, d.next_attempt_at,
       d.last_success_at
FROM s2s_outbox o LEFT JOIN s2s_peer_delivery d USING (peer_server_id)
WHERE o.id > COALESCE(d.delivered_through, 0)
GROUP BY o.peer_server_id, d.failures, d.failing_since, d.next_attempt_at, d.last_success_at
ORDER BY rows DESC;
SQL
```

Per-status counts of every client request come from the proxy. Caddy keeps no access log by
default, since it would hold users' IP addresses; during an incident add `log` to the site block
of the Caddyfile `RIM_CADDYFILE` names, `dc restart caddy`, and remove it afterwards.

## Incidents

Record each incident: start, what was seen, what was done, end, in an issue in
`rim-platform/rim-server` labelled `incident`, with no personal data in it (principal and server
ids are fine; names, addresses and message content are not).

### A spam wave

Signs: `relay.registration_pow` far above normal, many reports, DM requests from few senders.

1. Find the senders:
   `rim-sql -c "SELECT from_principal_id, count(*) FROM relay_dm_request WHERE created_at > now() - interval '1 hour' GROUP BY 1 ORDER BY 2 DESC LIMIT 20"`
   and `rim-relay abuse-reports list --open`, and the reports of DMs and groups in the app.
2. Look at each: `rim-relay principal show <principalId>` prints the account, who invited it,
   the accounts its invites created and the reports naming it.
3. Block each: `rim-relay principal block <principalId> --reason "spam wave <date>"`. Their
   sessions end, their invites that have uses left are revoked, their email can neither sign
   up nor sign in, their pending requests are ignored for everyone here, and their signups,
   logins and deliveries are refused from then on. The command prints the accounts the
   blocked one's invites created (username, principal id, created at).
4. When those accounts are part of the wave, block them all at once:
   `rim-relay principal block <principalId> --reason "spam wave <date>" --cascade` blocks the
   account and every account its invites created, recursively, in one transaction, each with
   the reason followed by "(invited by <username>)". When the wave came in through an invite,
   cascade from the account that gave it out. Accounts without a key identity (`no key
   identity` in the output) are reached only this way. Resolve the reports that named them.
5. Make the next account dearer: raise `RIM_REGISTRATION_POW_BITS` by 2 (four times the work)
   and, if the spam goes to people here, lower `RIM_RELAY_DM_REQUESTS_PER_DAY`; `dc up -d`.
6. When the senders are on another server, block that server (next section) and write to its
   operator (its discovery document names its origin: `rim-sql -c "SELECT origin FROM peer_server WHERE server_id = '<id>'"`).
7. Lower the settings again a day after the wave ends.

`rim-relay principal unblock <principalId>` lifts a block and its email block after a review.
It does not bring back the sessions or the invites the block ended, and it lifts one account:
unblock each account a cascade blocked.

### A hostile peer

Signs: one `peer_server_id` in many signature refusals, replays or rate limits; deliveries
refused as `invalid_envelope`; `rim_rate_limit_rejections_total{limiter="relay_peer_bytes"}`
climbing, one server past the 2 GiB of items a day this relay queues from a host; reports naming
one server.

1. `rim-relay peer block <serverId>`: refused both ways at once, its requests get
   `403 peer_not_allowed`, nothing is sent to it. Audited as `s2s.peer_blocked`.
2. A server never pinned here cannot be blocked that way (the command says so): add it to
   `RIM_S2S_DENIED_PEERS` in `deploy/.env` and `dc up -d`.
3. `rim-relay peer unblock <serverId>` when it is resolved. A peer refused as forked is not
   unblocked: check with its operator out of band, then `rim-relay peer reset <serverId>`.

### Traffic from one address

Signs: `rim_rate_limit_rejections_total{limiter="api_ip"}` climbing, one address over and over
in Caddy's access log, signup or login refusals from one range.

1. `rim-relay ip block <address or range> --reason "<what> <date>" --expires-in-days 7`. Every
   route but `/healthz` answers it 403 within 30 seconds; audited as `ip.block`.
2. A block is per address, so a client with many addresses comes back: block its range (a /24
   or a /48) and set `RIM_RATE_REQUESTS_PER_MIN_PER_IP` if it is not set yet.
3. `rim-relay ip list` shows what is blocked until when; `rim-relay ip unblock <range>` lifts
   one early. An expired block is ignored at once and deleted within 30 seconds.

### A stolen server key

Signs: a peer's operator reports requests signed by this server that it did not send; the host
or a backup of `/data` was exposed.

1. "Compromise: recover", including `--new-recovery-key` and the franking key rotation.
2. Rotate `POSTGRES_PASSWORD` (`ALTER USER rim PASSWORD …` in `rim-sql`, then `deploy/.env`,
   then `dc up -d`).
3. Find how it left: host logins, who had the backups. Rebuild the host if it was broken into.
4. Tell users what the thief could do until the recovery (sign as this server towards peers and
   receive what peers delivered for its users; never sign as a user or read their DMs) and when
   it stopped: in the instance notice (`RIM_INSTANCE_NOTICE`, shown on the sign-in page), in
   `#announcements` in the RIM workspace and in the incident's issue.

### A database leak

What a dump holds: accounts and who invited them, key logs, devices and cards (public anyway),
session token hashes, password hashes of the few password accounts, contacts, DM requests and
their senders' cards, queued host events (plaintext channel events are readable, DMs are
MLS-encrypted), abuse reports, blocked emails. No private key: the server and franking keys are
files in `/data`.

1. End every session: `rim-sql -c 'UPDATE session SET revoked_at = now() WHERE revoked_at IS NULL'`.
2. Rotate `POSTGRES_PASSWORD` as above, and find and close the way out.
3. If `/data` or a backup set went too: "A stolen server key".
4. Notify users as in "A stolen server key", and the authority where the law asks for it: the
   maintainer decides; under the GDPR a breach that risks people's rights goes to the Czech
   Office for Personal Data Protection (ÚOOÚ, <https://uoou.gov.cz>) within 72 hours of
   learning of it, and to the people affected without delay when the risk is high.

## Abuse policy

The text below is published with this page on the project site; the terms
(`deploy/terms.md`) carry the rules and the contact. Filled in:

> **Abuse on <relay name>**
>
> This relay carries messages for anyone who signs up. We block an account (its key identity)
> on this relay when it sends spam or unsolicited bulk requests, harasses or threatens people,
> impersonates someone, spreads malware or phishing, tries to break or overload the service, or
> uses it for anything illegal under the law of the Czech Republic or of the European Union. We block
> another server when it sends such traffic and its operator does not stop it, or when it
> attacks this relay.
>
> To report: use "Report" in the app, or open an issue at
> <https://github.com/rim-platform/rim-server/issues> with the principal id or server id, what
> happened and when. Issues are public: put no personal details in one. We cannot read
> encrypted messages; a report from the app carries what the sender's own request showed.
>
> The workspaces this relay hosts, the RIM community workspace among them, are moderated from
> the app: by each workspace's owners and moderators, who get the reports of its messages, and by
> the instance operators, who get the reports of direct messages and groups and block accounts
> and servers on the whole relay.
>
> This relay is a first test run, looked after by one person. We read reports as time allows,
> usually within a few days, and block clear spam or attacks as soon as we see them. We keep
> reports for one year and the blocked identity until it is unblocked. A blocked person may
> ask for a review the same way; we answer as time allows.

### Where the reports are

A report ends up in one of three places. The first two are the operators', on the app's
instance moderation page for an account made an instance operator (`operator grant`, "Command
reference"):

- **DM requests** a user reported (`reportDmRequest`), in `relay_abuse_report` with the
  reports from the abuse address. `abuse-reports list` shows them too, 100 a page (`--limit` up
  to 500, `--after <report id>` for the next page). A user reports one request once.
- **Messages in DMs and groups.** These live in the relay's direct space, which has no owners
  or moderators, so their reports (`message_report`) go to the instance operators only. A
  report of an encrypted message carries the message exactly as the sender's device sent it,
  verified by the server; opening it is audited (`e2ee.report_viewed`, ids only). The operator
  may delete the reported message and resolves the report; 90 days after the resolution the
  verified content is deleted. Until it is resolved the content is kept, so resolve every
  report once it is handled. There is no command for these: `rim-sql -c "SELECT id, created_at,
  status FROM message_report WHERE server_id = (SELECT id FROM server WHERE kind = 'direct')
  AND status = 'open'"` lists the open ones without the app.
- **Messages in a workspace** go to that workspace's owners and moderators, not to the
  operators.

Reports from the abuse address go into the same list as the DM requests:

```sh
rim-relay abuse-reports add --principal rimp1_… --reason "mail of <date>: phishing links"
rim-relay abuse-reports add --server rims1_… --reason "mail of <date>: spam from its users"
rim-relay abuse-reports list --open
rim-relay abuse-reports resolve <report id>
```

## Command reference

```sh
rim-relay server-key show | rotate | recover --recovery-key-file F [--new-recovery-key]
rim-relay franking-key show | rotate
rim-relay principal block <principalId> --reason <text> [--cascade] | unblock <principalId> | show <principalId> | list
rim-relay peer block <serverId> | unblock <serverId> | reset <serverId>
rim-relay abuse-reports list [--open] [--after <id>] [--limit N] | add [--principal P] [--server S] --reason <text> | resolve <id>
rim-relay invite create [--email A] [--max-uses N] [--expires-in-days D] [--public-url https://<domain>] | list | revoke <id>
rim-relay audit purge [--dry-run] [--older-than-days N]
rim-relay ip block <cidr> --reason <text> [--expires-in-days D] | unblock <cidr> | list
rim-relay operator grant <username> | revoke <username> | list
```

`operator grant` makes an account an instance operator: it reads the abuse reports, principal
blocks and IP blocks in the app and changes them as the `abuse-reports`, `principal` and `ip`
commands do, and handles the reports of messages in DMs and groups (`docs/api.md`, "Instance
moderation"). A blocked account or one scheduled for deletion cannot be granted the role;
`operator revoke` takes it back and `operator list` prints the operators.

Every one is audited (`target` `audit`): `relay.principal_blocked` (`scope` `server`, one per
account a block reaches, `cascade` true for those after the first),
`relay.principal_unblocked`, `s2s.peer_blocked`, `s2s.peer_unblocked`, `s2s.peer_reset`,
`relay.abuse_report_filed`, `relay.abuse_report_resolved`, `server_key.rotated`,
`server_key.recovered`, `instance_invite_issued`,
`instance_invite_revoked` (`issued_by` or `actor` `operator`), `ip.block` and `ip.unblock`
(`scope` `server`, with the `cidr`), `operator.granted` and `operator.revoked` (`scope`
`server`, with the `user_id`). The principal, report and IP lines carry `actor_id`: `cli` for
the command line, the operator's user id for a change made in the app. `audit purge` writes
`audit.retention.purged` (`trigger` `cli`), the same event the daily purge writes; the purge, its
export files and the retention of the audit files are in `docs/operations/audit-retention.md`.
