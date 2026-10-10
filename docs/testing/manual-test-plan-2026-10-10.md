# Manual test plan: Stages 3, 4 and 5 (2026-10-10)

_For Jakub's hands-on run after the Stage 3, 4 and 5 PRs of 2026-10-09 and 2026-10-10. Everything below
was verified only by automated tests against mocked clients or a local Postgres; nothing has
run on two real devices yet. Work through the scenarios in order: later ones build on earlier
state. Note what you saw next to each step; "expected" is what the specs promise._

## 0. Setup (30 min)

**Server** (rim-server, `master`):

```sh
cd ~/RustroverProjects && docker compose up -d postgres
cd rim-server
export DATABASE_URL=postgres://postgres:postgres@localhost:5432/rim_dev
sea migrate up
RIM_SERVER_HOST=127.0.0.1 RIM_SERVER_PORT=8080 RIM_OPEN_REGISTRATION=true \
RIM_KEY_LOGIN_AUDIENCES=localhost:8080,127.0.0.1:8080 \
RIM_CORS_ALLOWED_ORIGIN=http://localhost:1420 \
cargo run -p rim_server
```

Both audience variables matter: a non-loopback bind without `RIM_KEY_LOGIN_AUDIENCES` or `RIM_PUBLIC_BASE_URL` refuses to start (rim-server #74). The first start writes `data/server-key.json`, `data/franking-key.json` and a recovery key file it tells you to move away. `RIM_CORS_ALLOWED_ORIGIN` is required for **both** the desktop dev app and the web app: `npm run
tauri dev` serves the webview from `http://localhost:1420`, so every call to the server is
cross-origin, and without the variable the server blocks them and the sign-in page says "Backend
unreachable". Open registration makes signup easy.
Keep `psql` open on `rim_dev` for the server-side checks below.

**Desktop, two instances.** Each instance needs its own secret vault and credential store
entry, so run them as **two OS users** (or two machines) against the same server
(`rim-ui/docs/device-linking-desktop.md`, "Trying it with two app instances"):
`cd rim-ui && npm ci && npm run tauri dev`. Call them **A** (Ada) and **B** (Bob's laptop /
Ada's second device, depending on the scenario).

**Web.** `npm run dev` in rim-ui, open `http://localhost:1420` in Chrome or Firefox (both have
WebCrypto Ed25519). Call it **W**.

**What to capture when something fails:** the app's log (Settings > System if present, else
the terminal running `tauri dev`), the server log line with the request id, a screenshot, and
the step number here.

## 1. Accounts and keys (Stage 3)

| # | Do | Expected |
|---|---|---|
| 1.1 | On A, sign-in page > **Join** (the link under the form; needs `RIM_OPEN_REGISTRATION=true`, or paste a link from `cargo run -p rim_server -- bootstrap-user --email you@example.com`), choose **Create account with a key**, username `ada`, device name `Ada desktop` | Keys are created; a 12-word recovery phrase and the account id are shown as a numbered grid; "Create account" is disabled until "I saved it" is ticked |
| 1.2 | Copy the kit (keep it in a text file for later), tick, create | Signed in; Settings > Security shows the key identity, this device listed, "Key sign-in is on" and the **Sign out other sessions** offer |
| 1.3 | `psql`: `select username, password_hash, email, principal_id from "user";` | `ada` has `NULL` password and email and a `rimp1_…` id |
| 1.4 | Sign out on A; on the sign-in screen choose **Sign in with this device** | Signed in without a password |
| 1.5 | On W, create a second account with a **password** (`bob`); then on B sign in as `bob` with the password and in Settings > Security **set up key sign-in** | The phrase is shown once; after confirming, password login for `bob` is refused ("UNAUTHENTICATED") and `requestPasswordReset` sends nothing (check the server log) |
| 1.6 | On B as `bob`: **Sign out other sessions** | W's `bob` session is signed out ("Signed out 1 other session") |
| 1.7 | On A as `ada`: Settings > Security > **Link a device**. On B (signed out, or as a new OS user), sign-in screen > **Link this device with its code**, paste the code, device name `Ada laptop` | Both screens show the same `XXXX-XXXX` fingerprint; A shows the name and platform `desktop`; after Confirm, B signs in as `ada` and A lists the device |
| 1.8 | On B (linked, no identity key): Settings > Security | "Link a device" is **not** offered (no identity key); the device list is visible; **Take over the identity key** appears once a history archive is set up (see 1.11) |
| 1.9 | On W, sign-in page > **Link this device**; paste a fresh code from A | W shows the weaker-storage notice; the fingerprint matches; after confirm W signs in as `ada` with a browser key; A's confirmation card said platform `web` |
| 1.10 | On A: revoke `Ada laptop`; on B try to sign in with the device key | B gets "this device was removed" and offers to remove its keys |
| 1.11 | On A: Settings > History archive > set up a **local folder** (a folder synced or shared with B's OS user). Then Settings > Security > **Identity key backup** | Status shows the backup and the wrap written. Link B again (1.7), then on B set up the archive on the same folder and **Take over the identity key** with the phrase | B now offers "Link a device" and can revoke devices; A keeps its copy |
| 1.12 | On a fresh install (new OS user), set up the archive on that folder and choose **Unlock with this device's keys** after taking over, or the passphrase | The archive unlocks; history restores |
| 1.13 | Recovery: on a fresh install, sign-in page > **Recover an account**, paste the kit (or just the 12 words + account id), device name | Signed in; every other device of `ada` is signed out; A and B must link again |

## 2. Plugins (Stage 3, 2.3 and 2.4)

| # | Do | Expected |
|---|---|---|
| 2.1 | In rim-ui: `npm run plugin:bundle -- plugins/github-prs` (see `docs/web-plugins.md` for the exact command) | A `chat.rim.github-prs-0.1.0.rimbundle` and its `sha256 … size …` line |
| 2.2 | On A (desktop): Settings > Plugins > install the bundle's output folder | Consent prompt starts with "Runs sandboxed in this app…", lists `ui.panel` and "Connect to listed sites: api.github.com, avatars.githubusercontent.com"; the panel opens and lists PRs of a public repo you add |
| 2.3 | On A as an Owner: Settings > Plugins > **Sign and publish** the plugin into the workspace, status Recommended | The dialog shows id, version, hash and size; the registry entry appears in Workspace settings > Plugins |
| 2.4 | On W (signed in as `ada` or `bob`): palette > **Workspace plugins** | "Install GitHub Pull Requests · Recommended by <workspace> · signed by Ada" with the badge naming the workspace member; install, consent, panel works in the browser |
| 2.5 | Change the entry to **Required**; open the workspace on B | A one-time "required plugins" dialog with "Review and install" / "Not now"; nothing installs on its own |
| 2.6 | Publish version `0.1.1` with no new capabilities | W and A update automatically and say "Updated GitHub Pull Requests to 0.1.1" |
| 2.7 | Remove the entry | Installed copies stop and ask "Remove it from this device?" (Remove / Keep) |
| 2.8 | On W: Settings > Plugins > **Install from link** with the bundle served from any host that sends `Access-Control-Allow-Origin: *` and `#sha256=<hash>` in the link | Installs as "Unsigned · installed from a link (host)"; a wrong hash is refused |
| 2.9 | In the panel's frame, try `fetch("https://example.com")` from the browser devtools console of the frame | Blocked by CSP (console shows the policy error) |

## 3. Encrypted DMs and groups (Stage 4)

Both `ada` (A) and `bob` (B, a different OS user, signed in with his key) must have key sign-in.

| # | Do | Expected |
|---|---|---|
| 3.1 | On A, start a **new DM** with `bob` | The DM opens with a lock mark and "Messages from here on are end-to-end encrypted" |
| 3.2 | Send messages both ways; edit one; delete one; react; reply; start a thread and reply in it; mention `@bob` | Everything renders on both sides; the mention notifies B; edits show "edited" |
| 3.3 | `psql`: `select id, text, encrypted, length(mls_ciphertext), mls_epoch from message where encrypted order by sent_at;` | `text` is `NULL` for every encrypted row; epochs are ≥ 1 |
| 3.4 | On A, search for a word from an encrypted message (palette search) | Found "on this device" (local search); the server search (`searchMessages` through the API or the web app's search) never returns it |
| 3.5 | Attach an image and a PDF in the DM | Both sides show them after download; `psql`: `select filename, content_type, blob_sha256 from message_attachment order by created_at desc limit 2;` shows `encrypted` / `application/octet-stream` and a hash |
| 3.6 | Paste a link in a message | The sender's client fetched a preview; the receiver shows the same preview without fetching (check the server log: no `link_preview` fetch for that message) |
| 3.7 | Open a **plaintext DM that existed before** (create one between password accounts first, or use an older one) once both members have keys | The eligible device upgrades it: the marker "Messages from here on are end-to-end encrypted" appears; older messages carry the "sent before encryption" note; `psql`: `conversation.encrypted_at` is set |
| 3.8 | Create a **group** with `ada`, `bob` and a third key account; send; remove the third member | The remaining two keep sending; `psql`: `select incarnation, epoch from mls_group where conversation_id = …;` shows the epoch advancing on each membership change |
| 3.9 | Link a second device for `ada` (1.7) while the DM has history | The new device joins (Welcome) and shows "Earlier messages were sent before this device joined" with **Restore from history archive**; after a restore the placeholders are replaced |
| 3.10 | Revoke that second device from A; send a new message from B | The revoked device cannot read it (if it is still open it shows nothing new); A's send first commits the removal (no error visible, maybe a short delay) |
| 3.10b | Hardening checks (rim-ui #47): with `psql` set `conversation.encrypted_at = NULL` on the DM, reload A | A shows "Encryption was reset by the server" and refuses to send; nothing goes out in plaintext. Set it back afterwards |
| 3.11 | On A, break the group state on purpose: quit A, delete `message-cache/` under the app data dir of A (or use the "Forget encryption state" action if Settings offers one), start A | The DM shows "This device can't read new messages until another device re-adds it"; B's client re-adds it within about 15–30 s; A reads new messages again (old ones from the archive) |
| 3.12 | Forward an encrypted message into a **plaintext channel** | A confirmation says the text will be posted unencrypted; after it the channel shows the text |
| 3.13 | Report an encrypted message with an attachment from B; then as a third account that is a Moderator but not in the DM, open Settings > Workspace > Moderation | The report is "Encrypted · verified"; "Show reported message" shows the exact text, the "Verified: this is the exact message …" line, the device name, the attachment (download decrypts it), and "Delete message" removes it on both clients; a plain member's `messageReport` answers FORBIDDEN |
| 3.14 | On W as `ada` (linked browser) | The DM opens in the browser; W sends and receives; closing the tab for 60+ days would remove it (not testable now); the notice "Encrypted conversations in this browser are only as safe as this browser profile" shows |
| 3.15 | Install a plugin asking for `chat.read` and `chat.read.encrypted` (edit the SDK template's manifest) | Its consent line reads "Read your encrypted DMs and groups: this plugin sees their decrypted text…"; Settings > Plugins > Activity counts encrypted reads per day; a manifest with `chat.read.encrypted` but no `chat.read` is refused at install; without the encrypted grant `rim.chat.list_messages` on the DM answers `CAPABILITY_DENIED` |
| 3.16 | Open the agent in the encrypted DM (conversation menu) | Off by default; turning it on shows the model-provider warning; after that it answers from the local cache and asks before every post it makes there |

## 4. Regressions worth a glance

- Plaintext channels: send, search, pipelines, pins, the notification bell — unchanged.
- Password accounts that never enrolled a key: login, reset, email digests still work.
- Offline start on A (stop the server, start A): the cache opens; encrypted DMs still readable.
- The web app without a device key: encrypted conversations show the "set up this browser as a
  linked device" notice and nothing else leaks.

## 5. Open questions you may hit

- **Windows/macOS**: the secret vault's key goes into Credential Manager / Keychain; nothing has
  run there yet (`rim-ui/docs/secret-vault.md`).
- **Linux prompt count**: one Secret Service unlock per app start is the expectation.
- **Plugin installs under load**: a bundle install that restarts a plugin while another process
  is being spawned used to fail with "Text file busy"; the host now retries (rim-ui #53). If an
  install still leaves a plugin stopped, the log line names it.
- Anything that fails in 3.x with "could not be verified" is the client refusing a message
  whose cross-checks failed; that is a bug worth the full log, not a user error.

## 6. Two servers: relays and hosts (Stage 5)

Stage 5 only shows itself with **two rim-servers**. Run a second one from the same checkout on
another port, database and data directory; both on loopback with server-to-server traffic over
plain HTTP, which the `RIM_S2S_ALLOW_*` flags permit for local runs only:

```sh
# Server A (Ada's relay; also hosts her workspace)
createdb -h localhost -U postgres rim_dev_a; createdb -h localhost -U postgres rim_dev_b
DATABASE_URL=postgres://postgres:postgres@localhost:5432/rim_dev_a sea migrate up
DATABASE_URL=postgres://postgres:postgres@localhost:5432/rim_dev_b sea migrate up
DATABASE_URL=postgres://postgres:postgres@localhost:5432/rim_dev_a RIM_SERVER_HOST=127.0.0.1 RIM_SERVER_PORT=8080 \
RIM_PUBLIC_BASE_URL=http://127.0.0.1:8080 RIM_S2S_ALLOW_HTTP=true RIM_S2S_ALLOW_PRIVATE_ADDRESSES=true \
RIM_DATA_DIR=./data-a RIM_OPEN_REGISTRATION=true RIM_CORS_ALLOWED_ORIGIN=http://localhost:1420 cargo run -p rim_server
# Server B (Bob's relay), in a second terminal
DATABASE_URL=postgres://postgres:postgres@localhost:5432/rim_dev_b RIM_SERVER_HOST=127.0.0.1 RIM_SERVER_PORT=8090 \
RIM_PUBLIC_BASE_URL=http://127.0.0.1:8090 RIM_S2S_ALLOW_HTTP=true RIM_S2S_ALLOW_PRIVATE_ADDRESSES=true \
RIM_DATA_DIR=./data-b RIM_OPEN_REGISTRATION=true RIM_CORS_ALLOWED_ORIGIN=http://localhost:1420 cargo run -p rim_server
```

`RIM_PUBLIC_BASE_URL` is also the login audience, so `RIM_KEY_LOGIN_AUDIENCES` is not needed.
`curl -s http://127.0.0.1:8080/.well-known/rim-server | head -c 300` shows A's server id (`rims1_…`).
Point desktop **A** (Ada) at `http://127.0.0.1:8080` and desktop **B** (Bob) at `http://127.0.0.1:8090`
(the endpoint is chosen at sign-in). Both accounts are key accounts (1.1).

| # | Do | Expected |
|---|---|---|
| 6.1 | On A: Settings > Contacts > **My contact link**; show the QR. On B: Settings > Contacts > **Add contact**, paste the link (or scan) | B shows Ada's card name and a 4×4 fingerprint, the same one A's Settings > Security shows under "Fingerprint"; Ada is listed as a contact. Repeat the other way so each is the other's contact |
| 6.2 | On B: **Direct messages** in the rail > new message > pick Ada | The DM opens encrypted (lock mark). Send both ways; edit; react; attach a file. `psql` on `rim_dev_b`: `select count(*) from relay_item;` grows as A's events are queued; `select kind from server;` lists the `direct` space |
| 6.3 | On A, from a third account `carl` on B that is **not** Ada's contact: start a DM with Ada | Ada's bell shows a **message request** with Carl's name and fingerprint; "Read first" shows his message; **Ignore** hides it; **Block** also leaves the conversation. Accept a new one from a fourth account and it becomes a normal DM |
| 6.4 | `psql` on `rim_dev_a`: `select username, kind from "user";` | Bob and Carl appear as `r_…` rows of kind `remote` |
| 6.5 | On A as the Owner of a workspace hosted on A: create an invite; copy the **`rim-invite:1:` link** (Invite dialog). On B: **Add workspace > Join with a link**, paste | B joins Ada's workspace **through its own relay**: the workspace appears in B's rail; channel messages flow both ways; A's member list shows Bob with his card; Settings > Security on B lists the workspace under "Spaces through this relay" with an expiry under 24 h |
| 6.6 | On B: Settings > Security > revoke a second device of Bob's (link one first, 1.7) | The fan-out panel shows A's host acknowledging; `psql` on `rim_dev_a`: `select status from host_delegation …` ends for that device |
| 6.7 | Stop server B (Bob's relay) while B is in Ada's workspace; wait a minute | B offers **Connect directly to <A>** with the IP notice; after accepting, B keeps reading and sending under the "Connected directly" banner (A's log shows a direct key login for a remote user). Start B again: within two minutes the banner goes and nothing shows twice |
| 6.8 | Kick Bob from the workspace on A | B's workspace vanishes after the `MemberLeft`; `host_delegation` rows for Bob are ended |
| 6.9 | Tamper: on `rim_dev_b`, `update relay_envelope set signature = 'x' where id = (select max(id) from relay_envelope);` then trigger a sync on B | The item is refused ("could not be verified" in the log), nothing shows; no crash |
| 6.10 | Public relay: restart B with `RIM_REGISTRATION_POW_BITS=16 RIM_ROLE=relay` added; on a fresh desktop (new OS user) pointed at B, sign-in page > **Join** > key signup | The form shows "Proving this is a real device…" for well under a second, then the account exists; `curl -s http://127.0.0.1:8090/.well-known/rim-server` shows `"mode":"pow"` in the descriptor payload |
| 6.11 | On B: `cargo run -p rim_server -- principal block <bob's rimp1_ id> --reason test` (same env as the server) | Bob's desktop is signed out and cannot sign in ("blocked on this server"); `principal unblock` restores it |
| 6.12 | On B: `cargo run -p rim_server -- peer block <A's rims1_ id>`; send a message from A to Bob | A's server log shows `403 peer_not_allowed` from B and the message does not reach Bob until `peer unblock`; then it does |
| 6.13 | On B as Carl (a stranger to Ada): send a DM request to Ada; on A, in the request, choose **Report** | `cargo run -p rim_server -- abuse-reports list --open` on A lists it; `resolve <id>` clears it |

## 7. Regressions for Stage 5

- A single server with **no** `RIM_PUBLIC_BASE_URL` behaves as before: `/.well-known/rim-server`
  has `"s2s": null`, `/s2s/v1/ping` answers 404, every Stage 3–4 flow works unchanged.
- A desktop pointed at a single server still creates DMs in the direct space (rail entry "Direct
  messages"); old DMs inside workspaces still open.
