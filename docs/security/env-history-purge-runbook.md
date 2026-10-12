# `.env` secret exposure — remediation runbook

_Prepared 2026-07-13. The safe mitigations were done then (see below). **The history purge was
run on 2026-10-11**: every branch and tag on `origin` was rewritten with `git filter-repo` from a
mirror clone and force-pushed; no `.env` blob remains in any reachable history. Two mirror
backups from before the rewrite were kept locally and should be deleted once nothing needs them.
Still open: GitHub keeps the old commits reachable by SHA and through the early pull requests'
`refs/pull/*/head` until GitHub Support removes them, so the old values stay rotated._

_Update 2026-10-09: the server no longer reads `RIM_BOOTSTRAP_SECRET` at all (the deprecated
`bootstrap` mutation was removed; the first account comes from `rim_server bootstrap-user`), so
the leaked value cannot unlock anything. The DB credentials are still worth treating as exposed._

## What was already done this session (safe, reversible, high-value)

1. **Rotated `RIM_BOOTSTRAP_SECRET`** in the working `.env` — the leaked value is now dead.
   The old value remains in git history (and on GitHub) but no longer unlocks anything once
   the server boots with the new `.env`.
2. **`git rm --cached .env`** (staged) + confirmed `.gitignore` covers `.env` — the live
   secret will not be included in any future commit. The working `.env` stays on disk for the
   dev server and tests.

Together these are the actual mitigation: the exposed secret is invalidated and no longer
committed. The history purge below only removes the now-dead old value from history.

## Reality vs. the prior handoff (corrected)

The 2026-07 handoff note (`next-steps-2026-07.md`, since deleted) claimed `.env` was "now gitignored so it won't be
re-committed." That was inaccurate: `.env` was **still tracked and committed at HEAD** despite
`.gitignore` listing it (gitignore does not untrack an already-tracked file). It is on `origin`
across `master`, `feature/2/3/4/5`, and `feature/10-lets-start-over`. **Treat the old
`RIM_BOOTSTRAP_SECRET` and the DB credentials in `.env` as compromised** — rotation, not
history rewrite, is the real fix (done for the bootstrap secret; DB creds are local dev
`testdb` only — rotate if that container is ever exposed).

## The history purge (run when awake, coordinate with any collaborators first)

> ⚠️ This rewrites **every commit SHA** on all branches. It invalidates the SHAs referenced in
> docs/memory (`48b6eaf`, `9ea67d5`, etc.), breaks any other clone or open PR, and requires a
> force-push to shared GitHub branches. Announce it, ensure no one has unpushed work, and take
> the backup below first.

```sh
cd ~/RustroverProjects/rim-server

# 0. Full backup (mirror clone) — restore point if anything goes wrong
git clone --mirror . ../rim-server-backup-$(date +%Y%m%d).git

# 1. Install git-filter-repo (not currently installed on this box)
#    Arch:   sudo pacman -S git-filter-repo
#    or pip: pipx install git-filter-repo   (or: pip install --user git-filter-repo)

# 2. Purge .env from ALL history, all branches (filter-repo runs across all refs)
git filter-repo --path .env --invert-paths --force

# 3. filter-repo drops the 'origin' remote by design — re-add it
git remote add origin git@github.com:rim-platform/rim-server.git

# 4. Force-push the rewritten history for every branch + tags
git push origin --force --all
git push origin --force --tags
```

After the push:
- Every collaborator must re-clone (or hard-reset to the rewritten refs); old clones will not
  fast-forward.
- Update any SHA references in docs/memory that matter (they're historical notes, so mostly
  cosmetic).
- Because the secret was public before the purge, **keep the rotated `RIM_BOOTSTRAP_SECRET`** —
  do not restore the old value. GitHub may also retain the blob in cached views for a while;
  rotation is what protects you, not the scrub.

## If you'd rather not rewrite `master`

The exposure is already neutralized by the rotation + untrack. The purge is defense-in-depth
(removes a dead secret + the DB creds from history). Skipping it is a defensible risk decision
for a solo/private repo, provided the DB creds in `.env` are also rotated or the `testdb`
container is never network-exposed.
