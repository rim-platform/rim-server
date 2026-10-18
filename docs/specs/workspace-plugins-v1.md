# Workspace plugins v1

_Status: implemented, normative. Extends `docs/specs/key-identity-v1.md` (signed statements,
version 2 tagged keys, the public `keyLog`) and `docs/specs/web-plugins-v1.md` (the `WebSandboxed`
kind and the bundle format). This document is the contract between the server
(`WorkspacePluginService` and the bundle endpoints) and the clients for a per-workspace plugin
registry whose entries carry manifests signed by their publisher._

## Summary

1. Each workspace has a registry, the `workspace_plugin` table: at most one entry per `plugin_id`,
   holding the publisher's signed statement, its source (bundle hash or git commit), a status
   (`recommended` or `required`), and who approved it and when. An entry pins one exact version.
2. A publisher is a person: their identity key (key-identity-v1, version 2) signs, with context
   `rim.plugin-manifest.v1\0`, a payload embedding the exact `rim-plugin.json` text and binding the
   version and the bundle's SHA-256 and size (or a git commit and tree hash).
3. The server verifies against the publisher's key log (its own copy when the publisher is enrolled
   here, else one the submitter supplies, as for `enrollIdentityKey`). Every client verifies again
   before consent; the server is never trusted for the signature.
4. A statement stays valid across identity rotations; a later recovery rotation invalidates it.
5. Owners and Moderators manage entries; Members and Guests read. `workspacePlugins`,
   `addWorkspacePlugin`, `updateWorkspacePlugin`, `removeWorkspacePlugin`, and a
   `WorkspacePluginChanged` event.
6. Bundles are stored on the workspace's server, because the web app can fetch only from hosts that
   send CORS headers. The hash, not the server, decides whether bytes are right.
7. Signing happens in the desktop app, on a device holding the identity key: Settings > Plugins >
   "Sign and publish". No CLI signing in v1.
8. Clients list the registry in the palette with a trust badge; `required` plugins are offered once
   when the workspace opens, never installed silently; installs follow the pinned statement;
   removing an entry stops the plugin and offers to uninstall it. The web app installs only
   `WebSandboxed` entries.
9. Migration `add_workspace_plugin` adds two tables and touches nothing else.

## Roles

| Action | Owner | Moderator | Member | Guest |
|---|---|---|---|---|
| Read the registry, download a bundle | Yes | Yes | Yes | Yes |
| Upload a bundle | Yes | Yes | No | No |
| Add, update (statement or status) or remove an entry | Yes | Yes | No | No |

Signing is not a role: any person with a key identity signs, on a device that holds their identity
key, and the publisher need not be a member of the workspace or enrolled on its server. Approving
is the workspace's decision, held by the roles that manage it. Guests read the registry because an
entry holds nothing beyond which plugins the workspace uses, and a guest asked to install a
required plugin needs the entry to verify it.

## Signed manifests

### The statement

```json
{"v":2,"type":"plugin-manifest","principalId":"rimp1_txDLbnipi5i3kCJtJwBOUixzv_MY8OnU0mcy--Tyje0","keySeq":0,"publisherName":"Ada","pluginId":"dev.example.hello-web","version":"0.1.0","manifest":"{\"manifest_version\":1,…}","source":{"kind":"bundle","sha256":"5b154b3eb45c3ebc44f6820ba30a389ce85b43dea34bcbf652b9b1a5416fbe85","size":1605},"issuedAt":"2026-10-09T00:00:00Z"}
```

