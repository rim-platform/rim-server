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
| IP deny list | 30 seconds, first loaded at boot | none | Reads the IP blocks in force, so a block or unblock made with `rim_server ip` applies within 30 seconds, and deletes expired blocks. A read that fails keeps the list it had. |
| Workspace state expiry | hourly, first 5 minutes after boot | none | Deletes workspace state entries whose time to live ran out. Logs the count and adds it to `rim_workspace_state_purged_total`. |
| Conversation invitation expiry | hourly, first 5 minutes after boot | none | Deletes pending invitations to encrypted conversations not accepted within 30 days. Logs the count and adds it to `rim_conversation_invites_expired_total`. |
| WebSocket session recheck | `RIM_WS_SESSION_RECHECK_SECS` (default 30), first one interval after boot | `RIM_WS_SESSION_RECHECK_SECS`; 0 turns it off | Reads the sessions of this process's open WebSocket connections, one query per 500 sessions, and closes with `4401` the connections whose session is revoked, expired or gone: those ended in another process, from the command line or in the database. A read that fails is logged and closes nothing. |
| Account purge | hourly, first 5 minutes after boot | `RIM_ACCOUNT_DELETION_GRACE_DAYS` (default 14, 0 to 30) sets when a requested deletion is due | Purges the accounts whose deletion grace period ended, each in its own transaction, at most 100 a run: cancels their subscriptions at other servers, deletes what they own and leaves a tombstone row. Writes an `account.purged` audit line with the counts per account; an account whose purge fails is logged and tried again next run. `rim_server account purge [--dry-run]` runs it once. |

