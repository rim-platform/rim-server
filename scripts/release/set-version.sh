#!/usr/bin/env bash
# Sets the release version of the workspace and cuts CHANGELOG.md for it.
#
#   scripts/release/set-version.sh <version>
#
# Writes <version> to `[workspace.package]` in Cargo.toml and refreshes Cargo.lock for the
# workspace crates only (no dependency changes). Then `## [Unreleased]` in CHANGELOG.md becomes
# `## [<version>] - <today, UTC>` under a new empty `## [Unreleased]`, and the link references
# at the bottom are set for the new version. When the workspace already carries <version>, only
# the changelog is cut. Refuses an invalid version, an existing `## [<version>]` section and an
# `Unreleased` section without entries, before changing anything.
set -euo pipefail

die() {
  echo "set-version: $*" >&2
  exit 1
}

[[ $# -eq 1 ]] || die "usage: $0 <version>"
version=$1
# The same pattern the release workflow checks the tag against.
[[ $version =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]] ||
  die "'$version' is not a version such as 1.2.3"

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
cd "$root"
[[ -f Cargo.toml && -f CHANGELOG.md ]] || die "no Cargo.toml and CHANGELOG.md in $root"

current=$(awk '
  /^\[/ { in_pkg = ($0 == "[workspace.package]") }
  in_pkg && /^version[[:space:]]*=/ { gsub(/^version[[:space:]]*=[[:space:]]*"|".*$/, ""); print; exit }
' Cargo.toml)
[[ -n $current ]] || die "Cargo.toml has no version under [workspace.package]"

grep -qxF '## [Unreleased]' CHANGELOG.md || die "CHANGELOG.md has no '## [Unreleased]' section"
if grep -qF "## [$version]" CHANGELOG.md; then
  die "CHANGELOG.md already has a section [$version]; choose another version"
fi
# A line other than blanks and headings between `## [Unreleased]` and the next section.
awk '
  $0 == "## [Unreleased]" { in_unrel = 1; next }
  in_unrel && (/^## / || /^\[[^]]+\]: /) { exit }
  in_unrel && /[^[:space:]]/ && !/^### / { found = 1; exit }
  END { exit !found }
' CHANGELOG.md || die "the Unreleased section of CHANGELOG.md has no entries"

today=$(date -u +%F)
# The previous release is the first versioned section; the repository URL comes from the
# existing `[Unreleased]:` link, whatever form it has.
previous=$(sed -n 's/^## \[\([^]]*\)\].*/\1/p' CHANGELOG.md | grep -vxF Unreleased | head -n 1 || true)
repo=$(sed -nE 's#^\[Unreleased\]: (https?://[^ ]*)/(commits|compare|tree)/.*#\1#p' CHANGELOG.md | head -n 1 || true)
if grep -q '^\[Unreleased\]: ' CHANGELOG.md && [[ -z $repo ]]; then
  die "cannot read the repository URL from the [Unreleased] link in CHANGELOG.md"
fi
if [[ -n $previous ]]; then
  version_link="$repo/compare/v$previous...v$version"
else
  version_link="$repo/releases/tag/v$version"
fi

if [[ $version != "$current" ]]; then
  tmp=$(mktemp)
  awk -v v="$version" '
    /^\[/ { in_pkg = ($0 == "[workspace.package]") }
    in_pkg && !done && /^version[[:space:]]*=/ { print "version = \"" v "\""; done = 1; next }
    { print }
  ' Cargo.toml >"$tmp"
  cat "$tmp" >Cargo.toml
  rm -f "$tmp"
  cargo update --workspace --offline --quiet
  echo "set-version: workspace version $current -> $version"
else
  echo "set-version: workspace already at $version; cutting the changelog only"
fi

# Headings that are empty in Unreleased are placeholders: they are dropped from the released
# section and the new Unreleased gets the standard set back.
tmp=$(mktemp)
awk -v v="$version" -v d="$today" -v unrel_link="$repo/compare/v$version...HEAD" \
  -v ver_link="$version_link" '
  function flush_heading() {
    if (heading_buf != "") {
      if (heading_has_entries) released = released heading_buf
      else placeholders = 1
    }
    heading_buf = ""; heading_has_entries = 0
  }
  function end_unreleased(more) {
    flush_heading()
    in_unrel = 0
    printf "## [Unreleased]\n\n"
    if (placeholders)
      printf "### Added\n\n### Changed\n\n### Deprecated\n\n### Removed\n\n### Fixed\n\n### Security\n\n"
    sub(/\n+$/, "", released)
    printf "%s\n%s", released, (more ? "\n" : "")
  }
  $0 == "## [Unreleased]" && !cut {
    in_unrel = 1; cut = 1
    released = "## [" v "] - " d "\n"
    next
  }
  in_unrel && (/^## / || /^\[[^]]+\]: /) { end_unreleased(1) }
  in_unrel && /^### / {
    flush_heading()
    heading_buf = $0 "\n"
    next
  }
  in_unrel {
    if (heading_buf != "") {
      heading_buf = heading_buf $0 "\n"
      if (/[^[:space:]]/) heading_has_entries = 1
    } else {
      released = released $0 "\n"
    }
    next
  }
  /^\[Unreleased\]: / {
    print "[Unreleased]: " unrel_link
    print "[" v "]: " ver_link
    next
  }
  { print }
  END { if (in_unrel) end_unreleased(0) }
' CHANGELOG.md >"$tmp"
cat "$tmp" >CHANGELOG.md
rm -f "$tmp"
echo "set-version: CHANGELOG.md cut as [$version] - $today"
