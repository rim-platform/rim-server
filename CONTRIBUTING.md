# Contributing

Anyone can contribute. This repository is licensed under the Apache License 2.0 (see `LICENSE`),
and contributions are accepted under the same license.

Everyone taking part follows the code of conduct in `CODE_OF_CONDUCT.md`. A vulnerability is
not reported as an issue or a pull request: `SECURITY.md` says how to report it privately.

## Pull requests

Use whatever tools you like to write the code, including AI assistants. The pull request has to
describe what it implements and why, in your words. A diff without that is sent back unread.

Every pull request against `master` says:

- **What changes, and for whom**: the behaviour a user, an operator or a client author sees
  after it merges.
- **What it does not do**: the cases it leaves out on purpose, so a reviewer does not look for
  them.
- **How it was tested**: the commands you ran and the tail of their output (see "Gates" below).
  If a suite could not run, say which and why.
- **Which spec or doc it changes**: a change to a contract in `docs/specs/` or to a page in
  `docs/` is part of the same pull request as the code.

A change an operator, a user or a client author sees adds its line under `Unreleased` in
`CHANGELOG.md`; the top of that file says how.

Open an issue first for anything beyond a fix: a new feature, a new GraphQL root, a migration, a
new dependency, a change to a spec. Agreeing on the shape before the code saves both sides a
rewrite.

## What belongs in the core

The server is a decentralized chat platform plus the programmable layer: commands, keybinds,
macros, pipelines, plugins and the AI command surface. A feature for one kind of community (an
incident tracker, a game tool, a ticket workflow) is a vertical, and verticals are built as
plugins, bots or pipelines on top of the core, not inside it. When a request can be met by a
plugin, a bot, a pipeline or a macro, the core gains the primitive that makes it possible and
the vertical stays outside. `docs/vision.md` ("Principles") has the reasoning, and
`docs/architecture.md` ("Design Guardrails") the rules: every new server feature also answers
what it does in an encrypted conversation and with more than one server.

## Gates

