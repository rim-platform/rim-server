#!/usr/bin/env bash
# Restores a backup into a throwaway stack (project rim-drill), waits for /readyz, prints the
# server key and the row counts of a few tables, and tears the stack down with its volumes.
# Without an argument it takes the newest backup in RIM_BACKUP_DIR (./backups).
#
#   deploy/restore-drill.sh [backup]
#   COMPOSE_FILE=deploy/compose.yml:deploy/compose.relay.yml deploy/restore-drill.sh
#
# The drill never speaks for the server: server-to-server traffic and mail are off, only
# postgres, migrate and rim run, and no port is published. RIM_DRILL_KEEP=1 leaves it running.
set -euo pipefail

# shellcheck source=deploy/backup-lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/backup-lib.sh"

backup_dir=$(setting RIM_BACKUP_DIR ./backups)
if [[ $# -ge 1 ]]; then
	backup=$1
else
	backup=$(find "$backup_dir" -maxdepth 1 -name 'rim-backup-*.tar*' -printf '%T@ %p\n' 2>/dev/null |
		sort -n | tail -n 1 | cut -d' ' -f2-)
	[[ -n $backup ]] || die "no backup in $backup_dir"
fi
[[ -e $backup ]] || die "$backup does not exist"

drill_project=${RIM_DRILL_PROJECT:-rim-drill}
[[ $drill_project != "$(project_name 2>/dev/null)" ]] || die "the drill project is the deployment's own project"

# The drill's own env file: the deployment's images, nothing else of its settings.
drill_env=$(mktemp)
cat >"$drill_env" <<EOF
POSTGRES_PASSWORD=$(od -An -N16 -tx1 /dev/urandom | tr -d ' \n')
RIM_DOMAIN=localhost
RIM_PUBLIC_BASE_URL=
RIM_KEY_LOGIN_AUDIENCES=localhost:8080
RIM_SERVER_IMAGE=$(setting RIM_SERVER_IMAGE rim-server:local)
RIM_WEB_IMAGE=$(setting RIM_WEB_IMAGE rim-web:local)
RIM_HTTP_PORT=${RIM_DRILL_HTTP_PORT:-18080}
RIM_HTTPS_PORT=${RIM_DRILL_HTTPS_PORT:-18443}
EOF
RIM_BACKUP_AGE_IDENTITY=$(setting RIM_BACKUP_AGE_IDENTITY)
export RIM_BACKUP_AGE_IDENTITY
export COMPOSE_PROJECT_NAME=$drill_project RIM_ENV_FILE=$drill_env
env_file=$drill_env

teardown() {
	if [[ ${RIM_DRILL_KEEP:-} == 1 ]]; then
		say "left project $drill_project running; remove it with: docker compose -p $drill_project down -v"
	else
		say "tearing down project $drill_project"
		compose down -v --remove-orphans >/dev/null 2>&1 || true
	fi
	rm -f "$drill_env"
}
trap teardown EXIT

say "drill of $backup in project $drill_project"
compose down -v --remove-orphans >/dev/null 2>&1 || true
started=$SECONDS
RIM_RESTORE_YES=1 RIM_RESTORE_SERVICES=rim "$deploy_dir/restore.sh" "$backup"

say "data volume"
compose exec -T rim sh -c 'cd /data && ls -la && du -sh -- *'

say "row counts"
compose exec -T postgres psql -U rim -d rim -P footer=off <<'SQL'
SELECT t.table_name AS "table",
       (xpath('/row/c/text()',
              query_to_xml(format('SELECT count(*) AS c FROM %I', t.table_name), false, true, '')))[1]::text::bigint AS "rows"
FROM information_schema.tables t
WHERE t.table_schema = 'public'
  AND t.table_name IN ('user', 'session', 'device_key', 'server', 'channel', 'conversation',
                       'message', 'relay_item', 'command_audit_log', 'seaql_migrations')
ORDER BY 1;
SQL
say "drill passed in $((SECONDS - started))s"
