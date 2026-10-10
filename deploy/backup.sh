#!/usr/bin/env bash
# Backs up a running RIM deployment into one timestamped archive under RIM_BACKUP_DIR
# (./backups): a custom-format pg_dump of the database, a tar of the rim data volume (server
# key, franking keys, uploads, audit log and exports) and the env file. With
# RIM_BACKUP_AGE_RECIPIENT set the archive is encrypted with age and nothing unencrypted stays
# on disk. The server recovery key file is never included.
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
(cd "$set_dir" && du -h -- *)
du -h "$out"
printf '%s\n' "$out"
