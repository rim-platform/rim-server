# Track B follow-ups — CI feature-compile gap + test-lint debt + attribute-order convention

**Author:** iron-man · **Implementer:** hulk · **Priority:** low/medium — hygiene, prevents silent rot.
Bundles three small items surfaced during B2/B3 implementation.

## Item 1 — CI does not compile feature-gated test code (the real gap)
hulk proved that `cargo build/clippy --workspace` does **not** compile the code behind the
`db-tests` / `infra-db-tests` / `e2e-tests` features (those are `#![cfg(feature = "…")]`-gated test
binaries). As a result, feature-gated test code rots invisibly: 2 files had silently broken during
B2, 3 more during B3, each only discovered when someone ran the feature suite. CI has a blind spot.

**Fix (CI, no DB required for the compile check):** add a CI step that *compiles* (not runs) the
feature-gated code so breakage fails the build immediately:
```sh
cargo clippy --workspace --all-targets \
  --features "rim_infra_db/db-tests,rim_infra_services/infra-db-tests,rim_api/e2e-tests" \
  -- -D warnings
```
or, simpler, `cargo clippy --workspace --all-targets --all-features -- -D warnings`.
- **Key point:** `clippy`/`build` *compiles* the test binaries without *running* them, so this
  needs **no `DATABASE_URL`** and won't hit the loud-skip panic (that only fires at test *runtime*).
  It purely catches compile rot. Keep it a separate CI step from the actual db-test *run* job
  (which does need a Postgres service + `DATABASE_URL`, and already exists).
- Add it next to the existing `clippy -D warnings` step in the GitHub Actions workflow. Confirm
  `--all-features` doesn't enable anything runtime-hostile at compile time (it shouldn't — these are
  all empty marker features gating test cfg).

**Acceptance:** deleting a symbol used only by a `db-tests` file makes the new CI step fail. Verify
locally by running the command above and confirming it compiles today's tree clean.

## Item 2 — `bool_assert_comparison` clippy debt in repo tests
Pre-existing lint debt: 5 repo-test files use `assert_eq!(x, true)` / `assert_eq!(x, false)` which
clippy's `bool_assert_comparison` flags. These don't fail today only because the feature-gated test
code isn't linted (Item 1). Once Item 1 lands, they will. Fix them proactively:
- Replace `assert_eq!(expr, true)` → `assert!(expr)` and `assert_eq!(expr, false)` → `assert!(!expr)`.
- Find them with: `cargo clippy --workspace --all-targets --all-features 2>&1 | grep bool_assert`
  (after Item 1's command compiles them). hulk flagged ~5 repo-test files; fix each occurrence.

**Acceptance:** the `--all-features` clippy step (Item 1) is clean with `-D warnings`.

## Item 3 — document the automock / `async_trait` attribute-order convention
hulk noted an inconsistency across the codebase in the ordering of `#[cfg_attr(test, automock)]`
(or `#[cfg_attr(feature = "mocks", automock)]`) relative to `#[async_trait]` on trait definitions.
With `mockall::automock` + `async_trait`, the attribute order matters (`automock` must see the trait
before `async_trait` desugars it, or vice versa depending on setup), and mixed ordering is a
latent footgun for the next trait author.

**Fix (documentation only, no code change required unless a trait is actually miscompiling):**
- Pick the ordering that currently works across the codebase's mockable async traits (inspect a few
  known-good ones — e.g. the service traits in `rim_services` that already generate `Mock*` types —
  and codify whichever order they use).
- Add a short subsection to `CLAUDE.md` under Conventions: "Mockable async traits: put
  `#[cfg_attr(<mock-cfg>, automock)]` <above|below> `#[async_trait]`, consistently — mixed order is
  a compile footgun." State the chosen order and one-line why.
- Optionally normalize any existing trait that deviates (low value; only if trivial). The doc is the
  deliverable; a sweeping refactor is not in scope.

**Acceptance:** the convention is written in CLAUDE.md; no behavior change.

## Notes
- Items 1 and 2 are coupled: land Item 1 (which surfaces the lints) and Item 2 (which fixes them)
  together so the new CI step is green on first run.
- None of this touches production code paths, schema, or the GraphQL contract. Pure CI + test + docs.
