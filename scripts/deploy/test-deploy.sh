#!/usr/bin/env bash
# Tests deploy/upgrade.sh, deploy/rim-deploy.sh and the pruning of deploy/backup.sh with stub
# docker, cosign, curl and sleep commands in PATH, on throwaway deployment directories.
#
#   scripts/deploy/test-deploy.sh
#
# Needs bash, awk, tar, flock and sha256sum; works offline. Exits non-zero on the first failure.
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

# --- Stubs ---------------------------------------------------------------------------------
#
# Every call is appended to $work/calls. The STUB_* variables steer what the stubs answer.

mkdir "$work/bin"
cat >"$work/bin/docker" <<'STUB'
#!/usr/bin/env bash
printf 'docker %s\n' "$*" >>"$STUB_CALLS"
env_file=""
case $1 in
manifest)
  for image in $STUB_UNPUBLISHED; do
    [[ $3 != "$image" ]] || exit 1
  done
  exit 0
  ;;
volume)
  [[ -z ${STUB_VOLUME:-} ]] || printf '%s\n' "$STUB_VOLUME"
  exit 0
  ;;
compose) shift ;;
*) exit 0 ;;
esac
while [[ $1 == -* ]]; do
  [[ $1 != --env-file ]] || env_file=$2
  shift 2
done
version=$(sed -n 's/^RIM_VERSION=//p' "$env_file" | tail -n 1)
# The web client's version as compose picks it: RIM_WEB_IMAGE's tag, RIM_WEB_VERSION, RIM_VERSION.
web_version=$(sed -n 's/^RIM_WEB_IMAGE=.*://p' "$env_file" | tail -n 1)
[[ -n $web_version ]] || web_version=$(sed -n 's/^RIM_WEB_VERSION=//p' "$env_file" | tail -n 1)
[[ -n $web_version ]] || web_version=$version
case "$*" in
"config --services")
  printf '%s\n' postgres migrate rim caddy-init caddy
  [[ ${STUB_IMAGECHECK:-0} != 1 ]] || printf 'imagecheck\n'
  [[ ${STUB_WEB:-0} != 1 ]] || printf 'web\n'
  ;;
config) printf 'name: rim\nservices: {}\n' ;;
*/readyz) [[ $version != "${STUB_BAD_VERSION:-}" ]] && printf '{"status":"ok"}\n' ;;
*/healthz) printf '{"status":"ok","version":"%s"}\n' "$version" ;;
"ps --status running -q caddy") printf 'c0ffee\n' ;;
"ps --status running -q postgres") [[ ${STUB_POSTGRES:-0} != 1 ]] || printf 'c0ffee\n' ;;
"ps --status running -q "*) ;;
"ps --format "*" web")
  if [[ $web_version == "${STUB_BAD_WEB:-}" ]]; then
    printf 'running unhealthy\n'
  else
    printf 'running healthy\n'
  fi
  ;;
*) [[ ${STUB_FAIL:-} != "${1:-}" ]] ;;
esac
STUB
cat >"$work/bin/cosign" <<'STUB'
#!/usr/bin/env bash
printf 'cosign %s\n' "$*" >>"$STUB_CALLS"
for image in $STUB_UNSIGNED; do
  [[ $2 != "$image" ]] || exit 1
done
STUB
cat >"$work/bin/curl" <<'STUB'
#!/usr/bin/env bash
printf 'curl %s\n' "$*" >>"$STUB_CALLS"
out="" write=""
for arg in "$@"; do
  case $prev in
  -o) out=$arg ;;
  -w) write=$arg ;;
  esac
  prev=$arg
done
case $arg in
*/rim-platform/rim-ui/releases/latest)
  cp "$STUB_WEB_RELEASE" "$out"
  printf '%s' "${STUB_WEB_RELEASE_STATUS:-200}"
  ;;
*/releases/latest)
  cp "$STUB_RELEASE" "$out"
  printf '%s' "${STUB_RELEASE_STATUS:-200}"
  ;;
*.tar.gz) cp "$STUB_TARBALL" "$out" ;;
*/.well-known/rim-server) [[ ${STUB_PUBLIC_DOWN:-0} != 1 ]] ;;
https://hc.example.org/*) [[ ${STUB_HEARTBEAT_DOWN:-0} != 1 ]] ;;
*) exit 22 ;;
esac
STUB
printf '#!/bin/sh\n' >"$work/bin/sleep"
chmod +x "$work/bin/"*
export PATH=$work/bin:$PATH STUB_CALLS=$work/calls
export STUB_UNPUBLISHED="" STUB_UNSIGNED="" STUB_VOLUME="" STUB_IMAGECHECK=0 STUB_WEB=0
export STUB_BAD_VERSION="" STUB_FAIL="" STUB_BAD_WEB=""
export STUB_PUBLIC_DOWN=0
unset RIM_VERSION RIM_WEB_VERSION RIM_WEB_IMAGE COMPOSE_FILE

called() {
  grep -qxF -- "$1" "$work/calls"
}

not_called() {
  ! grep -qF -- "$1" "$work/calls"
}

expect_stderr() {
  grep -qF -- "$1" "$work/stderr" || fail "$2: stderr lacks '$1': $(cat "$work/stderr")"
}

# --- upgrade.sh ----------------------------------------------------------------------------

d=$work/deploy

# A deployment directory with upgrade.sh, the real backup-lib.sh, a stub backup.sh and an env
# file at RIM_VERSION $1 (none when empty).
setup_upgrade() {
  rm -rf "$d" "$work/calls"
  mkdir "$d"
  cp "$root/deploy/upgrade.sh" "$root/deploy/backup-lib.sh" "$d/"
  cat >"$d/backup.sh" <<'BACKUP'
#!/bin/sh
printf 'backup.sh\n' >>"$STUB_CALLS"
BACKUP
  chmod +x "$d/backup.sh"
  {
    printf 'POSTGRES_PASSWORD=secret\nRIM_DOMAIN=relay.example.org\n'
    [[ -z $1 ]] || printf 'RIM_VERSION=%s\n' "$1"
    printf 'RIM_LIMIT_ACCOUNTS=10\n'
  } >"$d/.env"
  : >"$work/calls"
  STUB_VOLUME=rim_rim_data STUB_IMAGECHECK=1 STUB_WEB=1 STUB_UNPUBLISHED="" STUB_UNSIGNED=""
  STUB_BAD_VERSION=""
  STUB_FAIL="" STUB_PUBLIC_DOWN=0
}

upgrade() {
  "$d/upgrade.sh" "$@" >"$work/stdout" 2>"$work/stderr"
}

env_version() {
  sed -n 's/^RIM_VERSION=//p' "$1"
}

# The order the commands that change the deployment ran in, one per line.
steps() {
  grep -E '^(backup\.sh|docker compose .* (pull.*|run --rm migrate|up -d.*|stop))$' "$work/calls" |
    sed 's/^docker compose \(-f [^ ]* \)*--env-file [^ ]* /compose /'
}

