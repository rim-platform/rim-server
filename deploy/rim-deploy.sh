#!/usr/bin/env bash
# Deploys rim-server and rim-ui releases on a host that runs published images. A rim-server
# release: fetches the release tag's deploy/ files from GitHub into the deployment directory,
# RIM_DEPLOY_DIR (/srv/rim-server), and runs the upgrade.sh among them. A rim-ui release:
# points the env file at its rim-web image and restarts the web container alone. Installed as
# /usr/local/sbin/rim-deploy; rim-deploy.timer runs the --check form every five minutes.
#
#   rim-deploy --check           deploys the latest published releases that are not running yet
#   rim-deploy <version>         deploys that rim-server version by hand, forward or back
#   rim-deploy web <version>     deploys that web client version by hand, forward or back
#
# --check does nothing while <dir>/HOLD exists. For each repository, rim-server first, it skips
# a latest release that is a pre-release or whose tag is not v<major>.<minor>.<patch>, one that
# already runs (RIM_VERSION in <dir>/.env; for rim-ui the web client's version below), and one
# that <dir>/FAILED (rim-ui: <dir>/FAILED-web) names: a deploy that fails writes its version
# there, and the version is tried again once the file is gone or by hand. When a version's
# images are not published yet (upgrade.sh's exit 2) the check ends quietly and the next one
# tries again. A server deploy that fails ends the check before the web client; the check exits
# non-zero when either part failed.
#
# Before anything is fetched, the version's rim-server image must carry the signature of the
# release workflow of its tag, since the tag's files run as root. The fetched files replace
# those of the same name in the directory; .env and .env.previous are never touched, SOURCE
# records the tag, and when the upgrade does not complete the files from before come back.
#
# The web client runs RIM_WEB_VERSION, else the tag of a RIM_WEB_IMAGE of
# ghcr.io/rim-platform/rim-web:<version>, else RIM_VERSION. Its deploy needs the deployment to
# run the `web` service and the rim-web image to carry the signature of rim-ui's release
# workflow of the tag; it keeps .env as .env.previous, writes RIM_WEB_VERSION (or, while the
# directory's compose.yml does not read it, RIM_WEB_IMAGE), pulls and restarts `web` and waits up
# to RIM_UPGRADE_TIMEOUT seconds (90) for it to be healthy, and puts .env.previous back when it
# is not. Any other RIM_WEB_IMAGE is a custom web image, which is never deployed over.
#
# One deploy runs at a time, under a lock on RIM_DEPLOY_LOCK (/run/rim-deploy.lock).
set -euo pipefail

repo=rim-platform/rim-server
web_repo=rim-platform/rim-ui
web_image=ghcr.io/rim-platform/rim-web
dir=${RIM_DEPLOY_DIR:-/srv/rim-server}
lock=${RIM_DEPLOY_LOCK:-/run/rim-deploy.lock}
timeout=${RIM_UPGRADE_TIMEOUT:-90}
issuer=https://token.actions.githubusercontent.com
version_pattern='[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?'

die() {
	printf 'rim-deploy: error: %s\n' "$*" >&2
	exit 1
}

say() {
	printf 'rim-deploy: %s\n' "$*" >&2
}

