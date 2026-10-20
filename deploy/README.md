# Deploying a public relay

`compose.relay.yml` turns `compose.yml` into a public relay: rim-server with `RIM_ROLE=relay`,
the public relay's quotas and proof-of-work signups, PostgreSQL 17 and Caddy terminating TLS,
and no web client. The relay holds its users' accounts, devices and cards, queues what other
servers send them and forwards their requests; it hosts no workspaces. This page brings a relay up for the first time. The operator's runbook
(`docs/operations/relay-public.md`) covers running it: keys, backups, quotas, monitoring,
incidents, the abuse policy and every command. `docs/relay.md` explains what a relay does, and
`docs/specs/server-to-server-v1.md` is the protocol.

A server that hosts workspaces uses `compose.yml` alone: `docs/self-hosting.md` covers it, and
its sections on updates, backups and monitoring apply to the relay too.

The project's public relay is `relay.rim-platform.org`; yours uses your own domain. `RIM_DOMAIN`
is the relay's host name, such as a `relay.` subdomain: Caddy answers for that name only, so the
apex of the domain can serve a website from elsewhere.

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
   EOF
   chmod 600 deploy/.env
   ```

   `RIM_VERSION` is the release you run, the tag this checkout is at without its `v`.
   `RIM_PUBLIC_BASE_URL` follows `RIM_DOMAIN`. Every other setting has the public relay's value
   in the compose files; set it in `.env` to change it (the quotas are listed in the runbook).
   Without `POSTGRES_PASSWORD` or `RIM_DOMAIN` every compose command stops and names it.
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

`deploy/upgrade.sh <version>` does the same after the checkout, with the signature check, the
migrations first and a rollback when the new version does not come up
(`docs/operations/upgrade.md`).

The operator's commands (`/app/rim-server` inside the `rim` container: invites that skip the
proof-of-work, principal and peer blocks, abuse reports, key rotation) are listed in the runbook,
"Command reference".

## Unattended upgrades from releases

A host that runs the published images needs no checkout: one directory holds the compose files,
the Caddyfiles, the scripts and `.env`, and `rim-deploy` keeps it on the latest release. On the
project's relay that directory is `/srv/rim-server`. Every five minutes `rim-deploy.timer` runs
`rim-deploy --check`, which asks the GitHub API for the latest published release of rim-server
that is not a pre-release. When that version is not `RIM_VERSION` in `.env`, it downloads the
tag's source tarball, copies its `deploy/` files over the directory, writes `SOURCE` (the tag,
the time and the tarball's SHA-256) and runs the tag's `upgrade.sh <version>` there
(`docs/operations/upgrade.md`): the signatures, the backup, the migrations, the start and the
rollback are its steps. Nothing in GitHub reaches the host, and no git runs on it. A release is
published when its `release.yaml` run is approved, so approving a release is the deployment; it
runs within five minutes, so approve it in a quiet hour.

The deployer writes nothing in `.env` but `RIM_VERSION`, and never touches `.env` or
`.env.previous` otherwise; every other file in the directory is the tag's, and the next deploy
overwrites a file edited on the host. Settings change in `.env` by hand, and so do the
**Upgrade:** lines of `CHANGELOG.md`, before the release is approved. When the upgrade does not
complete, the files from before come back.

Installing it, as root, with Docker (Compose v2.20 or later), cosign, curl and flock on the host:

```sh
mkdir -p /srv/rim-server        # then write /srv/rim-server/.env, chmod 600
install -m 0755 deploy/rim-deploy.sh /usr/local/sbin/rim-deploy
install -m 0644 deploy/systemd/rim-deploy.service deploy/systemd/rim-deploy.timer /etc/systemd/system/
systemctl daemon-reload
systemctl enable --now rim-deploy.timer
rim-deploy --check              # "no release yet", or it deploys the latest release
```

`.env` names the compose files for the scripts and for `docker compose` run in the directory,
relative to it: `COMPOSE_FILE=compose.yml:compose.relay.yml` for a relay. Leave `RIM_VERSION`
out before the first deploy, which then skips the backup (there is no data yet), starts the
stack and records the version. `rim-deploy` is installed by hand and changes rarely; when it
does, install it again from the new release the same way. `RIM_DEPLOY_DIR` names another
directory.

- **The log** is `journalctl -u rim-deploy` (`-f` follows a deploy as it runs);
  `systemctl list-timers rim-deploy.timer` shows the next check. Each check logs one line when
  there is nothing to do.
- **Not yet published images.** When `rim-web` of the version is not out yet, because rim-ui
  was not tagged first, `upgrade.sh` exits 2 before changing anything; the check puts the files
  back, ends quietly, and the next one tries again.
- **Brakes.** A release marked **pre-release** on GitHub is never deployed by the timer, which is
  how a release is published without deploying it. `touch /srv/rim-server/HOLD` stops the timer
  deploying anything until the file is removed. A deploy that fails writes its version to
  `FAILED`, and the timer leaves that version alone until the file is removed; the next release
  deploys as usual.
- **By hand.** `rim-deploy <version>` deploys that version without asking the API, forward or
  back, whether `HOLD` or `FAILED` exist or not; with `upgrade.sh`, a version older than the
  running one is a binary rollback. Touch `HOLD` before rolling back, or the next check deploys
  the latest release again.
- **The unit** runs as root, because it runs docker, with no new privileges, a read-only
  `/usr`, `/boot`, `/etc` and home directories, and a private `/tmp`; cosign and the docker
  client keep their caches in `/var/lib/rim-deploy`. A `v*` tag starts the whole chain, and
  only the maintainer may push one to rim-server and rim-ui; an image the release workflows did
  not sign does not verify, and `upgrade.sh` stops before it changes anything.

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
