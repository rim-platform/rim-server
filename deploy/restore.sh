#!/usr/bin/env bash
# Restores a backup written by backup.sh into a stopped RIM deployment: the database is
# dropped and recreated from the dump, the rim data volume is emptied and refilled from the
# archive, then the stack starts and the script waits for /readyz.
#
#   docker compose -f deploy/compose.yml stop rim
#   deploy/restore.sh backups/rim-backup-rim-20261010T020000Z.tar[.age]
#
# The env file is not restored: the backup's copy is in the archive as `env` for a new host.
# Encrypted backups need RIM_BACKUP_AGE_IDENTITY. RIM_RESTORE_YES=1 skips the confirmation;
# RIM_RESTORE_SERVICES names the services to start afterwards (all by default).
set -euo pipefail

# shellcheck source=deploy/backup-lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/backup-lib.sh"

[[ $# -eq 1 ]] || die "usage: $0 <backup archive or directory>"
backup=$1
[[ -e $backup ]] || die "$backup does not exist"

project=$(project_name)
if running rim; then
	die "the rim service of project $project is running; stop it first: docker compose … stop rim"
fi

if [[ ${RIM_RESTORE_YES:-} != 1 ]]; then
	printf 'This replaces the database and the data volume of project %s with %s.\n' \
		"$project" "$backup" >&2
	read -r -p "Type the project name to continue: " answer
	[[ $answer == "$project" ]] || die "not confirmed"
fi

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
set_dir=$(unpack_backup "$backup" "$work")
say "restoring $(sed -n 's/^created: //p' "$set_dir/MANIFEST") of project" \
	"$(sed -n 's/^project: //p' "$set_dir/MANIFEST")"

say "starting postgres"
compose up -d --wait postgres

say "restoring the database"
compose exec -T postgres psql -v ON_ERROR_STOP=1 -q -U rim -d postgres \
	-c 'DROP DATABASE IF EXISTS rim WITH (FORCE)' \
	-c 'CREATE DATABASE rim OWNER rim'
compose exec -T postgres pg_restore -U rim -d rim --clean --if-exists --no-owner \
	--exit-on-error --single-transaction <"$set_dir/rim.dump"

volume=$(data_volume)
if [[ -z $volume ]]; then
	compose create --no-recreate rim
	volume=$(data_volume)
	[[ -n $volume ]] || die "could not create the rim_data volume of project $project"
fi
say "restoring volume $volume"
docker run --rm -v "$volume":/data -v "$set_dir":/backup:ro "$helper_image" sh -c \
	'find /data -mindepth 1 -delete && tar xzf /backup/rim-data.tgz -C /data && chown -R 1001:1001 /data'

say "starting the stack"
# shellcheck disable=SC2086 # the service list splits on spaces
compose up -d ${RIM_RESTORE_SERVICES:-}
wait_ready
compose exec -T rim /app/rim-server server-key show
