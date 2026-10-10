# Transactional Email

RIM sends transactional email for password resets, invites, and notification digests. This
document covers the `RIM_SMTP_*` config matrix, TLS modes, the three wired flows, retry policy,
and the "unconfigured → log-only" fallback behavior. Implementation: Track B3 (2026-07-10).

## Config matrix

| `RIM_SMTP_HOST` | `RIM_SMTP_FROM` | Result |
|---|---|---|
| unset | — | `LogEmailSender` — every send is logged (`to`/`subject` only, never the body), nothing is delivered. |
| set | unset | Same as above — `RIM_SMTP_FROM` is also required. |
| set | set | `SmtpEmailSender` — real SMTP delivery via `lettre`. |

`RIM_FEATURES_DISABLED=email` overrides the matrix: every message is dropped, the digest job does
not run and `requestPasswordReset` answers `FEATURE_DISABLED`. Without SMTP, `serverInfo.features`
reports email as `UNAVAILABLE` (`docs/operations/features.md`).

Both `RIM_SMTP_HOST` and `RIM_SMTP_FROM` must be present to enable real delivery
(`SmtpConfig::from_parts`); this is the minimum needed to construct a `From:` mailbox and a
transport. Everything else is optional:

| Variable | Default | Notes |
|---|---|---|
| `RIM_SMTP_HOST` | — | Required for real delivery. |
| `RIM_SMTP_PORT` | `587` | |
| `RIM_SMTP_USERNAME` | unset | Omit for an unauthenticated relay (e.g. localhost dev SMTP). |
| `RIM_SMTP_PASSWORD` | unset | Redacted in `RIMConfig`'s `Debug` output. Only used if `RIM_SMTP_USERNAME` is also set. |
| `RIM_SMTP_FROM` | — | Required. e.g. `"RIM <noreply@example.com>"`. |
| `RIM_SMTP_TLS` | `starttls` | `starttls` \| `implicit` \| `none`. Unrecognized values fall back to `starttls`. |
| `RIM_PUBLIC_BASE_URL` | unset | e.g. `https://rim.example.com`. Required to build clickable links (see below). |

### TLS modes

- **`starttls`** (default) — connect in plaintext, then upgrade via `STARTTLS`. Standard for
  port 587.
- **`implicit`** — connection is wrapped in TLS from the first byte. Standard for port 465.
- **`none`** — no TLS at all. Only appropriate for a trusted local relay (dev, mailpit, a
  localhost container) — never use against a real mail server.

### `RIM_PUBLIC_BASE_URL` and link building

Every email that includes a link (`reset-password`, `join`, digest permalinks) needs a base
URL to build it from. If `RIM_PUBLIC_BASE_URL` is unset:
- Password reset / invite emails fall back to including the **bare token** instead of a link
  (a warning is logged). The user has to construct the URL themselves — acceptable for local
  dev, not for production.
- Notification digests are sent without links.

Links are always a **path segment**, never a `?token=` query string:
- Password reset: `{RIM_PUBLIC_BASE_URL}/reset-password/{raw_token}`
- Invite (server or instance): `{RIM_PUBLIC_BASE_URL}/join/{raw_token}`

This matters for two reasons: query strings would fail to route against the frontend's
`/reset-password/:token` and `/join/:token` routes, and the reset-password page ships a
`<meta name="referrer" content="no-referrer">` specifically so a path-segment token isn't
leaked via the `Referer` header on outbound links — a query-string token would defeat that.
The token is percent-encoded defensively before being placed in the path, even though the
`rim_rst_`/`rim_inv_`/`rim_iinv_`-prefixed base64url tokens are already path-safe.

## Wired flows

### Password reset

`request_reset` looks up the user by username, creates the reset token as before, then builds
the reset link and sends `templates::password_reset(to, link)`. **Send failure is non-fatal** —
the request still returns `Ok(())` either way, so reset requests never leak whether a username
exists and never fail because of an SMTP hiccup.

### Invites (server invites and instance invites)

Both `createInvite` and `createInstanceInvite` accept an optional `recipientEmail`. If provided:
1. The address is validated server-side (same format check used for signup emails) **before**
   the invite is persisted — a malformed address returns a validation error immediately, rather
   than silently dropping the invite email after the invite is already created.
2. After the invite is created, the invite link is built and
   `templates::invite(to, link)` is sent. **Send failure is non-fatal** — the invite is still
   created and the raw token is still returned in the response either way (the copy-paste flow
   keeps working even if email delivery fails).

If `recipientEmail` is omitted, no email is sent — the raw-token-only flow works exactly as
before Track B3.

### Notification digest (Track 1.3)

A user can ask for unread notifications by email: `digest` in their workspace notification
settings is `OFF` (default), `HOURLY` or `DAILY` (`docs/messaging.md`, Notification rules). A
periodic job (`rim_infra_services::notification::DefaultNotificationJobs`, driven from
`rim_server/src/app.rs`, every `RIM_NOTIFICATION_DIGEST_INTERVAL_SECS` seconds, default 900, minimum 60) sends one email per user per workspace when all of these hold:

- the oldest unread, un-emailed notification is at least one interval old (hourly: 60 minutes,
  daily: 24 hours);
- the previous digest in that workspace (the newest `emailed_at`), if any, is at least one
  interval old;
- the user has had no presence heartbeat (or disconnect) in the workspace for 15 minutes.

The email (template `notification_digest`) lists the count and the notifications grouped by
conversation, oldest first, at most 50 per email (the rest go out with the next digest): each
item is the first 80 characters of the message text, whitespace flattened, HTML-escaped in the
HTML part, linked to the message permalink
(`{RIM_PUBLIC_BASE_URL}/workspaces/{server}/conversations/{conversation}/messages/{message}`, with
`/threads/{thread}` for a reply; no links when the base URL is unset). The subject is only
"`N unread notification(s) in <workspace>`", so `LogEmailSender`, which logs recipient and subject,
never logs message text. After a successful send the listed notifications get `emailed_at`; a
failed send leaves them pending for the next run. Deleted messages are skipped. The job assumes it
runs on one node.

Notifications older than 90 days are deleted by an hourly retention task in the same loop.

## Retry policy

`SmtpEmailSender` retries **transient** failures internally — the caller only ever sees a final
`Ok`/`Err`:
- Up to 3 attempts total (1 initial + 2 retries), exponential backoff starting at 500ms,
  doubling each retry, capped at 5s.
- **Retried:** 4xx SMTP replies, connection/network/TLS errors — anything that isn't classified
  as permanent.
- **Not retried:** 5xx SMTP replies (invalid recipient, authentication failure) — these fail
  immediately, since retrying won't fix them.
- Each retry is logged at `warn` with the attempt number and error; the final failure (if any)
  is logged at `error`.

## Dependency

`lettre = "0.11"` (rustls-only feature set: `tokio1-rustls-tls`, `smtp-transport`, `builder`,
`pool` — no `native-tls`/OpenSSL, matching the rest of this workspace's pure-rustls stack).
Dual MIT/Apache-2.0 licensed, actively maintained (`lettre v0.11.22` as of this writing).

## Future work

Explicitly out of scope for Track B3 (tracked here so they aren't silently dropped):
- ~~**Notification digest engine**~~ — shipped in Track 1.3 (see "Notification digest" above).
- **Bounce handling, DKIM/SPF setup** — operator/infrastructure concerns, not application code.
- **Richer HTML email theming** — current HTML bodies are a basic inline-styled paragraph, not a
  branded template.
- **Live SMTP integration test** — out of scope per the implementation spec; a `lettre` stub
  transport or a mailpit container would be the path if this is ever wanted.
