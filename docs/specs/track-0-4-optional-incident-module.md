# Track 0.4 — Per-workspace feature flags; the incident module becomes optional

_Spec written 2026-10-04 by the coordinator. Implements `ROADMAP.md` Track 0.4. Status: ready
to implement. Decision 4 in the roadmap ("incidents off by default for new workspaces") is
assumed as recommended; it is a one-line constant to flip._

## Goal

A workspace that does not run services must never see the incident and on-call layer: no
`incident.*` / `oncall.*` commands in the palette, keybind picker, context menu or AI surface;
no incident banner, timeline, history, on-call or escalation UI; alert webhooks refused. The
mechanism is a general per-workspace feature flag table so later optional modules (and the
Track 2.4 workspace plugin registry) reuse it. Only one feature exists today: `incidents`,
which covers incidents, on-call schedules, escalation policies and alert ingestion together.

## Non-goals

- No data is deleted when a feature is turned off; turning it back on restores everything.
- No per-channel or per-user granularity.
- No change to the incident code paths themselves beyond the gate.
- No plugin-based incidents (that is the Track 2.6 spike).

## Server (rim-server)

### Domain (`rim_core`)

- `rim_core/src/workspace_feature/model.rs`: `pub enum WorkspaceFeature { Incidents }` with
  `as_str()` → `"incidents"` and `FromStr` (mirror `ServerMemberRole`); `pub struct
  WorkspaceFeatureState { server_id: ServerId, feature: WorkspaceFeature, enabled: bool,
  updated_by: Option<UserId>, updated_at: DateTime<Utc> }`.
- Repository trait `WorkspaceFeatureRepository` (mockable, `#[async_trait]` above `automock`):
  `list_for_server(ServerId) -> Vec<WorkspaceFeatureState>`, `set(server_id, feature, enabled,
  updated_by) -> WorkspaceFeatureState` (upsert), `is_enabled(server_id, feature) -> bool`.

### Persistence (`migration`, `entity`, `rim_infra_db`)

- Migration via `sea migrate generate add_workspace_feature`: table `workspace_feature`
  (`server_id uuid FK server ON DELETE CASCADE`, `feature text`, `enabled bool not null`,
  `updated_by uuid null FK user ON DELETE SET NULL`, `updated_at timestamptz not null`),
  primary key `(server_id, feature)`. Data step: insert `('incidents', true)` for every existing
  server so nothing regresses for current deployments. `down()` drops the table.
- Entity generated to a temp dir and merged by hand per CLAUDE.md; mapper in the usual
  `*_mapper.rs` style. SeaORM repo impl in `rim_infra_db/src/sea_orm_impl/workspace_feature.rs`,
  registered in `AppRepos::with_handle`. Run `cargo test -p migration` (registration guard).
- Defaults: a missing row means the feature's default. Default for `Incidents` is `false`
  (const `DEFAULT_ENABLED` on the enum; the migration's data step is what keeps existing
  workspaces on).

### Services (`rim_services`, `rim_infra_services`)

- New `WorkspaceFeatureService` trait: `list(actor, server_id) -> Vec<WorkspaceFeatureDto>`
  (any member), `set(actor, SetWorkspaceFeatureCmd { server_id, feature, enabled }) ->
  WorkspaceFeatureDto` (Owner only; Moderator gets `Forbidden`), and `is_enabled(server_id,
  feature) -> bool` for internal gating (no actor; callers already authorized). Add it to
  `AppServiceProvider` and `DefaultAppServiceProvider`, plus the mocked test provider in
  `rim_api/src/tests.rs`.
- New `ServiceError::FeatureDisabled { feature: String }` mapped to GraphQL extension code
  `FEATURE_DISABLED` in `rim_api/src/errors.rs` (follow the existing code table and tests).
- `CommandDefinitionDto` gains `pub feature: Option<WorkspaceFeatureDto>` (default `None`).
  Set `Some(Incidents)` on `incident.ack`, `incident.escalate`, `oncall.who`,
  `oncall.take_shift`. Update every struct-literal construction site (the compiler finds them;
  tests and user-command derivation set `None`).
- `DefaultCommandService`:
  - `list_commands`: after assembling builtins, when `context.workspace_id` is `Some`, drop
    commands whose `feature` is disabled for that workspace (one `list_for_server` call, not one
    per command). Without a workspace context, feature-gated builtins are hidden (safe default:
    a command that needs a workspace to run is useless without one).
  - `execute_command`: before dispatch (after capability/confirmation gates, next to the
    rate-limit gate so the audit line still fires) reject with `FeatureDisabled` when the
    definition has a feature that is disabled for `context.workspace_id`; if the command has a
    feature and no workspace context is given, also reject.
  - `command_discover`: goes through `list_commands`, so it is filtered automatically; add a
    test proving it.
  - Macros/pipelines that call a gated command fail at execution with `FeatureDisabled`; no
    special handling, but one test.
