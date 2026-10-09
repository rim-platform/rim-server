# Track B2 — Alert Ingestion Webhooks (implementation spec)

**Author:** iron-man · **Implementer:** hulk · **Requested by:** nick-fury
**Priority:** second in Track B (after B1). **Prereq:** `track-b-dbtests-loud-skip-fix.md` (done: db-tests panic without `DATABASE_URL`; spec deleted 2026-10-09).
**Touches incidents** — note the pre-existing red: 2 `incident_repo` db-tests currently fail on
`idx_incident_active_conversation` due to an unfinished April WIP refactor in `incident.rs`. That is
**not** caused by this work; do not fold its fix in here, but be aware your create-or-update path
interacts with that same partial-unique index (§5). Flag to nick-fury if it blocks your db-tests.

## Goal
Add token-authenticated webhook endpoints that accept alerts from external monitoring systems,
normalize them, dedupe by fingerprint, map severity, and create-or-update RIM incidents — then
broadcast a workspace event so the UI and pipelines react in realtime.

Endpoints (new non-GraphQL transport, FL2 pattern):
```
POST /api/hooks/alerts/generic
POST /api/hooks/alerts/alertmanager
POST /api/hooks/alerts/grafana
```

## Constraints
- FL2 pattern: `rim_server` handlers depend on `rim_services` **traits**, not
  `rim_infra_services` impls (CLAUDE.md FL2). No GraphQL, no session cookie, **no CSRF** (machine
  callers). Auth is a per-workspace ingest token in a header.
- Forward-only migrations; `sea migrate generate`. Entity edits hand-merged per CLAUDE.md.
- `cargo fmt`/`clippy -D warnings`/tests green; tests + docs for everything new. Typed errors.
- Tokens hashed at rest (never stored plaintext), mirroring session tokens.
- Rate-limit every endpoint (§7). Reject oversized bodies.

## Architecture overview
```
POST /api/hooks/alerts/{fmt}  (rim_server::alert_handlers)
  → verify ingest token (hash lookup)         [new IngestTokenService trait in rim_services]
  → rate-limit by token id                    [per-token limiter, app.rs-wired]
  → parse body by {fmt} → Vec<AlertEvent>     [rim_server-side parsers OR rim_services normaliser]
  → for each: IncidentService::ingest_alert() [new trait method — create-or-update-by-fingerprint]
  → each ingest emits WorkspaceEvent           [IncidentService gains Observers]
  → 202 Accepted (with per-alert outcome summary)
```

## §1 Ingest token — schema, model, hashing, service