server=ghcr.io/rim-platform/rim-server
imagecheck=ghcr.io/rim-platform/rim-imagecheck
web=ghcr.io/rim-platform/rim-web

# A version that is not one is refused before anything runs.
setup_upgrade 0.1.0
if upgrade 0.2; then fail "upgrade.sh 0.2 was accepted"; fi
expect_stderr "0.2 is not a version" "bad version"
[[ ! -s $work/calls ]] || fail "bad version: commands ran: $(cat "$work/calls")"

# rim-web not published yet at the web client's version: exit 2, nothing changed.
setup_upgrade 0.1.0
printf 'RIM_WEB_VERSION=0.1.5\n' >>"$d/.env"
STUB_UNPUBLISHED=$web:0.1.5
status=0
upgrade 0.2.0 || status=$?
[[ $status == 2 ]] || fail "unpublished rim-web: exit $status, not 2"
expect_stderr "rim-web 0.1.5 is not published yet; the web client runs that rim-ui release" "unpublished rim-web"
[[ $(env_version "$d/.env") == 0.1.0 ]] || fail "unpublished rim-web: the env file changed"
[[ -z $(steps) ]] || fail "unpublished rim-web: steps ran: $(steps)"
not_called cosign || fail "unpublished rim-web: cosign ran"

# rim-server not published yet: exit 2 as well.
setup_upgrade 0.1.0
STUB_UNPUBLISHED=$server:0.2.0
status=0
upgrade 0.2.0 || status=$?
[[ $status == 2 ]] || fail "unpublished rim-server: exit $status, not 2"
expect_stderr "rim-server 0.2.0 is not published yet" "unpublished rim-server"

# rim-imagecheck not published yet: exit 2 as well.
setup_upgrade 0.1.0
STUB_UNPUBLISHED=$imagecheck:0.2.0
status=0
upgrade 0.2.0 || status=$?
[[ $status == 2 ]] || fail "unpublished rim-imagecheck: exit $status, not 2"
expect_stderr "rim-imagecheck 0.2.0 is not published yet" "unpublished rim-imagecheck"

# The files of a release without the imagecheck service need no rim-imagecheck image.
setup_upgrade 0.1.0
STUB_IMAGECHECK=0 STUB_UNPUBLISHED=$imagecheck:0.2.0
upgrade 0.2.0 || fail "release without imagecheck: $(cat "$work/stderr")"
not_called "manifest inspect $imagecheck" || fail "release without imagecheck: rim-imagecheck was looked up"

# A relay without the web client needs no rim-web image.
setup_upgrade 0.1.0
STUB_WEB=0 STUB_UNPUBLISHED=$web:0.2.0
upgrade 0.2.0 || fail "relay without web client: $(cat "$work/stderr")"
not_called "manifest inspect $web" || fail "relay without web client: rim-web was looked up"

# An image whose signature does not verify: exit 1 before the backup, nothing changed.
setup_upgrade 0.1.0
STUB_UNSIGNED=$web:0.1.0
status=0
upgrade 0.2.0 || status=$?
[[ $status == 1 ]] || fail "unsigned image: exit $status, not 1"
expect_stderr "$web:0.1.0 is not signed by the rim-ui release workflow" "unsigned image"
[[ $(env_version "$d/.env") == 0.1.0 && -z $(steps) ]] || fail "unsigned image: something changed"

# The upgrade: images checked and verified against both release workflows, rim-web at the
# version that ran before, then backup, env file, pull, migrations, start, probes, version.
setup_upgrade 0.1.0
upgrade 0.2.0 || fail "upgrade 0.1.0 -> 0.2.0: $(cat "$work/stderr")"
issuer="--certificate-oidc-issuer https://token.actions.githubusercontent.com"
called "cosign verify $server:0.2.0 --certificate-identity https://github.com/rim-platform/rim-server/.github/workflows/release.yaml@refs/tags/v0.2.0 $issuer" ||
  fail "upgrade: rim-server not verified against its release workflow"
called "cosign verify $web:0.1.0 --certificate-identity https://github.com/rim-platform/rim-ui/.github/workflows/release.yaml@refs/tags/v0.1.0 $issuer" ||
  fail "upgrade: rim-web not verified against rim-ui's release workflow at the web client's version"
not_called "$web:0.2.0" || fail "upgrade: rim-web was looked up at the server's version"
grep -qxF RIM_WEB_VERSION=0.1.0 "$d/.env" || fail "upgrade: RIM_WEB_VERSION not pinned to 0.1.0"
! grep -q '^RIM_WEB_VERSION=' "$d/.env.previous" || fail "upgrade: .env.previous has RIM_WEB_VERSION"
called "cosign verify $imagecheck:0.2.0 --certificate-identity https://github.com/rim-platform/rim-server/.github/workflows/release.yaml@refs/tags/v0.2.0 $issuer" ||
  fail "upgrade: rim-imagecheck not verified against rim-server's release workflow"
expected=$'backup.sh\ncompose pull\ncompose run --rm migrate\ncompose up -d'
[[ $(steps) == "$expected" ]] || fail "upgrade: steps were $(steps)"
[[ $(env_version "$d/.env") == 0.2.0 ]] || fail "upgrade: RIM_VERSION is $(env_version "$d/.env")"
[[ $(env_version "$d/.env.previous") == 0.1.0 ]] || fail "upgrade: .env.previous is not 0.1.0"
grep -qxF RIM_LIMIT_ACCOUNTS=10 "$d/.env" || fail "upgrade: another setting was lost"
[[ $(grep -c '^RIM_VERSION=' "$d/.env") == 1 ]] || fail "upgrade: RIM_VERSION set twice"
called "curl -fsS https://relay.example.org/.well-known/rim-server" || fail "upgrade: no public check"
called "docker compose -f $d/compose.yml --env-file $d/.env exec -T caddy caddy reload --config /etc/caddy/Caddyfile --adapter caddyfile" ||
  fail "upgrade: Caddy not reloaded"
[[ $(tail -n 1 "$work/stdout") == 0.2.0 ]] || fail "upgrade: printed $(tail -n 1 "$work/stdout")"

# COMPOSE_FILE in the env file replaces the default compose file.
setup_upgrade 0.1.0
printf 'COMPOSE_FILE=compose.yml:compose.relay.yml\n' >>"$d/.env"
upgrade 0.2.0 || fail "COMPOSE_FILE in .env: $(cat "$work/stderr")"
not_called "docker compose -f" || fail "COMPOSE_FILE in .env: -f still passed"
setup_upgrade 0.1.0
upgrade 0.2.0 || fail "no COMPOSE_FILE: $(cat "$work/stderr")"
called "docker compose -f $d/compose.yml --env-file $d/.env pull" || fail "no COMPOSE_FILE: compose.yml not passed"

# The first run: no data volume, so no backup, and RIM_VERSION is added to the env file.
setup_upgrade ""
STUB_VOLUME=""
upgrade 0.1.0 || fail "first run: $(cat "$work/stderr")"
expect_stderr "nothing to back up" "first run"
[[ $(steps) == $'compose pull\ncompose run --rm migrate\ncompose up -d' ]] || fail "first run: steps were $(steps)"
[[ $(env_version "$d/.env") == 0.1.0 ]] || fail "first run: RIM_VERSION not added"

