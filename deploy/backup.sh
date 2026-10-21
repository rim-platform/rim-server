#!/usr/bin/env bash
# Backs up a running RIM deployment into one timestamped archive under RIM_BACKUP_DIR
# (./backups): a custom-format pg_dump of the database, a tar of the rim data volume (server
# key, franking keys, uploads, audit files and exports) and the env file. With
# RIM_BACKUP_AGE_RECIPIENT set the archive is encrypted with age and nothing unencrypted stays
# on disk. The server recovery key file is never included. With RIM_BACKUP_KEEP_DAYS set, a
# backup that succeeded then deletes this deployment's backups in RIM_BACKUP_DIR older than that
# many days; unset, every backup is kept. With RIM_BACKUP_HEARTBEAT_URL set, a backup that
# succeeded ends with a GET of that https URL, so a monitor that expects it each day alerts when
# a night is missed; a failed ping is a warning, not a failed backup.
#
#   deploy/backup.sh
#   COMPOSE_FILE=deploy/compose.yml:deploy/compose.relay.yml deploy/backup.sh
#
# The dump and the data archive are one set: restore them together with restore.sh.
set -euo pipefail

# shellcheck source=deploy/backup-lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/backup-lib.sh"

backup_dir=$(setting RIM_BACKUP_DIR ./backups)
recipient=$(setting RIM_BACKUP_AGE_RECIPIENT)
keep_days=$(setting RIM_BACKUP_KEEP_DAYS)
heartbeat=$(setting RIM_BACKUP_HEARTBEAT_URL)

if [[ -n $keep_days && ! $keep_days =~ ^[1-9][0-9]*$ ]]; then
	die "RIM_BACKUP_KEEP_DAYS=$keep_days is not a whole number of days, at least 1"
fi
if [[ -n $heartbeat && $heartbeat != https://* ]]; then
	die "RIM_BACKUP_HEARTBEAT_URL is not an https URL"
fi

if [[ -n $recipient ]]; then
	command -v age >/dev/null || die "RIM_BACKUP_AGE_RECIPIENT is set and age is not installed"
fi
running postgres || die "the postgres service is not running"
project=$(project_name)
volume=$(data_volume)
[[ -n $volume ]] || die "project $project has no rim_data volume"

stamp=$(date -u +%Y%m%dT%H%M%SZ)
name="rim-backup-$project-$stamp"
umask 077
mkdir -p "$backup_dir"
work=$(mktemp -d "$backup_dir/.$name.XXXXXX")
trap 'rm -rf "$work"' EXIT
set_dir=$work/$name
mkdir "$set_dir"

say "dumping the database"
compose exec -T postgres pg_dump -U rim -Fc rim >"$set_dir/rim.dump"
compose exec -T postgres pg_restore --list <"$set_dir/rim.dump" >/dev/null ||
	die "the dump does not read back"

say "archiving volume $volume"
if docker run --rm -v "$volume":/data:ro "$helper_image" test -e /data/server-recovery-key.txt; then
	say "warning: /data/server-recovery-key.txt is still on the host; it is left out of the" \
		"backup. Move it offline and delete it."
fi
docker run --rm -v "$volume":/data:ro "$helper_image" \
	tar czf - -C /data --exclude ./server-recovery-key.txt . >"$set_dir/rim-data.tgz"

if [[ -f $env_file ]]; then
	cp "$env_file" "$set_dir/env"
fi

version=unknown
if running rim; then
	version=$(compose exec -T rim /app/rim-server --version) || version=unknown
fi
{
	printf 'project: %s\n' "$project"
	printf 'created: %s\n' "$stamp"
	printf 'server: %s\n' "$version"
	printf 'volume: %s\n' "$volume"
} >"$set_dir/MANIFEST"
(cd "$set_dir" && sha256sum -- * >SHA256SUMS)

if [[ -n $recipient ]]; then
	out=$backup_dir/$name.tar.age
	recipient_args=(-r "$recipient")
	if [[ -f $recipient ]]; then
		recipient_args=(-R "$recipient")
	fi
	tar cf - -C "$work" "$name" | age "${recipient_args[@]}" -o "$work/archive"
else
	out=$backup_dir/$name.tar
	tar cf "$work/archive" -C "$work" "$name"
fi
chmod 600 "$work/archive"
mv "$work/archive" "$out"

say "wrote $out"
if [[ -n $keep_days ]]; then
	say "deleting backups of $project older than $keep_days days"
	# This project's archives only: a timestamp follows the name, so rim's never match rim-relay's.
	d='[0-9]'
	find "$backup_dir" -maxdepth 1 -type f ! -path "$out" \
		-name "rim-backup-$project-$d$d$d$d$d$d$d${d}T$d$d$d$d$d${d}Z.tar*" \
		-mtime +$((keep_days - 1)) -print -delete >&2
fi
(cd "$set_dir" && du -h -- *)
du -h "$out"
if [[ -n $heartbeat ]]; then
	say "pinging the backup heartbeat"
	curl -fsS -m 10 --retry 3 -o /dev/null "$heartbeat" || say "warning: the backup heartbeat ping failed"
fi
printf '%s\n' "$out"
