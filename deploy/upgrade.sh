#!/usr/bin/env bash
# Moves a RIM deployment to another release, forward or back, with the compose files and the
# env file next to this script. Run it from the files of the release it installs: a checkout at
# v<version>, or the directory rim-deploy fills from the release tag.
#
#   deploy/upgrade.sh [--dry-run] <version>
#   COMPOSE_FILE=deploy/compose.yml:deploy/compose.relay.yml deploy/upgrade.sh 0.2.0
#
# In order: the version's images are published (rim-web only when the web client runs; exit 2
# when one is not, with nothing changed), their cosign signatures name the release workflows,
# backup.sh runs (skipped when the deployment has no data volume yet), RIM_VERSION=<version>
# goes into the env file with the old file kept as .env.previous, the images are pulled, the
# migrations run and the stack starts; then /readyz and https://$RIM_DOMAIN/.well-known/rim-server
# must answer within RIM_UPGRADE_TIMEOUT seconds (90) each, and Caddy reloads its Caddyfile.
# When a step after the env file fails, the old env file comes back, the version before starts
# again (on a first deploy, with no version before, the stack stops) and the script exits 1
# with the logs' tails. A version older than the running one skips the migrations and starts
# rim and the web client without their dependencies, as a binary rollback. --dry-run checks the
# images and prints the rest without changing anything.
set -euo pipefail

# shellcheck source=deploy/backup-lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/backup-lib.sh"

usage="usage: $0 [--dry-run] <version>"
dry_run=0
version=""
for arg in "$@"; do
	case $arg in
	--dry-run) dry_run=1 ;;
	-*) die "$usage" ;;
	*)
		[[ -z $version ]] || die "$usage"
		version=$arg
		;;
	esac
done
[[ -n $version ]] || die "$usage"
[[ $version =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]] ||
	die "$version is not a version: <major>.<minor>.<patch>[-<pre-release>]"

[[ -f $env_file ]] || die "$env_file does not exist"
[[ -z ${RIM_VERSION:-} ]] || die "RIM_VERSION is set in the environment and overrides $env_file; unset it"
for name in RIM_SERVER_IMAGE RIM_WEB_IMAGE; do
	[[ -z $(setting "$name") ]] || die "$name is set and overrides RIM_VERSION; remove it to upgrade by version"
done

timeout=${RIM_UPGRADE_TIMEOUT:-90}
previous=$(setting RIM_VERSION)
domain=$(setting RIM_DOMAIN)
issuer=https://token.actions.githubusercontent.com

# The services the version's images run, and each image with the repository whose release
# workflow signs it.
services=$(compose config --services) || die "docker compose cannot read the deployment's files"
app_services=(rim)
images=("rim-server rim-server")
if grep -qx web <<<"$services"; then
	app_services+=(web)
	images+=("rim-web rim-ui")
fi

# Whether version $1 is older than $2: by major, minor and patch, and a pre-release before the
# release of the same numbers.
older() {
	local a=${1%%-*} b=${2%%-*}
	if [[ $a != "$b" ]]; then
		[[ $(printf '%s\n%s\n' "$a" "$b" | sort -V | sed -n 1p) == "$a" ]]
	else
		[[ $1 == *-* && $2 != *-* ]]
	fi
}

# Writes RIM_VERSION=<version> into the env file, replacing the line that sets it or adding one.
set_version() {
	local tmp
	tmp=$(mktemp "$env_file.XXXXXX")
	awk -v line="RIM_VERSION=$version" '
		/^RIM_VERSION=/ { if (!done) print line; done = 1; next }
		{ print }
		END { if (!done) print line }
	' "$env_file" >"$tmp"
	mv "$tmp" "$env_file"
}