A pull request is merged when all of these pass locally; paste their tails into the pull
request.

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo audit --deny unsound
```

A change to the shell scripts in `deploy/` or `scripts/` also passes these, which CI runs in
the `Lint & Format` job:

```sh
shellcheck -x deploy/*.sh scripts/release/*.sh scripts/deploy/*.sh
scripts/release/test-set-version.sh
scripts/deploy/test-deploy.sh                # upgrade.sh and rim-deploy.sh with stub commands
```

`cargo audit --deny unsound` fails on a known vulnerability or an unsoundness advisory in
`Cargo.lock`. An advisory that cannot be fixed yet goes in `.cargo/audit.toml` with why it does
not apply and when the entry comes out.

`cargo test --workspace` includes `cargo test -p migration`, which checks that every migration
file is registered in `migration/src/lib.rs` and that every migration has a `down` that undoes
its `up`. `sea migrate generate` has dropped registrations when it rewrites that file, so run it
after generating a migration.

Three feature-gated suites need a running PostgreSQL 17 and `DATABASE_URL`. They fail loudly
when `DATABASE_URL` is unset; leave the feature flag off to skip them.

```sh
cargo test -p rim_infra_db --features db-tests -- --test-threads=1
cargo test -p rim_api --features e2e-tests -- --test-threads=1
cargo test -p rim_infra_services --features infra-db-tests -- --test-threads=1
```

The `rim_api` end-to-end suite also runs two servers in one process
(`rim_api/tests/s2s_e2e.rs`) to test server-to-server traffic over loopback. The second server
uses a second database named like the one in `DATABASE_URL` with `b` appended
(`rim_test` and `rim_testb`); the suite creates it when it is missing, so the database user
needs the right to create databases.

Run suites that share a database with `--test-threads=1`.

## Code conventions

`CLAUDE.md` describes the layering, the patterns and the conventions; the points reviewers check
most:

- **Layers.** `rim_api` and the non-GraphQL transports in `rim_server` call service traits from
  `rim_services`, never their implementations, and only services reach repositories. The
  composition root in `rim_server/src/app.rs` is the one place that builds the implementations;
  `CLAUDE.md` describes both exceptions to the layer chain.
- **Errors.** No `unwrap` or `expect` in library code; `ServiceResult` at service boundaries.
- **Imports.** `std`, then external crates, then local modules, separated by blank lines.
- **Visibility.** New modules are `pub(crate)` unless something outside the crate uses them.
- **Migrations.** Generate them with `sea migrate generate <name>`, never by hand. A
  migration only expands the schema (a table, a column that is nullable or has a default, an
  index), so the previous release's server keeps working on it and an operator can roll the
  binary back. Removing or renaming takes two releases: one stops using the column or table, the
  next drops it, with an **Upgrade:** line in the changelog. Every `down` undoes its `up`.
  `docs/operations/upgrade.md` is the operator's side of this.
- **Comments** say what an item is and does, what it accepts and what it returns; rustdoc opens
  with a summary sentence and has `# Errors` and `# Panics` where they apply. No history, no
  references to docs, plans, tickets or pull requests, no finding ids, persona names or TODOs.
  `cargo test --workspace` fails on a comment that breaks this and names the line.

## Commit messages

A plain imperative first line of at most 72 characters ("Add slow mode to channels"), a blank
line, and a body that says why the change is made. No trailers of any kind: no
`Co-Authored-By`, no `Signed-off-by`.

## GraphQL schema changes

The clients in `rim-ui` generate their TypeScript types from the server's schema. A pull request
that adds or changes a GraphQL type, field, enum or argument exports the schema for them. With
the server running locally:

```sh
curl -s http://localhost:8080/api/sdl > ../rim-ui/schema.graphql
cd ../rim-ui && npm run codegen
```

Enum variants become `SCREAMING_SNAKE` and fields `camelCase` in the schema. New resolvers go
into the last `QueryGroupN` or `MutationGroupN` (or into an existing domain object) so the field
order of the existing schema does not change; `CLAUDE.md` explains why.

## Who can merge and publish

- **Every change is a pull request.** Nobody pushes to `master` directly, the maintainer
  included.
- **CI must pass.** The jobs of `.github/workflows/ci.yaml` are required checks; a pull request
  whose checks fail or did not run cannot merge.
- **The maintainer merges.** `.github/CODEOWNERS` names the maintainer, @dvorakj31, as the
  reviewer of every path, and only the maintainer has write access. Everyone else works from a
  fork.
- **The maintainer releases.** Only the maintainer can push a `v*` tag. The tag starts
  `.github/workflows/release.yaml`, which publishes nothing until the maintainer approves the
  run in the `release` environment.
- **Images are signed.** `ghcr.io/rim-platform/rim-server` is signed with cosign keyless by the
  release workflow. The certificate names the workflow and the tag, so an image built or signed
  anywhere else does not verify:

  ```sh
  v=<version>
  cosign verify ghcr.io/rim-platform/rim-server:$v \
    --certificate-identity https://github.com/rim-platform/rim-server/.github/workflows/release.yaml@refs/tags/v$v \
    --certificate-oidc-issuer https://token.actions.githubusercontent.com
  ```

  Each release also carries the image's SBOM (SPDX JSON), attested to the image;
  `cosign verify-attestation --type spdxjson` with the same flags checks it.
- **The maintainer holds the updater key.** The minisign key that signs rim-ui's update bundles
  is held by the maintainer, with an offline backup. It reaches a build only through rim-ui's
  `release` environment, whose runs the maintainer approves. A lost updater key cannot be
  replaced for apps already installed: they trust only its public key, so they get no further
  updates and have to be reinstalled by hand.

### Repository settings

The rules above live in the repository's settings on GitHub, which the maintainer keeps:

1. **Ruleset on `master`.** Require a pull request before merging, with one approval and review
   from Code Owners. Require the status checks `Lint & Format`, `Test & Coverage`,
   `Security Audit` and `Docker Build`. Block force pushes and deletion. Bypass list: the
   maintainer, for pull requests only, so the maintainer's own pull requests merge without a
   second reviewer.
2. **Ruleset on tags `v*`.** Restrict creations, updates and deletions. Bypass list: the
   maintainer only.
3. **Environment `release`.** Required reviewer: the maintainer, with "Prevent self-review" off
   (the maintainer pushes the tag and approves the run). Deployment branches and tags: the tag
   pattern `v*` only. Administrators may not bypass the protection rules.
4. **Actions.** Workflow permissions: read repository contents and packages. Actions may not
   create or approve pull requests. Workflows from forks need approval for all external
   contributors. Allowed actions: GitHub's own and those the workflows name (`docker/*`,
   `actions-rust-lang/setup-rust-toolchain`, `taiki-e/install-action`, `anchore/sbom-action`,
   `sigstore/cosign-installer`). Requiring actions pinned to a full commit SHA can follow once
   `ci.yaml` pins its actions as `release.yaml` does.
5. **Access.** Nobody but the maintainer holds the write role or above.
6. **Package `rim-server`.** After the first release: visibility public, and under "Manage
   Actions access" only this repository, with the write role.
