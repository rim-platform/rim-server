<!-- CONTRIBUTING.md explains each section. A pull request without them is sent back unread. -->

## What changes, and for whom

<!-- The behaviour a user, an operator or a client author sees after this merges, and why. -->

## What it does not do

<!-- The cases left out on purpose, so a reviewer does not look for them. -->

## How it was tested

<!-- The commands you ran and the tail of their output. If a suite could not run, say which and
why. -->

```text
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## Docs and specs

<!-- The page in `docs/` or the contract in `docs/specs/` this changes, or "none". A GraphQL
change also exports the schema for rim-ui. -->

## Changelog

<!-- The line added under `Unreleased` in CHANGELOG.md, or why there is none (refactor, tests,
internal documentation). -->
