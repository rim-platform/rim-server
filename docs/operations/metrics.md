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
`deploy/compose.yml` has the line commented out in the `rim` service;
`deploy/compose.monitoring.yml` sets it (below).

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

## Collecting them with Grafana Alloy

`deploy/compose.monitoring.yml`, on top of `deploy/compose.yml` (and `compose.relay.yml` for a
relay), sets `RIM_METRICS_ADDR=0.0.0.0:9090` and adds a Grafana Alloy collector configured by
`deploy/monitoring.alloy`. Alloy sits on the internal network next to rim-server, publishes no
port, and every 15 seconds collects:

| Job | What |
|---|---|
| `rim` | rim-server's `/metrics` on `rim:9090`, with `up`. |
| `rim-readyz` | `probe_success` of a GET of `http://rim:8080/readyz`: a `200` passes, a redirect fails. `instance` is that URL. |
| `node` | The host's CPU, memory, filesystem and network metrics (`node_*`), read from `/proc`, `/sys` and `/` mounted read-only. `instance` is `RIM_DOMAIN`. |
| `alloy` | The collector's own metrics, such as its remote-write queue. |
| `cadvisor` | With `deploy/compose.monitoring-containers.yml` only: each compose service's CPU and memory use (`container_cpu_usage_seconds_total`, `container_memory_working_set_bytes`), labelled `container_label_com_docker_compose_project` and `container_label_com_docker_compose_service`. |