- Incident, on-call, escalation and ingest-token services: add the gate at the service
  method entry for every mutation and query that takes a `server_id` (start incident, create
  schedule/policy/token, list schedules, and so on). Read paths may stay open if the data is
  needed to render history after turning the feature off; the spec choice is: **reads of
  existing data stay allowed, writes are gated.** `active_incident` for a conversation returns
  `None` when disabled so banners disappear.
- Alert webhooks (`rim_server/src/alert_handlers.rs`): resolve the ingest token to its
  workspace as today, then return `404 Not Found` with no body detail if the feature is
  disabled (do not reveal that the token is valid). Rate limiter still applies first.

### GraphQL (`rim_api`)

- Enum `WorkspaceFeature { INCIDENTS }`, object `WorkspaceFeatureState { feature, enabled,
  updatedAt }`.
- `Server` object gains `features: [WorkspaceFeatureState!]!` (resolved via a DataLoader if
  `myServers` lists many servers; a simple per-server call is acceptable if a loader is
  disproportionate, say which in the PR).
- Query `workspaceFeatures(serverId: ID!): [WorkspaceFeatureState!]!` (member).
- Mutation `setWorkspaceFeature(serverId: ID!, feature: WorkspaceFeature!, enabled: Boolean!):
  WorkspaceFeatureState!` (Owner; CSRF applies like every mutation). Emits an audit log entry
  through the existing audit path with the actor, workspace, feature and new value.
- `Command` GraphQL type gains `feature: WorkspaceFeature` (nullable).
- SDL changes → the frontend must re-export `schema.graphql` and run codegen.

### Tests (server)

- Unit: service authorization (Owner ok, Moderator forbidden, non-member not found), default
  resolution with no row, list filtering in `list_commands`, execution rejection, discover
  filtering, macro step rejection.
- `infra-db-tests`: repo upsert and list; migration data step enables incidents for a
  pre-existing server.
- `e2e-tests`: mutation + query round trip; `incident.ack` via `executeCommand` returns
  `FEATURE_DISABLED` when off and works when on; `commands` query omits incident commands when
  off; alert webhook returns 404 when off.
- Update `rim_infra_services/tests/ai_callable_inventory_test.rs` if it asserts on the
  inventory shape, and `docs/command_model.md` (new `feature` field), `docs/oncall.md`,
  `docs/alerts.md` (gate), `docs/capability_security_model.md` if it lists gates in order.

## Client (rim-ui)

- Codegen first (`schema.graphql`, `npm run codegen`).
- `src/features/workspaces/api/useWorkspaceFeatures.ts`: query hook keyed under the workspace
  query keys; `useSetWorkspaceFeature` mutation invalidating it and `myServers`. Live update on
  the `ConversationUpdated`-style events is not required; a manual refetch after the mutation
  is enough.
- A small `useFeatureEnabled(feature)` selector hook for the active workspace. Default to
  `false` while loading so gated UI never flashes in.
- Gate with it: `SideRail` incident history button; `ConversationHeader` incident banner,
  start-incident affordance and `useActiveIncident` (skip the query when disabled);
  `AppShell` incident timeline, postmortem preview, incidents main surface, start-incident
  dialog, on-call override modal; on-call and escalation pages/panels; the alert ingest token
  section in Integrations settings; the "Incident management" copy in `SettingsPage`. Commands
  are filtered by the server, but the client keybind picker and any hardcoded command lists
  must not reference gated ids when the feature is off (grep for `incident.` and `oncall.`).
- Settings → a new **Workspace** tab (Owner only; hide for others) listing features with a
  toggle: "Incidents and on-call. Incident timelines, on-call schedules, escalation policies
  and alert webhooks." Use `Dialog`/`Button` from `src/shared/ui`, design tokens only.
- Tests (Vitest): hook default-false behaviour; `SideRail` hides the incident button when
  off; `ConversationHeader` renders no banner when off; the Workspace settings tab toggles
  and calls the mutation; snapshot-free, behaviour assertions only.

## Definition of done

Server: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`
(plus the feature-gated clippy line from CI), `cargo test --workspace`, `cargo test -p
migration`, and with `DATABASE_URL` the `db-tests`, `infra-db-tests` and `e2e-tests` suites
single-threaded. Client: `npm run lint` (0 errors), `npm run format:check`, `npx tsc
--noEmit`, `npx vitest run`, `npm run build`. Docs updated as listed. Two PRs (server first,
client second, the client PR references the server SDL commit).