# Puts the env file from before back, starts the version before again and exits 1. With no
# version before, there is nothing to go back to and the stack is stopped.
roll_back() {
	say "the upgrade to $version failed: $1"
	compose logs --tail 40 migrate rim >&2 || true
	cp -p "$env_file.previous" "$env_file"
	if [[ -n $previous ]]; then
		say "rolling back to $previous"
		compose up -d --no-deps "${app_services[@]}" || true
		if retry_for "$timeout" compose exec -T rim curl -fsS http://localhost:8080/readyz >/dev/null; then
			say "$previous is running again"
		else
			say "warning: $previous is not ready either; see the logs above"
		fi
	else
		say "no version ran before; stopping the stack"
		compose stop || true
	fi
	exit 1
}

say "checking that the images of $version are published"
for entry in "${images[@]}"; do
	read -r image repository <<<"$entry"
	if ! docker manifest inspect "ghcr.io/rim-platform/$image:$version" >/dev/null 2>&1; then
		if [[ $image == rim-web ]]; then
			printf '%s %s is not published yet; tag rim-ui first\n' "$image" "$version" >&2
		else
			printf '%s %s is not published yet\n' "$image" "$version" >&2
		fi
		exit 2
	fi
done

say "verifying the signatures"
command -v cosign >/dev/null || die "cosign is not installed"
for entry in "${images[@]}"; do
	read -r image repository <<<"$entry"
	cosign verify "ghcr.io/rim-platform/$image:$version" \
		--certificate-identity "https://github.com/rim-platform/$repository/.github/workflows/release.yaml@refs/tags/v$version" \
		--certificate-oidc-issuer "$issuer" >/dev/null ||
		die "ghcr.io/rim-platform/$image:$version is not signed by the $repository release workflow"
done

downgrade=0
if [[ -n $previous ]] && older "$version" "$previous"; then
	downgrade=1
	say "$version is older than $previous: rolling the binary back, the migrations stay"
fi

backup=1
if [[ -z $(data_volume) ]]; then
	backup=0
	say "no data volume yet, so nothing to back up"
fi

if ((dry_run)); then
	((backup == 0)) || say "dry run: $deploy_dir/backup.sh"
	say "dry run: set RIM_VERSION=$version in $env_file (now ${previous:-unset}), the old file kept as .env.previous"
	if ((downgrade)); then
		say "dry run: compose pull ${app_services[*]}"
		say "dry run: compose up -d --no-deps ${app_services[*]}"
	else
		say "dry run: compose pull"
		say "dry run: compose run --rm migrate"
		say "dry run: compose up -d"
	fi
	say "dry run: wait for /readyz and https://$domain/.well-known/rim-server"
	exit 0
fi

((backup == 0)) || "$deploy_dir/backup.sh"
say "setting RIM_VERSION=$version (was ${previous:-unset}); the old env file stays as .env.previous"
cp -p "$env_file" "$env_file.previous"
set_version

if ((downgrade)); then
	compose pull "${app_services[@]}" || roll_back "pulling the images"
	compose up -d --no-deps "${app_services[@]}" || roll_back "starting $version"
else
	compose pull || roll_back "pulling the images"
	compose run --rm migrate || roll_back "the migrations"
	compose up -d || roll_back "starting $version"
fi

say "waiting for /readyz"
retry_for "$timeout" compose exec -T rim curl -fsS http://localhost:8080/readyz ||
	roll_back "/readyz did not answer within ${timeout}s"
if [[ -n $domain ]]; then
	say "waiting for https://$domain/.well-known/rim-server"
	retry_for "$timeout" curl -fsS "https://$domain/.well-known/rim-server" >/dev/null ||
		roll_back "https://$domain/.well-known/rim-server did not answer within ${timeout}s"
fi

# Caddy reads its Caddyfile at start; a reload applies one that changed with the release.
if running caddy; then
	compose exec -T caddy caddy reload --config /etc/caddy/Caddyfile --adapter caddyfile ||
		say "warning: Caddy did not reload its Caddyfile; it keeps the one it runs"
fi

health=$(compose exec -T rim curl -fsS http://localhost:8080/healthz || true)
running_version=$(sed -n 's/.*"version":"\([^"]*\)".*/\1/p' <<<"$health")
if [[ $running_version != "$version" ]]; then
	say "warning: /healthz reports version ${running_version:-unknown}, not $version"
fi
say "running $version"
printf '%s\n' "$running_version"
