# Release checklist

The steps that cut a rim-server release `v<version>`, in order. Copy the list into the release
pull request and tick it there. Steps marked **FILL** wait for the decisions about the
organisation's registry, release workflows and updater key; until then they are done by hand or
skipped, and the pull request says which.

The gates run locally on the release commit. CI counts only when it ran on that same commit.

## 1. Choose the version

- [ ] Read `Unreleased` in `CHANGELOG.md`. Every user-visible change since the last tag has a
      line; `git log --oneline v<previous>..master` shows what may be missing.
- [ ] Pick the version by Semantic Versioning. While the major version is 0, a breaking change to
      the GraphQL schema, the server-to-server protocol or a spec in `docs/specs/` raises the
      minor version; anything else raises the patch.
- [ ] If a GraphQL field or argument was removed or changed meaning, `API_VERSION` in
      `rim_api/src/types/server_info.rs` goes up by one.

## 2. Check the migrations

- [ ] List them: `git diff --stat v<previous>..master -- migration/src`.
- [ ] Each one only expands the schema, or drops what the previous release already stopped
      using. A drop (a contract) has an **Upgrade:** line in the changelog.
      `docs/operations/upgrade.md`, "Why migrations only go forward", is the rule.
- [ ] A migration that rewrites a large table says so in the changelog with the time it took on
      a copy of a real database.

## 3. Run the gates on the release commit

```sh
export DATABASE_URL=postgres://postgres:postgres@localhost:5432/rim_test
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy --workspace --all-targets \
  --features "rim_infra_db/db-tests,rim_infra_services/infra-db-tests,rim_api/e2e-tests" -- -D warnings
cargo test --workspace -- --test-threads=1
cargo test -p rim_infra_db --features db-tests -- --test-threads=1
cargo test -p rim_api --features e2e-tests -- --test-threads=1
cargo test -p rim_infra_services --features infra-db-tests -- --test-threads=1
cargo audit
docker build -t rim-server:release-check .
```

- [ ] Every command above passes. `cargo audit` findings are fixed or listed in the release pull
      request with the reason they do not apply.
- [ ] rim-ui's gates pass on its release commit (FILL: rim-ui's own checklist, once it has one).

## 4. Test the upgrade and the rollback

On a scratch host or compose project, never the production one:

- [ ] Start the previous release (`docs/self-hosting.md`), sign up, write messages in a channel
      and an encrypted conversation, upload a file, and take a backup with `deploy/backup.sh`.
- [ ] Upgrade to the release commit as `docs/operations/upgrade.md` says. `/readyz` answers 200
      and the data is there.
- [ ] Roll the binary back to the previous release with `dc up -d --no-deps rim web`.
      `/readyz` answers 200 and the previous client still works.
- [ ] Upgrade again: `migrate` has nothing to run and the server starts.
- [ ] `deploy/restore-drill.sh` on a production backup (FILL: which server's backups), built from
      the release commit: the new image applies its migrations to real data and reaches
      `/readyz`.

## 5. Check the clients against the server

- [ ] With the release commit running, `curl -s http://localhost:8080/api/sdl` matches the
      `schema.graphql` committed in rim-ui's release commit.
- [ ] In rim-ui, `npm run skew:check -- <previous rim-ui release>` lists what the new client sends
      that a server on the previous release refuses. Anything it lists goes into the changelog as
      an **Upgrade:** line (servers before the client).

## 6. Version, changelog, pull request

- [ ] Set `version` under `[workspace.package]` in the root `Cargo.toml` and run
      `cargo build --workspace` so `Cargo.lock` follows.
- [ ] Set rim-ui's version (`package.json`, `src-tauri/tauri.conf.json`, `src-tauri/Cargo.toml`).
      FILL: whether rim-ui's version follows the server's or has its own.
- [ ] In `CHANGELOG.md`, rename `Unreleased` to `[<version>] - <YYYY-MM-DD>`, start a new empty
      `Unreleased` above it, and set the links at the bottom:

      ```text
      [Unreleased]: https://github.com/rim-platform/rim-server/compare/v<version>...HEAD
      [<version>]: https://github.com/rim-platform/rim-server/compare/v<previous>...v<version>
      ```

      The first release links `[<version>]` to `…/releases/tag/v<version>` instead.
- [ ] Open the pull request "Release <version>" with this list ticked and the gate tails, and
      merge it.

## 7. Tag

```sh
git checkout master && git pull
git tag -s v<version> -m "rim-server <version>"       # FILL: who signs release tags, with which key
git push origin v<version>
```

- [ ] The tag points at the merged release commit, and `rim-server --version` built from it
      prints `<version>`.
- [ ] rim-ui is tagged at its release commit. FILL: rim-ui's tag name.

## 8. Publish

- [ ] Server image: FILL the registry and image name, and the release workflow that builds and
      pushes it on the tag. Until that workflow exists, build from the tag and push by hand:

      ```sh
      docker build -t <FILL registry>/rim-server:<version> .
      docker push <FILL registry>/rim-server:<version>
      ```

- [ ] Web client image from rim-ui's tag: FILL the image name and workflow.
- [ ] Desktop builds: FILL the rim-ui release workflow. The updater stays off until
      `src-tauri/updater.json` in rim-ui has the endpoint and the public key: FILL the updater
      endpoint, and where the signing key lives and who holds it.
- [ ] Release notes: the changelog section, published as a GitHub release on the tag:

      ```sh
      gh release create v<version> --repo rim-platform/rim-server \
        --title "rim-server <version>" --notes-file <the changelog section>
      ```

      The repositories are private: FILL whether releases wait until they are public.

## 9. After

- [ ] Upgrade the servers the project runs (FILL: which servers and relays) as
      `docs/operations/upgrade.md` says, a backup first.
- [ ] Watch `/readyz`, the logs and the metrics (`docs/operations/metrics.md`) for a day.
- [ ] Announce the release. FILL: where.
