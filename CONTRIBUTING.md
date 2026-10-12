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
the vertical stays outside. `ROADMAP.md` ("Principles for sequencing", "Architecture
guardrails") has the reasoning; every new server feature also answers what it does in an
encrypted conversation and with more than one server.

## Gates

A pull request is merged when all of these pass locally; paste their tails into the pull
request.

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

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
- **Comments** say what an item is and what it does, briefly. No history ("was", "used to",
  "now"), no references to plans, tickets, stages or pull requests, and no "see section N of the
  spec". The commit message and the pull request carry the why of a change.

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
