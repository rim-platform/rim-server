# Track B3 — Email Delivery (implementation spec)

**Author:** iron-man · **Implementer:** hulk · **Requested by:** nick-fury
**Priority:** third in Track B. **Prereq:** `track-b-dbtests-loud-skip-fix.md` (done: db-tests panic without `DATABASE_URL`; spec deleted 2026-10-09).
**Scope:** reliable **transactional** SMTP send wired to password reset, invites, and incident
notification. **No digest engine** — that is an explicit follow-up (§9).

## Goal
Replace the log-only email stub with a real async SMTP sender (lettre, rustls), config-driven via
`RIM_SMTP_*`, with retry/backoff, kept behind a mockable trait. Wire it to the flows that need it.

## Current state
- Trait `EmailSender` — `rim_infra_services/src/email.rs:4`: a single narrow method
  `async fn send_password_reset(&self, username: &str, token: &str) -> anyhow::Result<()>`. No
  message struct, no recipient address, no URL.
- Only impl: `LogEmailSender` (`email.rs:9`), logs the token. Hard-coded at `provider.rs:145`
  (`Arc::new(crate::email::LogEmailSender)`), passed to `DefaultPasswordResetService::new`.
- Password reset consumer: `rim_infra_services/src/password_reset.rs:70` calls
  `send_password_reset(&cmd.username, &raw_token)`; send errors are swallowed (warn). No reset URL is
  built anywhere; no base-URL config exists.
- Invites: `rim_infra_services/src/auth.rs` — `create_invite` (:624), `create_instance_invite`
  (:1228) generate + return a raw token; **no email is sent**, and the command DTOs carry no
  recipient email.
- Incident escalation: **greenfield** — `DefaultIncidentService` has no notify/escalate path.
- Config: `config` crate + `dotenvy`, `RIM_`-prefixed (`rim_server/src/config.rs`); custom `Debug`
  redacts secrets (config.rs:65). Service wiring flows `app.rs` → `DefaultAppServiceProvider::new`
  (provider.rs:79).
- Stack is 100% rustls (tokio 1.52, reqwest 0.12 rustls-tls, sea-orm runtime-tokio-rustls); no
  native-tls, no lettre yet. `rim_infra_services` tokio deps **lack the `time` feature** (needed for
  `tokio::time::sleep` backoff — add it).

## §1 Dependency
Add to `rim_infra_services/Cargo.toml` (next to `reqwest`), rustls feature set to match the stack:
```toml
lettre = { version = "0.11", default-features = false, features = [
    "tokio1-rustls-tls", "smtp-transport", "builder", "pool"
] }
```
- **Verify** the exact feature names against the resolved lettre 0.11.x (feature names have shifted
  across 0.10→0.11; `tokio1-rustls-tls`, `smtp-transport`, `builder`, `pool` are the intended set —
  adjust if the crate renamed them). Do **not** enable any `native-tls`/OpenSSL feature — we stay
  pure-rustls.
- **License/maintenance:** lettre is dual MIT/Apache-2.0 and actively maintained — acceptable.
  Confirm in the DoD (a one-line `cargo tree`/crates.io check).
- Add `time` to the `rim_infra_services` tokio features:
  `tokio = { version = "1", features = [..., "time"] }`.

## §2 Widen the trait to a message transport

**Decision:** make `EmailSender` a generic transport (`send(EmailMessage)`); move per-flow content
into small template functions. This is cleaner than growing one method per flow and keeps the trait
trivially mockable. Templating is pure (no I/O) and unit-testable.

`rim_infra_services/src/email.rs` (or split into `email/mod.rs`, `email/templates.rs`,
`email/smtp.rs`, `email/log.rs`):
```rust
#[derive(Debug, Clone)]
pub struct EmailMessage {
    pub to: String,               // recipient address
    pub subject: String,
    pub body_text: String,        // required plain-text body
    pub body_html: Option<String>,// optional HTML alternative
}

#[async_trait]
pub trait EmailSender: Send + Sync {
    /// Send one message. Implementations retry transient failures internally (§4).
    /// Returns Err only after retries are exhausted or on a permanent error.
    async fn send(&self, message: EmailMessage) -> anyhow::Result<()>;
}
```
Impls:
- `LogEmailSender` — logs `to`/`subject` (and, in DEBUG builds only, a truncated body). **Never log
  full tokens/links in a way that leaks secrets to prod logs** — the current stub logs the raw token;
  the new log impl should log that a reset email *would* be sent to `<to>` with subject, not the token
  body, at info level. Keep it as the dev/unconfigured fallback.
- `SmtpEmailSender` — holds a lettre `AsyncSmtpTransport<Tokio1Executor>` (pooled) built from
  `SmtpConfig`, plus a `from` address and retry policy. `send` builds a lettre `Message`
  (`.from().to().subject()` + multipart alternative if `body_html` is set) and sends with retry (§4).

## §3 Config + wiring