It remote-writes them to `GRAFANA_CLOUD_METRICS_URL` with `GRAFANA_CLOUD_METRICS_USER` and
`GRAFANA_CLOUD_METRICS_TOKEN` from `deploy/.env` (`deploy/.env.example` says where Grafana Cloud
shows them); any Prometheus remote-write endpoint with basic authentication works the same way.
Samples the endpoint does not accept are kept in Alloy's volume and retried for up to two
hours. The dashboards and alert rules that read these series are in the
[rim-monitoring](https://github.com/rim-platform/rim-monitoring) repository.

### Container metrics

`deploy/compose.monitoring-containers.yml`, on top of the files above, adds cAdvisor and a
Docker socket proxy, and `deploy/monitoring-containers.alloy` to Alloy's configuration
directory. It needs rootful Docker on Linux with the `overlay2` storage driver
(`docker info --format '{{.Driver}}'`), and `DOCKER_GID` in `deploy/.env`, the group of
`/var/run/docker.sock` (`stat -c %g /var/run/docker.sock`).

Only the proxy (`wollomatic/socket-proxy`) mounts the socket. It answers cAdvisor alone, on a
network of the two with no way out, and only `HEAD /_ping`, `GET /version`, `GET /info` and
`GET /containers/<id>/json`; any other path or method is refused, so nothing can create,
start, stop or remove a container, exec into one, read its logs or copy files out of it.
cAdvisor runs unprivileged without capabilities, reads the host's cgroups and the read-only
layer ids under `/var/lib/docker/image/overlay2/layerdb/mounts`, and has no way out either;
Alloy scrapes it on the internal network and forwards only the two series above.

What remains: inspecting a container returns its configuration, environment variables
included, and other containers' environments hold secrets such as `DATABASE_URL` with the
database password and `GRAFANA_CLOUD_METRICS_TOKEN`. A compromised cAdvisor could read them;
it has no route to the internet itself, but it shares the internal network with the other
services.

## The metrics

Names start with `rim_`; counters end in `_total`, and the unit is part of the name
(`_seconds`, `_bytes`, `_connections`, `_items`).

Labels describe a fixed set of categories. They never contain user or device ids, conversation
ids, message contents, raw paths, GraphQL aliases or client-chosen operation names.

### Accounts and activity

| Metric | Type | Labels | Meaning |
|---|---|---|---|
| `rim_registered_accounts` | gauge | | Current local accounts, excluding remote identities and deleted accounts. |
| `rim_registrations_total` | counter | | Successful local registrations since this process started. |
| `rim_active_users` | gauge | `window` | Distinct local accounts with authenticated application activity in the last `24h` or `7d`. |

Registered accounts are accounts, not verified individual people. Activity means a successfully
authenticated session request; it can include background synchronization and does not prove
that a person is reading. Heartbeats on an already-open socket do not refresh this value by
themselves. A persistent last-activity timestamp survives session removal and process restarts;
writes are throttled to once per five minutes per user, so window boundaries can lag by that
much. The gauge is sampled from the database rather than summing session or per-process counts.

### Messages

| Metric | Type | Labels | Meaning |
|---|---|---|---|
| `rim_messages_created_total` | counter | `encryption` | New committed messages, including replies and forwards; `plaintext` or `encrypted`. |

Edits and retries returning an existing encrypted message do not create another count. A message
is counted once where its creation commits, regardless of the number of recipients, devices or
relays. Encrypted message contents are never inspected for monitoring. This measures server
acceptance, not delivery or a human read receipt.

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

### GraphQL and connected clients

| Metric | Type | Labels | Meaning |
|---|---|---|---|
| `rim_graphql_operations_total` | counter | `operation_type`, `field`, `outcome` | Executed GraphQL requests, including errors returned with HTTP 200. |
| `rim_graphql_operation_duration_seconds` | histogram | `operation_type`, `field`, `outcome` | GraphQL request duration, separate from subscription lifetimes. |
| `rim_graphql_failed_operations_total` | counter | `kind` | Failed requests classified as `internal`, `unknown` or `client`, counted once per request. |
| `rim_websocket_connections` | gauge | | Open authenticated GraphQL WebSocket connections. |
| `rim_graphql_subscriptions` | gauge | | Active GraphQL subscription streams. |
| `rim_graphql_subscription_responses_total` | counter | `outcome` | Responses yielded by subscription streams. |
| `rim_graphql_subscription_duration_seconds` | histogram | | Subscription stream lifetime, including cancellation. |

`outcome` is `success` or `error`. `operation_type` is `query`, `mutation`, `subscription` or
`unknown`. `field` names the selected root field only for `sendMessage`, `replyToMessage`,
`forwardMessage`, `sendEncryptedMessage`, `messages`, `threadMessages` and `messageChanges`;
other fields use `other`, several selected fields use `multiple`, and unclassifiable requests
use `unknown`. Aliases and fragments do not introduce client-chosen labels. A request selecting
several fields still counts once; this is a request rate rather than a count of resolver calls.

A failed request containing an internal error is classified as `internal`; an unrecognized or
missing error code is `unknown`; known authentication, authorization, validation and business
errors are `client`. Parsing and schema validation failures are also `client`, even without an
error code. Internal takes precedence over unknown, then client. Alert on internal
and unknown failures separately from expected client refusals. Subscription response errors are
counted on the stream metrics instead of treating the socket's successful HTTP upgrade as proof
that all its operations succeed.

These metrics observe requests that reach the GraphQL schema. Transport refusals before schema
execution, such as CSRF checks or edge rate limiting, appear in HTTP and refusal metrics.

Connections are not people: one account may have several tabs or devices, and one connection
may have several subscriptions. Connection and subscription gauges return to zero when their
last live connection or stream ends, including timeout, cancellation and session revocation.

### Server-to-server and forwarded requests

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
| `rim_ip_blocked_requests_total` | counter | | Requests refused because their address is on the IP deny list (`rim_server ip block`). |
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
| `s2s_peer` | Server-to-server requests per peer (`RIM_S2S_PEER_RATE_PER_MIN`). |
| `peer_discovery` | Discovery document fetches, one per peer and sixty in all per minute. |
| `relay_forward_device`, `relay_forward_user` | Requests a local user forwards, per device per minute and per user per day. |
| `host_forward_device` | Forwarded requests run here, per remote device per minute. |
| `dm_invites` | Invites a local user sends to people on other servers per day. |
| `dm_requests` | DM requests filed here by one sender per day, or past the recipient's pending limit. |
| `stranger_claim` | Key package claims by servers with no tie to the user. |
| `delegations_per_relay` | Active delegations one relay holds on this host (`RIM_S2S_SUBSCRIPTIONS_PER_PEER`). |
| `direct_revocation` | Device revocations brought directly, per address. |
| `api_ip` | Requests to `/api` routes per client address (`RIM_RATE_REQUESTS_PER_MIN_PER_IP`). |
| `api_mutations` | Mutation fields per session, per device for requests forwarded by another server, or per address otherwise (`RIM_RATE_MUTATIONS_PER_MIN_PER_SESSION`). |
| `messages` | Messages per user (`RIM_RATE_MESSAGES_PER_MIN_PER_USER`). |
| `ws_connections` | Open WebSocket connections per user (`RIM_RATE_WS_CONNECTIONS_PER_USER`). |
| `ws_subscriptions` | Subscriptions per WebSocket connection (`RIM_RATE_SUBSCRIPTIONS_PER_CONNECTION`). |

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

`state` is `queued` for stored items eligible for delivery, `held` for items behind a DM request
the recipient has not answered. These gauges count stored rows: acknowledged or expired rows
can remain until cleanup runs. They are not exact counts of pending delivery or unread messages.
Queue byte gauges sum the bytes charged to each recipient item. Several items may share one
stored envelope, so these gauges are not physical PostgreSQL or filesystem usage.

### Pending device delivery

| Metric | Type | Labels | Meaning |
|---|---|---|---|
| `rim_relay_pending_items` | gauge | | Recipient queue rows awaiting acknowledgement by at least one eligible device. |
| `rim_relay_pending_bytes` | gauge | | Bytes of those recipient rows, counted once per row. |
| `rim_relay_pending_device_items` | gauge | | Pending item/device pairs; one row waiting for two devices counts twice. |
| `rim_relay_pending_users` | gauge | | Users with at least one pending delivery. |
| `rim_relay_pending_devices` | gauge | | Non-revoked devices with at least one pending delivery. |
| `rim_relay_oldest_pending_age_seconds` | gauge | | Age of the oldest pending recipient row; zero when there is no pending delivery. |
| `rim_relay_enqueued_items_total` | counter | | Recipient queue rows inserted by committed deliveries, including held items. |
| `rim_relay_ack_requests_total` | counter | | Successful device acknowledgement requests, including repeated cursors. |
| `rim_relay_removed_items_total` | counter | `reason` | Actual queue rows removed as `acknowledged`, `expired` or `overflow`. |

Pending delivery uses each eligible device's acknowledgement cursor. Held DM requests, expired
items, revoked devices and items a newly added device is not eligible to receive do not count.
A device-specific item only counts against its target device. One disconnected device can keep
an item pending even after another device has acknowledged it.

Relay items include message events and other events, such as membership or notification changes.
Label these panels as relay items rather than messages. Acknowledgement means the client device
has processed an item, not that its user has read a message. Items remain subject to the relay's
retention period and byte cap; see [Relay delivery](../relay.md).

For conversations hosted here, accepted local members fetch persisted conversation history
when they reconnect. Their messages do not become relay queue rows merely because a local
device is offline. Pending relay delivery therefore does not count every message an offline
client could fetch, nor the messages its user has not read.

The acknowledgement request rate is not a delivered-item rate: one request can acknowledge
many items or repeat a previous cursor. A row removed as acknowledged has completed delivery
to all its eligible devices. Cleanup and overflow counters describe rows actually removed,
not the number of affected conversations. Other deletion paths, such as rejecting a DM request
or deleting an account, are not included in those removal counters.

### Database sampling health

| Metric | Type | Labels | Meaning |
|---|---|---|---|
| `rim_database_sample_success` | gauge | `sample` | `1` when the most recent sample succeeded, `0` when it failed or timed out. |
| `rim_database_sample_last_success_timestamp_seconds` | gauge | `sample` | Unix time of the last successful sample; zero until the first success. |
| `rim_database_sample_failures_total` | counter | `sample` | Failed or timed-out samples. |

`sample` is `usage`, `delivery`, `relay` (stored queue rows) or `migrations`.

Database-backed gauges are read when the metrics listener starts and every 30 seconds after.
A read that fails or takes over five seconds leaves its last successful values in place. Check
sampler freshness alongside these values instead of treating an unchanged gauge as proof that
collection is healthy.

### Scope and history

Metrics describe the deployment being scraped. A relay measures its local accounts and delivery
queue; message creation is measured by the workspace host that commits the message. A relay
does not learn how many messages another server has stored or whether another server's users
have read them.

Counters reset when a process restarts; use `rate()` or `increase()` over the desired interval.
Prometheus only has history from when collection started, and events between the last scrape
and a crash can be lost. These counters are operational measurements, not an accounting ledger.
Do not interpret missing metrics or stale samples as zero. Database-backed gauges describe a
shared database: scraping multiple processes using that database does not create additional
users or queue items, so do not sum their identical gauges.

### Audit retention

| Metric | Type | Labels | Meaning |
|---|---|---|---|
| `rim_audit_rows_purged_total` | counter | | Command audit rows the daily retention purge deleted (`docs/operations/audit-retention.md`). |
| `rim_audit_retention_days` | gauge | | `RIM_AUDIT_RETENTION_DAYS`, set at boot; `0` when rows are kept forever. |

### Scheduled purges

| Metric | Type | Labels | Meaning |
|---|---|---|---|
| `rim_attachments_cleaned_total` | counter | | Attachment uploads the daily cleanup marked failed because they never completed (`docs/operations/background-jobs.md`). |
| `rim_workspace_state_purged_total` | counter | | Expired workspace state entries the hourly purge deleted. |
| `rim_conversation_invites_expired_total` | counter | | Expired conversation invitations the hourly purge deleted. |

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