# A dry run checks the images and changes nothing.
setup_upgrade 0.1.0
upgrade --dry-run 0.2.0 || fail "dry run: $(cat "$work/stderr")"
called "cosign verify $server:0.2.0 --certificate-identity https://github.com/rim-platform/rim-server/.github/workflows/release.yaml@refs/tags/v0.2.0 $issuer" ||
  fail "dry run: images not verified"
[[ -z $(steps) ]] || fail "dry run: steps ran: $(steps)"
[[ $(env_version "$d/.env") == 0.1.0 && ! -e $d/.env.previous ]] || fail "dry run: the env file changed"
expect_stderr "dry run: compose run --rm migrate" "dry run"

# The new version never gets ready: the env file comes back, the version before starts
# without the migrations, and the exit is 1.
setup_upgrade 0.1.0
STUB_BAD_VERSION=0.2.0
status=0
upgrade 0.2.0 || status=$?
[[ $status == 1 ]] || fail "rollback: exit $status, not 1"
expect_stderr "/readyz did not answer" "rollback"
expect_stderr "0.1.0 is running again" "rollback"
[[ $(env_version "$d/.env") == 0.1.0 ]] || fail "rollback: RIM_VERSION is $(env_version "$d/.env")"
expected=$'backup.sh\ncompose pull\ncompose run --rm migrate\ncompose up -d\ncompose up -d --no-deps rim imagecheck web'
[[ $(steps) == "$expected" ]] || fail "rollback: steps were $(steps)"
called "docker compose -f $d/compose.yml --env-file $d/.env logs --tail 40 migrate rim" || fail "rollback: no logs"

# The public origin does not answer: rolled back as well.
setup_upgrade 0.1.0
STUB_PUBLIC_DOWN=1
if upgrade 0.2.0; then fail "public origin down: the upgrade passed"; fi
expect_stderr "https://relay.example.org/.well-known/rim-server did not answer" "public origin down"
[[ $(env_version "$d/.env") == 0.1.0 ]] || fail "public origin down: not rolled back"

# The migrations fail: rolled back before anything restarted.
setup_upgrade 0.1.0
STUB_FAIL=run
if upgrade 0.2.0; then fail "failed migrations: the upgrade passed"; fi
expect_stderr "failed: the migrations" "failed migrations"
expected=$'backup.sh\ncompose pull\ncompose run --rm migrate\ncompose up -d --no-deps rim imagecheck web'
[[ $(steps) == "$expected" ]] || fail "failed migrations: steps were $(steps)"
[[ $(env_version "$d/.env") == 0.1.0 ]] || fail "failed migrations: not rolled back"

# A failed first run has no version to go back to: the stack stops.
setup_upgrade ""
STUB_VOLUME="" STUB_BAD_VERSION=0.1.0
if upgrade 0.1.0; then fail "failed first run: the upgrade passed"; fi
expect_stderr "no version ran before; stopping the stack" "failed first run"
[[ $(steps | tail -n 1) == "compose stop" ]] || fail "failed first run: steps were $(steps)"
[[ -z $(env_version "$d/.env") ]] || fail "failed first run: RIM_VERSION stayed"

# An older version is a binary rollback: no migrations, rim, imagecheck and web without
# dependencies.
setup_upgrade 0.2.0
upgrade 0.1.0 || fail "downgrade: $(cat "$work/stderr")"
expected=$'backup.sh\ncompose pull rim imagecheck web\ncompose up -d --no-deps rim imagecheck web'
[[ $(steps) == "$expected" ]] || fail "downgrade: steps were $(steps)"

# A pre-release is older than its release, and the release newer than its pre-release.
setup_upgrade 0.2.0
upgrade 0.2.0-rc.1 || fail "release -> pre-release: $(cat "$work/stderr")"
not_called "run --rm migrate" || fail "release -> pre-release: the migrations ran"
setup_upgrade 0.2.0-rc.1
upgrade 0.2.0 || fail "pre-release -> release: $(cat "$work/stderr")"
called "docker compose -f $d/compose.yml --env-file $d/.env run --rm migrate" ||
  fail "pre-release -> release: the migrations did not run"
setup_upgrade 0.10.0
upgrade 0.9.1 || fail "0.10.0 -> 0.9.1: $(cat "$work/stderr")"
not_called "run --rm migrate" || fail "0.10.0 -> 0.9.1 was not a downgrade"

# An image override would make RIM_VERSION change nothing: refused.
setup_upgrade 0.1.0
printf 'RIM_SERVER_IMAGE=ghcr.io/rim-platform/rim-server@sha256:00\n' >>"$d/.env"
if upgrade 0.2.0; then fail "image override: the upgrade passed"; fi
expect_stderr "RIM_SERVER_IMAGE is set" "image override"
setup_upgrade 0.1.0
printf 'RIM_IMAGECHECK_IMAGE=rim-imagecheck:local\n' >>"$d/.env"
if upgrade 0.2.0; then fail "image check override: the upgrade passed"; fi
expect_stderr "RIM_IMAGECHECK_IMAGE is set" "image check override"

# backup-lib.sh's wait_ready, which restore.sh uses: the answer when ready, else the logs and
# exit 1 after RIM_READY_TIMEOUT.
setup_upgrade 0.1.0
# shellcheck source=deploy/backup-lib.sh
(source "$d/backup-lib.sh" && wait_ready) >"$work/stdout" 2>"$work/stderr" || fail "wait_ready: $(cat "$work/stderr")"
[[ $(cat "$work/stdout") == '{"status":"ok"}' ]] || fail "wait_ready printed $(cat "$work/stdout")"
STUB_BAD_VERSION=0.1.0
: >"$work/calls"
# shellcheck source=deploy/backup-lib.sh
if (source "$d/backup-lib.sh" && RIM_READY_TIMEOUT=9 wait_ready) 2>"$work/stderr"; then
  fail "wait_ready passed while not ready"
fi
expect_stderr "the server was not ready within 9s" "wait_ready"
[[ $(grep -c '/readyz$' "$work/calls") == 4 ]] || fail "wait_ready: $(grep -c '/readyz$' "$work/calls") probes, not 3 and the last"

# --- backup.sh -----------------------------------------------------------------------------

