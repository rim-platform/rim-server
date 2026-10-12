# Self-hosting a RIM server

`deploy/compose.yml` runs one RIM server on one domain: rim-server with `RIM_ROLE=both` (it
hosts workspaces and is its users' relay), PostgreSQL 17, the web client, and Caddy terminating
TLS. Caddy sends `/api` (with `/api/ws`), `/.well-known/…` and `/s2s/…` to rim-server and every
other path to the web client, so the client and the API share one origin. The health probes
stay inside the compose network.
A public relay is the same file with `deploy/compose.relay.yml` on top; `deploy/README.md`
covers it.

Everything marked **FILL** is the operator's choice, recorded here once made.

## Before the first start

1. A Linux host with Docker (Compose v2.20 or later), ports 80 and 443 reachable from the
   internet, and DNS `A`/`AAAA` records of your domain pointing at it. Caddy gets the
   certificate from Let's Encrypt on the first request.
2. This repository at the release you run. The compose files, the Caddyfile and the backup
   scripts come from the checkout; the images are pulled:

   ```sh
   git clone https://github.com/rim-platform/rim-server.git
   cd rim-server
   git checkout v<version>
   ```

   The compose files run `ghcr.io/rim-platform/rim-server` and `ghcr.io/rim-platform/rim-web`
   at `RIM_VERSION` from `deploy/.env`; set it to the same `<version>`. Left unset it is
   `latest`, which moves with every release. To build both images on the host instead, check
   out rim-ui next to this repository and add `-f deploy/compose.dev.yml` to every compose
   command (the alias below) with `up --build`.
3. `deploy/.env` from the example, filled in:

   ```sh
   cp deploy/.env.example deploy/.env
   chmod 600 deploy/.env
   sed -i "s|^POSTGRES_PASSWORD=.*|POSTGRES_PASSWORD=$(openssl rand -base64 32 | tr -d '/+=')|" deploy/.env
   $EDITOR deploy/.env          # RIM_VERSION, RIM_DOMAIN, and SMTP for password resets and invites
   ```

   Every variable the compose files read is in `.env.example` with what it does. Without
   `POSTGRES_PASSWORD` or `RIM_DOMAIN` every compose command stops and names the variable.
4. Check the configuration:

   ```sh
   alias dc='docker compose -f deploy/compose.yml'
   dc config >/dev/null && echo ok
   ```

The commands below use that alias, from the repository root.

### Verify the images

The release workflows sign every image with [cosign](https://docs.sigstore.dev) keyless. The
certificate names the repository's release workflow and the tag it ran for, so an image built or
signed anywhere else does not verify. Before the first start and after every upgrade:

```sh
v=0.1.0                                      # your RIM_VERSION
dc pull
cosign verify ghcr.io/rim-platform/rim-server:$v \
  --certificate-identity https://github.com/rim-platform/rim-server/.github/workflows/release.yaml@refs/tags/v$v \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com
cosign verify ghcr.io/rim-platform/rim-web:$v \
  --certificate-identity https://github.com/rim-platform/rim-ui/.github/workflows/release.yaml@refs/tags/v$v \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com
```

cosign prints the digest it verified, which is also in the GitHub release notes. Setting
`RIM_SERVER_IMAGE` and `RIM_WEB_IMAGE` in `deploy/.env` to `…@sha256:<digest>` makes compose run
exactly those images. Each release also carries the image's SBOM (SPDX JSON), attested to the
image: `cosign verify-attestation --type spdxjson` with the same flags checks it.

### The web client's endpoint

The web client finds the API through `VITE_GRAPHQL_ENDPOINT`, baked in when its image is built.
The published image is built with `/api`, and `deploy/compose.dev.yml` passes
`RIM_WEB_GRAPHQL_ENDPOINT`, default `/api`: the client resolves it against the page's own
address, so one image serves any domain and needs no CORS. The image's
content security policy allows `connect-src 'self'`, which covers the API and its WebSocket on
that origin; the browser console notes that it ignores the extra `/api` source the image adds.
`https://<your domain>/api` gives the same result without that note and ties the image to the
domain.

Browsers send an `Origin` header with the client's requests even on one origin. The server
accepts its own origin without configuration: that of `RIM_PUBLIC_BASE_URL` when set, else the
scheme and `Host` the request arrived with (`X-Forwarded-Proto` and `X-Forwarded-Host` count
only with `RIM_TRUST_FORWARDED_HEADERS=true`). Any other origin gets `400 Origin is not
allowed`, with two exceptions: the packaged desktop app's origins (`tauri://localhost` on macOS
and Linux, `http://tauri.localhost` on Windows), which no web page can carry, and one extra
origin named by `RIM_CORS_ALLOWED_ORIGIN`, such as a client served from another domain or the
desktop development app at `http://localhost:1420`. The compose file leaves it as a commented
example.

## First start

```sh
dc up -d
dc logs -f rim
```

The `migrate` service applies the migrations and exits; the server starts after it. `dc ps`
shows `rim` as `healthy` once `/readyz` passes.

### The server recovery key

On its first start the server creates its key and writes the recovery key to
`/data/server-recovery-key.txt`, and it logs a warning at every start until the file is gone.
The recovery key replaces a lost or stolen server key; whoever holds it can make any key this
server's, so it never stays on the host:

```sh
dc exec rim cat /data/server-recovery-key.txt
# store the line offline: a password manager and a printed copy, held by two people
dc exec rim rm /data/server-recovery-key.txt
```

`dc exec rim /app/rim-server server-key show` names the recovery key's public half, so a copy can
be checked without using it. Backups never include the file (below). `docs/server-identity.md`
has the key commands, rotation and recovery.

### The first account

Signups are invite-only by default. Issue a single-use link for your own account; it works for
7 days (`--expires-in-days` changes that):

```sh
dc exec rim /app/rim-server invite create --public-url https://rim.example.org
```

`--email you@example.org` binds the link to that address: it then admits only a password signup
with it, not a key-only one.

Open the link it prints (`https://rim.example.org/join/…`) and sign up. For other people, a link
for up to 50 signups that lasts up to 30 days:

```sh
dc exec rim /app/rim-server invite create --max-uses 10 --expires-in-days 7 --public-url https://rim.example.org
dc exec rim /app/rim-server invite list
dc exec rim /app/rim-server invite revoke <id>
```

The owner of a workspace can also invite from the app. `RIM_OPEN_REGISTRATION=true` lets
anyone sign up, rate-limited per address; `RIM_REGISTRATION_POW_BITS` asks a proof-of-work of
signups without an invite.

### Check it answers

```sh
dc exec rim curl -fsS http://localhost:8080/readyz; echo
curl -s https://rim.example.org/.well-known/rim-server | head -c 300; echo
curl -sI https://rim.example.org/ | head -n 1                 # the web client
dc exec rim /app/rim-server --version
```

## Updates

Take a backup first (below): migrations only go forward, so a backup is the way back.

```sh
deploy/backup.sh
git fetch --tags && git checkout v<version>
$EDITOR deploy/.env                          # RIM_VERSION=<version>
dc pull                                      # then verify them, as above
dc up -d
dc logs -f rim
```

`up` runs `migrate` before it starts the new server. To run the migrations alone,
`dc run --rm migrate`. Rolling the image back leaves the migrations in place: the previous
release runs on them, started with `dc up -d --no-deps rim web`, and anything further back is the
backup taken before the update. `docs/operations/upgrade.md` has the whole procedure, what to
read in `CHANGELOG.md` first, and the rollback.

## Backups

`deploy/backup.sh` writes one archive, `rim-backup-<project>-<UTC time>.tar`, under
`RIM_BACKUP_DIR` (default `./backups`, which Git ignores). It holds:

| File | What |
|---|---|
| `rim.dump` | `pg_dump -Fc` of the database, checked to read back |
| `rim-data.tgz` | the `rim_data` volume: server key, franking keys, uploads, the audit log and audit exports |
| `env` | `deploy/.env` |
| `MANIFEST`, `SHA256SUMS` | project, time, server version, checksums |

The recovery key file is never in it; the script warns while it is still on the host. The
database and the data volume are one set: the server key file must match the key log in the
database, and the server refuses to start on a mixed set. Never restore one without the other.

With `RIM_BACKUP_AGE_RECIPIENT` set (an `age1…` recipient or a file of recipients), the archive
is encrypted with [age](https://age-encryption.org) to `….tar.age` and nothing unencrypted is
left on disk. Keep the identity that decrypts it off the host, with the recovery key's holders.

```sh
deploy/backup.sh
COMPOSE_FILE=deploy/compose.yml:deploy/compose.relay.yml deploy/backup.sh     # a relay
```

The scripts read `COMPOSE_FILE` and `COMPOSE_PROJECT_NAME` the way docker compose does,
`RIM_ENV_FILE` for another env file, and the `RIM_BACKUP_*` settings from the environment or
`deploy/.env`.

**How often.** Nightly, and before every update or key rotation. A cron entry on the host:

```cron
15 3 * * * cd /srv/rim-server && deploy/backup.sh >>/var/log/rim-backup.log 2>&1
```

**Where copies go.** Off the host, encrypted: **FILL** the destination (object storage, another
machine) and how long copies are kept. The archive holds the server's private key and every
user's data; treat it like the host itself.

### Restore

`deploy/restore.sh` restores an archive into a stopped stack: it drops and recreates the
database from the dump, empties the data volume and refills it, starts the stack and waits for
`/readyz`. It asks for the project name before it replaces anything (`RIM_RESTORE_YES=1` skips
that).

```sh
dc stop rim
deploy/restore.sh backups/rim-backup-rim-20261010T031500Z.tar
RIM_BACKUP_AGE_IDENTITY=~/rim-backup.key deploy/restore.sh backups/….tar.age
```

On a new host, check out the repositories, take `deploy/.env` from the archive's `env` file, build
or pull the images, and run the same command. A newer image applies its migrations to the
restored database when it starts.

### The restore drill

A backup is good once it has been restored. `deploy/restore-drill.sh` restores the newest archive
(or the one given) into a throwaway project, `rim-drill`, waits for `/readyz`, prints the server
key and the row counts of a few tables, and removes the project and its volumes. The drill's
server has server-to-server traffic and mail off and publishes no port, so it never speaks for the
real one; it runs next to the live stack on the same host.

```sh
deploy/restore-drill.sh
deploy/restore-drill.sh backups/rim-backup-rim-20261010T031500Z.tar
```

Run it after the first backup, every quarter, and after changing the backup setup. Compare the
server id and key seq with `dc exec rim /app/rim-server server-key show` and the row counts with
production's. Record each drill: **FILL** where.

## Monitoring

- **Probes.** rim-server answers `/healthz` (200 while the process runs) and `/readyz` (200
  when the database answers, no migration is pending and the key files are readable, else 503
  naming the failing checks) on its own port, without a session. They are for Docker and the
  compose network: the `rim` service's healthcheck uses `/readyz`, and Caddy answers both paths
  with 404. From the host:

  ```sh
  dc ps                                                    # rim, postgres and web: healthy
  dc exec rim curl -fsS http://localhost:8080/readyz; echo
  dc exec rim curl -fsS http://localhost:8080/healthz; echo
  ```

  An outside uptime check watches the public origin instead, for example
  `https://<domain>/.well-known/rim-server` and the web client's `/`.
- **Metrics.** Uncomment `RIM_METRICS_ADDR: 0.0.0.0:9090` in the `rim` service and run
  Prometheus in a container on the compose project's `internal` network, scraping `rim:9090`.
  Never publish the port or route it through Caddy. `docs/operations/metrics.md` lists the
  metrics and what to alert on.
- **Logs.** JSON on stdout: `dc logs --since 1h rim`. Audit events (`"target":"audit"`) also go
  to `/data/audit.log`; `docs/operations/audit-retention.md` covers its rotation and
  `RIM_AUDIT_RETENTION_DAYS` for the audit table.
- **Background jobs.** `docs/operations/background-jobs.md` lists what the server runs on a
  timer, how often, and the setting that changes it.
- **Disk.** The `postgres_data` and `rim_data` volumes, and `RIM_BACKUP_DIR`.

## Limits and blocking

A server open to the internet can limit what one client sends: requests per address, mutations
per session, messages per user, and WebSocket connections and subscriptions. Every limit is off
until its `RIM_RATE_*` variable is set in `deploy/.env` (the "Request limits" block of
`deploy/.env.example`); `docs/api.md` ("Limits") says what clients see past each one. An
address or range that keeps misbehaving is blocked outright with
`dc exec rim /app/rim-server ip block 203.0.113.0/24 --reason "scraping" --expires-in-days 30`:
it gets 403 `{"error":"blocked"}` on every route but `/healthz`, server-to-server routes
included. Leave out `--expires-in-days` to block until `ip unblock`; `ip list` shows every block
with its expiry. A running server reads the list every 30 seconds, so a change applies within
that; each block and unblock is written to the audit log as `ip.block` or `ip.unblock`. Behind
Caddy the address is the client's, since `RIM_TRUST_FORWARDED_HEADERS` is on; behind another
proxy it must replace `X-Forwarded-For` (next sections), or one forged header unblocks a client.

### Instance limits

A public instance can also cap how much each person holds: workspaces per user, members and
channels per workspace, devices per account, DM requests to strangers per day, macros,
pipelines and plugins per workspace, invites per user per day, uses per invite and the bytes
of a message. Each is a `RIM_LIMIT_*` variable, off until set (the "Instance limits" block of
`deploy/.env.example` has values for a small test instance); past one a write is refused with
a message naming the cap, and clients read every cap from `serverInfo.limits`. Devices and DM
requests also follow the relay quotas `RIM_RELAY_DEVICES_PER_USER` and
`RIM_RELAY_DM_REQUESTS_PER_DAY`: the stricter number applies. A negative or non-numeric value,
or `RIM_LIMIT_MESSAGE_BYTES` above 65536, stops the server at boot.

`RIM_PROBATION_HOURS` slows down accounts made to spam: for that many hours after its creation
an account cannot create a workspace or send a DM request to someone it shares no workspace
with and has not added as a contact, and with `RIM_RATE_PROBATION_MESSAGES_PER_MIN` it sends
messages at that lower rate. Existing accounts are judged by their age, so turning it on does
not hold back anyone older than the setting.

## Optional features

Attachments, link previews, email, custom emoji, workspace plugins and the AI command surface
can be switched off with `RIM_FEATURES_DISABLED` (commented out in the `rim` service);
`docs/operations/features.md` lists what each one covers and what clients see.

## Notice and terms

`RIM_INSTANCE_NOTICE` is a short text, at most 500 characters, that clients show people on
your server, such as who runs it and how to reach you. `RIM_TERMS_URL` is the address of your
terms of service; once it is set, every signup has to accept those terms and the server records
on the account when it did and which URL it accepted. Both are commented out in
`deploy/.env.example` and reach the `rim` service when set; clients read them from
`serverInfo`. A notice over 500 characters or a terms URL that is not an absolute http(s) URL
stops the server at boot. Changing the terms URL does not ask existing accounts to accept again.

## Another proxy

Caddy is an example. Any TLS proxy works if it:

- sends `/api` (with `/api/ws` as a WebSocket upgrade), `/.well-known/…` and `/s2s/…` to
  `rim:8080` unchanged, and the rest to `web:80`;
- keeps `/healthz` and `/readyz` off the public side;
- accepts bodies up to 32 MiB;
- sets `X-Forwarded-For` itself, replacing what the client sent;
- is the only way in.

`deploy/README.md` has an nginx example for the relay's paths.
