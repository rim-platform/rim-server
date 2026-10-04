# Alert Ingestion Webhooks

RIM can turn alerts from external monitoring systems into incidents automatically. Point
Prometheus Alertmanager, Grafana, or any tool that can send a webhook at one of the three
endpoints below, using a per-workspace **ingest token** for authentication. Implementation:
Track B2 (2026-07-09).

## Endpoints

```
POST /api/hooks/alerts/generic
POST /api/hooks/alerts/alertmanager
POST /api/hooks/alerts/grafana
```

All three are machine-to-machine only: no session cookie, no CSRF token. Authentication is a
bearer ingest token, not a browser session, so CSRF protection (which defends against a
browser silently replaying a victim's cookie) doesn't apply here.

Every alert in a batch creates, updates, or resolves exactly one incident in the ingest
token's configured target channel, and the response is a `202 Accepted` with a summary:

```json
{ "received": 3, "created": 1, "updated": 1, "resolved": 1, "ignored": 0 }
```

## Authentication

Present the raw token in either header (checked in this order):

```
Authorization: Bearer rim_ingest_<...>
X-RIM-Ingest-Token: rim_ingest_<...>
```

`X-RIM-Ingest-Token` exists because some monitoring tools' webhook integrations don't let you
set a custom `Authorization` value. Missing or unrecognized/revoked tokens get `401`.

### Minting a token

Ingest tokens are created via GraphQL, by a server **owner**, targeting one channel:

```graphql
mutation {
  createIngestToken(input: {
    serverId: "<server-id>"
    conversationId: "<channel-id>"
    label: "Prometheus prod"
    severityDefault: SEV_2   # optional — see "Severity mapping" below
  }) {
    rawToken   # rim_ingest_... — shown exactly once, never retrievable again
    token { id label createdAt }
  }
}
```

List and revoke:

```graphql
query { ingestTokens(serverId: "<server-id>") { id label createdAt revokedAt lastUsedAt } }
mutation { revokeIngestToken(tokenId: "<token-id>") }
```

Revocation is permanent — there's no un-revoke; mint a new token instead. Tokens are hashed
(SHA-256, same approach as session tokens) before being stored; the raw token is never
persisted and can't be recovered after creation.

If `RIM_INGEST_TOKEN_TTL_SECONDS` is set, tokens also expire after that many seconds from
creation (checked against `createdAt`, not a separate stored expiry). Unset (the default):
tokens never expire on their own — revocation is the only way to invalidate one.

## Rate limits

Each ingest token is rate-limited independently (not by source IP — the token itself is the
trusted principal, and alert sources are often behind shared infrastructure IPs where per-IP
limiting would be either too loose or too strict). Default: 120 requests/minute per token,
configurable via `RIM_ALERT_RATE_LIMIT_PER_MIN`. Exceeding the limit returns `429` with a
`Retry-After: <seconds>` header. This is in-memory, single-node — consistent with every other
rate limiter in this codebase (Phase-1 single-node posture).

Request bodies are capped at 256 KiB by default (`RIM_ALERT_MAX_BODY_BYTES`); oversize
requests get `413`.

## Payload formats

### Generic (`/api/hooks/alerts/generic`)

RIM's own JSON format — use this for anything that doesn't have a native format below. Either
a single alert object, or a batch:

```json
{ "title": "disk full", "severity": "sev1", "summary": "root partition at 95%" }
```

```json
{
  "alerts": [
    { "title": "disk full", "status": "firing", "severity": "sev1", "fingerprint": "disk-web-1" },
    { "title": "disk full", "status": "resolved", "fingerprint": "disk-web-1" }
  ]
}
```

| Field | Required | Notes |
|---|---|---|
| `title` | yes | Becomes the incident title. |
| `status` | no | `"firing"` (default) or `"resolved"`. |
| `fingerprint` | no | Dedup key. Derived if omitted — see below. |
| `severity` | no | One of the labels in the severity table below. |
| `summary` | no | Free text; becomes the incident's `Detection` phase note. |
| `labels` | no | Arbitrary `string → string` map; used in fingerprint derivation. |

Malformed JSON or a missing `title` returns `400`.

### Alertmanager (`/api/hooks/alerts/alertmanager`)

The standard [Prometheus Alertmanager webhook receiver](https://prometheus.io/docs/alerting/latest/configuration/#webhook_config)
payload:

```json
{
  "status": "firing",
  "alerts": [
    {
      "status": "firing",
      "labels": { "alertname": "HighCPU", "severity": "critical", "instance": "web-1" },
      "annotations": { "summary": "CPU above 90%" },
      "startsAt": "2026-06-01T12:00:00Z",
      "fingerprint": "abc123"
    }
  ]
}
```

Mapping: `alerts[].fingerprint` → dedup key (derived if absent) · `labels.alertname` → title ·
`labels.severity` → severity (via the mapping table) · `annotations.summary` (falling back to
`annotations.description`) → summary · `status` → firing/resolved. One incident create/update
per element of `alerts[]`.

### Grafana (`/api/hooks/alerts/grafana`)

Grafana ≥10 unified alerting's contact-point webhook payload — Grafana reuses the Alertmanager
contact-point shape, so the mapping is identical to the Alertmanager endpoint above. If a
future Grafana version diverges from this shape, it'll need its own parser; today they share
one (only the recorded `source` differs: `"grafana"` vs `"alertmanager"`).

## Severity mapping

Alert severity labels are matched case-insensitively:

| Label(s) | RIM severity |
|---|---|
| `critical`, `crit`, `p1`, `sev1`, `page` | Sev1 |
| `high`, `error`, `p2`, `sev2` | Sev2 |
| `warning`, `warn`, `p3`, `sev3` | Sev3 |
| `info`, `low`, `p4`, `sev4` | Sev4 |

An unrecognized or missing label falls back to the ingest token's configured
`severityDefault` (set at token creation); if that's also unset, the incident is filed as
**Sev3**.

## Fingerprint & dedup semantics

The fingerprint is the dedup key: at most one **non-resolved** incident exists per
`(server, fingerprint)` at a time, enforced both in the service layer and by a partial unique
database index (`idx_incident_active_fingerprint`).

- **Alertmanager / Grafana**: the source webhook's own `fingerprint` is used directly.
- **Generic / anything omitting `fingerprint`**: derived deterministically as
  `sha256(source | title | sorted labels)`, truncated to 32 hex characters. The same alert
  (same source, title, and labels) always derives the same fingerprint, so repeat generic
  alerts without an explicit fingerprint still dedup correctly.

Dispatch per alert:

| Alert status | Existing non-resolved incident for this fingerprint? | Result |
|---|---|---|
| firing | none, and no other active incident on the target channel | **Created** — new incident |
| firing | none, but a *different* incident is already active on the target channel | **Updated** — the alert is attached as a note on the existing incident rather than creating a second one (channels have at most one active incident at a time) |
| firing | yes | **Updated** — recorded as a repeat-firing note; no duplicate incident |
| resolved | yes | **Resolved** — incident marked resolved, channel topic restored |
| resolved | none | **Ignored** — nothing to resolve (logged, not an error) |

Every create and resolve broadcasts a `WorkspaceEvent` (`IncidentDeclared` / `IncidentUpdated`)
to the target channel's members in realtime, and — because the pipeline evaluator subscribes
to the same broadcast stream — is automatically visible to command pipelines with no extra
wiring.

## Actor model

Alert ingestion has no human user behind it. Rather than inventing an unpersisted "webhook"
principal, RIM attributes every alert-created action to the ingest token's `createdBy` user
(the owner who minted the token) — validated to be a server member at token-creation time.
Incident history and audit trails read as "declared by `<owner>` via ingest token `<label>`,"
which stays honest about provenance while keeping the existing authorization model intact.