# The value of a setting in the deployment's env file, without quotes; empty when unset.
setting() {
	local value=""
	if [[ -f $dir/.env ]]; then
		value=$(sed -n "s/^$1=//p" "$dir/.env" | tail -n 1)
		value=${value%\"}
		value=${value#\"}
	fi
	printf '%s' "$value"
}

# docker compose in the deployment directory on its files and env file: COMPOSE_FILE from the
# environment or the env file, else compose.yml.
compose() {
	local args=()
	if [[ -z ${COMPOSE_FILE:-} && -z $(setting COMPOSE_FILE) ]]; then
		args+=(-f "$dir/compose.yml")
	fi
	(cd "$dir" && docker compose "${args[@]}" --env-file "$dir/.env" "$@")
}

# The version of the web client the deployment runs, printed; returns 1 when RIM_WEB_IMAGE
# names a custom image, not a published rim-web version.
current_web() {
	local image version
	image=$(setting RIM_WEB_IMAGE)
	if [[ -n $image ]]; then
		[[ $image =~ ^ghcr\.io/rim-platform/rim-web:($version_pattern)$ ]] || return 1
		printf '%s' "${BASH_REMATCH[1]}"
		return 0
	fi
	version=$(setting RIM_WEB_VERSION)
	printf '%s' "${version:-$(setting RIM_VERSION)}"
}

# Whether the deployment runs the `web` service.
runs_web() {
	local services
	services=$(compose config --services) || die "docker compose cannot read the deployment's files"
	grep -qx web <<<"$services"
}

# Puts back the files the deploy replaced and removes those it added, with the directories
# that are left empty.
restore_files() {
	local file parent
	say "putting back the files from before"
	cp -pR "$work/kept/." "$dir/"
	while IFS= read -r file; do
		rm -f "$dir/$file"
		parent=$(dirname "$file")
		while [[ $parent != . ]] && rmdir "$dir/$parent" 2>/dev/null; do
			parent=$(dirname "$parent")
		done
	done <"$work/added"
	restore=0
}

# Whether the rim-server image of version $1 is published and signed by the rim-server release
# workflow of tag v$1. Returns 1 with `status` set to 2 when it is not published yet; exits
# when its signature does not verify.
signed_release() {
	local image=ghcr.io/$repo:$1
	if ! docker manifest inspect "$image" >/dev/null 2>&1; then
		say "$image is not published yet"
		status=2
		return 1
	fi
	command -v cosign >/dev/null || die "cosign is not installed"
	cosign verify "$image" \
		--certificate-identity "https://github.com/$repo/.github/workflows/release.yaml@refs/tags/v$1" \
		--certificate-oidc-issuer "$issuer" >/dev/null ||
		die "$image is not signed by the release workflow of v$1; nothing from the tag runs"
}

# Downloads the tag v<version>, puts its deploy/ files into the directory and runs their
# upgrade.sh, whose exit status it leaves in `status`; the files from before come back unless
# that is 0.
deploy() {
	local version=$1 url file
	url=https://github.com/$repo/archive/refs/tags/v$version.tar.gz

	# The tag's files run as root, so they are fetched only for a version whose image an
	# approved release run signed: a tag on its own installs nothing.
	status=0
	signed_release "$version" || return 0

	say "fetching $url"
	curl -fsSL --retry 3 -o "$work/source.tar.gz" "$url" || die "could not download $url"
	mkdir "$work/source"
	tar xzf "$work/source.tar.gz" -C "$work/source" --strip-components 1 --no-same-owner
	[[ -x $work/source/deploy/upgrade.sh ]] || die "v$version has no deploy/upgrade.sh"

	# The files the tag replaces are kept, and those it adds are listed, so the directory can
	# be put back as it was. Files are copied in place: a compose file mount follows the file,
	# not its name.
	mkdir "$work/kept"
	: >"$work/added"
	(cd "$work/source/deploy" && find . -type f -print0) >"$work/files"
	restore=1
	while IFS= read -r -d '' file; do
		file=${file#./}
		case $file in
		.env | .env.previous | HOLD | FAILED | FAILED-web | SOURCE) continue ;;
		esac
		if [[ -e $dir/$file ]]; then
			mkdir -p "$work/kept/$(dirname "$file")"
			cp -p "$dir/$file" "$work/kept/$file"
		else
			printf '%s\n' "$file" >>"$work/added"
		fi
		mkdir -p "$dir/$(dirname "$file")"
		cp -p "$work/source/deploy/$file" "$dir/$file"
	done <"$work/files"
	if [[ -e $dir/SOURCE ]]; then
		cp -p "$dir/SOURCE" "$work/kept/SOURCE"
	else
		printf 'SOURCE\n' >>"$work/added"
	fi
	{
		printf 'deploy/ of %s, tag v%s\n' "$repo" "$version"
		printf 'fetched %s from %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$url"
		printf 'sha256 %s\n' "$(sha256sum "$work/source.tar.gz" | cut -d' ' -f1)"
	} >"$dir/SOURCE"

	say "upgrading to $version"
	status=0
	(cd "$dir" && ./upgrade.sh "$version") || status=$?
	if ((status == 0)); then
		restore=0
	else
		restore_files
	fi
}

# Writes the web client's version $1 into the env file: RIM_WEB_VERSION, dropping a
# RIM_WEB_IMAGE, when the directory's compose.yml reads RIM_WEB_VERSION; else RIM_WEB_IMAGE.
set_web() {
	local tmp name=RIM_WEB_IMAGE value=$web_image:$1
	if grep -q RIM_WEB_VERSION "$dir/compose.yml"; then
		name=RIM_WEB_VERSION
		value=$1
	fi
	tmp=$(mktemp "$dir/.env.XXXXXX")
	awk -v name="$name" -v line="$name=$value" '
		index($0, name "=") == 1 { if (!done) print line; done = 1; next }
		/^RIM_WEB_IMAGE=/ { next }
		{ print }
		END { if (!done) print line }
	' "$dir/.env" >"$tmp"
	mv "$tmp" "$dir/.env"
}

# Whether the web container becomes healthy (running, when its image has no health check)
# within RIM_UPGRADE_TIMEOUT seconds.
web_healthy() {
	local tries=$(((timeout + 2) / 3)) state
	while ((tries-- > 0)); do
		state=$(compose ps --format '{{.State}} {{.Health}}' web 2>/dev/null) || state=""
		state=${state% }
		if [[ $state == "running healthy" || $state == running ]]; then
			return 0
		fi
		((tries == 0)) || sleep 3
	done
	return 1
}

# Deploys the web client at version $1 and leaves the outcome in `status`: 0, 2 when its image
# is not published yet, 1 when it did not come up and the env file from before is back. Exits
# when the image's signature does not verify.
deploy_web() {
	local version=$1 image=$web_image:$1
	status=0
	if ! docker manifest inspect "$image" >/dev/null 2>&1; then
		say "$image is not published yet"
		status=2
		return 0
	fi
	command -v cosign >/dev/null || die "cosign is not installed"
	cosign verify "$image" \
		--certificate-identity "https://github.com/$web_repo/.github/workflows/release.yaml@refs/tags/v$version" \
		--certificate-oidc-issuer "$issuer" >/dev/null ||
		die "$image is not signed by the rim-ui release workflow of v$version"

	say "deploying the web client $version; the old env file stays as .env.previous"
	cp -p "$dir/.env" "$dir/.env.previous"
	set_web "$version"
	if compose pull web && compose up -d --no-deps web && web_healthy; then
		say "the web client runs $version"
		return 0
	fi
	say "error: the web client $version did not become healthy within ${timeout}s; putting the env file from before back"
	compose logs --tail 40 web >&2 || true
	cp -p "$dir/.env.previous" "$dir/.env"
	if compose up -d --no-deps web && web_healthy; then
		say "the web client from before runs again"
	else
		say "warning: the web client from before is not healthy either; see the logs above"
	fi
	status=1
}

# Asks the GitHub API for the latest release of repository $1 and sets `latest` to its version;
# leaves it empty, saying why, when there is none to deploy. Returns 1 when the API fails.
latest_release() {
	local code tag
	latest=""
	code=$(curl -sS -o "$work/release.json" -w '%{http_code}' \
		-H 'Accept: application/vnd.github+json' \
		"https://api.github.com/repos/$1/releases/latest") || {
		say "error: the GitHub API did not answer"
		return 1
	}
	# GitHub answers 404 when there is no published release.
	if [[ $code == 404 ]]; then
		say "$1: no release yet"
		return 0
	fi
	if [[ $code != 200 ]]; then
		say "error: the GitHub API answered $code: $(head -c 300 "$work/release.json")"
		return 1
	fi
	if grep -Eq '"(prerelease|draft)": *true' "$work/release.json"; then
		say "$1: the latest release is a pre-release or a draft; nothing to deploy"
		return 0
	fi
	tag=$(grep -o '"tag_name": *"[^"]*"' "$work/release.json" | sed -n '1s/.*"\([^"]*\)"$/\1/p')
	if [[ -z $tag ]]; then
		say "error: the latest release of $1 has no tag_name"
		return 1
	fi
	if [[ $tag != v* || ! ${tag#v} =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
		say "$1: the latest release's tag $tag is not v<major>.<minor>.<patch>; ignoring it"
		return 0
	fi
	latest=${tag#v}
}

# The rim-server part of --check. Sets `failed` when the API or the deploy failed, and
# `server_failed` as well for the latter. Called outside a condition, so that `set -e` holds in
# the deploy.
check_server() {
	local version current
	if ! latest_release "$repo"; then
		failed=1
		return 0
	fi
	version=$latest
	[[ -n $version ]] || return 0
	current=$(setting RIM_VERSION)
	if [[ $version == "$current" ]]; then
		say "$version is running; nothing to do"
		return 0
	fi
	if [[ -f $dir/FAILED && $(<"$dir/FAILED") == "$version" ]]; then
		say "$version failed to deploy before; remove $dir/FAILED to try it again, or run rim-deploy $version"
		return 0
	fi
	say "release $version is out; ${current:-nothing} runs"
	deploy "$version"
	case $status in
	0) rm -f "$dir/FAILED" ;;
	2) say "$version is not deployable yet; the next check tries again" ;;
	*)
		printf '%s\n' "$version" >"$dir/FAILED"
		say "error: the deploy of $version failed; $dir/FAILED holds it back from the next checks"
		failed=1
		server_failed=1
		;;
	esac
}

# The rim-ui part of --check. Sets `failed` when the API or the deploy failed; called outside a
# condition as check_server is.
check_web() {
	local version current
	if ! current=$(current_web); then
		say "RIM_WEB_IMAGE names a custom web image; the web client is not deployed"
		return 0
	fi
	if [[ -z $(setting RIM_VERSION) ]]; then
		say "no server version runs yet; the web client waits for it"
		return 0
	fi
	if ! runs_web; then
		say "the deployment runs no web client"
		return 0
	fi
	if ! latest_release "$web_repo"; then
		failed=1
		return 0
	fi
	version=$latest
	[[ -n $version ]] || return 0
	if [[ $version == "$current" ]]; then
		say "web client $version is running; nothing to do"
		return 0
	fi
	if [[ -f $dir/FAILED-web && $(<"$dir/FAILED-web") == "$version" ]]; then
		say "web client $version failed to deploy before; remove $dir/FAILED-web to try it again, or run rim-deploy web $version"
		return 0
	fi
	say "rim-ui release $version is out; the web client runs $current"
	deploy_web "$version"
	case $status in
	0) rm -f "$dir/FAILED-web" ;;
	2) say "web client $version is not deployable yet; the next check tries again" ;;
	*)
		printf '%s\n' "$version" >"$dir/FAILED-web"
		say "error: the deploy of web client $version failed; $dir/FAILED-web holds it back from the next checks"
		failed=1
		;;
	esac
}

