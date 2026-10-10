# shellcheck shell=bash
# Helpers shared by backup.sh, restore.sh and restore-drill.sh; sourced, never run.
#
# COMPOSE_FILE and COMPOSE_PROJECT_NAME pick the deployment as they do for docker compose
# itself; without COMPOSE_FILE it is deploy/compose.yml. RIM_ENV_FILE names the env file,
# deploy/.env by default.

deploy_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
env_file=${RIM_ENV_FILE:-$deploy_dir/.env}
# shellcheck disable=SC2034 # read by the scripts that source this file
helper_image=${RIM_BACKUP_HELPER_IMAGE:-alpine:3.22}

die() {
	printf 'error: %s\n' "$*" >&2
	exit 1
}

say() {
	printf '==> %s\n' "$*" >&2
}

# docker compose on the deployment's files and env file.
compose() {
	local args=()
	if [[ -z ${COMPOSE_FILE:-} ]]; then
		args+=(-f "$deploy_dir/compose.yml")
	fi
	if [[ -f $env_file ]]; then
		args+=(--env-file "$env_file")
	fi
	docker compose "${args[@]}" "$@"
}

# A setting from the environment, else from the env file, else the default given.
setting() {
	local name=$1 default=${2:-} value=${!1:-}
	if [[ -z $value && -f $env_file ]]; then
		value=$(sed -n "s/^${name}=//p" "$env_file" | tail -n 1)
		value=${value%\"}
		value=${value#\"}
	fi
	printf '%s' "${value:-$default}"
}

# The compose project name the deployment runs under.
project_name() {
	compose config | sed -n '1s/^name: //p'
}

# The docker volume behind the `rim_data` volume of the project; empty when it does not exist.
data_volume() {
	docker volume ls -q \
		--filter "label=com.docker.compose.project=$(project_name)" \
		--filter "label=com.docker.compose.volume=rim_data"
}

# Whether a service of the project has a running container.
running() {
	[[ -n $(compose ps --status running -q "$1") ]]
}

# Waits until the server's /readyz answers 200, up to RIM_READY_TIMEOUT seconds (180).
wait_ready() {
	local deadline=$((SECONDS + ${RIM_READY_TIMEOUT:-180})) answer=""
	say "waiting for /readyz"
	while ((SECONDS < deadline)); do
		if answer=$(compose exec -T rim curl -fsS http://localhost:8080/readyz 2>/dev/null); then
			printf '%s\n' "$answer"
			return 0
		fi
		sleep 3
	done
	compose exec -T rim curl -sS http://localhost:8080/readyz >&2 || true
	compose logs --tail 40 rim >&2 || true
	die "the server was not ready within ${RIM_READY_TIMEOUT:-180}s"
}

# Unpacks a backup (an archive, an age-encrypted archive, or an unpacked directory) into the
# directory given and prints the path of the backup's own directory inside it.
unpack_backup() {
	local backup=$1 into=$2 identity
	if [[ -d $backup ]]; then
		cp -a "$backup" "$into/"
	elif [[ $backup == *.age ]]; then
		identity=$(setting RIM_BACKUP_AGE_IDENTITY)
		[[ -n $identity ]] || die "$backup is encrypted: set RIM_BACKUP_AGE_IDENTITY to the age identity file"
		command -v age >/dev/null || die "$backup is encrypted and age is not installed"
		age -d -i "$identity" "$backup" | tar xf - -C "$into"
	else
		tar xf "$backup" -C "$into"
	fi
	local found
	found=$(find "$into" -mindepth 1 -maxdepth 2 -name rim.dump -printf '%h\n' | head -n 1)
	[[ -n $found ]] || die "$backup holds no rim.dump"
	(cd "$found" && sha256sum --quiet -c SHA256SUMS) || die "$backup fails its checksums"
	printf '%s\n' "$found"
}
