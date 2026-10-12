# Changelog

What changes between rim-server releases, for the people who run a server and the people who
write a client. The format is [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the
versions follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html); a release is the
tag `v<version>`, the version every crate in the workspace carries.

## How entries are written

- A pull request that changes what an operator, a user or a client author sees adds its line
  under `Unreleased` in the same pull request. Refactors, tests and internal documentation do
  not.
- Each line goes under one heading: `Added`, `Changed`, `Deprecated`, `Removed`, `Fixed` or
  `Security`. It says what changed for the reader, not how it was built.
- A change the operator has to act on (a new required setting, a migration that rewrites a
  large table, a contract migration, a changed default) starts with **Upgrade:** and says what
  to do. `docs/operations/upgrade.md` is how an upgrade runs.
- A change to the GraphQL schema or a spec in `docs/specs/` names the type, field or spec.
- At release time `Unreleased` becomes `## [<version>] - <YYYY-MM-DD>` and a new empty
  `Unreleased` starts above it (`docs/operations/release-checklist.md`).

## [Unreleased]

### Added

- `docs/operations/upgrade.md`: how to move a server to a new release and back.
- Release images: every release tag publishes `ghcr.io/rim-platform/rim-server:<version>`
  (also `<major>.<minor>`, `sha-<short sha>` and `latest`), signed with cosign keyless by the
  release workflow and with an SPDX SBOM attested to it and attached to the GitHub release.
  `docs/self-hosting.md`, "Verify the images", checks the signature.
- GraphQL `ServerInfo.auditRetentionDays`: the days command audit rows are kept before they are
  exported and deleted, null when they are kept forever. Readable without a session, like the
  rest of `serverInfo`.

### Changed

- **Upgrade:** `deploy/compose.yml` runs the published images `ghcr.io/rim-platform/rim-server`
  and `ghcr.io/rim-platform/rim-web` at `RIM_VERSION` (default `latest`) instead of building
  them. Set `RIM_VERSION` in `deploy/.env` to the release you run, and run `pull` then `up -d`
  where you ran `up -d --build`. Operators who build locally add `-f deploy/compose.dev.yml` to
  every compose command; `RIM_SERVER_IMAGE` and `RIM_WEB_IMAGE` still name other images.
- The packaged desktop app's origins (`tauri://localhost`, `http://tauri.localhost` and
  `https://tauri.localhost`) are accepted by every server without `RIM_CORS_ALLOWED_ORIGIN`.

### Fixed

- A local account's first use of the direct space, any operation that needs membership there,
  joins it as a guest instead of being refused.

[Unreleased]: https://github.com/rim-platform/rim-server/commits/master