# With RIM_BACKUP_KEEP_DAYS, a backup deletes this project's backups older than that many days
# and keeps newer ones, other projects' and other files.
b=$work/backup
mkdir -p "$b/backups"
cp "$root/deploy/backup.sh" "$root/deploy/backup-lib.sh" "$b/"
printf 'POSTGRES_PASSWORD=secret\nRIM_BACKUP_DIR=%s\nRIM_BACKUP_KEEP_DAYS=30\n' "$b/backups" >"$b/.env"
old_backups=(rim-backup-rim-20260801T031500Z.tar.age rim-backup-rim-20260820T031500Z.tar)
kept_backups=(rim-backup-rim-20260920T031500Z.tar rim-backup-rim-relay-20260801T031500Z.tar notes.txt)
for name in "${old_backups[@]}"; do touch -d '31 days ago' "$b/backups/$name"; done
for name in "${kept_backups[@]:1}"; do touch -d '40 days ago' "$b/backups/$name"; done
touch -d '29 days ago' "$b/backups/${kept_backups[0]}"
: >"$work/calls"
STUB_POSTGRES=1 STUB_VOLUME=rim_rim_data STUB_FAIL="" "$b/backup.sh" >"$work/stdout" 2>"$work/stderr" ||
  fail "backup with RIM_BACKUP_KEEP_DAYS: $(cat "$work/stderr")"
new_backup=$(tail -n 1 "$work/stdout")
[[ -f $new_backup ]] || fail "backup with RIM_BACKUP_KEEP_DAYS: no new archive ($new_backup)"
for name in "${old_backups[@]}"; do
  [[ ! -e $b/backups/$name ]] || fail "backup with RIM_BACKUP_KEEP_DAYS: $name was kept"
done
for name in "${kept_backups[@]}"; do
  [[ -e $b/backups/$name ]] || fail "backup with RIM_BACKUP_KEEP_DAYS: $name was deleted"
done

# A keep that is not a whole number of days stops the script before it backs anything up.
printf 'RIM_BACKUP_KEEP_DAYS=0\n' >>"$b/.env"
: >"$work/calls"
if STUB_POSTGRES=1 STUB_VOLUME=rim_rim_data "$b/backup.sh" >"$work/stdout" 2>"$work/stderr"; then
  fail "RIM_BACKUP_KEEP_DAYS=0 was accepted"
fi
expect_stderr "RIM_BACKUP_KEEP_DAYS=0 is not a whole number of days" "RIM_BACKUP_KEEP_DAYS=0"
[[ ! -s $work/calls ]] || fail "RIM_BACKUP_KEEP_DAYS=0: commands ran: $(cat "$work/calls")"

# Without RIM_BACKUP_HEARTBEAT_URL nothing is pinged; with it, a backup that succeeded pings it,
# and a ping that fails leaves the backup a success.
printf 'POSTGRES_PASSWORD=secret\nRIM_BACKUP_DIR=%s\n' "$b/backups" >"$b/.env"
: >"$work/calls"
STUB_POSTGRES=1 STUB_VOLUME=rim_rim_data "$b/backup.sh" >"$work/stdout" 2>"$work/stderr" ||
  fail "backup without a heartbeat: $(cat "$work/stderr")"
not_called "curl" || fail "backup without a heartbeat: curl ran"
printf 'RIM_BACKUP_HEARTBEAT_URL=https://hc.example.org/ping/abc\n' >>"$b/.env"
: >"$work/calls"
STUB_POSTGRES=1 STUB_VOLUME=rim_rim_data "$b/backup.sh" >"$work/stdout" 2>"$work/stderr" ||
  fail "backup with a heartbeat: $(cat "$work/stderr")"
grep -q "^curl .* https://hc.example.org/ping/abc$" "$work/calls" || fail "backup with a heartbeat: no ping"
[[ -f $(tail -n 1 "$work/stdout") ]] || fail "backup with a heartbeat: the last line is not the archive"
: >"$work/calls"
STUB_HEARTBEAT_DOWN=1 STUB_POSTGRES=1 STUB_VOLUME=rim_rim_data "$b/backup.sh" >"$work/stdout" 2>"$work/stderr" ||
  fail "backup with a failing heartbeat failed: $(cat "$work/stderr")"
expect_stderr "the backup heartbeat ping failed" "failing heartbeat"

# A heartbeat that is not an https URL stops the script before it backs anything up.
printf 'RIM_BACKUP_HEARTBEAT_URL=http://hc.example.org/ping/abc\n' >>"$b/.env"
: >"$work/calls"
if STUB_POSTGRES=1 STUB_VOLUME=rim_rim_data "$b/backup.sh" >"$work/stdout" 2>"$work/stderr"; then
  fail "an http heartbeat was accepted"
fi
expect_stderr "RIM_BACKUP_HEARTBEAT_URL is not an https URL" "http heartbeat"
[[ ! -s $work/calls ]] || fail "http heartbeat: commands ran: $(cat "$work/calls")"

# --- rim-deploy.sh -------------------------------------------------------------------------

srv=$work/srv
export RIM_DEPLOY_DIR=$srv RIM_DEPLOY_LOCK=$work/rim-deploy.lock
export STUB_RELEASE=$work/release.json STUB_TARBALL=$work/source.tar.gz STUB_RELEASE_STATUS=200
export STUB_WEB_RELEASE=$work/web-release.json STUB_WEB_RELEASE_STATUS=200

# A tarball like GitHub's archive of tag v$1: its deploy/ holds a stub upgrade.sh that records
# its argument and directory and exits with STUB_UPGRADE_EXIT, a changed compose.yml, a new
# file and an env file that must never be copied.
make_tarball() {
  local top=$work/tar/rim-server-$1
  rm -rf "$work/tar"
  mkdir -p "$top/deploy/systemd" "$top/rim_server"
  cat >"$top/deploy/upgrade.sh" <<'UPGRADE'
#!/bin/sh
printf 'upgrade.sh %s in %s\n' "$1" "$PWD" >>"$STUB_CALLS"
exit "${STUB_UPGRADE_EXIT:-0}"
UPGRADE
  chmod +x "$top/deploy/upgrade.sh"
  printf 'compose of %s\n' "$1" >"$top/deploy/compose.yml"
  printf '{}\n' >"$top/deploy/default-workspace.json"
  printf '[Unit]\n' >"$top/deploy/systemd/rim-deploy.service"
  printf 'POSTGRES_PASSWORD=from-the-tag\n' >"$top/deploy/.env"
  printf 'fn main() {}\n' >"$top/rim_server/main.rs"
  tar czf "$STUB_TARBALL" -C "$work/tar" "rim-server-$1"
}

# A release answer of the GitHub API for tag $1, pre-release when $2 is true.
release() {
  printf '{\n  "tag_name": "%s",\n  "draft": false,\n  "prerelease": %s\n}\n' "$1" "${2:-false}" >"$STUB_RELEASE"
}

# The same for rim-ui's latest release.
web_release() {
  printf '{\n  "tag_name": "%s",\n  "draft": false,\n  "prerelease": %s\n}\n' "$1" "${2:-false}" >"$STUB_WEB_RELEASE"
}

