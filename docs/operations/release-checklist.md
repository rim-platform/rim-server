# Release checklist

The steps that cut a rim-server release `v<version>`, in order. Copy the list into the release
pull request and tick it there. Steps marked **FILL** wait for decisions not taken yet, such as
the key that signs release tags; until then they are done by hand or skipped, and the pull
request says which. Only the maintainer tags and publishes;
`CONTRIBUTING.md`, "Who can merge and publish", lists the repository settings that enforce it.

The gates run locally on the release commit. CI counts only when it ran on that same commit.

## Before the first public release

One-time sign-offs for the launch. Tick them in the first release pull request, with a link or
a date for each.

- [ ] The state of the outside cryptographic review of Stages 3 and 4 is linked here. `0.0.x`
      releases go out before its sign-off; `0.1.0` waits for it (section 1). The brief is
      `docs/security/crypto-review-brief-2026-10.md`; its "Status as of 2026-10-14" names the
      numbered items already fixed.
- [ ] A public operating profile is chosen, set in the deployment's `deploy/.env` and tested with
      the packaged client, including how the client shows a refusal and retries. Each of these is
      off when unset or 0 (the message burst follows the per-minute rate), and
      `deploy/compose.relay.yml` sets none of them:
      - API rates: `RIM_RATE_REQUESTS_PER_MIN_PER_IP`, `RIM_RATE_MUTATIONS_PER_MIN_PER_SESSION`,
        `RIM_RATE_MESSAGES_PER_MIN_PER_USER`, `RIM_RATE_MESSAGES_BURST`.
      - WebSocket and subscriptions: `RIM_RATE_WS_CONNECTIONS_PER_USER`,
        `RIM_RATE_SUBSCRIPTIONS_PER_CONNECTION`.
      - Instance caps: `RIM_LIMIT_WORKSPACES_PER_USER`, `RIM_LIMIT_MEMBERS_PER_WORKSPACE`,
        `RIM_LIMIT_CHANNELS_PER_WORKSPACE`, `RIM_LIMIT_DEVICES_PER_USER`,
        `RIM_LIMIT_DM_REQUESTS_PER_DAY`, `RIM_LIMIT_MACROS_PER_WORKSPACE`,
        `RIM_LIMIT_PIPELINES_PER_WORKSPACE`, `RIM_LIMIT_PLUGINS_PER_WORKSPACE`,
        `RIM_LIMIT_INVITES_PER_USER_PER_DAY`, `RIM_LIMIT_INVITE_MAX_USES`,
        `RIM_LIMIT_MESSAGE_BYTES`, `RIM_LIMIT_PENDING_INVITES_PER_USER`.
      - Probation: `RIM_PROBATION_HOURS`, `RIM_RATE_PROBATION_MESSAGES_PER_MIN`.

      `deploy/.env.example`, "Request limits" and "Instance limits", says what each one limits.
- [ ] `RIM_SIGNUP_REQUIRE_KEYS` stays on (the default) in the deployment's `deploy/.env`, and the
      packaged desktop client and the web client both sign up with a key against it. A
      deployment that sets it to `false` says why here.
- [ ] `docs/testing/manual-test-plan-2026-10-12.md` is run on the packaged client and the HTTPS
      deployment being released, and the results are recorded: the "Steps passed" and "Not run"
      sections of `docs/testing/manual-test-findings-2026-10-12.md` are filled.
- [ ] The upgrade, rollback and restore drill of section 4 ran for this release, with its date
      recorded there.
