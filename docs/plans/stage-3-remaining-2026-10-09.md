# Stage 3, the remaining work (plan, 2026-10-09)

_Written 2026-10-09 after rim-server #35 and rim-ui #27 merged. Stage 3 ("You are a key") is
Track 7.2 plus Tracks 2.3 and 2.4 in `ROADMAP.md`. This file lists what is left, as PRs in the
order they ship, and is the brief each PR's implementer works from. Contract for everything
identity-related: `docs/specs/key-identity-v1.md`._

## Done so far

| Repo | PR | What |
|---|---|---|
| rim-server | #30 | `rim_identity`, key tables, enrollment, device add and revoke, rotation and recovery, profile cards, challenge login |
| rim-server | #33, #34 | Algorithm-agile statements (version 2), public `keyLog`, the 256-entry cap |
| rim-ui | #24 | Linux keyring service for secrets |
| rim-ui | #26 | Desktop client: enroll (recovery kit v1), sign in with the device key, rotate, publish the card, recover from the kit |
| rim-ui | #28 | S3-3: recovery kit v2, the 12-word recovery phrase |
| rim-server | #36 | S3-2: `docs/specs/device-linking-v1.md` |
| rim-server | #37 | S3-1: key-only signup (`signupWithKey`, nullable password and email) |
| rim-ui | #29 | S3-5: device linking on the desktop (old and new device) |
| rim-ui | #30 | S3-6: key-only signup on the desktop join pages |
| rim-server | #39 | S3-4: device linking on the server (`device_link` table, service, GraphQL, `rim_identity::device_link`) |
| rim-server, rim-ui | #41, #32 | S3-8a: `logoutOtherSessions` and the sign-out offer |
| rim-server, rim-ui | #40, #31 | CLAUDE.md: the coordinating session reviews; comment and commit rules |
| rim-ui | #33 | S3-7: the web app as a linked device (WebCrypto keys, link by code, key sign-in) |
| rim-ui | #35 | S3-8b: identity-key backup on the drive, takeover on a linked device, archive key wrapped under the identity key |
| rim-server | #45 | W4-0: `web-plugins-v1.md` and `workspace-plugins-v1.md` |
| rim-server | #46 | S3-10a: workspace plugin registry, signed manifests, bundle endpoints |
| rim-ui | #36 | S3-9: web plugins (`WebSandboxed` kind, frame shell, host, SDK transport, GitHub PRs showcase) |
| rim-server | #48 | `ServerMember.principalId`, for the trust badge |
| rim-ui | #37 | S3-10b: registry client (verify on both platforms, sign and publish, palette, required/update/removal flows) |

The user id shape in plugins (7.0), the local cache, local search and the drive archive
(Stage 2) are also in.

## Status

**Stage 3 is complete as of 2026-10-09.** Every row below shipped; the table above lists the PRs.
What remains is manual: one real run of device linking (desktop ↔ desktop, desktop ↔ browser),
of the identity-key takeover on a linked device, and of a plugin signed on one account and
installed on another, on the desktop and on the web. Stage 4 (MLS encryption) starts only on
Jakub's go.

## The PRs, as planned

Sizes: S under a day, M a day or two, L several days of agent work.

### Wave 1 (independent, run in parallel)

| # | Repo | PR | Size |
|---|---|---|---|
| S3-1 | rim-server | **Key-only signup.** `signupWithKey` creates an account from a key log, a device certificate and a login proof, with no password and no email. `user.password_hash` and `user.email` become nullable; password login and password reset refuse such accounts with the same timing and answers as for enrolled accounts. Honors the registration mode (bootstrap, instance and workspace invite tokens; open mode with the signup rate limit). Sets the session and CSRF cookies like `signupWithInvite`. Spec section replaces the "Deferred" note. | M |
| S3-2 | rim-server | **Device linking spec**, `docs/specs/device-linking-v1.md`: the server-side link mailbox (anonymous new device, signed-in old device, sealed messages the server cannot read, short TTL, unguessable link id, rate limits), the QR and paste-able code format, the sealed message format (key derived from the one-time secret), what the old device hands over (the device certificate; whether the archive key rides along), the GraphQL surface, the failure and retry rules, and what the web client needs from it (S3-7). | S |
| S3-3 | rim-ui + spec | **12-word recovery phrase** (recovery kit v2). A BIP39 English phrase (128-bit entropy, checksum word) derives the recovery seed; the kit shows the phrase instead of the `RK1-` key; readers accept both kit formats; recovery takes a phrase or an `RK1-` key. The spec's "Recovery kit" section documents v2 beside v1. | S/M |

