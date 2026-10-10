# Metrics

rim-server exposes Prometheus metrics in the text exposition format at `GET /metrics` on a
listener of its own. The API port never serves them.

## Turning them on

| Variable | Meaning |
|---|---|
| `RIM_METRICS_ADDR` | `ip:port` of the metrics listener. Unset (the default), no recorder is installed and no listener opens. A value that is not an `ip:port` address stops the server at boot. |

On a host, `127.0.0.1:9090` keeps the endpoint on the machine; run Prometheus there or scrape it
through an SSH tunnel. In a container, `127.0.0.1` is the container itself, so bind
`0.0.0.0:9090` and do not publish the port: Prometheus reaches it over the compose network.
`deploy/compose.yml` has the line commented out in the `rim` service.

The endpoint has no authentication. Keep it on an internal network, and never route it through
Caddy or any other public reverse proxy: it shows request rates, limiter names and queue sizes
to whoever can read it.

A minimal scrape configuration:

```yaml
scrape_configs:
  - job_name: rim
    static_configs:
      - targets: ["rim:9090"]
```

## The metrics

Names start with `rim_`; counters end in `_total`, and the unit is part of the name
(`_seconds`, `_bytes`, `_connections`, `_items`).

### Requests

| Metric | Type | Labels | Meaning |
|---|---|---|---|
| `rim_http_requests_total` | counter | `route`, `status` | HTTP requests on the API port. |
| `rim_http_request_duration_seconds` | histogram | `route`, `status` | Time from receiving a request to sending its response head. Buckets from 5 ms to 30 s. A WebSocket upgrade counts until the `101`, not for the life of the socket. |
| `rim_s2s_requests_total` | counter | `outcome` | Requests received under `/s2s/v1/`, counted once server-to-server traffic is on. |
| `rim_forwarded_requests_total` | counter | `direction`, `outcome` | Requests forwarded to a host. |

`route` is a route class, never the raw path, so the number of series stays fixed:

| `route` | Paths |
|---|---|
| `api` | `/api`, `/api/ws`, `/api/sdl`, `/api/hooks/…` and anything else under `/api` not listed below |
| `files` | `/api/files/…`, `/api/attachments/…`, `/api/reports/…`, `/api/plugin-bundles/…` |
| `remote-forward` | `/api/remote/…`, a local user's request to a space hosted elsewhere |
| `s2s` | `/s2s/…` |
| `wellknown` | `/.well-known/…` |
| `health` | `/healthz`, `/readyz`, `/livez` |
| `other` | everything else, including unknown paths that answer `404` |

`status` is the status class: `1xx`, `2xx`, `3xx`, `4xx` or `5xx`.

`rim_s2s_requests_total{outcome}`:

| `outcome` | Meaning |
|---|---|
| `ok` | Every check passed and the endpoint answered below `500`; a `404` for an unknown principal is `ok`. |
| `signature_invalid` | The signature, its parameters, the key, the body digest or the clock window was refused (`401`), or the nonce was a replay (`409`). |
| `peer_blocked` | The peer is denied, outside the host allow-list, blocked, could not be pinned from its origin, or its key log forked (`403`). |
| `rate_limited` | The per-peer limit refused it, or the endpoint answered `429`. |
| `error` | A malformed or oversized request before the checks, or an answer of `500` or above. |

`rim_forwarded_requests_total{direction, outcome}`: `direction="outgoing"` is this server as a
relay sending a local user's request through `/api/remote/…`; `direction="incoming"` is this
server as a host running a forwarded request at `/s2s/v1/forward/…`.

| `outcome` | Meaning |
|---|---|
| `ok` | Answered below `400`. |
| `refused` | Answered `4xx` other than `429`: no session, a bad device request, not delegated, a replayed device seq, refused by the host. |
| `rate_limited` | Answered `429`, by this server or passed through from the host. |
| `upstream_unavailable` | Outgoing only: the host could not be reached or gave no verified answer (`502`). |
| `error` | Answered `500` or above. |

### Refusals

| Metric | Type | Labels | Meaning |
|---|---|---|---|
| `rim_rate_limit_rejections_total` | counter | `limiter` | Requests a rate limiter or quota refused. |
| `rim_pow_rejections_total` | counter | | Key-only signups whose proof-of-work did not meet `RIM_REGISTRATION_POW_BITS` or was malformed. |

`limiter` values:

| `limiter` | What it limits |
|---|---|
| `login` | Password logins per address, per address and account, and per account. |
| `key_auth_failure` | Failed key logins per address. |
| `key_challenge` | Key login challenges per address. |
| `signup` | Open and proof-of-work signups per address. |
| `preview_invite` | Invite token previews per address. |
| `password_reset` | Password reset requests per address. |
| `password_reset_account` | Password reset requests per account; the caller sees success and no email is sent. |
| `device_link_start`, `device_link_offer`, `device_link_answer`, `device_link_refusal` | Device linking steps. |
| `mls_claim`, `mls_one_time_claim`, `mls_readd` | MLS key package claims and re-add requests. |
| `command`, `command_discover` | Command runs per user and command, and command discovery. |
| `workspace_plugin_change`, `plugin_bundle_upload` | Workspace plugin changes and bundle uploads per user. |
| `alert_ingest` | Alert webhook requests per ingest token. |
| `s2s_peer` | Server-to-server requests per peer (`RIM_S2S_PEER_RATE_PER_MIN`). |
| `peer_discovery` | Discovery document fetches, one per peer and sixty in all per minute. |
| `relay_forward_device`, `relay_forward_user` | Requests a local user forwards, per device per minute and per user per day. |
| `host_forward_device` | Forwarded requests run here, per remote device per minute. |
| `dm_invites` | Invites a local user sends to people on other servers per day. |
| `dm_requests` | DM requests filed here by one sender per day, or past the recipient's pending limit. |
| `stranger_claim` | Key package claims by servers with no tie to the user. |
| `delegations_per_relay` | Active delegations one relay holds on this host (`RIM_S2S_SUBSCRIPTIONS_PER_PEER`). |
| `direct_revocation` | Device revocations brought directly, per address. |

Slow mode in a conversation is not counted here.

### State

| Metric | Type | Labels | Meaning |
|---|---|---|---|
| `rim_db_pool_connections` | gauge | | Open connections in the database pool, read on each scrape. |
| `rim_db_pool_connections_in_use` | gauge | | Pool connections checked out by a query or transaction, read on each scrape. |
| `rim_db_pool_max_connections` | gauge | | The most connections the pool opens. |
| `rim_relay_queue_items` | gauge | `state` | Items queued for local users' devices. |
| `rim_relay_queue_bytes` | gauge | `state` | Bytes of those items. |
| `rim_pending_migrations` | gauge | | Migrations this binary has that the database has not applied. |
| `rim_build_info` | gauge | `version` | Always `1`; `version` is the server version. |

`state` is `queued` for items waiting for devices to read them, `held` for items behind a DM
request the recipient has not answered. The relay queue and the pending migrations are read from
the database when the listener starts and every 30 seconds after; a read that fails or takes over
5 seconds leaves the last value in place and logs a warning.

### Audit retention

| Metric | Type | Labels | Meaning |
|---|---|---|---|
| `rim_audit_rows_purged_total` | counter | | Command audit rows the daily retention purge deleted (`docs/operations/audit-retention.md`). |
| `rim_audit_retention_days` | gauge | | `RIM_AUDIT_RETENTION_DAYS`, set at boot; `0` when rows are kept forever. |

## Example alert rules

Readiness is checked from outside, by probing `/readyz` on the API port with the Prometheus
blackbox exporter (module `http_2xx`, job `rim-readyz`). The second rule fires when peers' signatures
start failing: a peer whose key changed without a rotation, clock skew, or someone probing the
S2S API.

```yaml
groups:
  - name: rim
    rules:
      - alert: RimNotReady
        expr: probe_success{job="rim-readyz"} == 0
        for: 2m
        labels:
          severity: page
        annotations:
          summary: "rim-server at {{ $labels.instance }} has failed its readiness check for 2 minutes"

      - alert: RimS2sSignatureFailures
        expr: sum by (instance) (rate(rim_s2s_requests_total{outcome="signature_invalid"}[5m])) > 0.2
        for: 10m
        labels:
          severity: ticket
        annotations:
          summary: "rim-server at {{ $labels.instance }} refuses over 12 server-to-server signatures a minute"
          description: "Check the warnings 'refused a server-to-server signature' for the peer and reason."
```

Other useful expressions:

```promql
# Share of API requests answered with 5xx
sum(rate(rim_http_requests_total{route="api",status="5xx"}[5m]))
  / sum(rate(rim_http_requests_total{route="api"}[5m]))

# 95th percentile API latency
histogram_quantile(0.95, sum by (le) (rate(rim_http_request_duration_seconds_bucket{route="api"}[5m])))

# A deploy that started before its migrations ran
rim_pending_migrations > 0

# The pool is close to exhausted
rim_db_pool_connections_in_use / rim_db_pool_max_connections > 0.9
```