# The host's directory at RIM_VERSION 0.1.0 with the files of 0.1.0, running the web client,
# whose latest release, 0.1.0, it runs.
setup_srv() {
  rm -rf "$srv" "$work/calls"
  mkdir "$srv"
  printf 'POSTGRES_PASSWORD=secret\nRIM_VERSION=0.1.0\n' >"$srv/.env"
  printf 'compose of 0.1.0\n' >"$srv/compose.yml"
  printf 'deploy/ of 0.1.0\n' >"$srv/SOURCE"
  printf '#!/bin/sh\nexit 0\n' >"$srv/upgrade.sh"
  chmod +x "$srv/upgrade.sh"
  : >"$work/calls"
  export STUB_UPGRADE_EXIT=0 STUB_RELEASE_STATUS=200 STUB_WEB_RELEASE_STATUS=200
  STUB_WEB=1 STUB_BAD_WEB="" STUB_UNPUBLISHED="" STUB_UNSIGNED=""
  release v0.2.0
  web_release v0.1.0
  make_tarball 0.2.0
}

rim_deploy() {
  "$root/deploy/rim-deploy.sh" "$@" >"$work/stdout" 2>"$work/stderr"
}

# The files of 0.1.0 are back, the tag's are gone.
expect_files_of_010() {
  [[ $(cat "$srv/compose.yml") == "compose of 0.1.0" ]] || fail "$1: compose.yml is $(cat "$srv/compose.yml")"
  [[ $(cat "$srv/SOURCE") == "deploy/ of 0.1.0" ]] || fail "$1: SOURCE not put back"
  [[ ! -e $srv/default-workspace.json && ! -e $srv/systemd/rim-deploy.service ]] ||
    fail "$1: the tag's new files stayed"
  [[ $(env_version "$srv/.env") == 0.1.0 ]] || fail "$1: the env file changed"
}

# The running version: nothing fetched.
setup_srv
release v0.1.0
rim_deploy --check || fail "same version: $(cat "$work/stderr")"
expect_stderr "0.1.0 is running; nothing to do" "same version"
not_called ".tar.gz" || fail "same version: the tarball was fetched"

# HOLD: not even the API is asked.
setup_srv
touch "$srv/HOLD"
rim_deploy --check || fail "HOLD: $(cat "$work/stderr")"
expect_stderr "HOLD exists" "HOLD"
not_called "curl" || fail "HOLD: curl ran"

# A pre-release is ignored.
setup_srv
release v0.2.0 true
rim_deploy --check || fail "pre-release: $(cat "$work/stderr")"
expect_stderr "pre-release" "pre-release"
not_called ".tar.gz" || fail "pre-release: the tarball was fetched"

# No release yet.
setup_srv
printf '{"message":"Not Found"}\n' >"$STUB_RELEASE"
STUB_RELEASE_STATUS=404
rim_deploy --check || fail "no release: $(cat "$work/stderr")"
expect_stderr "no release yet" "no release"

# An API error fails the check.
setup_srv
STUB_RELEASE_STATUS=403
if rim_deploy --check; then fail "API error: the check passed"; fi
expect_stderr "the GitHub API answered 403" "API error"

# A tag that is not v<major>.<minor>.<patch> is ignored.
for tag in v0.2 v0.2.0-rc.1 0.2.0 'v0.2.0;reboot' release-1; do
  setup_srv
  release "$tag"
  rim_deploy --check || fail "tag $tag: $(cat "$work/stderr")"
  expect_stderr "is not v<major>.<minor>.<patch>; ignoring it" "tag $tag"
  not_called ".tar.gz" || fail "tag $tag: the tarball was fetched"
done

# A new release: the tag's deploy/ files are put in place, the env file is kept, SOURCE names
# the tag, and its upgrade.sh runs from the directory.
setup_srv
rim_deploy --check || fail "new release: $(cat "$work/stderr")"
grep -q "^curl .* https://github.com/rim-platform/rim-server/archive/refs/tags/v0.2.0.tar.gz$" "$work/calls" ||
  fail "new release: the tag was not fetched"
called "upgrade.sh 0.2.0 in $srv" || fail "new release: upgrade.sh did not run in $srv"
[[ $(cat "$srv/compose.yml") == "compose of 0.2.0" ]] || fail "new release: compose.yml not replaced"
[[ -f $srv/default-workspace.json && -f $srv/systemd/rim-deploy.service ]] || fail "new release: new files missing"
[[ $(cat "$srv/.env") == $'POSTGRES_PASSWORD=secret\nRIM_VERSION=0.1.0' ]] || fail "new release: the env file was touched"
grep -qF "tag v0.2.0" "$srv/SOURCE" || fail "new release: SOURCE is $(cat "$srv/SOURCE")"
[[ -x $srv/upgrade.sh ]] || fail "new release: upgrade.sh is not executable"
[[ ! -e $srv/rim_server ]] || fail "new release: files outside deploy/ were copied"

# rim-web not published yet: the check ends quietly and the files from before come back.
setup_srv
STUB_UPGRADE_EXIT=2
rim_deploy --check || fail "not published yet: $(cat "$work/stderr")"
expect_stderr "the next check tries again" "not published yet"
expect_files_of_010 "not published yet"
[[ ! -e $srv/FAILED ]] || fail "not published yet: FAILED written"

# The rim-server image not published yet: nothing is fetched and the check ends quietly.
setup_srv
STUB_UNPUBLISHED=$server:0.2.0
rim_deploy --check || fail "server image not published: $(cat "$work/stderr")"
expect_stderr "the next check tries again" "server image not published"
not_called ".tar.gz" || fail "server image not published: the tarball was fetched"
[[ ! -e $srv/FAILED ]] || fail "server image not published: FAILED written"
STUB_UNPUBLISHED=""

# An image without the release workflow's signature: nothing from the tag is fetched or run.
setup_srv
STUB_UNSIGNED=$server:0.2.0
if rim_deploy --check; then fail "unsigned image: the check passed"; fi
expect_stderr "is not signed by the release workflow of v0.2.0" "unsigned image"
not_called ".tar.gz" || fail "unsigned image: the tarball was fetched"
not_called "upgrade.sh" || fail "unsigned image: upgrade.sh ran"
called "cosign verify $server:0.2.0 --certificate-identity https://github.com/rim-platform/rim-server/.github/workflows/release.yaml@refs/tags/v0.2.0 $issuer" ||
  fail "unsigned image: cosign was not asked for the release workflow's identity"
STUB_UNSIGNED=""

# A failed upgrade: exit 1, the files from before, and FAILED holds the version back.
setup_srv
STUB_UPGRADE_EXIT=1
if rim_deploy --check; then fail "failed upgrade: the check passed"; fi
expect_files_of_010 "failed upgrade"
[[ $(cat "$srv/FAILED") == 0.2.0 ]] || fail "failed upgrade: FAILED does not hold 0.2.0"
: >"$work/calls"
STUB_UPGRADE_EXIT=0
rim_deploy --check || fail "after a failure: $(cat "$work/stderr")"
expect_stderr "0.2.0 failed to deploy before" "after a failure"
not_called ".tar.gz" || fail "after a failure: the tarball was fetched again"

