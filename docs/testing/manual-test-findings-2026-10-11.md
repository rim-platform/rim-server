# Manual test findings (2026-10-11)

_Findings from Jakub's hands-on run of `docs/testing/manual-test-plan-2026-10-10.md`. Recorded
as reported; nothing here is fixed yet. Step numbers refer to that plan._

## Run

| | |
|---|---|
| rim-server | `master` at `a9753e0` |
| rim-ui | |
| Clients | |

## Summary

| # | Step | Area | Repo | Severity | Title | Status |
|---|---|---|---|---|---|---|
| F-1 | | Connectivity | rim-ui | blocker | UI shows Offline while the server is running | triaged |
| F-2 | | Direct messages | rim-server | blocker | Web: "Failed to load conversations." on Direct messages | fixed (rim-server #137) |
| F-3 | | Direct messages | rim-ui | blocker | Desktop: no Direct messages entry | fixed (rim-ui #71); root cause as F-1 |
| F-4 | | Messaging | rim-ui | major | Desktop: message sent during "Reconnecting..." reaches the server but is not shown | fixed (rim-ui #71); root cause as F-1 |
| F-5 | | AI Agent | rim-ui | major | Only old models (gpt-4o, Opus 4) offered after setting a provider key | fixed (rim-ui #72) |

Severity: **blocker** (the scenario cannot continue), **major** (wrong result, workaround
exists), **minor** (cosmetic or wording). Status: **open**, **triaged**, **fixed** (with the PR).

## Findings

<!-- One section per finding, numbered F-1, F-2, ... in the order reported.

### F-n: <short title>

- **Step:** <plan step, e.g. 3.4>
- **Repo:** rim-server | rim-ui | both | unknown
- **Severity:** blocker | major | minor
- **Did:** <what was done>
- **Expected:** <what the plan or spec promises>
- **Saw:** <what happened>
- **Evidence:** <server log line and request id, app log, screenshot, psql output>
- **Notes:** <suspected cause, once looked at>
-->

### F-1: UI shows Offline while the server is running

- **Step:**
- **Repo:** unknown
- **Severity:** blocker
- **Did:** Started the server, opened the UI.
- **Expected:** The UI connects to the running server.
- **Saw:** The UI shows "Offline. Showing what is saved on this device; sending is off until the
  server is back."
- **Evidence:** Server was not listening at the time of the first report; after restart, CORS
  answered the dev origin with 200, the web client worked, the desktop stayed offline.
- **Notes:** Desktop only (the web app has no offline mode). Ruled out from the coordinator's
  machine: CORS for `http://localhost:1420`, schema mismatch, IPv6 `localhost`, GIO network
  monitor, HSTS, a stored relay endpoint. Remaining suspect: the WebSocket to `/api/ws` never
  connects in the WebKitGTK webview while HTTP works; the socket authenticates by cookie only,
  so a handshake without the session cookie gets 401 and the client retries forever. Needs the
  server log lines containing `WS:` or the webview inspector's view of the `/api/ws` request.
  Also noted: the dev start command needs `RIM_CORS_ALLOWED_ORIGIN=http://localhost:1420`, and
  the packaged Tauri origins (`tauri://localhost`, `http://tauri.localhost`) are not accepted
  by CORS yet (release blocker, `rim_server/src/origin.rs`).

### F-2: Web: "Failed to load conversations." on Direct messages

- **Step:**
- **Repo:** unknown
- **Severity:** blocker
- **Did:** On W, clicked **Direct messages**.
- **Expected:** The Conversations panel lists the user's conversations (or is empty).
- **Saw:** The Conversations panel shows "Failed to load conversations."
- **Evidence:** `rim_dev`: the account had a `server_member` row for the workspace only, none for
  the direct space; `inboxConversations(directSpaceId)` answered FORBIDDEN.
- **Notes:** The spec says local users join the direct space on first use, but the code joined
  them only on creating a DM or being added to one. Fixed in rim-server #137: `relayInfo` with a
  session and the direct-space inbox listing both make the caller a guest of the direct space.

### F-3: Desktop: no Direct messages entry

- **Step:**
- **Repo:** rim-ui
- **Severity:** blocker
- **Did:** Looked for **Direct messages** in the desktop app.
- **Expected:** Direct messages is available, as on the web.
- **Saw:** There is no Direct messages entry in the desktop app.
- **Evidence:** The entry is gated on `relayInfo`, which the relay runtime fetched once per
  sign-in and retried only when the WebSocket connected.
- **Notes:** Consequence of F-1 (no socket). rim-ui #71 retries the relay start with backoff,
  when offline mode ends and on socket connect, so the entry appears once the server answers.

### F-4: Desktop: message sent during "Reconnecting..." reaches the server but is not shown

- **Step:**
- **Repo:** rim-ui
- **Severity:** major
- **Did:** On the desktop app, while it showed the connection as lost ("Reconnecting..."), sent a
  message.
- **Expected:** The connection state matches reality, and a sent message appears in the sender's
  own view.
- **Saw:** Nothing appears in the desktop UI, but the message was sent: it is visible on the web
  client. So the desktop sends while claiming the connection is lost, and does not show its own
  message.
- **Evidence:** `useSendMessageMutation` did not touch the timeline on success; only the socket's
  NewMessage event appended the message.
- **Notes:** Consequence of F-1 (no socket), but a defect on its own. rim-ui #71 inserts the sent
  message through the same dedupe path the socket event uses. The banner itself stays until F-1's
  root cause is found.

### F-5: Only old models offered after setting a provider key

- **Step:**
- **Repo:** rim-ui
- **Severity:** major
- **Did:** Saved an OpenAI key in Settings → AI Agent.
- **Expected:** The model dropdown offers the provider's current models.
- **Saw:** Only gpt-4.1, o3-mini, gpt-4o and gpt-4o-mini.
- **Evidence:** The dropdown rendered a static 2025 table until "Refresh models" was pressed;
  saving a key never fetched.
- **Notes:** Fixed in rim-ui #72: the live list is fetched on key save and on opening the panel
  with a key, filtered to chat-capable models, newest first; fallback tables updated to current
  ids.

## Questions and suggestions

Not defects: questions raised and UX suggestions made during the run.

| # | Area | Repo | Item | Status |
|---|---|---|---|---|
| S-1 | Recovery phrase | rim-ui | Use hex instead of words for the recovery phrase? | closed: keep words |
| S-2 | Settings | rim-ui | Settings tabs are laid out in a row; vertical tabs may work better | done (rim-ui #75) |
| S-3 | AI Agent | both | Server-provided default AI agent; Copilot as a provider | open: recommendation recorded |
| S-4 | Workspaces | both | Two workspaces with the same name | closed: duplicates across owners kept, owner shown (rim-server #139, rim-ui #74); one owner may not reuse a name (rim-server #140) |

### S-1: Use hex instead of words for the recovery phrase?

- **Raised:** During key creation the phrase is ordinary words ("dog", "law", ...). Secure, but is
  it enough? Could hex symbols be used instead?
- **Answer:** Hex would not add security. The phrase is 128 random bits encoded as 12 BIP39
  English words (11 bits each, 4-bit checksum); the same 128 bits in hex are 32 characters and
  exactly as hard to guess. Words are easier to copy correctly, and the client already refuses
  an unknown word (naming its position), a wrong word count and a failed checksum. 128 bits
  matches the strength of the Ed25519 key it protects. If more bits were ever wanted, the
  route is 24 words (256 bits), not hex.
- **Follow-up raised:** Words come from a fixed dictionary, so the choice is limited; each hex
  character still has 2^4 options, so hex should be more secure.
- **Follow-up answer:** The dictionary is the alphabet, like 0-9a-f is for hex, only larger: 2048
  options per word (2^11) against 16 per hex character (2^4). An attacker knows the alphabet
  either way. What counts is the total: 12 words give 2048^12 = 2^132 combinations (128 random
  bits plus a 4-bit checksum); 32 hex characters give 16^32 = 2^128. The words are produced
  from random bits, not chosen by a person, so every combination is equally likely. Matching 12
  words would take 33 hex characters.
- **Second follow-up raised:** An attacker would not generate words again; they would try
  permutations of the known words.
- **Second follow-up answer:** The attacker does not know which 12 words are in the phrase, only
  the 2048-word list. Trying "permutations of the known words" means trying every sequence of
  12 picks from 2048, with repeats allowed: 2048^12 = 2^132. That is the same search as
  brute-forcing hex, which is trying every sequence of 32 picks from the 16 known hex symbols
  (2^128). Only if the 12 words themselves leaked, without their order, would it collapse to
  12! (about 2^29) orderings; a hex string leaked without its order would be just as broken.
- **Third follow-up raised:** The attacker does know the words: it is an open-source project.
- **Third follow-up answer:** Open source publishes the algorithm and the 2048-word list (the
  list is the public BIP39 standard anyway), not any user's phrase. The phrase comes from 16
  bytes drawn from the OS random generator on the user's device at signup
  (`rim-ui/src-tauri/src/key_identity/recovery_kit.rs`, `generate_entropy` via
  `getrandom::fill`); those bytes are never in the code or sent to the server. Knowing the list
  is the same as knowing that hex uses `0-9a-f`. The design assumes the attacker knows
  everything except the random bytes (Kerckhoffs's principle), and the 2^128 count already
  includes that.
- **Decision:** Keep 12 words (Jakub, 2026-10-11).

### S-2: Settings tabs in a row; vertical may be better

- **Raised:** The settings screen shows its tabs in a horizontal row. Vertical tabs (a side
  list) may be better.
- **Notes:** Nineteen tabs in one row. Replaced in rim-ui #75 by a left-hand navigation column
  with grouped sections (You, Messaging, Programmable, Extensions, Workspace, About).

### S-3: Server-provided default AI agent; Copilot as a provider

- **Raised:** Anyone can set their own key; should the server offer a default agent? Copilot is
  missing from the providers.
- **Answer:** Keys are per device in the desktop secret vault; the web app has no agent. A
  server-configured OpenAI-compatible endpoint with a streaming proxy route, per-user rate limit
  and an audit line without content would give the web app an agent and the workspace a default;
  recommended after the public release. Copilot's backend is not a public API and third-party
  clients risk account suspension; the sanctioned route is GitHub Models (OpenAI-compatible). A
  generic "OpenAI-compatible" provider with a configurable base URL covers GitHub Models, Ollama,
  vLLM and the server default in one change.
- **Decision:** Open.

### S-4: Two workspaces with the same name

- **Raised:** Can two users on one relay create same-named workspaces? Should one user be able to?
- **Answer:** Names are labels, ids are identity; uniqueness per relay would not hold across
  relays, so duplicates across owners stay allowed and the owner is shown to tell them apart. One
  account owning two same-name workspaces has no use case and is usually a retry mistake.
- **Decision:** Keep duplicates across owners; show the owner in the rail, the switcher and on
  the invite page (rim-server #139, rim-ui #74); refuse a name the creator already owns, case and
  whitespace ignored, with CONFLICT (rim-server #140). Jakub, 2026-10-11.

## Steps passed

Steps run without a finding, so the gaps in the run are visible.

## Not run

Steps skipped and why.