usage="usage: rim-deploy --check | rim-deploy <version> | rim-deploy web <version>"
[[ ($# -eq 1 && $1 != web) || ($# -eq 2 && $1 == web) ]] || die "$usage"
[[ -d $dir ]] || die "$dir does not exist"

exec 9>"$lock"
flock -n 9 || die "another deploy is running"
work=$(mktemp -d)
# Set while the directory holds files of a deploy that has not completed; the EXIT trap then
# puts the files from before back.
# shellcheck disable=SC2034 # read by the EXIT trap
restore=0
trap 'if ((restore)); then restore_files; fi; rm -rf "$work"' EXIT

if [[ $1 == --check ]]; then
	if [[ -e $dir/HOLD ]]; then
		say "$dir/HOLD exists; deploying nothing until it is removed"
		exit 0
	fi
	failed=0
	server_failed=0
	check_server
	if ((server_failed)); then
		say "the web client is not deployed in the run of a failed server deploy"
	else
		check_web
	fi
	exit "$failed"
fi

if [[ $1 == web ]]; then
	version=$2
	[[ $version =~ ^$version_pattern$ ]] ||
		die "$version is not a version: <major>.<minor>.<patch>[-<pre-release>]"
	current_web >/dev/null ||
		die "RIM_WEB_IMAGE names a custom web image; remove it from $dir/.env to deploy the web client by version"
	runs_web || die "the deployment runs no web client"
	deploy_web "$version"
	if ((status == 0)); then
		rm -f "$dir/FAILED-web"
	fi
	exit "$status"
fi

version=$1
[[ $version =~ ^$version_pattern$ ]] ||
	die "$version is not a version: <major>.<minor>.<patch>[-<pre-release>]"
deploy "$version"
if ((status == 0)); then
	rm -f "$dir/FAILED"
fi
exit "$status"
