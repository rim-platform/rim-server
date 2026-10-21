# Audit retention

rim-server keeps two audit records, and retention treats them differently:

| Record | Where | Retention |
|---|---|---|
| Command audit log | the `command_audit_log` table, read by the admin audit view (`listAuditLog`) | `RIM_AUDIT_RETENTION_DAYS`, this page |
| Audit files | `audit.<YYYY-MM-DD>.log` in `RIM_AUDIT_LOG_PATH` (and every audit event on stdout) | `RIM_AUDIT_LOG_RETENTION_DAYS`, default 90; "The audit files", below |

With `RIM_AUDIT_RETENTION_DAYS` unset the table keeps every row forever. The public relay's
overlay, `deploy/compose.relay.yml`, sets it to 90.

The admin audit view shows the retention in force. It reads `serverInfo.auditRetentionDays`,
readable without a session: the configured days, or null when rows are kept forever.

## Turning it on

| Variable | Meaning |
|---|---|
| `RIM_AUDIT_RETENTION_DAYS` | Days a command audit row is kept, 1 to 3650. Unset (the default) keeps rows forever. A value outside the range, or one that is not a whole number, stops the server at boot. |

With a retention set, the server purges rows older than that many days a minute after it starts
and then every 24 hours. A purge runs in two steps:

1. **Export.** Every row older than the cutoff is written, oldest first, to
   `<RIM_DATA_DIR>/audit-exports/audit-<oldest>-<newest>.jsonl.gz`, where the two timestamps
   are the `created_at` of the first and last row (`20260712T030000Z`). The file is gzipped
   JSON Lines, one row per line. It is written under a hidden `.partial` name, synced to disk and
   only then renamed; a name already taken gets a `-2`, `-3`, … suffix.
2. **Delete.** Only then are the exported rows deleted, 5000 per statement, so a large table
   never holds one long lock. Rows newer than the last exported row are never deleted, even if
   they appear below the cutoff during the run.

If the export fails (the directory cannot be created, the disk is full, the database stops
answering while the rows are read), nothing is deleted and the server logs a warning; the next
run tries again. If a delete fails halfway, the rows deleted so far are gone and are in the
file; the rest stay, and the next run exports them again into a new file, so the same row can
appear in two export files.

Each line holds the fields of the admin audit view, plus the parent macro row:

```json
{"id":"0192…","actorUserId":"0192…","serverId":"0192…","commandId":"/topic","initiator":"palette","argsRedacted":{"topic":"[redacted]"},"sensitivity":"low","dangerous":false,"confirmationKind":"none","typedVerb":"write","outcome":"success","errorKind":null,"capabilityCheckOutcome":null,"confirmationOutcome":null,"parentAuditId":null,"createdAt":"2026-07-12T03:00:00.123456+00:00"}
```

`typedVerb` and `outcome` are the stored lowercase values (`delete`, `permission_denied`), not
the GraphQL enum names.

Read one with:

```sh
zcat audit-20260712T030000Z-20260713T025959Z.jsonl.gz | jq .
```

The server never deletes an export. They hold user and workspace ids and the redacted command
arguments: keep them where the backups go, encrypted, and delete them when your own retention
for archived audit data runs out (**FILL**: how long, and where they are copied).

Each run is on record itself: an audit event `audit.retention.purged` (`target` `audit`) with
`trigger` (`job` or `cli`), `cutoff`, `exported`, `deleted` and `export_path` (empty when nothing
was old enough). A purge that stops logs `audit.retention.failed` with the `error` instead.

More than one server process on the same database each runs the job; the second run of a day
finds nothing left to purge. `docs/operations/background-jobs.md` lists the server's other
scheduled jobs.

## The command

The same purge, by hand:

```text
rim_server audit purge [--dry-run] [--older-than-days <1-3650>]
```

Without `--older-than-days` it uses `RIM_AUDIT_RETENTION_DAYS` and refuses to run when that is
unset. `--dry-run` deletes and writes nothing; it prints how many rows would go and their date
range. In the relay deployment:

```sh
rim-relay audit purge --dry-run
# would purge 1832 audit rows older than 2026-07-12T09:14:03+00:00 (90 days), from 2026-04-01T08:00:12+00:00 to 2026-07-12T09:10:44+00:00
rim-relay audit purge
# purged 1832 audit rows older than 2026-07-12T09:14:05+00:00 (90 days), from … to …
# exported 1832 rows to /data/audit-exports/audit-20260401T080012Z-20260712T091044Z.jsonl.gz
rim-relay audit purge --older-than-days 365 --dry-run
```

Exit code 0 on success, 1 on an error (no retention configured, the database unreachable, a
failed export), 2 on a usage error.

## Metrics

| Metric | Type | Meaning |
|---|---|---|
| `rim_audit_rows_purged_total` | counter | Audit rows the purge deleted. |
| `rim_audit_retention_days` | gauge | `RIM_AUDIT_RETENTION_DAYS`, set at boot; `0` when rows are kept forever. |

Both need `RIM_METRICS_ADDR` (`docs/operations/metrics.md`). A purge run from the command line
is a separate process and is not counted.

## The audit files

The server writes the audit events (`"target":"audit"`) to one file per UTC day,
`audit.<YYYY-MM-DD>.log`, in the directory `RIM_AUDIT_LOG_PATH` names (a file path there means
its directory; unset, the working directory). Every audit event is on stdout as well.

| Variable | Meaning |
|---|---|
| `RIM_AUDIT_LOG_RETENTION_DAYS` | Days the audit files are kept, 1 to 3650, default 90. A value outside the range, or one that is not a whole number, stops the server at boot. |

A minute after it starts and then every 24 hours, the server deletes the audit files whose day
is more than that many days back: a line is deleted between that many days and two days more
after it was written.
Only names of that form are touched: an `audit.log` left by an older server is not written to
any more and is never deleted; delete it once its retention for you has run out. The audit
files hold IP addresses: the signup, login and rate-limit events carry the client's
`remote_ip`.

In the compose deployments the files are in `/data`, the `rim_data` volume
(`rim-relay_rim_data` on a relay), and so in every backup.
