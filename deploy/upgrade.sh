#!/usr/bin/env bash
# Moves a RIM deployment to another release, forward or back, with the compose files and the
# env file next to this script. Run it from the files of the release it installs: a checkout at
# v<version>, or the directory rim-deploy fills from the release tag.
#
#   deploy/upgrade.sh [--dry-run] <version>
#   COMPOSE_FILE=deploy/compose.yml:deploy/compose.relay.yml deploy/upgrade.sh 0.2.0
#
# In order: the version's images are published (rim-imagecheck and rim-web only when their
# services run; exit 2 when one is not, with nothing changed), their cosign signatures name the
# release workflows, backup.sh runs (skipped when the deployment has no data volume yet),
# RIM_VERSION=<version> goes into the env file with the old file kept as .env.previous, the
# images are pulled, the migrations run and the stack starts; then /readyz and
# https://$RIM_DOMAIN/.well-known/rim-server must answer within RIM_UPGRADE_TIMEOUT seconds (90)
# each, and Caddy reloads its Caddyfile.
# When a step after the env file fails, the old env file comes back, the version before starts
# again (on a first deploy, with no version before, the stack stops) and the script exits 1
# with the logs' tails. A version older than the running one skips the migrations and starts
# rim, the profile picture check and the web client without their dependencies, as a binary
# rollback. --dry-run checks the images and prints the rest without changing anything.
#
# The web client is a rim-ui release of its own, RIM_WEB_VERSION, which the upgrade leaves as it
# is: its rim-web image is checked and verified at that version against rim-ui's release workflow
# of the tag v<RIM_WEB_VERSION>. Unset, it becomes the version that ran before, or on a first
# deploy the version installed, and is written into the env file with RIM_VERSION. A
# RIM_WEB_IMAGE of ghcr.io/rim-platform/rim-web:<version> becomes RIM_WEB_VERSION=<version> in
# the same write; any other RIM_WEB_IMAGE is a custom web image, neither checked nor changed.
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
for name in RIM_VERSION RIM_WEB_VERSION RIM_WEB_IMAGE; do
	[[ -z ${!name:-} ]] || die "$name is set in the environment and overrides $env_file; unset it"
done
for name in RIM_SERVER_IMAGE RIM_IMAGECHECK_IMAGE; do
	[[ -z $(setting "$name") ]] || die "$name is set and overrides RIM_VERSION; remove it to upgrade by version"
done

timeout=${RIM_UPGRADE_TIMEOUT:-90}
previous=$(setting RIM_VERSION)
domain=$(setting RIM_DOMAIN)
issuer=https://token.actions.githubusercontent.com

# The services the version's images run, and each image with the repository whose release
# workflow signs it and the version it runs at.
services=$(compose config --services) || die "docker compose cannot read the deployment's files"
app_services=(rim)
images=("rim-server rim-server $version")
if grep -qx imagecheck <<<"$services"; then
	app_services+=(imagecheck)
	images+=("rim-imagecheck rim-server $version")
fi

# The web client's version: RIM_WEB_VERSION, or the tag of a published RIM_WEB_IMAGE, which the
# env file write turns into RIM_WEB_VERSION (convert_web). Unset, it is pinned to the version
# that ran before, or on a first deploy to the version installed (pin_web). A custom
# RIM_WEB_IMAGE leaves web_version empty.
web_version=$(setting RIM_WEB_VERSION)
web_image=$(setting RIM_WEB_IMAGE)
convert_web=0
pin_web=0
if [[ $web_image =~ ^ghcr\.io/rim-platform/rim-web:([0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?)$ ]]; then
	web_version=${BASH_REMATCH[1]}
	convert_web=1
elif [[ -n $web_image ]]; then
	web_version=""
fi
if grep -qx web <<<"$services"; then
	app_services+=(web)
	if [[ -n $web_image ]] && ((!convert_web)); then
		say "RIM_WEB_IMAGE=$web_image is not a published rim-web version: the web image is not checked and stays as it is"
	else
		if [[ -z $web_version ]]; then
			web_version=${previous:-$version}
			pin_web=1
		fi
		images+=("rim-web rim-ui $web_version")
	fi
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

# Writes RIM_VERSION=<version> into the env file, replacing the line that sets it or adding one,
# with RIM_WEB_VERSION=<web version> the same way when it is converted or pinned; a converted
# RIM_WEB_IMAGE line goes.
set_version() {
	local tmp web=""
	((!convert_web && !pin_web)) || web=$web_version
	tmp=$(mktemp "$env_file.XXXXXX")
	awk -v line="RIM_VERSION=$version" -v web="$web" -v drop_image="$convert_web" '
		/^RIM_VERSION=/ { if (!done) print line; done = 1; next }
		/^RIM_WEB_VERSION=/ && web != "" { if (!web_done) print "RIM_WEB_VERSION=" web; web_done = 1; next }
		/^RIM_WEB_IMAGE=/ && drop_image { next }
		{ print }
		END {
			if (!done) print line
			if (web != "" && !web_done) print "RIM_WEB_VERSION=" web
		}
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
	read -r image repository image_version <<<"$entry"
	if ! docker manifest inspect "ghcr.io/rim-platform/$image:$image_version" >/dev/null 2>&1; then
		if [[ $image == rim-web ]]; then
			printf '%s %s is not published yet; the web client runs that rim-ui release (RIM_WEB_VERSION)\n' \
				"$image" "$image_version" >&2
		else
			printf '%s %s is not published yet\n' "$image" "$image_version" >&2
		fi
		exit 2
	fi
done

say "verifying the signatures"
command -v cosign >/dev/null || die "cosign is not installed"
for entry in "${images[@]}"; do
	read -r image repository image_version <<<"$entry"
	cosign verify "ghcr.io/rim-platform/$image:$image_version" \
		--certificate-identity "https://github.com/rim-platform/$repository/.github/workflows/release.yaml@refs/tags/v$image_version" \
		--certificate-oidc-issuer "$issuer" >/dev/null ||
		die "ghcr.io/rim-platform/$image:$image_version is not signed by the $repository release workflow"
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
	if ((convert_web)); then
		say "dry run: replace RIM_WEB_IMAGE=$web_image with RIM_WEB_VERSION=$web_version"
	elif ((pin_web)); then
		say "dry run: set RIM_WEB_VERSION=$web_version"
	fi
	if [[ -n $web_version ]] && grep -qx web <<<"$services"; then
		say "dry run: the web client stays at rim-web $web_version"
	fi
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
if ((convert_web)); then
	say "replacing RIM_WEB_IMAGE=$web_image with RIM_WEB_VERSION=$web_version"
elif ((pin_web)); then
	say "setting RIM_WEB_VERSION=$web_version, the web client's version"
fi
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