Add to `RIMConfig` (`config.rs`), `Option` + accessor pattern, mapping `RIM_SMTP_*`:
- `smtp_host: Option<String>`, `smtp_port: Option<u16>` (default 587),
  `smtp_username: Option<String>`, `smtp_password: Option<String>`,
  `smtp_from: Option<String>` (e.g. `"RIM <noreply@example.com>"`),
  `smtp_tls: Option<String>` (`"starttls"` default | `"implicit"` | `"none"` for dev/localhost),
  `public_base_url: Option<String>` (e.g. `https://rim.example.com`) — required to build reset/invite
  links; without it, links can't be built (see §5).
- **Redact `smtp_password`** in the custom `Debug` impl (config.rs:65) — add it to the `[REDACTED]`
  set alongside `database_url`/`bootstrap_secret`.

Selection + wiring in `app.rs` (mirror the storage_service block ~app.rs:80):
```rust
let email_sender: Arc<dyn EmailSender> = match SmtpConfig::from_rim_config(config) {
    Some(smtp) => Arc::new(SmtpEmailSender::new(smtp)?),   // all required RIM_SMTP_* present
    None => {
        tracing::warn!("RIM_SMTP_* not configured; using LogEmailSender (emails will not be delivered)");
        Arc::new(LogEmailSender)
    }
};
```
Thread `email_sender` + `public_base_url` into `DefaultAppServiceProvider::new` (add params;
this mirrors how `storage_service` is injected) and forward to the consumers below. Do **not**
hard-code the sender in `provider.rs` anymore — remove the inline `Arc::new(LogEmailSender)` at
provider.rs:145.

`SmtpConfig::from_rim_config` returns `Some` only when host+from (the minimum) are present; missing
credentials with a present host is allowed (unauthenticated relay / localhost). Document the matrix.

## §4 Retry/backoff (greenfield — nothing to reuse)
`SmtpEmailSender::send` wraps the lettre send in a bounded retry loop:
- Attempts: 3 (configurable constant). Backoff: exponential with jitter, base ~500ms
  (`tokio::time::sleep`), cap ~5s. Total worst-case well under a request timeout.
- **Retry only transient errors** — connection/timeout/temporary SMTP 4xx. Do **not** retry permanent
  failures (5xx like invalid recipient, auth failure) — classify via lettre's error type; on
  permanent error return `Err` immediately. Document the classification.
- Log each retry at `warn` with attempt number and a redacted error; final failure at `error`.
- Keep it a small private helper (`send_with_retry`), unit-testable by injecting a fake "transport"
  behind a tiny internal fn or by making the retry loop generic over a `FnMut`-returning future — a
  pure retry-policy unit test (counting attempts on a closure that fails N times) is sufficient; a
  live SMTP test is not required (see §7).

## §5 Wire the consumers

### Password reset (exists — finish it)
`password_reset.rs`: the service already has `user_repo`. Change `request_reset` to:
1. Look up the user by username to get their **email address** (add/confirm a `find_by_username`
   returning the email; if the user/email is absent, keep the current no-op-but-Ok behavior to avoid
   user enumeration — do not error).
2. Build the reset link `{public_base_url}/reset-password/{raw_token}` — the token is a **path
   segment**, not a query param (confirmed with vision: route `/reset-password/:token`,
   `AppRouter.tsx:18`, page reads `useParams`). If `public_base_url` is unset, log a warn and fall
   back to including the bare token (dev). **Do not use a `?token=` query string** — it would fail to
   route AND defeat the page's `<meta name="referrer" content="no-referrer">`, which exists
   specifically so a path token isn't leaked via `Referer`. URL-encode the token segment defensively
   (base64url tokens are path-safe, but encode anyway).
3. Build the `EmailMessage` via a `templates::password_reset(to, link)` function and call
   `email_sender.send(msg).await`; keep the send **non-fatal** (log warn on Err, still return Ok) so
   reset requests don't leak whether an address exists and don't fail on SMTP hiccups.

The `DefaultPasswordResetService` constructor keeps taking `Arc<dyn EmailSender>` (now the widened
trait) and additionally needs `public_base_url` — add it as a constructor field.

### Invites (currently send nothing)
Two paths in `auth.rs`: `create_invite` (server) and `create_instance_invite` (instance). To send an
invite email you must add an **optional recipient email** to the commands:
- Add `recipient_email: Option<String>` to `CreateInviteCmd` and `CreateInstanceInviteCmd`, and a
  nullable `recipientEmail` (camelCase) on both GraphQL input objects (vision approved this as
  additive; raw-token return flow unchanged). When `Some`, **validate the email format server-side**
  and return `ServiceError::Validation` on a malformed address so the UI can surface it (do this
  before persisting the invite). Keep validation simple and dependency-free (a basic
  single-`@`/non-empty-local/non-empty-domain-with-dot check, or a small `once_cell` regex) — no new
  email-parsing crate.
