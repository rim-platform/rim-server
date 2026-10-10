# Background jobs

rim-server runs these jobs inside its own process on a timer; there is no cron to set up. Each
starts with the server and stops when it shuts down. A run that fails logs an error and the next
run tries again. More than one server process on the same database each runs every job; the
purges are idempotent, so a second run finds nothing left to do.

| Job | Interval | Setting | What it does |
|---|---|---|---|
| Presence and typing | 30 seconds | none | Moves users without a heartbeat to away after 90 seconds and offline after 180, and drops stale typing indicators. |
| Pipeline evaluator | on every event | none | Runs the workspace pipelines that match each event. |
| Notification digest | 15 minutes | `RIM_NOTIFICATION_DIGEST_INTERVAL_SECS` (at least 60) | Emails due notification digests. Off when the email feature is disabled. |
| Notification and MLS retention | hourly | none | Deletes notifications older than 90 days and the MLS rows the encryption retention rules let go. |
| Server-to-server upkeep | every minute | `RIM_RELAY_RETENTION_DAYS` (default 30) for the relay queue | Deletes expired server-to-server nonces, relay queue items and DM requests past retention, outbox rows past their retention, old seen-request records and ended delegations; picks up a server key or franking key rotated by the CLI. |
| Revocation fan-out | 5 seconds | runs while server-to-server traffic is on | Sends device revocations due for another attempt to the hosts holding the person. |
| Outbox writer and sender | on every event | runs while server-to-server traffic is on | Turns events into signed envelopes and delivers each relay's outbox in order. |
| Metrics sampler | 30 seconds | runs when `RIM_METRICS_ADDR` is set | Reads the relay queue, the database pool and pending migrations into gauges (`docs/operations/metrics.md`). |
| Audit retention | daily, first a minute after boot | `RIM_AUDIT_RETENTION_DAYS`; unset, the job does not run | Exports and deletes command audit rows past retention (`docs/operations/audit-retention.md`). |
| Stale attachment cleanup | daily, first 5 minutes after boot | none | Marks attachment uploads still pending 15 minutes after they started as failed, so they stop counting as in progress. Logs the count and adds it to `rim_attachments_cleaned_total`. |
| Workspace state expiry | hourly, first 5 minutes after boot | none | Deletes workspace state entries whose time to live ran out. Logs the count and adds it to `rim_workspace_state_purged_total`. |

