# Release preparation, first run and side lanes (plan, 2026-10-10)

_Written 2026-10-10 after Stage 5 closed. The roadmap's cross-cutting rows "Releases and
updates", "Self-hosting and operations" and "First run and docs" are the source. What needs the
pending decision about the organisation, hosting and CI (registry name, release workflows, the
updater's endpoint and key, SDK licences) is held; everything that does not is here._

## Shape

Three lanes run at once. **D** prepares deployment without publishing anything. **F** builds the
one first-run path. **S** takes side lanes off the roadmap that touch neither pillar's critical
path. Held: hybrid post-quantum signatures (after the outside review), the PWA (after F-1),
release workflows, the registry, the SDK licences.

## Deployment preparation (rim-server unless noted)

| # | What | Size | Needs |
|---|---|---|---|
| D-1 | **Version and health.** One workspace version (`version.workspace = true` in every crate), `rim-server --version`, the version in the first log line. `GET /healthz` answers 200 with `{"status":"ok","version":...}` while the process runs. `GET /readyz` answers 200 only when the database answers, no migration is pending and the configured key files are readable, else 503 with the failing checks named. Both unauthenticated, no CSRF, no session, no CORS dependence, rate-limited like the discovery document. The Dockerfile `HEALTHCHECK` uses `/healthz`. GraphQL `serverInfo { version, apiVersion }` (append to the last `QueryGroupN`; `apiVersion` is the SDL's major version, starts at 1), no session required. Compose files use `/readyz` for the `rim` healthcheck. | M | — |
| D-2 | **Metrics.** Prometheus text format at `GET /metrics` on a second listener, `RIM_METRICS_ADDR` (unset = off; `127.0.0.1:9090` is the documented value). Through the `metrics` facade so services record without depending on an exporter. First set: HTTP requests and latency by route class and status; DB pool size and in use; relay queue items and bytes per queue state; rate-limit and proof-of-work rejections by limiter; S2S requests by peer outcome; forwarded requests; pending migrations; `rim_build_info{version}`. Names `rim_*`, units in the name, documented in `docs/operations/metrics.md`. | M | — |
| D-3 | **Compose for a normal server.** `deploy/compose.yml`: postgres, migrate, rim (role `both`), caddy, and the web client (rim-ui's Dockerfile) on the same domain with `/api`, `/.well-known` and the S2S paths proxied to rim, so one origin and no CORS. Image names are variables (`RIM_SERVER_IMAGE`, `RIM_WEB_IMAGE`) defaulting to a local build. `compose.relay.yml` becomes an override of it. `deploy/.env.example`. `docs/self-hosting.md`: first start, the recovery key, upgrades, backups (pointing at D-4), what to monitor (D-2). | M | D-1 |
| D-4 | **Backup and restore.** `deploy/backup.sh` (pg_dump custom format plus a tar of the data directory: server key, franking keys, uploads, audit log; one timestamped archive, optional age encryption) and `deploy/restore.sh` into a stopped stack. `deploy/restore-drill.sh` restores the newest backup into a throwaway stack on other ports and waits for `/readyz`. The runbook section says how often and where copies go. | S | D-3 |
| D-5 | **Upgrade guide and release checklist.** `CHANGELOG.md` (Keep a Changelog, `Unreleased` section started), `docs/operations/upgrade.md` (pull the tag, run `migrate up`, restart; rolling the binary back leaves migrations in place, so migrations stay forward-only: expand now, contract one release later), the rule in `CONTRIBUTING.md`, a `cargo test -p migration` check that every migration has a `down`, and `docs/operations/release-checklist.md` with the organisation-dependent steps as `FILL` markers. | S | D-1 |
| D-6 | **Updater, dormant** (rim-ui). `tauri-plugin-updater` and `tauri-plugin-process` wired; endpoint and public key come from `src-tauri/updater.json`, empty in the repo. With an empty key the check never runs and Settings → About says "updates are not configured in this build"; with a key it checks at launch and daily, shows the available version, downloads with progress, restarts on confirm. About shows the client version and the server's `serverInfo.version`, with a banner when the server's `apiVersion` is above what the client knows. `npm run skew:check -- <git ref>` validates every operation the client sends against the `schema.graphql` committed at that ref and lists the ones a server of that version would refuse. | M | D-1 for the banner; the rest none |
| D-7 | **Instance feature flags.** `RIM_FEATURES_DISABLED` (comma-separated, default empty, unknown name refuses to start). A `ServerFeature` enum in `rim_core` next to `WorkspaceFeature`; the first PR starts with an inventory of toggleable modules (attachments, link previews, email, custom emoji, incidents, workspace plugin registry, AI surfaces; E2EE conversations are not toggleable). Each feature is `enabled`, `disabled` or `unavailable` with a reason (SMTP missing, storage missing). One gate service combines instance and workspace flags; a workspace cannot enable what the instance disabled. Exposed in `serverInfo.features` and in the discovery descriptor. Resolvers of a disabled module answer `FEATURE_DISABLED`; the upload handler checks the same gate. The client hides surfaces of disabled or unavailable features the way it hides workspace features today, and the workspace settings page shows a disabled feature as locked by the operator. | M | D-1; Jakub's nod on the design |

## First run (rim-ui)

| # | What | Size | Needs |
|---|---|---|---|
| F-1 | **One first-run path**, desktop and web: pick a server (URL with a reachability check that names the actual problem: unreachable, wrong path, CORS refused, server too old), create a key (default) or sign in, the recovery kit with a confirm step before the shell opens, link a second device, pick a relay when the server is a relay. The web key signup screen (Stage 5 leftover) lands here. Spec: Jakub's findings from `docs/testing/manual-test-plan-2026-10-10.md`. | L | Jakub's test run |
| F-2 | **After signup**: a checklist in the shell (recovery kit confirmed, first plugin installed, drive chosen) and a banner until the kit is confirmed; it reads the same state the onboarding wrote. | M | F-1 |

## Side lanes

| # | Repo | What | Size |
|---|---|---|---|
| S-1 | rim-ui | **Message rendering**: fenced code blocks with language hint and highlighting, image and video embeds for attachments and for links the server previewed, quoting (`>` blockquotes and the reply quote). Markdown stays the format, DOMPurify the backstop, no new content model. Encrypted conversations render the same way from the decrypted text. | M |
| S-2 | rim-ui | **Keyboard polish and the Vim profile**: a plain default shortcut set that works without modes; Vim mode as an opt-in modal layer (normal and insert, `hjkl` across panes and lists, `:` opens the palette, `/` search, `gg`/`G`, `Esc` back to normal); every plugin command bindable; a shareable profile file (minimal, vim, mouse-friendly). Each surface the shell has stays reachable without a mouse; a test asserts it per route. | M |
| S-3 | rim-server | **Audit retention job**: a daily job with `RIM_AUDIT_RETENTION_DAYS` (unset = keep forever), an export to the data directory before purge, a `rim-server audit purge --dry-run` CLI, the admin audit view shows the retention. | S |
| S-4 | rim-server | **Stage 5 leftovers**: a remote member's avatar set through the forwarded `updateMyProfile`; instance invites on a pure relay (`rim-server invite create` works with `RIM_ROLE=relay`); the two-server e2e closes its pools per test so the suite fits one Postgres connection budget. | S |

## Order

Wave A, now, in parallel: D-1, D-2, D-6, S-1, S-2, S-4. Wave B: D-3 and D-4 after D-1, D-7
after D-1 and Jakub's nod, S-3. F-1 starts when Jakub's test run has produced its findings; F-2
follows it. D-5 closes the wave.

Test databases: `rim_test` D-1, `rim_test2` D-2, `rim_test3` D-3 and D-4, `rim_test4` S-4 then
S-3 and D-7. A database is reassigned only after its previous agent has reported.

## Rules for every PR

- Branch `feature/<slug>-<6 random chars>` off `master` in a worktree (`../rim-server-wt-<slug>`,
  `../rim-ui-wt-<slug>`; rim-ui worktrees run `npm ci` first). Never edit the main rim-ui
  checkout: Jakub's `tauri dev` runs there.
- One PR per row; commit messages are plain imperative sentences with a body that says why and
  no trailers of any kind.
- Implementers run every gate and paste the tails; the coordinating session reviews, fixes,
  opens and merges. CI is over quota; the local gates are the gate.
- Comments say what a thing is and does; no history, no doc references.
- Migrations are additive. A single server with no new configuration keeps working exactly as
  before: every switch here defaults to today's behaviour.
- Anything that needs the organisation, hosting or CI decision is written as a `FILL` marker or
  a variable with a local default, never guessed.

## Shipped

| # | PR | Date |
|---|---|---|
| D-1 | rim-server #98 | 2026-10-10 |
| D-2 | rim-server #94 | 2026-10-10 |
| D-6 | rim-ui #57 (+ #58, an incident mutation fix the skew check found) | 2026-10-10 |
| S-1 | rim-ui #59 | 2026-10-10 |
| S-2 | rim-ui #60 | 2026-10-10 |
| S-3 | rim-server #96 (+ #97: 72 audit events now reach the audit log) | 2026-10-10 |
| S-4 | rim-server #95 | 2026-10-10 |

D-7 now runs after the incident drop of `cleanup-2026-10-10.md`, with instance flags only.