- [ ] Desktop distribution is in place as section 8 checks it. rim-ui's release workflow builds
      installers for Linux x86_64 and Windows x86_64 on the tag, and the in-app updater reads
      `latest.json` from GitHub Releases. macOS is build-from-source only. Windows installers are
      unsigned and stay so for `0.0.x`; the install guide documents the SmartScreen prompt ("More
      info", then "Run anyway"). Every new version prompts again, because SmartScreen builds the
      reputation of an unsigned installer per file. One follow-up stays open: an Apple Developer
      account for signed macOS builds.
- [ ] The `release` environment in both repositories has the maintainer as its only required
      reviewer and allows deployments from `v*` tags only (`CONTRIBUTING.md`, "Repository
      settings").
- [ ] The updater key is generated with `tauri signer generate`, and the maintainer holds the
      private key and its password with an offline backup. In rim-ui's `release` environment the
      secrets `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` hold them. The
      rim-ui repository variables are set: `UPDATER_PUBKEY` to the public key, `UPDATER_ENDPOINT`
      to `https://github.com/rim-platform/rim-ui/releases/latest/download/latest.json` and
      `RELEASE_GRAPHQL_ENDPOINT` to `https://relay.rim-platform.org/api`, the server that release
      builds of the desktop app start with.
- [ ] DNS for `relay.rim-platform.org` points at the public relay, and
      `curl -s https://relay.rim-platform.org/.well-known/rim-server` answers with its descriptor.
      The apex `rim-platform.org` is the website and does not serve the API.
- [ ] The credentials the old `.env` exposed stay rotated: rewriting history does not revoke them
      (`docs/security/env-history-purge-runbook.md`).

## 1. Choose the version

- [ ] Read `Unreleased` in `CHANGELOG.md`. Every user-visible change since the last tag has a
      line; `git log --oneline v<previous>..master` shows what may be missing.
- [ ] Pick the version by Semantic Versioning. While the version is `0.0.x`, every release raises
      the patch and any change may break, including the GraphQL schema, the server-to-server
      protocol and the specs in `docs/specs/`; a breaking change still gets its **Upgrade:**
      line. `0.1.0` follows once the outside cryptographic review is signed off. From then, while
      the major version is 0, a breaking change raises the minor version and anything else
      raises the patch.
- [ ] The version is a plain `<major>.<minor>.<patch>`: there are no release candidates.
- [ ] rim-server and rim-ui release together. Both carry the same version and are tagged the
      same day, even when one of them has no changes.
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
cargo audit --deny unsound
docker build -t rim-server:release-check .
```

- [ ] Every command above passes. `cargo audit` findings are fixed or listed in
      `.cargo/audit.toml` with the reason they do not apply and when the entry comes out.
- [ ] rim-ui's gates pass on its release commit (FILL: rim-ui's own checklist, once it has one).

## 4. Test the upgrade and the rollback

On a scratch host or compose project, never the production one. The release is not published
yet, so the new images are built from the release commits with `deploy/compose.dev.yml`:

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
- [ ] Record the drill in the release pull request: `Drill run: <YYYY-MM-DD>`, the host, and the
      previous release it started from.

## 5. Check the clients against the server

- [ ] With the release commit running, `curl -s http://localhost:8080/api/sdl` matches the
      `schema.graphql` committed in rim-ui's release commit.
- [ ] In rim-ui, `npm run skew:check -- <previous rim-ui release>` lists what the new client sends
      that a server on the previous release refuses. Anything it lists goes into the changelog as
      an **Upgrade:** line (servers before the client).

## 6. Version, changelog, pull request

- [ ] Run `scripts/release/set-version.sh <version>`. It sets `version` under
      `[workspace.package]` in the root `Cargo.toml`, refreshes `Cargo.lock` for the workspace
      crates only, and cuts `CHANGELOG.md`: `Unreleased` becomes `[<version>] - <YYYY-MM-DD>`
      (today in UTC) under a new empty `Unreleased`, and the links at the bottom compare with the
      previous release (the first release links to `…/releases/tag/v<version>`). It refuses an
      invalid version, a version the changelog already has and an empty `Unreleased`, before it
      changes anything. `scripts/release/test-set-version.sh` tests it; CI's lint job runs that.
- [ ] In rim-ui, run `npm run version:set -- <version>`. It sets the same version in
      `package.json`, `src-tauri/Cargo.toml` and the lockfiles and cuts rim-ui's `CHANGELOG.md`
      the same way. The compose files run both images at one `RIM_VERSION`, and rim-ui's
      release workflow refuses a tag that differs from `package.json`.
- [ ] Open the pull request "Release <version>": the scripts' diff, with this list ticked and
      the gate tails. Merge it.

## 7. Tag

```sh
git checkout master && git pull
git tag -s v<version> -m "rim-server <version>"       # FILL: the key that signs release tags
git push origin v<version>
```

- [ ] The tag points at the merged release commit, and `rim-server --version` built from it
      prints `<version>`. Only the maintainer can push a `v*` tag.
- [ ] rim-ui is tagged `v<version>` at its release commit the same way.

## 8. Publish

Pushing the tag starts `.github/workflows/release.yaml` in each repository. Its
`build-and-publish` job waits in the `release` environment until the maintainer approves it. The
job checks the tag against the version in `Cargo.toml` (rim-ui: `package.json`) and the
changelog's section for it, builds the image for `linux/amd64`, pushes the tags `<version>`,
`<major>.<minor>`, `sha-<short sha>` and `latest`, signs the digest with cosign keyless and
attests an SPDX SBOM to it. The next job creates the GitHub release with the changelog section
and the SBOM. rim-ui's workflow also builds the desktop installers, one leg for Linux x86_64 and
one for Windows x86_64, and attaches them to its release with `latest.json` for the updater,
`SHA256SUMS` signed with cosign keyless and a build-provenance attestation for each bundle.

- [ ] Approve the run of both repositories on its page ("Review deployments", environment
      `release`), after checking that it ran for the tag just pushed.
- [ ] Both runs pass. One that fails before "Build and push" published nothing: fix the cause,
      delete the tag and push it again. Once an image is pushed the version is spent, and a fix
      goes out as the next patch.
- [ ] Verify the signatures. The certificate must name each repository's release workflow at
      the tag; any other identity fails:

      ```sh
      v=<version>
      cosign verify ghcr.io/rim-platform/rim-server:$v \
        --certificate-identity https://github.com/rim-platform/rim-server/.github/workflows/release.yaml@refs/tags/v$v \
        --certificate-oidc-issuer https://token.actions.githubusercontent.com
      cosign verify ghcr.io/rim-platform/rim-web:$v \
        --certificate-identity https://github.com/rim-platform/rim-ui/.github/workflows/release.yaml@refs/tags/v$v \
        --certificate-oidc-issuer https://token.actions.githubusercontent.com
      ```

      The digest cosign prints matches the one in the GitHub release notes, and
      `cosign verify-attestation --type spdxjson` with the same flags passes.
- [ ] The first release only: set both packages public (package settings on GitHub), as
      `CONTRIBUTING.md`, "Who can merge and publish", lists.
- [ ] Desktop builds: wait for both legs of rim-ui's run. The release is published only when
      every asset is attached; until then it is not the `latest` the updater reads.
- [ ] Install one Linux bundle and one Windows bundle from the release page, sign in and send a
      message. Build macOS from the tag once, as rim-ui's README says, and start it.
- [ ] `latest.json` is attached to the release, and
      `https://github.com/rim-platform/rim-ui/releases/latest/download/latest.json` names
      `<version>`. From the second release on: Settings > About in the previous version offers
      the new one, and installing it from there works.
- [ ] Verify the checksums and the provenance. The certificate must name rim-ui's release
      workflow at the tag:

      ```sh
      v=<version>
      gh release download v$v --repo rim-platform/rim-ui --dir rim-ui-$v
      cd rim-ui-$v
      cosign verify-blob SHA256SUMS --bundle SHA256SUMS.sigstore.json \
        --certificate-identity https://github.com/rim-platform/rim-ui/.github/workflows/release.yaml@refs/tags/v$v \
        --certificate-oidc-issuer https://token.actions.githubusercontent.com
      sha256sum --check --ignore-missing SHA256SUMS
      gh attestation verify <one bundle> --repo rim-platform/rim-ui
      ```
- [ ] A broken desktop release is rolled back by marking its GitHub release as a pre-release,
      which moves the updater's `latest` back to the previous release, or by deleting its
      assets. Its version and images are never reused: the fix goes out as the next patch.

## 9. After

- [ ] Upgrade the public relay `relay.rim-platform.org` and the other servers the project runs
      (FILL: which servers besides the relay) as `docs/operations/upgrade.md` says, a backup
      first.
- [ ] Watch `/readyz`, the logs and the metrics (`docs/operations/metrics.md`) for a day.
- [ ] Announce the release. FILL: where.
