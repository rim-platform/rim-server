# Upgrading a server

How a running server moves to a new release, and back. `docs/self-hosting.md`, "Updates", is the
short form for `deploy/compose.yml`; this page adds what to read first, the rollback and why it
works. The commands use the `dc` alias from `docs/self-hosting.md` (a relay: the alias in
`deploy/README.md`), from the repository root.

A release is the git tag `v<version>`. `CHANGELOG.md` lists what each release changes, and a
line starting with **Upgrade:** is something the operator does before or after the upgrade.

## Before

1. Note the version running now: `dc exec rim /app/rim-server --version`. It is also in
   `/healthz` and in the first log line.
2. Read every `CHANGELOG.md` section between that version and the new one, and do what its
   **Upgrade:** lines say.
3. Go one release at a time when you can. Skipping releases brings the schema to the newest
   one just the same, but the binary rollback below is only safe to the release right before
   the new one, and after a skip that is not the release you ran.
4. Take a backup (`docs/self-hosting.md`, "Backups"). It is the only way back further than the
   release before.

## Upgrade

```sh
deploy/backup.sh
git fetch --tags && git checkout v<version>
git -C ../rim-ui checkout <rim-ui tag>        # FILL: how a rim-ui tag is matched to a server release
dc build                                      # with published images: set the tags in deploy/.env, then dc pull
dc run --rm migrate                           # applies the new migrations, then exits
dc up -d
dc logs -f rim
```

`dc run --rm migrate` runs `/app/migration up` from the new image against the database while the
old server is still serving; that is safe because a release's migrations only add (below). `dc up
-d` alone also runs it before it starts the new server; running it first shows the migrations
succeed before anything restarts. Published images are named by **FILL** (the registry and its
image names); `RIM_SERVER_IMAGE` and `RIM_WEB_IMAGE` in `deploy/.env` point at them.

Check the new server:

```sh
dc ps                                                     # rim: healthy
dc exec rim /app/rim-server --version
dc exec rim curl -fsS http://localhost:8080/readyz; echo
```

When `/readyz` fails, `curl -s` without `-f` shows the body; a `migrations` check with the
reason `N pending` means the migrations did not run.

Without compose the order is the same: run the new build's migrations while the old server
keeps serving, `DATABASE_URL=… /app/migration up` (or `cargo run --release -p migration -- up`
from a checkout), then restart `rim-server` on the new binary.

## Rolling back

**The binary, to the release before.** Migrations stay applied; the previous release's server
runs on the newer schema. The previous image's `migrate` service does not: it refuses a database
holding migrations it does not know,

```text
Custom Error: Migration file of version '…' is missing, this migration has been applied but its file is missing
```

and `rim` waits for `migrate` to succeed. So start the old server without its dependencies:

```sh
git checkout v<previous version>
git -C ../rim-ui checkout <its rim-ui tag>
dc build                                      # with published images: the previous tags in deploy/.env, then dc pull
dc up -d --no-deps rim web
```

A relay has no web client: `dc up -d --no-deps rim`. The old server's `/readyz` passes: it
counts only the migrations it knows as pending. Until the next upgrade, a plain `dc up -d` stops
at the failing `migrate`; use `--no-deps` as above. When you upgrade again there is no migration
left to run.

**Further back.** Restore the backup taken before the upgrade (`docs/self-hosting.md`,
"Restore"). Everything written since that backup is lost.

**Never `migrate down` a server with data.** Each migration has a `down`, for development and
tests: on a real database it drops tables and columns with their rows, and some refuse to run
once there are rows to lose.

## Why migrations only go forward

A release's migrations expand the schema: new tables, new columns that are nullable or have a
default, new indexes. The previous release's server ignores what it does not know, so it keeps
working on the expanded schema, which is what makes the binary rollback and the upgrade with the
old server still running safe. Removing something takes two releases: the first stops using the
table or column, the next one's migration drops it (the contract). A rename is an expand (the new
name, filled from the old), then a contract one release later.

So the release right before always runs on the current schema. One further back may not: it
may still use what a contract dropped. A release with a contract migration says so in
`CHANGELOG.md` with an **Upgrade:** line, which tells an operator who skipped the release before
it that the backup is their only way back.