# A copy that fails halfway: the files from before come back and upgrade.sh never runs.
# SOURCE as a directory makes the last step before upgrade.sh fail.
setup_srv
rm "$srv/SOURCE"
mkdir "$srv/SOURCE"
if rim_deploy --check; then fail "failed copy: the check passed"; fi
not_called "upgrade.sh" || fail "failed copy: upgrade.sh ran"
[[ $(cat "$srv/compose.yml") == "compose of 0.1.0" ]] || fail "failed copy: compose.yml is $(cat "$srv/compose.yml")"
[[ ! -e $srv/default-workspace.json && ! -e $srv/systemd && ! -e $srv/FAILED ]] ||
  fail "failed copy: files stayed"

# By hand the version deploys despite FAILED, and success removes it.
setup_srv
printf '0.2.0\n' >"$srv/FAILED"
rim_deploy 0.2.0 || fail "by hand: $(cat "$work/stderr")"
called "upgrade.sh 0.2.0 in $srv" || fail "by hand: upgrade.sh did not run"
[[ ! -e $srv/FAILED ]] || fail "by hand: FAILED stayed"

# By hand, HOLD does not stop a rollback, and the API is not asked.
setup_srv
touch "$srv/HOLD"
make_tarball 0.0.9
rim_deploy 0.0.9 || fail "rollback by hand: $(cat "$work/stderr")"
called "upgrade.sh 0.0.9 in $srv" || fail "rollback by hand: upgrade.sh did not run"
not_called "api.github.com" || fail "rollback by hand: the API was asked"

# By hand, exit 2 is passed on.
setup_srv
STUB_UPGRADE_EXIT=2
status=0
rim_deploy 0.2.0 || status=$?
[[ $status == 2 ]] || fail "by hand, not published: exit $status, not 2"
expect_files_of_010 "by hand, not published"

# A version that is not one is refused by hand.
setup_srv
if rim_deploy '0.2.0;reboot'; then fail "bad version by hand: accepted"; fi
expect_stderr "is not a version" "bad version by hand"
[[ ! -s $work/calls ]] || fail "bad version by hand: commands ran"

# One deploy at a time.
setup_srv
exec 8>"$RIM_DEPLOY_LOCK"
flock 8
if rim_deploy --check; then fail "lock: a second deploy ran"; fi
expect_stderr "another deploy is running" "lock"
exec 8>&-

# --- rim-deploy.sh: the web client -----------------------------------------------------------

web_identity() {
  printf -- '--certificate-identity https://github.com/rim-platform/rim-ui/.github/workflows/release.yaml@refs/tags/v%s %s' "$1" "$issuer"
}

# The setting $1 of the deployment's env file $2 (default .env).
env_setting() {
  sed -n "s/^$1=//p" "$srv/${2:-.env}"
}

# The running version is read from RIM_WEB_VERSION, from a published RIM_WEB_IMAGE and from
# RIM_VERSION: nothing to do in each case.
setup_srv
release v0.1.0
printf 'RIM_WEB_VERSION=0.1.5\n' >>"$srv/.env"
web_release v0.1.5
rim_deploy --check || fail "web at RIM_WEB_VERSION: $(cat "$work/stderr")"
expect_stderr "web client 0.1.5 is running; nothing to do" "web at RIM_WEB_VERSION"
setup_srv
release v0.1.0
printf 'RIM_WEB_IMAGE=%s:0.1.6\n' "$web" >>"$srv/.env"
web_release v0.1.6
rim_deploy --check || fail "web at RIM_WEB_IMAGE: $(cat "$work/stderr")"
expect_stderr "web client 0.1.6 is running; nothing to do" "web at RIM_WEB_IMAGE"
setup_srv
release v0.1.0
rim_deploy --check || fail "web at RIM_VERSION: $(cat "$work/stderr")"
expect_stderr "web client 0.1.0 is running; nothing to do" "web at RIM_VERSION"
[[ -z $(steps) ]] || fail "web already running: steps ran: $(steps)"

# A custom RIM_WEB_IMAGE is never deployed over: rim-ui's API is not asked, and by hand it is
# refused.
setup_srv
release v0.1.0
printf 'RIM_WEB_IMAGE=rim-web:local\n' >>"$srv/.env"
web_release v0.1.1
rim_deploy --check || fail "custom web image: $(cat "$work/stderr")"
expect_stderr "RIM_WEB_IMAGE names a custom web image" "custom web image"
not_called "rim-ui/releases/latest" || fail "custom web image: the rim-ui API was asked"
if rim_deploy web 0.1.1; then fail "custom web image by hand: accepted"; fi
expect_stderr "RIM_WEB_IMAGE names a custom web image; remove it" "custom web image by hand"
[[ $(env_setting RIM_WEB_IMAGE) == rim-web:local ]] || fail "custom web image: RIM_WEB_IMAGE changed"

# A new rim-ui release on the files of a compose.yml that does not read RIM_WEB_VERSION: its
# image verified against rim-ui's release workflow, RIM_WEB_IMAGE written, the old env file
# kept, the web container pulled and restarted alone, the server left as it is.
setup_srv
release v0.1.0
web_release v0.1.1
cp "$srv/.env" "$work/env-before"
rim_deploy --check || fail "new web release: $(cat "$work/stderr")"
called "cosign verify $web:0.1.1 $(web_identity 0.1.1)" ||
  fail "new web release: rim-web not verified against rim-ui's release workflow of v0.1.1"
[[ $(env_setting RIM_WEB_IMAGE) == "$web:0.1.1" ]] || fail "new web release: RIM_WEB_IMAGE is $(env_setting RIM_WEB_IMAGE)"
[[ -z $(env_setting RIM_WEB_VERSION) ]] || fail "new web release: RIM_WEB_VERSION written for an old compose.yml"
[[ $(env_version "$srv/.env") == 0.1.0 ]] || fail "new web release: RIM_VERSION changed"
cmp -s "$srv/.env.previous" "$work/env-before" || fail "new web release: .env.previous is not the env file from before"
[[ $(steps) == $'compose pull web\ncompose up -d --no-deps web' ]] || fail "new web release: steps were $(steps)"
not_called ".tar.gz" || fail "new web release: the server's tag was fetched"
[[ ! -e $srv/FAILED-web ]] || fail "new web release: FAILED-web written"

# The same on files whose compose.yml reads RIM_WEB_VERSION: it replaces the setting and drops
# a published RIM_WEB_IMAGE.
setup_srv
release v0.1.0
printf 'compose of 0.2.0, which reads RIM_WEB_VERSION\n' >"$srv/compose.yml"
printf 'RIM_WEB_VERSION=0.1.0\nRIM_WEB_IMAGE=%s:0.1.0\n' "$web" >>"$srv/.env"
web_release v0.1.1
rim_deploy --check || fail "new web release, new compose.yml: $(cat "$work/stderr")"
[[ $(env_setting RIM_WEB_VERSION) == 0.1.1 ]] || fail "new compose.yml: RIM_WEB_VERSION is $(env_setting RIM_WEB_VERSION)"
! grep -q '^RIM_WEB_IMAGE=' "$srv/.env" || fail "new compose.yml: RIM_WEB_IMAGE stayed"
[[ $(grep -c '^RIM_WEB_VERSION=' "$srv/.env") == 1 ]] || fail "new compose.yml: RIM_WEB_VERSION set twice"
grep -qxF POSTGRES_PASSWORD=secret "$srv/.env" || fail "new compose.yml: another setting was lost"