**Decision: reuse the session-token hashing approach (SHA-256, no salt, O(1) hash lookup).** Do NOT
use Argon2 (that's for passwords). Mirror `rim_auth::session_token`:
- Generate: a `generate_ingest_token()` alongside `generate_session_token`
  (`rim_auth/src/session_token.rs`) — 32 random bytes, base64url-nopad, prefix `rim_ingest_`.
  (Or reuse `generate_invite_token_with_prefix("rim_ingest_")` from
  `rim_infra_services/src/auth.rs:381` — prefer adding a dedicated `rim_auth` fn for layer cleanliness.)
- Hash: reuse `rim_auth::session_token::hash_token` (SHA-256 hex). Store only the hash.

**Migration** `sea migrate generate create_workspace_ingest_token` — new table (mirror
`session`/`invite` shape from `m20260212_000001`):
```
workspace_ingest_token
  id                 uuid  PK
  server_id          uuid  NOT NULL  FK→server ON DELETE CASCADE
  conversation_id    uuid  NOT NULL  FK→conversation ON DELETE CASCADE   -- target channel for incidents
  created_by         uuid  NOT NULL  FK→user                             -- acting principal (see §4)
  label              varchar(128) NOT NULL                               -- human name ("Prometheus prod")
  token_hash         varchar(128) NOT NULL                               -- SHA-256 hex, UNIQUE
  severity_default   varchar(20) NULL                                    -- fallback sev if alert omits
  created_at         timestamptz NOT NULL DEFAULT now()
  revoked_at         timestamptz NULL
  last_used_at       timestamptz NULL
-- UNIQUE index on token_hash (idx_workspace_ingest_token_hash_uniq)
-- index on (server_id) for listing
```
**Entity:** hand-add `entity/src/workspace_ingest_token.rs` (generate into a temp dir via
`sea generate entity`, then merge — per CLAUDE.md; do not regenerate over the whole `entity/src`).
**Core model + repo:** `rim_core/src/ingest_token/{model.rs,repository.rs}`
(`IngestToken`, `NewIngestToken`, `IngestTokenId` via `model_id!`), repo trait: `create`,
`find_by_token_hash`, `list_for_server`, `revoke`, `touch_last_used`. **Repo impl:**
`rim_infra_db/src/sea_orm_impl/ingest_token.rs` + a `*_mapper.rs`. Add the field to `AppRepos`
(both the struct and `with_handle` in `rim_infra_services/src/repos.rs`) and to the hand-built set
in `app.rs` — actually only `with_handle` now that Track A made `app.rs` use it (verify).

**Service** `rim_services/src/ingest_token/service.rs`:
```rust
#[async_trait] pub trait IngestTokenService: Send + Sync {
    async fn create_token(&self, cmd: CreateIngestTokenCmd) -> ServiceResult<CreateIngestTokenResultDto>;
    async fn list_tokens(&self, query: ListIngestTokensQuery) -> ServiceResult<Vec<IngestTokenDto>>;
    async fn revoke_token(&self, cmd: RevokeIngestTokenCmd) -> ServiceResult<()>;
    /// Verify a presented raw token → the token record (or Unauthenticated). Hashes the input,
    /// looks up by hash, checks revoked_at. Also `touch_last_used` (best-effort).
    async fn authenticate_token(&self, raw_token: &str) -> ServiceResult<IngestTokenDto>;
}
```
- `create_token`/`list_tokens`/`revoke_token` require `actor` = server **owner** (reuse
  `ensure_server_owner`, `rim_infra_services/src/util.rs`). `create_token` validates the target
  `conversation_id` is a channel in the server and that `created_by` (= actor) is a member.
- `CreateIngestTokenResultDto { token: IngestTokenDto, raw_token: String }` — raw token returned
  **once** (like invites); never retrievable again.
- `authenticate_token` returns `Unauthenticated` on unknown/revoked. It is the webhook auth path.
- Wire the impl `DefaultIngestTokenService` in `provider.rs` + add to `AppServiceProvider` trait.

**GraphQL management** (`rim_api`): mutations `createIngestToken`, `revokeIngestToken`, query
`ingestTokens(serverId)` — owner-only, standard resolver pattern. Raw token surfaced once in the
create payload. (This is additive to the SDL; include the delta for vision.)

## §2 FL2 handlers + routing

New file `rim_server/src/alert_handlers.rs`. Register in `rim_server/src/server.rs` alongside the
file handlers (server.rs:~106):
```rust
.service(web::resource("/api/hooks/alerts/generic").route(web::post().to(alert_handlers::ingest_generic)))
.service(web::resource("/api/hooks/alerts/alertmanager").route(web::post().to(alert_handlers::ingest_alertmanager)))
.service(web::resource("/api/hooks/alerts/grafana").route(web::post().to(alert_handlers::ingest_grafana)))
```
Handlers take `web::Data` for the services they need. Expose them on `AppRuntime` (`app.rs`) and pass
via `app_data` (mirror how `storage_service` etc. are threaded, server.rs:112):
- `web::Data<Arc<dyn IngestTokenService>>`
- `web::Data<Arc<dyn IncidentService>>` (currently NOT on `AppRuntime` — add it)
- `web::Data<Arc<AlertRateLimiter>>` (§7)
- `web::Data<web::PayloadConfig>` or enforce a max body size inline.

Handler shape (all three share a helper):
```rust
pub async fn ingest_alertmanager(
    req: HttpRequest,
    body: web::Bytes,
    tokens: web::Data<Arc<dyn IngestTokenService>>,
    incidents: web::Data<Arc<dyn IncidentService>>,
    limiter: web::Data<Arc<AlertRateLimiter>>,
) -> HttpResponse {
    // 1. extract raw token from header  → 401 if missing
    // 2. tokens.authenticate_token(raw) → 401 on Unauthenticated (map via map_service_error_to_response)
    // 3. limiter.check_and_record(token.id) → 429 with Retry-After if Limited
    // 4. parse body as Alertmanager payload → Vec<AlertEvent> (400 on malformed)
    // 5. for each AlertEvent: incidents.ingest_alert(IngestAlertCmd{ token_ctx, event }) collect outcomes
    // 6. 202 Accepted { "received": N, "created": .., "updated": .., "resolved": .. }
}
```
- **Token header:** accept `Authorization: Bearer <token>` and also `X-RIM-Ingest-Token: <token>`
  (monitoring systems vary). Parse in a shared `extract_ingest_token(&req) -> Option<String>`.
- **Error → HTTP:** reuse `map_service_error_to_response` (`file_handlers.rs:14`). Add a helper for
  429 (`ServiceError::RateLimited { retry_after_seconds }` → 429 + `Retry-After` header; confirm the
  error crate already carries this — it does, `rim_services/src/lib.rs`).
- **Body limit:** cap at e.g. 256 KiB; return 413/400 on oversize. Set via `PayloadConfig` on the
  resources or check `body.len()`.
- No CSRF, no session. Document in the handler why (machine callers, token-authenticated).

## §3 Payload normalization → `AlertEvent`

Common type (in `rim_services`, so both transport and incident service share it — put in
`rim_services/src/alert/mod.rs`):
```rust
#[derive(Debug, Clone)]
pub struct AlertEvent {
    /// Stable dedup key. Callers may supply one; otherwise derive (see below).
    pub fingerprint: String,
    pub status: AlertStatus,            // Firing | Resolved
    pub title: String,                  // incident title / alertname
    pub severity: Option<IncidentSeverity>,
    pub source: AlertSource,            // Generic | Alertmanager | Grafana
    pub summary: Option<String>,        // annotation/description → incident detail / phase note
    pub started_at: Option<DateTime<Utc>>,
    pub labels: BTreeMap<String, String>,
}
pub enum AlertStatus { Firing, Resolved }
pub enum AlertSource { Generic, Alertmanager, Grafana }
```
Parsers (one per format; keep the JSON parsing in `rim_server` handlers **or** a
`rim_services::alert::normalize` module — prefer `rim_services` so it's unit-testable without actix
and reused if a CLI/other transport arrives). Each returns `ServiceResult<Vec<AlertEvent>>`:
- **Generic** (`ingest_generic`): RIM's own documented JSON — a single object or `{ "alerts": [...] }`
  where each item has `fingerprint?`, `status` (`"firing"|"resolved"`, default firing), `title`,
  `severity?` (`"sev1".."sev4"`), `summary?`, `labels?`. If `fingerprint` omitted, derive it
  (below). This is the format we control and document in `docs/alerts.md`.
- **Alertmanager** (`ingest_alertmanager`): the standard AM webhook payload — top-level
  `{ status, alerts: [{ status, labels{alertname, severity, ...}, annotations{summary,description},
  startsAt, fingerprint }] }`. Map `alerts[].fingerprint` → `fingerprint`; `labels.alertname` →
  `title`; `labels.severity` → severity via the mapping (§6); `annotations.summary||description` →
  `summary`; `status` → `AlertStatus`. Emits one `AlertEvent` per element of `alerts[]`.
- **Grafana** (`ingest_grafana`): Grafana unified-alerting webhook (contact-point payload) —
  `{ status, alerts: [{ status, labels, annotations, fingerprint, startsAt }] }` (shape close to AM;
  Grafana reuses the AM contact model). Map analogously. Confirm field names against Grafana ≥10
  unified alerting when implementing; keep the parser tolerant (missing fields → sensible defaults),
  and document the assumed schema version in `docs/alerts.md`.

**Fingerprint derivation** (when caller omits): `sha256_hex(source + "|" + title + "|" +
sorted(labels))` truncated to a readable length. Document it. Fingerprint is the dedup key (§5).

Parse failures → `ServiceError::Validation` → 400. Unknown severity label → fall back to the token's
`severity_default`, else `Sev3`.

## §4 Incident create-or-update (`IncidentService::ingest_alert`)

**Actor model decision:** the webhook has no human user. Rather than weaken
`ensure_server_member` or invent an unpersisted principal, the ingest **acts as the token's
`created_by` user** (validated to be a server member at token-creation time). This keeps auditing and
authorization intact and is honest ("declared by <owner> via ingest token <label>"). Note the
`IntegrationKind::Webhook` + `PrincipalId` approach (`rim_core/src/integration`,
`rim_core/src/principal`) as a cleaner future model, but it needs integration/principal persistence
that doesn't exist yet — out of scope; use `created_by`.

New trait method (`rim_services/src/incident/service.rs`) + command:
```rust
async fn ingest_alert(&self, cmd: IngestAlertCmd) -> ServiceResult<IngestAlertOutcome>;

pub struct IngestAlertCmd {
    pub actor: Actor,                    // = token.created_by
    pub server_id: ServerId,
    pub conversation_id: ConversationId, // = token target channel
    pub source: AlertSource,
    pub event: AlertEvent,
}
pub enum IngestAlertOutcome { Created(IncidentDto), Updated(IncidentDto), Resolved(IncidentDto), Ignored }
```
Impl in `rim_infra_services/src/incident.rs` (open a write txn, like `start_incident`):
1. `ensure_server_member(actor)` + `ensure_conversation_in_server(server_id, conversation_id)`.
2. Look up an existing **non-resolved** incident for `(server_id, alert_fingerprint)` — new repo
   method `find_by_fingerprint(server_id, fingerprint)` using the new column (§5).
3. Dispatch on `event.status`:
   - **Firing + none exists** → create (reuse the `start_incident` internals: create incident row
     with `alert_fingerprint`, `source`, severity from event, title; create `Detection` phase;
     snapshot/rewrite conversation topic). Respect the existing **one-active-incident-per-conversation**
     invariant (`idx_incident_active_conversation`): if a *different* active incident already owns
     the target conversation, do not violate the index — either attach the alert as a phase note to
     that incident or return `Ignored` with a logged warning. Decide: **attach a `Detection`/timeline
     phase note to the existing active incident and return `Updated`** (don't error, don't duplicate).
   - **Firing + exists (same fingerprint)** → `Updated`: bump `last_seen`/add a phase note; optionally
     raise severity if the new event is higher. Do not create a duplicate.
   - **Resolved + exists** → `resolve_incident` (status→Resolved, stamp `resolved_at`, restore topic)
     → `Resolved`.
   - **Resolved + none** → `Ignored` (resolve for something we never opened; log at info).
4. Emit the appropriate `WorkspaceEvent` (§8).

This reuses existing incident invariants; the only genuinely new persistence is the fingerprint/source
columns and the fingerprint lookup.

## §5 Incident schema additions

`sea migrate generate add_incident_alert_fields`:
```sql
ALTER TABLE incident ADD COLUMN alert_fingerprint varchar(128) NULL;
ALTER TABLE incident ADD COLUMN source            varchar(32)  NOT NULL DEFAULT 'manual';
-- dedup: at most one non-resolved incident per (server, fingerprint)
CREATE UNIQUE INDEX idx_incident_active_fingerprint
  ON incident (server_id, alert_fingerprint)
  WHERE alert_fingerprint IS NOT NULL AND status <> 'resolved';
```
- `source`: `'manual' | 'generic' | 'alertmanager' | 'grafana'`. Default `'manual'` so existing rows
  and human-declared incidents are correct without backfill.
- The partial unique index enforces dedup at the DB level (belt to the service's `find_by_fingerprint`
  suspenders) and mirrors the existing `idx_incident_active_conversation` partial-unique pattern.
  Handle its unique violation as "already exists → treat as Updated" (map the DB conflict, don't 500).
- **Entity/model/DTO/repo updates (hand-merged):** add `alert_fingerprint: Option<String>` and
  `source: String` to `entity/src/incident.rs`, the `incident_mapper.rs`, `rim_core` `Incident`/
  `NewIncident`, `IncidentDto`, and the repository (`find_by_fingerprint`, and `create`/`NewIncident`
  to carry the two fields). This is the bulk of the mechanical work — enumerate each file in the DoD.
- **Interaction with the pre-existing red:** the 2 failing `incident_repo` db-tests are about
  `idx_incident_active_conversation` from unfinished April WIP. Your new index is independent; if the
  WIP breakage blocks a clean db-test run, report to nick-fury rather than fixing the WIP here.

## §6 Severity mapping
Central function `rim_services::alert::map_severity(label: &str) -> Option<IncidentSeverity>`:
- `critical`/`crit`/`p1`/`sev1`/`page` → `Sev1`
- `high`/`error`/`p2`/`sev2` → `Sev2`
- `warning`/`warn`/`p3`/`sev3` → `Sev3`
- `info`/`low`/`p4`/`sev4` → `Sev4`
- unknown → `None` (caller falls back to token `severity_default`, then `Sev3`).
Case-insensitive. Unit-test the table. Document it in `docs/alerts.md` so operators can label alerts
to hit the tier they want.

## §7 Rate limiting
Reuse the existing pattern (`rim_infra_services/src/registration_rate_limiter.rs` —
`RegistrationRateLimiter`, public, DashMap sliding-window, bounded buckets). Add an
`AlertRateLimiter` keyed by **ingest-token id** (not IP — a token is the trusted principal):
- Either instantiate `RegistrationRateLimiter::new(window_secs, limit)` and key on `token.id.to_string()`
  (pass a constant "ingest" as the ip/first key), or add a thin `AlertRateLimiter` wrapper if the
  two-key API is awkward. Prefer the wrapper for a clean call `limiter.check_and_record(token_id, now)`.
- Defaults: e.g. 120 requests/min per token (configurable — see §9). On `Limited` return HTTP 429 +
  `Retry-After: <secs>`.
- Construct at boot in `app.rs`, store on `AppRuntime`, pass as `web::Data<Arc<AlertRateLimiter>>`.
- This is in-memory/single-node (consistent with the other limiters and the documented Phase-1
  single-node posture). Note it.

## §8 WorkspaceEvent broadcast
Add incident variants to `WorkspaceEvent` (`rim_services/src/events/types.rs`):
```rust
IncidentDeclared { workspace_id: ServerId, conversation_id: ConversationId, incident_id: IncidentId,
                   severity: Option<String>, title: String, member_user_ids: Vec<UserId> },
IncidentUpdated  { workspace_id: ServerId, conversation_id: ConversationId, incident_id: IncidentId,
                   status: String, member_user_ids: Vec<UserId> },
```
- The exhaustive matches in `workspace_id()` and `member_user_ids()` (types.rs ~227/254) are
  compiler-enforced — add arms for the new variants. `member_user_ids` = the target conversation's
  members (for visibility filtering in the subscription layer), fetched in the service before emit.
- **`DefaultIncidentService` currently holds no `Observers`.** Add `observers: Observers` to the
  service (thread it through `IncidentRepos`/constructor and `provider.rs:260`, mirroring how
  `DefaultMessageService` gets its `Observers`). Emit after commit in `ingest_alert` (and it's fine
  if `start_incident`/`resolve_incident` also start emitting — but scope emission to the ingest path
  if you want to keep the diff minimal; declaring the event on all incident creates is the more
  consistent choice — recommend emitting from the shared create/resolve internals so manual incidents
  broadcast too).
- The existing pipeline evaluator background task (`app.rs:122`) subscribes to the broadcaster, so
  incident events will reach pipelines automatically. Guard against loops is already handled
  (`is_pipeline_event`). Note this so hulk knows incident→pipeline is now wired for free.
- Add GraphQL subscription exposure only if vision needs it now; otherwise the event flowing to
  existing `workspaceEvents` subscribers is enough. Coordinate the new event names with vision.

## §9 Config
Add to `RIMConfig` (`rim_server/src/config.rs`, mirror the `Option<T>` + accessor pattern):
- `alert_rate_limit_per_min: Option<u32>` (default 120), `alert_max_body_bytes: Option<usize>`
  (default 262144), `ingest_token_ttl_seconds: Option<u64>` (optional expiry; if unset, tokens don't
  expire — revocation only). No secrets here, so no `Debug` redaction needed.

## §10 Tests
- **Parser unit tests** (`rim_services::alert`, no actix/DB): generic single + batch, Alertmanager
  sample payload → N events with correct fields, Grafana sample payload, malformed → Validation,
  fingerprint derivation deterministic, severity mapping table.
- **Incident service db-tests** (`infra-db-tests`, loud `db_conn()`): firing→create; second identical
  firing (same fingerprint)→update, no duplicate; resolved→resolve; resolved-with-no-open→Ignored;
  a second *different* active incident on the same conversation is handled without violating
  `idx_incident_active_conversation`; the `idx_incident_active_fingerprint` unique conflict is mapped
  to Updated, not a 500.
- **Ingest token db-tests:** create→hash stored (not plaintext), `authenticate_token` roundtrip,
  revoked token → Unauthenticated, unknown token → Unauthenticated, owner-only create/revoke.
- **Handler tests** (`rim_api`/`rim_server` e2e or actix `test::TestRequest`): 401 no/invalid token,
  429 over rate limit (+ `Retry-After`), 400 malformed body, 413/400 oversize, 202 happy path with
  outcome summary; and a full path test: POST alertmanager firing → an incident exists in the target
  channel; POST resolved → incident resolved.
- Add the new tables to the db-tests `reset_db`/TRUNCATE set (`workspace_ingest_token`; `incident`
  already covered).

## §11 Docs
`docs/alerts.md`: the three endpoints, auth header formats, the generic JSON schema, the Alertmanager
and Grafana mappings, severity label table, fingerprint/dedup semantics, rate limits, and how to mint
an ingest token (owner via GraphQL). Doc comments on every new trait/method/struct. SDL delta
(ingest-token mutations/query + new WorkspaceEvent variants) handed to nick-fury for vision.

## Rollback
Forward-only. `down` migrations drop the new incident columns/index and the `workspace_ingest_token`
table. Handlers/services/events revert as code. Dropping `source`/`alert_fingerprint` loses alert
provenance on existing incidents (acceptable on rollback). The new WorkspaceEvent variants are
additive; removing them is a code revert (coordinate with vision if the frontend started consuming them).