### Wave 2 (after wave 1)

| # | Repo | PR | Size |
|---|---|---|---|
| S3-4 | rim-server | **Device linking, server side**, from the S3-2 spec: the link table or store, the mutations and query, e2e test. | M |
| S3-5 | rim-ui | **Device linking, desktop**: Settings > Security > "Link a device" shows the QR and the code; the sign-in screen's "Link this device" takes the code; the new device makes its key, seals it, polls; the old device verifies, signs the certificate with the identity key, calls `addDeviceKey`, republishes the profile card, answers through the mailbox; the new device signs in. | M |
| S3-6 | rim-ui | **Key-only signup UI**: the join and open-signup pages offer "Create account with a key" on desktop (no password or email), reusing the enrollment draft and the recovery kit; password signup stays for the web app until S3-7. | S/M |

### Wave 3

| # | Repo | PR | Size |
|---|---|---|---|
| S3-7 | rim-ui | **The web app as a linked device**: non-extractable Ed25519 WebCrypto keys in IndexedDB, version 2 statements signed in the browser, "Link this device" from the sign-in page, key sign-in; `deviceKeys` capability on for the web with the weaker-storage notice. Needs S3-4 and S3-5. | L |
| S3-8a | rim-server + rim-ui | **Sign out other sessions** after enrolling (`logoutOtherSessions`). Shipped. | S |
| S3-8b | rim-ui + spec | **Identity-key backup on the drive**, unlocked by the recovery phrase, and the archive key wrapped under the identity key (the archive's second unwrap path), so a linked device can take over the identity key when the first device is gone. | M |

### Wave 4 (plugin pillar, the rest of Stage 3)

| # | Repo | PR | Size |
|---|---|---|---|
| W4-0 | rim-server | **Specs**: `docs/specs/web-plugins-v1.md` and `docs/specs/workspace-plugins-v1.md`. Shipped. | S |
| S3-9 | rim-ui | **Web-capable plugins** (Track 2.3): the `WebSandboxed` kind in a sandboxed iframe (frame shell, `MessagePort` transport, `network.fetch`, storage, bundles, SDK transport and browser build, the GitHub PRs showcase). | L |
| S3-10a | rim-server | **Workspace plugin registry**: `rim_identity::plugin_manifest`, the two tables, `WorkspacePluginService`, GraphQL, the bundle endpoints, the event. | M |
| S3-10b | rim-ui | **Registry client**: verification on both platforms, sign-and-publish, the palette source, install/required/update/removal flows, add from files. | M/L |

## Rules for every PR

- Branch `feature/<slug>-<6 random chars>` off `master`; one PR per row; commit messages are
  plain imperative sentences with no trailers (no `Co-Authored-By`).
- rim-server: `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo test --workspace`, and the e2e suite (`cargo test -p rim_api --features e2e-tests --
  --test-threads=1` with `DATABASE_URL` on a test database) all green before review. A GraphQL
  change is followed by an SDL export into `rim-ui/schema.graphql` and `npm run codegen` there.
- rim-ui: `npm run build`, `npm test`, `npm run lint`, `npm run format:check`, and in
  `src-tauri/` `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`,
  `cargo test`.
- Every PR is reviewed (Hawkeye) and its BLOCK findings fixed before it ships.
- Migrations are additive: expand now, contract a release later.
