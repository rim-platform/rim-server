#!/usr/bin/env bash
# Tests set-version.sh on a throwaway workspace built from the changelogs in testdata/.
#
#   scripts/release/test-set-version.sh
#
# Needs bash, awk and cargo; works offline. Exits non-zero on the first failure.
set -euo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
data=$here/testdata
today=$(date -u +%F)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

# A one-crate workspace at version $1 with the script and CHANGELOG.md from testdata/$2.
setup() {
  rm -rf "$work/ws"
  mkdir -p "$work/ws/scripts/release" "$work/ws/demo/src"
  cp "$here/set-version.sh" "$work/ws/scripts/release/"
  cp "$data/$2" "$work/ws/CHANGELOG.md"
  cat >"$work/ws/Cargo.toml" <<TOML
[workspace]
resolver = "3"
members = ["demo"]

[workspace.package]
version = "$1"
edition = "2024"
TOML
  cat >"$work/ws/demo/Cargo.toml" <<'TOML'
[package]
name = "demo"
version.workspace = true
edition.workspace = true
TOML
  : >"$work/ws/demo/src/lib.rs"
  (cd "$work/ws" && cargo generate-lockfile --offline --quiet)
}

run() {
  "$work/ws/scripts/release/set-version.sh" "$@" >/dev/null 2>"$work/stderr"
}

expect_changelog() {
  sed "s/@DATE@/$today/g" "$data/$1" >"$work/expected"
  diff -u "$work/expected" "$work/ws/CHANGELOG.md" || fail "CHANGELOG.md differs from $1"
}

expect_version() {
  grep -qxF "version = \"$1\"" "$work/ws/Cargo.toml" || fail "Cargo.toml is not at $1"
  grep -A1 -xF 'name = "demo"' "$work/ws/Cargo.lock" | grep -qxF "version = \"$1\"" ||
    fail "Cargo.lock is not at $1"
}

# A refused run changes nothing and says why.
expect_refused() {
  local reason=$1
  shift
  local before after
  before=$(cat "$work/ws/Cargo.toml" "$work/ws/Cargo.lock" "$work/ws/CHANGELOG.md")
  if run "$@"; then fail "set-version.sh $* was not refused"; fi
  grep -qF "$reason" "$work/stderr" || fail "set-version.sh $* did not say '$reason'"
  after=$(cat "$work/ws/Cargo.toml" "$work/ws/Cargo.lock" "$work/ws/CHANGELOG.md")
  [[ $before == "$after" ]] || fail "refused set-version.sh $* changed files"
}

# The first release: the version moves, and the links point at the release tag.
setup 0.0.0 changelog.md
run 0.0.1 || fail "set-version.sh 0.0.1: $(cat "$work/stderr")"
expect_version 0.0.1
expect_changelog changelog-0.0.1.md

expect_refused "already has a section [0.0.1]" 0.0.1
expect_refused "has no entries" 0.0.2
expect_refused "is not a version" 0.2
expect_refused "is not a version" v0.0.2
expect_refused "usage" 

# A later release at the version the workspace already carries: only the changelog is cut,
# and its link compares with the previous release.
sed -i 's/^version = "0.0.1"$/version = "0.0.2"/' "$work/ws/Cargo.toml"
sed -i 's/^## \[Unreleased\]$/&\n\n### Security\n\n- A second fix./' "$work/ws/CHANGELOG.md"
cp "$work/ws/Cargo.toml" "$work/Cargo.toml.before"
run 0.0.2 || fail "set-version.sh 0.0.2: $(cat "$work/stderr")"
cmp -s "$work/Cargo.toml.before" "$work/ws/Cargo.toml" || fail "Cargo.toml changed at the same version"
expect_changelog changelog-0.0.2.md

# Empty headings are placeholders: dropped from the release, restored in Unreleased.
setup 0.0.1 placeholders.md
run 0.0.2 || fail "set-version.sh 0.0.2: $(cat "$work/stderr")"
expect_version 0.0.2
expect_changelog placeholders-0.0.2.md

echo "set-version.sh: all checks passed"
