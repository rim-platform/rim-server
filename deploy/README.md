# Deploying a public relay

`compose.relay.yml` runs a public relay: rim-server with `RIM_ROLE=relay`, PostgreSQL 17 and
Caddy terminating TLS. The relay holds its users' accounts, devices and cards, queues what other
servers send them and forwards their requests; it hosts no workspaces. The operator's runbook is
`docs/oncall/relay-public.md`; the protocol is `docs/specs/server-to-server-v1.md`.

The compose file's default domain, `relay.rim.chat`, is a working name: that domain is not
registered. Use your own.

## Before the first start

1. A host with Docker (Compose v2.20 or later), ports 80 and 443 open, and DNS `A`/`AAAA`
   records of your domain pointing at it. Caddy gets the certificate from Let's Encrypt on the
   first request.
2. Create `deploy/.env` (Git ignores it):

   ```sh
   cat > deploy/.env <<EOF
   POSTGRES_PASSWORD=$(openssl rand -base64 32 | tr -d '/+=')
   RIM_DOMAIN=relay.example.org
   RIM_PUBLIC_BASE_URL=https://relay.example.org
   EOF
   chmod 600 deploy/.env
   ```

   Every other setting has the public relay's value in the compose file; set it in `.env` to
   change it (the quotas are listed in the runbook). Without `POSTGRES_PASSWORD` the database
   container refuses to start.
3. Check the configuration: `docker compose -f deploy/compose.relay.yml config`.

## Start

```sh
docker compose -f deploy/compose.relay.yml up -d --build
docker compose -f deploy/compose.relay.yml logs -f rim
```

The `migrate` service applies the migrations and exits; the server starts after it. On its
first start the server creates its key, writes `/data/server-recovery-key.txt` and logs a
warning about it at every start until the file is gone. Move it offline now, as the runbook's
"The server recovery key" says:

```sh
docker compose -f deploy/compose.relay.yml exec rim cat /data/server-recovery-key.txt
# copy the line to the offline places the runbook names, then:
docker compose -f deploy/compose.relay.yml exec rim rm /data/server-recovery-key.txt
```

Check it answers:

```sh
curl -s https://relay.example.org/.well-known/rim-server | head -c 300; echo
docker compose -f deploy/compose.relay.yml exec rim /app/rim-server server-key show
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
docker compose -f deploy/compose.relay.yml ps rim          # healthy once /readyz passes
docker compose -f deploy/compose.relay.yml exec rim curl -s http://localhost:8080/readyz
docker compose -f deploy/compose.relay.yml exec rim /app/rim-server --version
```

## The operator's commands

The CLI is `/app/rim-server` inside the `rim` container and reads the same environment:

```sh
alias rim-relay='docker compose -f deploy/compose.relay.yml exec rim /app/rim-server'
rim-relay server-key show
rim-relay principal block rimp1_… --reason "spam wave 2026-11-02"
rim-relay peer block rims1_…
rim-relay abuse-reports list --open
rim-relay bootstrap-user --email ops@example.org --public-url https://relay.example.org
```

`bootstrap-user` prints a single-use signup link. Its token works as a registration token in
`signupWithKey`, so it is also how the operator lets someone in without the proof-of-work: one
link per person. (`createInstanceInvite` needs the owner of a workspace, and a relay has none.)

## Updating

```sh
git pull
docker compose -f deploy/compose.relay.yml up -d --build
```

The migrations run before the new server starts. Take a backup first (runbook, "Backups").

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
