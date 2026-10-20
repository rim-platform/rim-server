#!/usr/bin/env bash
# Deploys rim-server releases on a host that runs published images: fetches the release tag's
# deploy/ files from GitHub into the deployment directory, RIM_DEPLOY_DIR (/srv/rim-server),
# and runs the upgrade.sh among them. Installed as /usr/local/sbin/rim-deploy; rim-deploy.timer
# runs the --check form every five minutes.
#
#   rim-deploy --check       deploys the latest published release when it is not running yet
#   rim-deploy <version>     deploys that version by hand, forward or back, without the API
#
# --check does nothing while <dir>/HOLD exists, when the latest release is a pre-release or its
# tag is not v<major>.<minor>.<patch>, when it already runs (RIM_VERSION in <dir>/.env), and
# when <dir>/FAILED names it: a deploy that fails writes its version there, and the version is
# tried again once the file is gone or by hand. When upgrade.sh finds the version's images not
# published yet (its exit 2) the check ends quietly and the next one tries again.
#
# The fetched files replace those of the same name in the directory; .env and .env.previous
# are never touched, SOURCE records the tag, and when the upgrade does not complete the files
# from before come back. One deploy runs at a time, under a lock on RIM_DEPLOY_LOCK
# (/run/rim-deploy.lock).
set -euo pipefail

repo=rim-platform/rim-server
dir=${RIM_DEPLOY_DIR:-/srv/rim-server}
lock=${RIM_DEPLOY_LOCK:-/run/rim-deploy.lock}

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

# Downloads the tag v<version>, puts its deploy/ files into the directory and runs their
# upgrade.sh, whose exit status it leaves in `status`; the files from before come back unless
# that is 0.
deploy() {
	local version=$1 url file
	url=https://github.com/$repo/archive/refs/tags/v$version.tar.gz

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
		.env | .env.previous | HOLD | FAILED | SOURCE) continue ;;
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

[[ $# -eq 1 ]] || die "usage: rim-deploy --check | rim-deploy <version>"
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
	# The latest release that is published and not a pre-release; GitHub answers 404 when there
	# is none.
	status=$(curl -sS -o "$work/release.json" -w '%{http_code}' \
		-H 'Accept: application/vnd.github+json' \
		"https://api.github.com/repos/$repo/releases/latest") || die "the GitHub API did not answer"
	if [[ $status == 404 ]]; then
		say "no release yet"
		exit 0
	fi
	[[ $status == 200 ]] || die "the GitHub API answered $status: $(head -c 300 "$work/release.json")"
	if grep -Eq '"(prerelease|draft)": *true' "$work/release.json"; then
		say "the latest release is a pre-release or a draft; nothing to deploy"
		exit 0
	fi
	tag=$(grep -o '"tag_name": *"[^"]*"' "$work/release.json" | sed -n '1s/.*"\([^"]*\)"$/\1/p')
	[[ -n $tag ]] || die "the latest release has no tag_name"
	version=${tag#v}
	if [[ $tag != v* || ! $version =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
		say "the latest release's tag $tag is not v<major>.<minor>.<patch>; ignoring it"
		exit 0
	fi
	current=$(setting RIM_VERSION)
	if [[ $version == "$current" ]]; then
		say "$version is running; nothing to do"
		exit 0
	fi
	if [[ -f $dir/FAILED && $(<"$dir/FAILED") == "$version" ]]; then
		say "$version failed to deploy before; remove $dir/FAILED to try it again, or run rim-deploy $version"
		exit 0
	fi
	say "release $version is out; ${current:-nothing} runs"
	deploy "$version"
	case $status in
	0) rm -f "$dir/FAILED" ;;
	2) say "$version is not deployable yet; the next check tries again" ;;
	*)
		printf '%s\n' "$version" >"$dir/FAILED"
		die "the deploy of $version failed; $dir/FAILED holds it back from the next checks"
		;;
	esac
	exit 0
fi

version=$1
[[ $version =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]] ||
	die "$version is not a version: <major>.<minor>.<patch>[-<pre-release>]"
deploy "$version"
if ((status == 0)); then
	rm -f "$dir/FAILED"
fi
exit "$status"