Every rule of key-identity-v1's "Signed statements" applies: it travels as `{ payload, signature
}`, the payload is the exact UTF-8 text that was signed and is never re-serialized, the signature
covers `context || payload` in tagged form, and unknown fields, duplicate fields and a wrong `type`
are refused.

| Purpose | Context bytes | Signed by | Max payload |
|---|---|---|---|
| Plugin manifest | `rim.plugin-manifest.v1\0` | The identity key of the log entry at `keySeq` | 96 KiB |

| Field | Rule |
|---|---|
| `v` | `2`. Version 1 is not accepted for this type: it has no statements to keep valid |
| `type` | `plugin-manifest` |
| `principalId` | The publisher's principal id (`rimp1_…`) |
| `keySeq` | The `seq` of the key log entry whose `identityKey` signed |
| `publisherName` | The name the publisher chose: trimmed, 1 to 80 characters, no control characters. Self-asserted |
| `pluginId`, `version` | Equal to the manifest's `plugin_id` and `version`; `version` is 1 to 64 characters |
| `manifest` | The exact text of `rim-plugin.json`, at most 64 KiB, a JSON object |
| `source` | Below |
| `issuedAt` | RFC 3339, informational |

| `source.kind` | Fields | Allowed for |
|---|---|---|
| `bundle` | `sha256`: 64 lowercase hex characters, the bundle hash (web-plugins-v1, "Bundles"); `size`: 1 to 8,388,608 | Both kinds |
| `git` | `url`: an `https://` or `ssh://` URL the desktop's git install accepts, at most 2,048 bytes; `commit`: 40 or 64 lowercase hex characters; `treeSha256`: 64 lowercase hex characters | `DesktopLocalProcess` only |

**`treeSha256`** is SHA-256 over every file of the commit's tree, sorted by path in byte order:
the path's bytes, a zero byte, the file's bytes, a zero byte. That is the layout of the desktop's
bundle fingerprint (`compute_bundle_fingerprint`), applied to committed files only: no `.git`,
nothing untracked or ignored, so dependencies installed after the checkout do not count. A tree
with a symbolic link or a submodule cannot be signed. It gives a git source a SHA-256 binding,
which a SHA-1 commit id alone would not.

Decisions:

- **The manifest is embedded, not hashed.** The statement is then self-contained: the registry,
  the palette and the consent prompt read the plugin's name, commands and capabilities from signed
  bytes before anything is downloaded, and the bundle check (its `rim-plugin.json` byte-identical
  to `manifest`) still ties them to the code. The cost is at most 64 KiB per entry.
- **The identity key signs, not a device key or a new publisher key.** A device key is one of
  many, revocable, and would make every verifier check a certificate chain and revocations. A
  separate publisher key would be a new kind of key with its own rotation story, where the roadmap
  wants every identity to go through one principal model. The identity key is the person, and its
  log is public.
- **Hashes are lowercase hex**, unlike key-identity's base64url, because publishers compare them
  with `sha256sum` and the bundler's output.

### Verification

In the same words as in `web-plugins-v1.md`:

> A client verifies a registry entry before it offers to install it, and again before it installs
> or updates, without trusting the server:
>
> 1. Read the statement: `type` `plugin-manifest`, `v` 2, payload at most 96 KiB, no unknown or
>    duplicate fields (a browser checks duplicates with its own scan, since `JSON.parse` keeps the
>    last one).
> 2. Get the publisher's key log: `keyLog(principalId)` from the workspace's server or, when that
>    answers `[]`, the entry's `publisherKeyLog`. Replay it from the inception; its principal id
>    must be the statement's `principalId`.
> 3. `keySeq` must be an entry of the log, and no later entry may be a recovery rotation
>    (`"signer":"recovery"`). The signature must verify over `rim.plugin-manifest.v1\0` followed by
>    the payload, under that entry's `identityKey`.
> 4. The `manifest` text must be a valid manifest for this host, with `plugin_id` equal to
>    `pluginId` and `version` equal to `version`. A `WebSandboxed` manifest must come with a
>    `bundle` source.
> 5. The fetched bundle must have exactly `size` bytes and the SHA-256 `sha256`, and its
>    `rim-plugin.json` must be byte-identical to `manifest`. For a `git` source (desktop only), the
>    checkout of `commit` must have the tree hash `treeSha256`.
> 6. Only then does the client show the publisher, the trust badge and the capability consent.

The desktop checks a git source after the clone and checkout of `commit`, before any post-install
step runs. The server runs steps 1 to 4 when an entry is added or its statement replaced, with two
differences: in step 2 it uses its own `key_log_entry` rows when the principal is enrolled here and
ignores a supplied log, and otherwise requires `publisherKeyLog` (at most 256 entries); in step 4 it
checks only what it needs to store the entry (a JSON object; `manifest_version` 1; a reverse-domain
`plugin_id` with at least three segments of ASCII letters, digits and hyphens, not under `dev.rim.`
or `com.rim.`; `display_name` 1 to 80 characters; `runtime_kind` one of the two kinds). Capability
and command rules are the host's, which is the authority on what it will run. The server never
fetches a bundle or a repository.

