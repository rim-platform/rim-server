# Deploying a public relay

`compose.relay.yml` turns `compose.yml` into a public relay: rim-server with the public relay's
quotas and proof-of-work signups, PostgreSQL 17 and Caddy terminating TLS, and no web client
unless the operator adds it. The relay holds its users' accounts, devices and cards, queues what
other servers send them and forwards their requests; it hosts no workspaces unless the operator
turns that on ("A relay that hosts a workspace"). This page brings a relay up for the first
time. The operator's runbook (`docs/operations/relay-public.md`) covers running it: keys,
backups, quotas, monitoring, incidents, the abuse policy and every command. `docs/relay.md`
explains what a relay does, and `docs/specs/server-to-server-v1.md` is the protocol.

A server that hosts workspaces uses `compose.yml` alone: `docs/self-hosting.md` covers it, and
its sections on updates, backups and monitoring apply to the relay too.

The project's public relay is `relay.rim-platform.org`, with its web client on
`chat.rim-platform.org`; yours uses your own domain. `RIM_DOMAIN` is the relay's host name, such
as a `relay.` subdomain: Caddy answers for that name only (and for `RIM_WEB_DOMAIN` when it is
set), so the apex of the domain can serve a website from elsewhere.

## Before the first start

1. A host with Docker (Compose v2.20 or later), ports 80 and 443 open, and DNS `A`/`AAAA`
   records of your domain pointing at it. Caddy gets the certificate from Let's Encrypt on the
   first request.
2. Create `deploy/.env` (Git ignores it; `deploy/.env.example` lists every setting):

   ```sh
   cat > deploy/.env <<EOF
   POSTGRES_PASSWORD=$(openssl rand -base64 32 | tr -d '/+=')
   RIM_DOMAIN=relay.example.org
   RIM_VERSION=0.0.1
   RIM_ROLE=relay
   RIM_CADDYFILE=./Caddyfile.relay
   EOF
   chmod 600 deploy/.env
   ```

   `RIM_VERSION` is the release you run, the tag this checkout is at without its `v`.
   `RIM_PUBLIC_BASE_URL` follows `RIM_DOMAIN`. `RIM_ROLE` and `RIM_CADDYFILE` say what the relay
   is: a relay that serves the API alone, as here, or one that hosts a workspace with the web
   client (below). Every other setting has the public relay's value in the compose files;
   set it in `.env` to change it (the quotas are listed in the runbook). Without
   `POSTGRES_PASSWORD`, `RIM_DOMAIN`, `RIM_ROLE` or `RIM_CADDYFILE` every compose command stops
   and names it.
3. Check the configuration. The commands on this page and in the runbook use this alias, from
   the repository root:

   ```sh
   alias dc='docker compose -f deploy/compose.yml -f deploy/compose.relay.yml'
   dc config --services          # postgres, migrate, rim, caddy-init, caddy: no web client
   ```

   The relay runs only `ghcr.io/rim-platform/rim-server:<RIM_VERSION>` (or
   `RIM_SERVER_IMAGE`). Verify it as `docs/self-hosting.md`, "Verify the images", says. To
   build it from this checkout instead, add `-f deploy/compose.dev.yml` to the alias and use
   `up --build`; the relay needs no rim-ui checkout either way.

## Start

```sh
dc up -d
dc logs -f rim
```

The `migrate` service applies the migrations and exits; the server starts after it. On its
first start the server creates its key, writes `/data/server-recovery-key.txt` and logs a
warning about it at every start until the file is gone. Move it offline now, as the runbook's
"The server recovery key" says:

```sh
dc exec rim cat /data/server-recovery-key.txt
# copy the line to the offline places the runbook names, then:
dc exec rim rm /data/server-recovery-key.txt
```

Check it answers:

```sh
curl -s https://relay.example.org/.well-known/rim-server | head -c 300; echo
dc exec rim /app/rim-server server-key show
```

The descriptor in the discovery document says `"registration":{"mode":"pow","powBits":24}`.

## A relay that hosts a workspace

A relay can host workspaces too, as the project's relay does: it runs `RIM_ROLE=both` and
hosts the RIM community workspace, which every new account joins, with the web client. Three
settings in `deploy/.env` turn it on:

```sh
RIM_ROLE=both
RIM_CADDYFILE=./Caddyfile
COMPOSE_PROFILES=web
```

`RIM_ROLE=both` lets accounts here create workspaces, `./Caddyfile` sends every path that is not
the API, discovery or server-to-server to the web client, and the `web` profile starts it.
`dc config --services` then lists `web` too. To serve the web client on a domain of its own,
as `chat.rim-platform.org` is, add `RIM_WEB_DOMAIN` ("The web client on its own domain" below).

The first run, in short: start the stack as above, read the owner claim link with
`dc exec rim cat /data/owner-claim.txt`, open it in a browser and sign up through it. That
account owns the server's default workspace (`docs/self-hosting.md`, "A default workspace").