# A new server release and a new rim-ui release: both deploy, the server first.
setup_srv
web_release v0.1.1
rim_deploy --check || fail "server and web: $(cat "$work/stderr")"
called "upgrade.sh 0.2.0 in $srv" || fail "server and web: upgrade.sh did not run"
[[ $(env_setting RIM_WEB_IMAGE) == "$web:0.1.1" ]] || fail "server and web: the web client was not deployed"
upgrade_line=$(grep -n '^upgrade.sh' "$work/calls" | cut -d: -f1)
pull_line=$(grep -n 'pull web$' "$work/calls" | cut -d: -f1)
((upgrade_line < pull_line)) || fail "server and web: the web client deployed before the server"

# A pre-release, a draft or a tag that is not v<major>.<minor>.<patch> of rim-ui is ignored.
setup_srv
release v0.1.0
web_release v0.1.1 true
rim_deploy --check || fail "web pre-release: $(cat "$work/stderr")"
expect_stderr "rim-platform/rim-ui: the latest release is a pre-release or a draft" "web pre-release"
[[ -z $(steps) ]] || fail "web pre-release: steps ran: $(steps)"
setup_srv
release v0.1.0
web_release v0.1.1-rc.1
rim_deploy --check || fail "web pre-release tag: $(cat "$work/stderr")"
expect_stderr "is not v<major>.<minor>.<patch>; ignoring it" "web pre-release tag"
[[ -z $(steps) ]] || fail "web pre-release tag: steps ran: $(steps)"

# HOLD stops the web client too.
setup_srv
release v0.1.0
web_release v0.1.1
touch "$srv/HOLD"
rim_deploy --check || fail "web HOLD: $(cat "$work/stderr")"
not_called "curl" || fail "web HOLD: curl ran"
[[ -z $(steps) ]] || fail "web HOLD: steps ran: $(steps)"

# A relay without the web client: rim-ui's API is not asked. Before a server version runs, the
# web client waits.
setup_srv
release v0.1.0
web_release v0.1.1
STUB_WEB=0
rim_deploy --check || fail "no web service: $(cat "$work/stderr")"
expect_stderr "the deployment runs no web client" "no web service"
not_called "rim-ui/releases/latest" || fail "no web service: the rim-ui API was asked"
if rim_deploy web 0.1.1; then fail "no web service by hand: accepted"; fi
expect_stderr "the deployment runs no web client" "no web service by hand"
setup_srv
printf 'POSTGRES_PASSWORD=secret\n' >"$srv/.env"
release v0.1.0
STUB_UNPUBLISHED=$server:0.1.0
web_release v0.1.1
rim_deploy --check || fail "no server version: $(cat "$work/stderr")"
expect_stderr "no server version runs yet; the web client waits for it" "no server version"
[[ -z $(steps) ]] || fail "no server version: steps ran: $(steps)"
STUB_UNPUBLISHED=""

# rim-web not published yet: quiet, nothing changed, the next check tries again.
setup_srv
release v0.1.0
web_release v0.1.1
STUB_UNPUBLISHED=$web:0.1.1
cp "$srv/.env" "$work/env-before"
rim_deploy --check || fail "web not published: $(cat "$work/stderr")"
expect_stderr "web client 0.1.1 is not deployable yet; the next check tries again" "web not published"
cmp -s "$srv/.env" "$work/env-before" || fail "web not published: the env file changed"
[[ -z $(steps) && ! -e $srv/FAILED-web ]] || fail "web not published: something ran or FAILED-web written"
STUB_UNPUBLISHED=""

# rim-web without rim-ui's signature: exit 1, nothing changed.
setup_srv
release v0.1.0
web_release v0.1.1
STUB_UNSIGNED=$web:0.1.1
cp "$srv/.env" "$work/env-before"
if rim_deploy --check; then fail "unsigned web image: the check passed"; fi
expect_stderr "$web:0.1.1 is not signed by the rim-ui release workflow of v0.1.1" "unsigned web image"
cmp -s "$srv/.env" "$work/env-before" || fail "unsigned web image: the env file changed"
[[ -z $(steps) ]] || fail "unsigned web image: steps ran: $(steps)"
STUB_UNSIGNED=""

# The new web client never gets healthy: the env file from before comes back, the web client
# from before starts again, FAILED-web holds the version back, and the check exits 1.
setup_srv
release v0.1.0
web_release v0.1.1
STUB_BAD_WEB=0.1.1
cp "$srv/.env" "$work/env-before"
status=0
RIM_UPGRADE_TIMEOUT=6 rim_deploy --check || status=$?
[[ $status == 1 ]] || fail "unhealthy web: exit $status, not 1"
expect_stderr "the web client 0.1.1 did not become healthy within 6s" "unhealthy web"
expect_stderr "the web client from before runs again" "unhealthy web"
cmp -s "$srv/.env" "$work/env-before" || fail "unhealthy web: the env file is $(cat "$srv/.env")"
expected=$'compose pull web\ncompose up -d --no-deps web\ncompose up -d --no-deps web'
[[ $(steps) == "$expected" ]] || fail "unhealthy web: steps were $(steps)"
[[ $(cat "$srv/FAILED-web") == 0.1.1 ]] || fail "unhealthy web: FAILED-web does not hold 0.1.1"
: >"$work/calls"
rim_deploy --check || fail "after a web failure: $(cat "$work/stderr")"
expect_stderr "web client 0.1.1 failed to deploy before" "after a web failure"
[[ -z $(steps) ]] || fail "after a web failure: steps ran: $(steps)"

# A pull that fails is rolled back the same way.
setup_srv
release v0.1.0
web_release v0.1.1
STUB_FAIL=pull
cp "$srv/.env" "$work/env-before"
if rim_deploy --check; then fail "failed web pull: the check passed"; fi
cmp -s "$srv/.env" "$work/env-before" || fail "failed web pull: the env file changed"
STUB_FAIL=""

# A server deploy that fails ends the check before the web client.
setup_srv
web_release v0.1.1
STUB_UPGRADE_EXIT=1
if rim_deploy --check; then fail "failed server deploy: the check passed"; fi
expect_stderr "the web client is not deployed in the run of a failed server deploy" "failed server deploy"
not_called "rim-ui/releases/latest" || fail "failed server deploy: the rim-ui API was asked"
[[ -z $(env_setting RIM_WEB_IMAGE) ]] || fail "failed server deploy: the web client was deployed"