- After persisting the invite, if `recipient_email` is `Some`, build the invite link
  `{public_base_url}/join/{raw_token}` — **path segment**, confirmed with vision (route `/join/:token`,
  `JoinPage`). There is **no `/invite` route**, and bare `/join` is open-registration only — do NOT
  point invite emails at either; always include the token segment. Same no-query-string /
  `no-referrer` rationale as password reset. Send via `templates::invite(...)`. Best-effort: log warn
  on failure, still return the invite result (the raw token is still returned in the DTO as today, so
  a copy-paste flow keeps working).
- `DefaultAuthService` must gain `email_sender: Arc<dyn EmailSender>` + `public_base_url` (thread
  through `provider.rs`). This is the main new wiring in `auth.rs`.
- Bootstrap (`auth.rs:404`) sends nothing (placeholder `@local.invalid` email) — leave it.

### Incident notification (minimal, not full escalation)
Full escalation policies / on-call chains are Track C. For B3, provide **one transactional
notification**: when an incident is **declared** (manual `start_incident` and, once B2 lands, the
alert `ingest_alert` create path), email the workspace **owner(s)** a "[SEVn] incident declared"
message with a link to the incident channel.
- Add `email_sender: Arc<dyn EmailSender>` + `public_base_url` to `DefaultIncidentService`.
- Add `templates::incident_declared(to, incident, link)`.
- Resolve owner email(s) via `server_member_repo` (owners) + `user_repo` (email). Send best-effort
  (log on failure; never fail the incident create because email failed).
- Gate the whole thing so it's a **no-op when email is unconfigured** (`LogEmailSender`) — i.e. it
  still "sends" to the log sink, which is harmless.
- Respect `notification_preference` if a relevant pref exists
  (`rim_infra_services/src/notification_preference.rs`); if there's no incident-email pref field yet,
  send to owners unconditionally and note adding a pref as a follow-up. **Do not** build a new
  preference schema in B3.

> Note: this couples lightly to B2 (the alert create path). If B2 hasn't landed when B3 is
> implemented, wire only the manual `start_incident` path; B2's `ingest_alert` reuses the same
> shared create internals, so the notification fires there automatically once B2 lands. Call this
> out so the two specs compose without rework.

## §6 Trait/mocking impact
Widening `EmailSender` changes the mock. Update the password-reset unit tests
(`password_reset.rs` test module) that currently mock `send_password_reset` to mock `send`
instead (assert a single `send` call with the expected `to`/subject, or a captured message). Any
other `EmailSender` mock in tests moves to the new method. The trait stays `#[cfg(...)]`-mockable via
mockall (a `MockEmailSender` if one is generated) or a hand fake — follow whatever the existing
password-reset tests use.

## §7 Tests
- **Template unit tests** (pure): `password_reset`, `invite`, `incident_declared` produce a message
  with the expected `to`, non-empty subject, body containing the link/token, and (if HTML) both parts.
- **Retry-policy unit test:** a closure that fails `N-1` times then succeeds → `send_with_retry`
  returns Ok after N attempts; a closure that always fails a *permanent* error → returns Err after 1
  attempt (no retries); always-transient → Err after max attempts. No live SMTP.
- **Config test:** `SmtpConfig::from_rim_config` returns `None` when host/from absent, `Some` when
  present; `Debug` output does not contain the password (assert the redaction).
- **Password-reset service test:** with a `MockEmailSender`, `request_reset` for a known user calls
  `send` once with the user's email; for an unknown user, returns Ok and does **not** send (no
  enumeration). Send failure → still Ok (non-fatal).
- **Invite service test:** `create_invite` with `recipient_email: Some` calls `send`; with `None`
  does not; failure is non-fatal and the token is still returned.
- No integration test against a real SMTP server (out of scope; would need a test MTA). If desired
  later, a `lettre` stub transport or mailpit container is the path — note as follow-up.

## §8 Docs
`docs/email.md`: the `RIM_SMTP_*` config matrix (which combos enable SMTP vs fall back to log), TLS
modes, `RIM_PUBLIC_BASE_URL` requirement for links, the three wired flows, retry policy, and the
"unconfigured → log-only, no delivery" behavior. Doc comments on the trait, `EmailMessage`,
`SmtpEmailSender`, `SmtpConfig`, templates, and `send_with_retry`. SDL delta (optional
`recipientEmail` on invite inputs) handed to nick-fury for vision.

## §9 Explicitly out of scope (follow-ups)
- **Notification digest engine** (batching, schedules) — roadmap B3 mentions it; NOT built here.
- Full incident escalation policies / on-call chains / push — Track C.
- Per-user incident-email notification preferences (send to owners for now).
- Bounce handling, DKIM/SPF setup (operator/infra concern), HTML email theming beyond a basic body.
Note each in `docs/email.md` under "Future work".

## Rollback
No schema change (unless you add a notification-pref column — you should not in B3). Reverting is a
code revert + removing the lettre dep + `RIM_SMTP_*` config. Because every send path is best-effort
and falls back to `LogEmailSender` when unconfigured, disabling email is just unsetting `RIM_SMTP_*`.