### The web client on its own domain

With `RIM_WEB_DOMAIN` set, `./Caddyfile` answers for a second domain too: the web client is
served there and calls the API on that same origin, and `RIM_DOMAIN` stays the server's name,
the one apps sign in to, other servers reach, and the discovery document names. A browser that
opens `RIM_DOMAIN` is redirected to the web domain with its path and query, so links the server
builds on `RIM_PUBLIC_BASE_URL`, such as invite and owner claim links, open the web client.

```sh
RIM_WEB_DOMAIN=chat.example.org     # an A/AAAA record for it points at this host too
```

`docs/self-hosting.md`, "The web client on its own domain", says what the server accepts from
that origin.

## Health, updates and operation

The probes (`/healthz`, `/readyz`) and their checks are the same as on any server:
`docs/self-hosting.md`, "Monitoring". Caddy does not proxy them, so they are reachable from
inside the compose network only:

```sh
dc ps rim          # healthy once /readyz passes
dc exec rim curl -s http://localhost:8080/readyz
```

Update as `docs/self-hosting.md`, "Updates", says, with the relay's files named for the backup:

```sh
COMPOSE_FILE=deploy/compose.yml:deploy/compose.relay.yml deploy/backup.sh
git fetch --tags && git checkout v<version>
$EDITOR deploy/.env          # RIM_VERSION=<version>
dc pull && dc up -d
```

The operator's commands (`/app/rim-server` inside the `rim` container: invites that skip the
proof-of-work, principal and peer blocks, abuse reports, key rotation) are listed in the runbook,
"Command reference".

## Metrics to Grafana Cloud

`compose.monitoring.yml` on top turns on the server's metrics listener and adds a Grafana Alloy
collector (`monitoring.alloy`): every 15 seconds it scrapes the server's metrics (job `rim`),
probes `/readyz` (job `rim-readyz`) and reads the host's CPU, memory and filesystems (job
`node`), and remote-writes them to Grafana Cloud. Nothing new is published; the collector only
connects out. Add the three settings `deploy/.env.example` describes under "Monitoring" to
`deploy/.env`, then put the overlay in the alias:

```sh
alias dc='docker compose -f deploy/compose.yml -f deploy/compose.relay.yml -f deploy/compose.monitoring.yml'
dc up -d
dc logs alloy          # no "level=error" once it scrapes and sends
```

Each compose service's CPU and memory use (job `cadvisor`) comes from a second overlay,
`compose.monitoring-containers.yml`, on rootful Docker on Linux. It runs cAdvisor behind a
Docker socket proxy and a containerd socket proxy that let it make only read-only calls;
`docs/operations/metrics.md`, "Container metrics", says which and what they still expose. It
covers the containerd image store, the default of a new Docker 29 install; on the older
`overlay2` storage driver (`docker info --format '{{.Driver}}'`) add
`compose.monitoring-containers-overlay2.yml` after it. Set `DOCKER_GID` in `deploy/.env` and
add the overlay after `compose.monitoring.yml`:

```sh
echo "DOCKER_GID=$(stat -c %g /var/run/docker.sock)" >> deploy/.env
alias dc='docker compose -f deploy/compose.yml -f deploy/compose.relay.yml -f deploy/compose.monitoring.yml -f deploy/compose.monitoring-containers.yml'
dc up -d
```

The dashboards and alert rules that read these series are in the
[rim-monitoring](https://github.com/rim-platform/rim-monitoring) repository;
`docs/operations/metrics.md` lists the metrics.

## Another proxy

Caddy is an example. Any TLS proxy works if it:

- passes `/api` (with `/api/ws` as a WebSocket upgrade, and `/api/remote/…`), `/s2s/v1/…` and
  `/.well-known/rim-server` to `rim:8080` unchanged: no path rewriting, no compression, no body
  changes, since server-to-server requests and answers are signed over method, URL and body;
- accepts request bodies up to 32 MiB;
- sets `X-Forwarded-For` to the client's address itself, replacing what the client sent, because
  the server trusts it (`RIM_TRUST_FORWARDED_HEADERS=true`) for its per-IP limits, signups
  included; and sets `X-Forwarded-Proto`;
- is the only way in: port 8080 is never published.

With nginx:

```nginx
server {
    listen 443 ssl;
    http2 on;
    server_name relay.example.org;
    client_max_body_size 32m;
    gzip off;

    location ~ ^/(api(/|$)|s2s/v1/|\.well-known/rim-server$) {
        proxy_pass http://127.0.0.1:8080;
        proxy_http_version 1.1;
        proxy_set_header Host $host;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection $connection_upgrade;
        proxy_set_header X-Forwarded-For $remote_addr;
        proxy_set_header X-Forwarded-Proto https;
        proxy_read_timeout 1h;
    }
}
```

(`$connection_upgrade` is the usual `map $http_upgrade $connection_upgrade { default upgrade;
'' close; }` in the `http` block.)