# By hand: HOLD and FAILED-web do not stop it, it goes back as well as forward, success
# removes FAILED-web, and the API is not asked.
setup_srv
touch "$srv/HOLD"
printf '0.0.9\n' >"$srv/FAILED-web"
rim_deploy web 0.0.9 || fail "web by hand: $(cat "$work/stderr")"
[[ $(env_setting RIM_WEB_IMAGE) == "$web:0.0.9" ]] || fail "web by hand: RIM_WEB_IMAGE is $(env_setting RIM_WEB_IMAGE)"
[[ ! -e $srv/FAILED-web ]] || fail "web by hand: FAILED-web stayed"
not_called "api.github.com" || fail "web by hand: the API was asked"
called "cosign verify $web:0.0.9 $(web_identity 0.0.9)" || fail "web by hand: not verified"

# By hand, not published is exit 2, and a version that is not one is refused.
setup_srv
STUB_UNPUBLISHED=$web:0.1.1
status=0
rim_deploy web 0.1.1 || status=$?
[[ $status == 2 ]] || fail "web by hand, not published: exit $status, not 2"
STUB_UNPUBLISHED=""
setup_srv
if rim_deploy web '0.1.1;reboot'; then fail "bad web version by hand: accepted"; fi
expect_stderr "is not a version" "bad web version by hand"
[[ ! -s $work/calls ]] || fail "bad web version by hand: commands ran"
if rim_deploy web; then fail "web without a version: accepted"; fi
expect_stderr "usage:" "web without a version"

# A server tag never overwrites FAILED-web.
setup_srv
printf 'from the tag\n' >"$work/tar/rim-server-0.2.0/deploy/FAILED-web"
tar czf "$STUB_TARBALL" -C "$work/tar" rim-server-0.2.0
printf '0.1.1\n' >"$srv/FAILED-web"
rim_deploy 0.2.0 || fail "FAILED-web in a tag: $(cat "$work/stderr")"
[[ $(cat "$srv/FAILED-web") == 0.1.1 ]] || fail "FAILED-web in a tag: overwritten"

# --- upgrade.sh: the web client's version ---------------------------------------------------

# RIM_WEB_VERSION stays as it is, and rim-web is checked at it.
setup_upgrade 0.1.0
printf 'RIM_WEB_VERSION=0.1.5\n' >>"$d/.env"
upgrade 0.2.0 || fail "upgrade with RIM_WEB_VERSION: $(cat "$work/stderr")"
called "cosign verify $web:0.1.5 $(web_identity 0.1.5)" || fail "upgrade with RIM_WEB_VERSION: rim-web not verified at 0.1.5"
not_called "$web:0.2.0" || fail "upgrade with RIM_WEB_VERSION: rim-web looked up at 0.2.0"
[[ $(sed -n 's/^RIM_WEB_VERSION=//p' "$d/.env") == 0.1.5 ]] || fail "upgrade with RIM_WEB_VERSION: it changed"

# The first run pins the web client to the version installed.
setup_upgrade ""
STUB_VOLUME=""
upgrade 0.1.0 || fail "first run, web: $(cat "$work/stderr")"
grep -qxF RIM_WEB_VERSION=0.1.0 "$d/.env" || fail "first run: RIM_WEB_VERSION not written"

# A published RIM_WEB_IMAGE becomes RIM_WEB_VERSION in the same write as RIM_VERSION, and the
# old env file keeps it.
setup_upgrade 0.1.0
printf 'RIM_WEB_IMAGE=%s:0.1.3\n' "$web" >>"$d/.env"
upgrade 0.2.0 || fail "RIM_WEB_IMAGE conversion: $(cat "$work/stderr")"
called "cosign verify $web:0.1.3 $(web_identity 0.1.3)" || fail "RIM_WEB_IMAGE conversion: rim-web not verified at 0.1.3"
grep -qxF RIM_WEB_VERSION=0.1.3 "$d/.env" || fail "RIM_WEB_IMAGE conversion: RIM_WEB_VERSION not written"
! grep -q '^RIM_WEB_IMAGE=' "$d/.env" || fail "RIM_WEB_IMAGE conversion: RIM_WEB_IMAGE stayed"
grep -qxF "RIM_WEB_IMAGE=$web:0.1.3" "$d/.env.previous" || fail "RIM_WEB_IMAGE conversion: .env.previous lost it"
[[ $(env_version "$d/.env") == 0.2.0 ]] || fail "RIM_WEB_IMAGE conversion: RIM_VERSION is $(env_version "$d/.env")"
setup_upgrade 0.1.0
printf 'RIM_WEB_IMAGE=%s:0.1.3\n' "$web" >>"$d/.env"
cp "$d/.env" "$work/env-before"
STUB_BAD_VERSION=0.2.0
if upgrade 0.2.0; then fail "RIM_WEB_IMAGE conversion rolled back: the upgrade passed"; fi
cmp -s "$d/.env" "$work/env-before" || fail "RIM_WEB_IMAGE conversion rolled back: the env file is $(cat "$d/.env")"
setup_upgrade 0.1.0
printf 'RIM_WEB_IMAGE=%s:0.1.3\n' "$web" >>"$d/.env"
upgrade --dry-run 0.2.0 || fail "RIM_WEB_IMAGE dry run: $(cat "$work/stderr")"
expect_stderr "dry run: replace RIM_WEB_IMAGE=$web:0.1.3 with RIM_WEB_VERSION=0.1.3" "RIM_WEB_IMAGE dry run"
expect_stderr "dry run: the web client stays at rim-web 0.1.3" "RIM_WEB_IMAGE dry run"
grep -qxF "RIM_WEB_IMAGE=$web:0.1.3" "$d/.env" || fail "RIM_WEB_IMAGE dry run: the env file changed"

# Any other RIM_WEB_IMAGE is left as it is and its image not checked.
setup_upgrade 0.1.0
printf 'RIM_WEB_IMAGE=%s@sha256:00\n' "$web" >>"$d/.env"
upgrade 0.2.0 || fail "custom RIM_WEB_IMAGE: $(cat "$work/stderr")"
expect_stderr "is not a published rim-web version: the web image is not checked and stays as it is" "custom RIM_WEB_IMAGE"
not_called "manifest inspect $web" || fail "custom RIM_WEB_IMAGE: rim-web was looked up"
grep -qxF "RIM_WEB_IMAGE=$web@sha256:00" "$d/.env" || fail "custom RIM_WEB_IMAGE: it changed"
! grep -q '^RIM_WEB_VERSION=' "$d/.env" || fail "custom RIM_WEB_IMAGE: RIM_WEB_VERSION written"

# A relay without the web client gets no RIM_WEB_VERSION.
setup_upgrade 0.1.0
STUB_WEB=0
upgrade 0.2.0 || fail "relay, web version: $(cat "$work/stderr")"
! grep -q '^RIM_WEB_VERSION=' "$d/.env" || fail "relay: RIM_WEB_VERSION written"

# A web setting in the environment would override the env file: refused.
setup_upgrade 0.1.0
if RIM_WEB_VERSION=0.1.5 upgrade 0.2.0; then fail "RIM_WEB_VERSION in the environment: accepted"; fi
expect_stderr "RIM_WEB_VERSION is set in the environment" "RIM_WEB_VERSION in the environment"

echo "deploy scripts: all tests passed"
