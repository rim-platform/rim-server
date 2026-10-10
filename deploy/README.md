# Deploying a public relay

`compose.relay.yml` turns `compose.yml` into a public relay: rim-server with `RIM_ROLE=relay`,
the public relay's quotas and proof-of-work signups, PostgreSQL 17 and Caddy terminating TLS,
and no web client. The relay holds its users' accounts, devices and cards, queues what other
servers send them and forwards their requests; it hosts no workspaces. The operator's runbook is
`docs/oncall/relay-public.md`; the protocol is `docs/specs/server-to-server-v1.md`.

A server that hosts workspaces uses `compose.yml` alone: `docs/self-hosting.md` covers it, and
its sections on updates and backups apply to the relay too.

The relay's working name, `relay.rim.chat`, is not a registered domain. Use your own.

## Before the first start

1. A host with Docker (Compose v2.20 or later), ports 80 and 443 open, and DNS `A`/`AAAA`
   records of your domain pointing at it. Caddy gets the certificate from Let's Encrypt on the
   first request.
2. Create `deploy/.env` (Git ignores it; `deploy/.env.example` lists every setting):

   ```sh
   cat > deploy/.env <<EOF
   POSTGRES_PASSWORD=$(openssl rand -base64 32 | tr -d '/+=')
   RIM_DOMAIN=relay.example.org
   EOF
   chmod 600 deploy/.env
   ```

   `RIM_PUBLIC_BASE_URL` follows `RIM_DOMAIN`. Every other setting has the public relay's value
   in the compose files; set it in `.env` to change it (the quotas are listed in the runbook).
   Without `POSTGRES_PASSWORD` or `RIM_DOMAIN` every compose command stops and names it.
3. Check the configuration. The commands on this page and in the runbook use this alias, from
   the repository root:

   ```sh
   alias dc='docker compose -f deploy/compose.yml -f deploy/compose.relay.yml'
   dc config --services          # postgres, migrate, rim, caddy: no web client
   ```

   The relay builds only `rim-server:local` (or uses `RIM_SERVER_IMAGE`); it needs no rim-ui
   checkout.

## Start

```sh
dc up -d --build
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

## Health

The server answers two probes on its own port, without a session. Caddy does not proxy them,
so they are reachable from inside the compose network only:

- `GET /healthz` answers 200 with `{"status":"ok","version":"…"}` while the process serves
  requests. The image's `HEALTHCHECK` uses it.
- `GET /readyz` answers 200 when the database answers, no migration is pending and the server
  key file and franking key file are readable. Otherwise it answers 503 with `"status":"fail"`
  and the failing checks named under `checks`; the log says why each one failed. The `rim`
  service's compose healthcheck uses it.

```sh
dc ps rim          # healthy once /readyz passes
dc exec rim curl -s http://localhost:8080/readyz
dc exec rim /app/rim-server --version
```

## The operator's commands

The CLI is `/app/rim-server` inside the `rim` container and reads the same environment:

```sh
alias rim-relay='docker compose -f deploy/compose.yml -f deploy/compose.relay.yml exec rim /app/rim-server'
rim-relay server-key show
rim-relay principal block rimp1_… --reason "spam wave 2026-11-02"
rim-relay peer block rims1_…
rim-relay abuse-reports list --open
rim-relay invite create --max-uses 10 --expires-in-days 7 --public-url https://relay.example.org
rim-relay bootstrap-user --email ops@example.org --public-url https://relay.example.org
```

`invite create` prints a signup link whose token works as a registration token in
`signupWithKey`, so it is how the operator lets people in without the proof-of-work: up to 50
signups per link, for at most 30 days. `invite list` shows the live ones with their use counts and
`invite revoke <id>` ends one. `bootstrap-user` prints a single-use link for one address.
(`createInstanceInvite` in the app needs the owner of a workspace, and a relay has none.)

`audit purge --dry-run` counts the command audit rows past `RIM_AUDIT_RETENTION_DAYS` and
`audit purge` exports them to `/data/audit-exports/` and deletes them; with the variable set the
server does the same daily. `docs/operations/audit-retention.md` has the details and the
logrotate setup for `/data/audit.log`.

## Updating

```sh
COMPOSE_FILE=deploy/compose.yml:deploy/compose.relay.yml deploy/backup.sh
git pull
dc up -d --build
```

The migrations run before the new server starts. The backup scripts and how often to run them
are in the runbook, "Backups".

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