### Rotation and recovery

A statement signed at `keySeq` n is valid while no recovery rotation follows entry n in the
publisher's log; after one it is refused with `the publisher recovered their key after keySeq n`.

- **Identity rotations keep it valid**, like device certificates: rotations are meant to be cheap,
  and invalidating on every rotation would make a publisher re-sign every plugin in every
  workspace each time.
- **A recovery rotation invalidates it**, because a recovery says the identity key was lost and
  may be in someone else's hands, who could sign new statements naming the old `keySeq`. A
  publisher who suspects a leak rotates with the recovery key for this reason; that is also the
  rotation that revokes every device.
- **Rejected:** valid only at the head (as above, too costly), and valid whatever follows (a thief
  of a lost key could publish under the person's name indefinitely).
- **Residual risk:** after an identity rotation the retired key can still sign statements naming
  its own `keySeq`. That is accepted, because an identity rotation is made by the holder of the
  current key, who chooses it over a recovery only when the old key is not compromised.

After an invalidation the server reports `signatureStatus: PUBLISHER_RECOVERED` (computed when the
entry is read, from the `signer` of the log entries after `keySeq`; their signatures were verified
when stored). Clients refuse new installs and updates of the entry. An installed copy keeps running,
because its bytes are the ones the person consented to and are pinned by hash, and shows "The
publisher recovered their key: ask for a re-signed version". A moderator fixes the entry with
`updateWorkspacePlugin` and a statement at the new `keySeq`.

### `rim_identity`

A new module, `plugin_manifest`: `Purpose::PluginManifest` (the context and the 96 KiB cap); a
`KeyHistory`, the replayed log keeping each entry's identity key and signer; and
`verify_plugin_manifest(&KeyHistory, &SignedStatement)`, which runs steps 1 and 3 and the field
rules, and answers the parsed statement with `Validity::Valid` or `Validity::PublisherRecovered {
at_seq }`, where `at_seq` is the seq of the first recovery rotation after `keySeq`. Of the
manifest it reads only `manifest_version`, `plugin_id`, `display_name`, `version` and
`runtime_kind` (each must be present with its JSON type); the server's step 4 checks on their
values live in the server. `read_plugin_manifest` runs the field rules without the signature, for
statements read back from storage. The server and the Tauri host call it; the web app's
TypeScript port is checked against appendix A.

## Bundles on the server

A bundle reference, in the same words as in `web-plugins-v1.md`: **a registry entry names a web
bundle by `bundleSha256` (64 lowercase hex characters, the SHA-256 of the bundle file's bytes),
`bundleSize` (its length in bytes, at most 8 MiB) and `bundleUrl` (where to fetch it; by default
the workspace server's copy, `/api/plugin-bundles/<serverId>/<bundleSha256>`). The SHA-256 and the
size are inside the publisher's signed statement; the URL is not, because it says only where the
bytes are, and the hash decides whether they are the right ones.**

The server keeps copies because the web app fetches with `fetch()`, which needs CORS headers that
release hosts (GitHub releases, most object stores by default) do not send; a copy also keeps a
required plugin installable when the publisher's host goes away. An entry may still name an external
`https://` URL: clients fetch it with `credentials: "omit"`, and the web app needs that host to send
`Access-Control-Allow-Origin: *`. The server's own copy (a `bundleUrl` that is a path, or a URL on
the API's origin) needs the session, so clients fetch it with credentials, and send credentials to
no other host.

Two endpoints in `rim_server`, in the `file_handlers.rs` pattern (a non-GraphQL transport that
depends on a `PluginBundleService` trait in `rim_services`, never on its implementation):

- **`POST /api/plugin-bundles/{serverId}`**: session and CSRF header; the caller is an Owner or
  Moderator of `serverId`. The body is the bundle file (`Content-Type:
  application/vnd.rim.plugin-bundle+json`; one file and no fields, so not multipart, and the
  server hashes it as it reads), at most 8 MiB. The server checks the bundle's structure (format,
  version, sorted unique paths, path rules, `rim-plugin.json` present), stores the bytes through
  `StorageService` under `plugin-bundles/<serverId>/<sha256>`, and answers `201` (`200` when that
  hash was already there) with `{ "sha256": "…", "size": 1605, "url":
  "/api/plugin-bundles/<serverId>/<sha256>" }`. Refusals, checked in this order: `401` without a
  session, `403` for a missing CSRF header, `403` for a `serverId` that is not a UUID, `415` for
  another `Content-Type`, `413` over 8 MiB, then `429` over the rate limit, `403` for a non-member
  or a role below Moderator, `422` not a bundle or over the storage quota (with the reason).
  Bundle bytes count toward the workspace's storage quota, which attachments and bundles share.
- **`GET /api/plugin-bundles/{serverId}/{sha256}`**: session; the caller is a member of
  `serverId`, else `404`, as for an unknown hash. Answers the bytes with `Content-Type:
  application/vnd.rim.plugin-bundle+json`, `X-Content-Type-Options: nosniff`,
  `Content-Disposition: attachment` and `Cache-Control: private, max-age=31536000, immutable`, with
  the CORS headers of `/api` (the server's CORS policy covers every route).

Each upload that passes the role and structure checks also deletes, best effort, the workspace's
bundles no entry names that are over 24 hours old. It runs before the upload looks for the hash,
so a bundle re-uploaded after it aged out is stored again (`201`) rather than deleted after a
`200`.

## Storage

Migration `add_workspace_plugin`, generated with `sea migrate generate add_workspace_plugin`:

```sql
CREATE TABLE plugin_bundle (
    server_id uuid NOT NULL,
    sha256 text NOT NULL,
    size_bytes bigint NOT NULL,
    storage_key text NOT NULL,
    uploaded_by uuid NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT pk_plugin_bundle PRIMARY KEY (server_id, sha256),
    CONSTRAINT chk_plugin_bundle_sha256 CHECK (sha256 ~ '^[0-9a-f]{64}$'),
    CONSTRAINT chk_plugin_bundle_size CHECK (size_bytes BETWEEN 1 AND 8388608),
    CONSTRAINT fk_plugin_bundle_server FOREIGN KEY (server_id)
        REFERENCES server (id) ON DELETE CASCADE ON UPDATE CASCADE,
    CONSTRAINT fk_plugin_bundle_uploaded_by FOREIGN KEY (uploaded_by)
        REFERENCES "user" (id) ON DELETE SET NULL ON UPDATE CASCADE
);
CREATE INDEX idx_plugin_bundle_created_at ON plugin_bundle (created_at);

CREATE TABLE workspace_plugin (
    id uuid PRIMARY KEY,
    server_id uuid NOT NULL,
    plugin_id text NOT NULL,
    version text NOT NULL,
    display_name text NOT NULL,
    runtime_kind text NOT NULL,
    status text NOT NULL,
    source_kind text NOT NULL,
    git_url text NULL,
    git_commit text NULL,
    git_tree_sha256 text NULL,
    bundle_url text NULL,
    bundle_sha256 text NULL,
    bundle_size bigint NULL,
    statement_payload text NOT NULL,
    statement_signature text NOT NULL,
    publisher_principal_id text NOT NULL,
    publisher_key_seq integer NOT NULL,
    publisher_key_log jsonb NULL,
    approved_by uuid NULL,
    approved_at timestamptz NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT uq_workspace_plugin_server_plugin UNIQUE (server_id, plugin_id),
    CONSTRAINT chk_workspace_plugin_runtime_kind
        CHECK (runtime_kind IN ('desktop_local_process', 'web_sandboxed')),
    CONSTRAINT chk_workspace_plugin_status CHECK (status IN ('recommended', 'required')),
    CONSTRAINT chk_workspace_plugin_source CHECK (
        (source_kind = 'git'
            AND git_url IS NOT NULL AND git_commit IS NOT NULL AND git_tree_sha256 IS NOT NULL
            AND bundle_url IS NULL AND bundle_sha256 IS NULL AND bundle_size IS NULL)
        OR (source_kind = 'bundle'
            AND bundle_url IS NOT NULL AND bundle_sha256 IS NOT NULL AND bundle_size IS NOT NULL
            AND git_url IS NULL AND git_commit IS NULL AND git_tree_sha256 IS NULL)),
    CONSTRAINT chk_workspace_plugin_web_bundle
        CHECK (runtime_kind <> 'web_sandboxed' OR source_kind = 'bundle'),
    CONSTRAINT chk_workspace_plugin_key_seq CHECK (publisher_key_seq >= 0),
    CONSTRAINT fk_workspace_plugin_server FOREIGN KEY (server_id)
        REFERENCES server (id) ON DELETE CASCADE ON UPDATE CASCADE,
    CONSTRAINT fk_workspace_plugin_approved_by FOREIGN KEY (approved_by)
        REFERENCES "user" (id) ON DELETE SET NULL ON UPDATE CASCADE
);
CREATE INDEX idx_workspace_plugin_bundle ON workspace_plugin (server_id, bundle_sha256)
    WHERE bundle_sha256 IS NOT NULL;
```

- The unique constraint's index serves the registry listing (by `server_id`, ordered by
  `plugin_id`); `idx_workspace_plugin_bundle` serves the download check and the cleanup.
- `statement_payload` and `statement_signature` are the statement as signed. Every other column
  except `status`, `bundle_url`, `publisher_key_log` and the approval columns is copied from the
  verified payload, never from input. `publisher_key_log` holds the supplied log (a JSON array of
  `{ payload, signature }`) only for a publisher not enrolled here; an update may replace it with a
  longer log that extends it, and a log that diverges from it is `VALIDATION`.
- `approved_by` and `approved_at` are set on add and on every update. `approved_by` becomes null
  if that account is deleted.
- The registry belongs to the workspace and lives on the server that hosts it, next to its roles
  and bans (`server-to-server-v1.md`), so it needs no origin column.
- **Rollback.** `down` drops `workspace_plugin`, then `plugin_bundle`. Nothing references them.
  Files under `plugin-bundles/` stay in storage, and an operator may delete that prefix. Clients
  keep what they installed; a registry install whose entry is gone is treated as removed.

Code: a `workspace_plugin` module in `rim_core`, `rim_services`, `rim_infra_services`
(`DefaultWorkspacePluginService`) and `rim_infra_db`, and the endpoints above in `rim_server`.

## GraphQL

```graphql
enum PluginRuntimeKind { DESKTOP_LOCAL_PROCESS WEB_SANDBOXED }
enum WorkspacePluginStatus { RECOMMENDED REQUIRED }
enum PluginSourceKind { BUNDLE GIT }
enum PluginSignatureStatus { VALID PUBLISHER_RECOVERED }

type PluginSource {
  kind: PluginSourceKind!
  bundleUrl: String  bundleSha256: String  bundleSize: Int     # BUNDLE
  gitUrl: String  gitCommit: String  gitTreeSha256: String      # GIT
}

type WorkspacePlugin {
  id: ID!
  serverId: ID!
  pluginId: String!
  version: String!
  displayName: String!
  runtimeKind: PluginRuntimeKind!
  status: WorkspacePluginStatus!
  source: PluginSource!
  manifest: String!                       # the exact text inside the statement
  statement: SignedStatement!
  publisherPrincipalId: String!
  publisherName: String!
  publisherKeySeq: Int!
  publisherKeyLog: [SignedStatement!]!    # this server's log when enrolled here, else the stored one
  signatureStatus: PluginSignatureStatus!
  approvedBy: User
  approvedAt: DateTime!
  updatedAt: DateTime!
}

input AddWorkspacePluginInput {
  serverId: ID!
  statement: SignedStatementInput!
  status: WorkspacePluginStatus!
  bundleUrl: String                        # BUNDLE only; omitted: this server's copy
  publisherKeyLog: [SignedStatementInput!] # required when the publisher is not enrolled here
}

input UpdateWorkspacePluginInput {
  serverId: ID!
  pluginId: String!
  statement: SignedStatementInput          # a new version, or a re-signed one
  status: WorkspacePluginStatus
  bundleUrl: String
  publisherKeyLog: [SignedStatementInput!]
}

type Query {
  workspacePlugins(serverId: ID!): [WorkspacePlugin!]!                  # members and guests
}
type Mutation {
  addWorkspacePlugin(input: AddWorkspacePluginInput!): WorkspacePlugin!       # Owner, Moderator
  updateWorkspacePlugin(input: UpdateWorkspacePluginInput!): WorkspacePlugin! # Owner, Moderator
  removeWorkspacePlugin(serverId: ID!, pluginId: String!): Boolean!         # Owner, Moderator
}
```

- `addWorkspacePlugin` creates the entry for the statement's `pluginId`. With a `bundle` source
  and no `bundleUrl`, a bundle with that hash and size must have been uploaded to this workspace;
  a given `bundleUrl` must be `https://` and at most 2,048 bytes, and is not allowed for a `git`
  source.
- `updateWorkspacePlugin` changes any of statement, status and URL (at least one; a
  `publisherKeyLog` alone changes nothing). A new statement must name the same `pluginId`; its
  version and runtime kind may change, and a lower version is an ordinary update, because a pin is
  exact and versions are never compared. A new statement with a `bundle` source and no `bundleUrl`
  points at this server's copy, which must be uploaded, even when the entry had an external URL.
  A `publisherKeyLog` without a statement re-verifies the stored statement against it, keeping the
  stored source. Without a `publisherKeyLog`, a publisher not enrolled here is verified against
  the log the entry keeps, when the statement names the same publisher.
- `removeWorkspacePlugin` deletes the entry and answers `true`. The bundle goes with the next
  cleanup.

| Situation | Code |
|---|---|
| No session | `UNAUTHENTICATED` |
| Not a member of `serverId`; a mutation by a Member or Guest | `FORBIDDEN` |
| `add` for a `pluginId` the registry has | `CONFLICT` (`dev.example.hello-web is already in this workspace's plugins`) |
| `add` when the registry holds 100 entries | `CONFLICT` |
| `update` or `remove` of an entry that does not exist | `NOT_FOUND` |
| A statement that fails verification, with the reason (`signature does not verify`, `the publisher recovered their key after keySeq 0`, `pluginId does not match the manifest`) | `VALIDATION` |
| Publisher not enrolled here and no `publisherKeyLog`; a log that does not replay | `VALIDATION` |
| `bundle 5b154b3e… is not uploaded to this workspace`; a bad `bundleUrl` | `VALIDATION` |
| `update` naming another `pluginId`, or changing nothing | `VALIDATION` |
| Over a rate limit | `RATE_LIMITED` |

The three mutations need the CSRF header like any session mutation. The resolvers are new
`WorkspacePluginQueries` and `WorkspacePluginMutations` objects appended to the last query and
mutation groups (`QueryGroup7`, `MutationGroup7`), or to a new group when the last one already
holds five.

**Event.** `WorkspaceEvent::WorkspacePluginChanged { workspace_id, plugin_id, change }`, with
`change` one of `added`, `updated` or `removed`, serialized like the other events (`{"type":
"WorkspacePluginChanged", …}`). It is visible to every member, guests included. It carries
no entry data: clients refetch `workspacePlugins`.

## Client behaviour (rim-ui)

### Signing and publishing

Settings > Plugins offers "Sign and publish…" on an installed plugin when the device holds the
account's identity key (`identityKeys` and `LogStatus.holdsIdentityKey`, which includes a linked
device that took the key over with the recovery phrase). The dialog:

1. Builds the source: `git` for a plugin installed from git (the checkout's remote URL, `HEAD` and
   its `treeSha256`); otherwise `bundle`, a `.rimbundle` built from the installed directory with
   the SDK bundler's algorithm (refused past 8 MiB or 256 files, pointing to `rim-plugin-bundle`),
   or a `.rimbundle` the developer picks. It shows id, version, kind, SHA-256 and size, to compare
   with what `rim-plugin-bundle` printed.
2. Takes `publisherName`, defaulting to the profile card's display name, replays the account's log,
   signs at its head `keySeq` on the Tauri side (the key never reaches the webview), and verifies
   the result with `rim_identity`.
3. For an Owner or Moderator of the current workspace: a status, then "Publish" (upload the bundle
   if the source is one, then `addWorkspacePlugin`, or `updateWorkspacePlugin` after showing
   "Replaces 0.1.0 signed by Ada"). For anyone else: "Export" saves
   `<plugin_id>-<version>.rimsig.json`, `{"format":"rim-plugin-signature","v":1,"statement":{…},
   "publisherKeyLog":[…]}`, beside the `.rimbundle`, for a moderator to verify and add with
   Workspace settings > Plugins > "Add from files" (desktop or web).

No CLI signs in v1, and the web app never signs: it holds no identity key.

### The registry in the palette

A "Workspace plugins" source lists the entries this device has not installed, or has at another
statement:

```
Install Kubernetes Pods                  Recommended by Acme · signed by Ada (rimp1_txDLbnip)
Install GitHub Pull Requests             Required by Acme · signed by RIM team (rimp1_nz1POml8)
```

The **trust badge** is `Signed by <publisherName> · rimp1_<first 8 characters after the prefix>`,
with the whole id on hover and copy. `publisherName` is self-asserted, so when a member of the
workspace has this `principalId` (`ServerMember.principalId`, from `serverMembers`), the badge
shows that member's name from the workspace instead; otherwise the name is quoted and marked "not a
member of this workspace". On the
web, `DesktopLocalProcess` entries are listed with "Needs the desktop app" and cannot be chosen.
An entry that fails verification is listed as "Signature does not verify" with the reason, and is
never offered.

### Installing

Choosing an entry runs the verification steps, then opens the install dialog: the badge, the
workspace's status, the source (`Bundle 5b154b3e… · 1.6 KB`, or `git github.com/acme/k8s-pods @
1a2b3c4d`), then the capability prompt with its kind line (web-plugins-v1, "Consent text"), and for
a git source the post-install consent as today. The install record keeps `{ kind: "registry",
endpoint, serverId, pluginId, statementSha256, publisherPrincipalId }`, where `statementSha256` is
the SHA-256 of the statement's payload.

### Required plugins

When a workspace opens, after the registry loads, one dialog lists its `required` entries that
this device can install and has not (or has at another statement), each with its badge and "Review
and install", plus "Not now". Nothing installs without the person going through consent. "Not now"
is remembered per (server, plugin id, statement hash), so the dialog returns only when the entry's
statement or status changes; Settings and the palette keep showing "Required by Acme, not
installed". The web app lists required desktop-kind entries once as "Needs the desktop app". The
server enforces nothing: `required` is the workspace's strong recommendation.

### Updates, removal and other sources

- **Updates.** On workspace open and on `WorkspacePluginChanged`, the client compares each registry
  install's `statementSha256` with the entry's. A changed entry is verified, then installed at once
  when it keeps the runtime kind and the publisher, asks for no capability the installed copy did
  not ask for and the person has not approved, adds no AI-callable command beyond the installed
  copy's and those consented, and adds no post-install step; the approvals carry over to the new
  copy, and the person sees "Updated Kubernetes Pods to 0.3.0". Otherwise the update waits behind a prompt that
  shows what is new, with a warning when the publisher changed. A failed update keeps the old copy.
- **Removal.** When the entry disappears, the client stops and disables the plugin and asks "Acme
  removed Kubernetes Pods from its plugins. Remove it from this device?" "Remove" uninstalls it and
  deletes its storage; "Keep" turns it into a local install with no registry source and no updates.
  It is stopped first because a plugin may be removed for being harmful.
- **Other sources.** Another workspace's entry for an installed plugin id shows "Installed" when
  its statement hash matches, or "Installed from <other workspace>" with an offer to switch
  (verification and consent again). A different publisher than the installed copy's is called out.
  A local, unsigned copy with the same id is offered "Replace with Acme's signed version".

## Marketplace v0 (out of scope)

A curated git-backed index would be a repository of `.rimsig.json` files and their bundles, which
any client verifies with the steps above and a workspace adds with `addWorkspacePlugin`, so it needs
no new statement.

## Limits and audit

| What | Limit | Key |
|---|---|---|
| `addWorkspacePlugin`, `updateWorkspacePlugin`, `removeWorkspacePlugin` | 60 an hour | User |
| Bundle uploads | 20 an hour, 8 MiB each | User |
| Registry entries | 100 | Workspace |
| `publisherKeyLog` | 256 entries | Input |
| Stored bundles | The workspace's storage quota | Workspace |

The limiters are the in-memory `RegistrationRateLimiter`, per node; correctness never depends on
them. Audit events `workspace_plugin_added`, `workspace_plugin_updated`, `workspace_plugin_removed`
and `plugin_bundle_uploaded` carry the server id, the acting user id, the plugin id, version,
status, publisher principal id and bundle hash (a bundle upload: the hash, the size and whether it
was stored or already there), never the statement text or bundle bytes.

## Test plan

- **`rim_identity::plugin_manifest`.** Appendix A verifies, with key-identity appendix B's inception
  as the log. Refused: a flipped payload byte; the key log context; `v` 1; an unknown and a
  duplicate field; `keySeq` 1 on a one-entry log; `pluginId` or `version` unlike the manifest's; a
  web manifest with a `git` source; `size` 0 or 8 MiB plus one; uppercase hex; a manifest over 64
  KiB. An appended identity rotation keeps it `Valid`; a recovery rotation makes it
  `PublisherRecovered { at_seq }`.
- **Service** (mock repositories): the roles table, every errors row, the local log winning over a
  supplied one, a snapshot only for a publisher not enrolled here and replaced only by an extending
  log, the default URL needing an uploaded bundle of that size, the approval columns, the cap, one
  event per change.
- **`db-tests`:** unique and check constraints; cascade on workspace delete; `approved_by` set null
  on user delete; cleanup deleting only unnamed bundles over 24 hours old; migration up and down.
- **e2e** (`rim_api/tests/workspace_plugin_e2e.rs`): upload web-plugins-v1 appendix A's bundle
  (`201`, then `200`); add appendix A's statement as an Owner with the inception as
  `publisherKeyLog` (`VALID`); read as a Member and a Guest; download as a member (`200`) and a
  non-member (`404`); a Member's add `FORBIDDEN`; a tampered signature `VALIDATION`; a status update
  emitting the event; removal, then `NOT_FOUND`. The e2e harness executes the schema directly, so
  it drives the bundle endpoints through `PluginBundleService`; the endpoints' status codes and
  headers, and the CSRF header on the upload and the mutations, are tested in `rim_server` with
  actix test requests.
- **Clients:** verification in the Tauri host and in TypeScript on appendix A and one refusal per
  step; `treeSha256` on a fixture repository; the signing dialog's statement verifying with
  `rim_identity` and its hash equal to `rim-plugin-bundle`'s; the badge with and without a
  matching member; the required dialog shown once and again after a statement change, never
  installing on its own; each update rule; removal with "Remove" and "Keep"; the web app refusing
  desktop-kind entries.

## Deferred

- **Signed approvals**: an Owner's signature over the entry's status, so a client stops trusting
  the server for "recommended by Acme".
- **Freshness of the publisher's log.** A server could withhold a later recovery rotation, keeping
  old signatures valid for its clients. Fetching profile cards from several relays would answer
  this; relays do not fan out reads yet.
- Enforcing required plugins, and per-workspace limits on capabilities (`capability_security_model.md`,
  "Admin Restriction Direction").
- CLI signing; release channels (more than one version per entry); the server mirroring external
  bundles; pushing re-verification to clients when a publisher recovers.
- The marketplace v0 index and its economics.

## Appendix A: known-answer statement

Publisher: key-identity-v1 appendix B (identity key from seed 1, principal
`rimp1_txDLbnipi5i3kCJtJwBOUixzv_MY8OnU0mcy--Tyje0`); its key log is appendix B's inception alone,
so `keySeq` is 0. The bundle is web-plugins-v1 appendix A. The `manifest` field holds that
bundle's `rim-plugin.json`. Signed with Python's `cryptography`, checked with Node's `crypto`. Never
use these keys for real statements.

Payload (693 bytes):

```
{"v":2,"type":"plugin-manifest","principalId":"rimp1_txDLbnipi5i3kCJtJwBOUixzv_MY8OnU0mcy--Tyje0","keySeq":0,"publisherName":"Ada","pluginId":"dev.example.hello-web","version":"0.1.0","manifest":"{\"manifest_version\":1,\"plugin_id\":\"dev.example.hello-web\",\"display_name\":\"Hello Web\",\"version\":\"0.1.0\",\"description\":\"Says hello from a sandboxed frame.\",\"runtime_kind\":\"WebSandboxed\",\"entrypoint\":\"main.mjs\",\"commands\":[{\"id\":\"greet\",\"title\":\"Hello Web: Greet\",\"description\":\"Greets the signed-in user.\"}]}","source":{"kind":"bundle","sha256":"5b154b3eb45c3ebc44f6820ba30a389ce85b43dea34bcbf652b9b1a5416fbe85","size":1605},"issuedAt":"2026-10-09T00:00:00Z"}
```

Signature:

```
ed25519:FRHxrYnev03jlVo484kL4PLh7GMArjEE4q1wiBNMSx7rD8CAis8YIivjSccshXkEdvYEw-wAIjzQgzDRj6kGCg
```
