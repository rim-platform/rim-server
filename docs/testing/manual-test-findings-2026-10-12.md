# Manual test findings (2026-10-12)

_Findings from Jakub's hands-on run of `docs/testing/manual-test-plan-2026-10-12.md`. Recorded
as reported. Step numbers refer to that plan._

## Run

| | |
|---|---|
| rim-server | `master` at `8b3ddbf` |
| rim-ui | `master` at `ed7434d` |
| Clients | desktop app via `tauri dev`; web client |

## Summary

| # | Step | Area | Repo | Severity | Title | Status |
|---|---|---|---|---|---|---|
| F-1 | 2.x | Direct messages | both | major | Own account can be added as a contact; DM with it fails with the group message | fixed (rim-server #187, rim-ui #109) |
| F-2 | 1.x | Arrival | rim-ui | major | Web: opening an invite link while signed in shows the sign-up form | fixed (rim-ui #109) |
| F-3 | 0.x | Connectivity | rim-server | minor | Desktop shows the server offline in tauri dev | closed (configuration) |
| F-4 | 8 | Messaging | both | minor | Typing indicator reads 'undefined is typing...' | fixed (rim-server #186, rim-ui #110) |
| F-5 | 8 | Search | rim-ui | minor | Search: no suggestions after from:@ or in:# | fixed (rim-ui #111) |
| F-6 | 1 | Arrival | rim-ui | major | Web: a workspace joined from an invite appears in the rail only after a reload | fixed (rim-ui #112) |
| F-7 | 8 | Unread state | rim-ui | major | Unread badge on Inbox stays after everything is read; no unread badge on workspace icons | fixed (rim-ui #112) |

Severity: **blocker** (the scenario cannot continue), **major** (wrong result, workaround
exists), **minor** (cosmetic or wording). Status: **open**, **triaged**, **in progress**,
**fixed** (with the PR), **closed** (not a defect, with the reason).

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

### F-1: Own account can be added as a contact; DM with it fails with the group message

- **Step:** 2.x
- **Repo:** both
- **Severity:** major
- **Did:** In the direct space, added the signed-in account itself as a contact, then started a
  DM with that contact.
- **Expected:** The app does not offer the viewer as a contact, and the server refuses both the
  contact and a DM with oneself.
- **Saw:** The contact was added. Starting the DM answered "group conversations require at least
  2 members".
- **Evidence:** `addContact` accepted any principal id, the caller's own included.
  `createConversation` drops the caller from `remoteMembers`, so a single remote member that is
  the caller left no members, the request fell through to the group path, and the service
  refused it with the two-members message.
- **Notes:** Fixed in rim-server #187: `addContact` refuses the caller's own principal with
  `VALIDATION` "cannot add yourself as a contact", and `createConversation` with `remoteMembers`
  naming only the caller answers `VALIDATION` "cannot create DM with yourself", the wording of
  the in-workspace DM check. A group naming the caller among others is still created. The
  rim-ui half (the contact picker hides the viewer) shipped as rim-ui #109. A notes-to-self
  conversation is a possible later feature.

### F-2: Web: opening an invite link while signed in shows the sign-up form

- **Step:** 1.x
- **Repo:** rim-ui
- **Severity:** major
- **Did:** Registered on the web client, signed in, opened a workspace invite link in a new tab.
- **Expected:** The join page offers "Join workspace" to the signed-in account.
- **Saw:** The sign-up form.
- **Evidence:**
- **Notes:** `JoinPage` reads the in-memory session without running the session hydration that
  `LoginPage` and `AppShell` run, so a fresh tab has no session yet when the page decides. Fix in
  rim-ui.

### F-3: Desktop shows the server offline in tauri dev

- **Step:** 0.x
- **Repo:** rim-server
- **Severity:** minor
- **Did:** Started the desktop app with `tauri dev` against the local server.
- **Expected:** The desktop app connects to the running server.
- **Saw:** The app showed the server as offline.
- **Evidence:** `RIM_CORS_ALLOWED_ORIGIN` held a comma-separated list of origins.
- **Notes:** The variable takes one origin, so no origin matched; `http://localhost:1420` alone
  fixes it. Candidate for a clearer boot message or an example in the README.

### F-4: Typing indicator reads 'undefined is typing...'

- **Step:** 8
- **Repo:** both
- **Severity:** minor
- **Did:** Typed in a conversation on one client while watching another.
- **Expected:** "<display name> is typing...".
- **Saw:** "undefined is typing...".
- **Evidence:**
- **Notes:** The server's `TypingStarted` event has never carried a name; the client's event type
  declares `username` and prints it. Fix: the server adds `username` and `display_name` to the
  event; the client prefers the event's name and falls back to the cached member, then
  "Someone".

### F-5: Search: no suggestions after from:@ or in:#

- **Step:** 8
- **Repo:** rim-ui
- **Severity:** minor
- **Did:** Opened search with Ctrl+Shift+F and typed `from:@` and `in:#`.
- **Expected:** A dropdown of members or channels to pick from.
- **Saw:** Nothing; the operator must be typed blind.
- **Evidence:**
- **Notes:** Enhancement: a suggestion popover in the search overlay fed by the member list and
  the visible conversations.

### F-6: Web: a workspace joined from an invite appears in the rail only after a reload

- **Step:** 1
- **Repo:** rim-ui
- **Severity:** major
- **Did:** Signed in on the web client, opened a workspace invite link, clicked **Join
  workspace**.
- **Expected:** The workspace appears in the left rail at once.
- **Saw:** It appears after a page reload.
- **Evidence:**
- **Notes:** The join-by-invite mutation does not invalidate the `myServers` query; the plain
  join hook does.

### F-7: Unread badge on Inbox stays after everything is read; no unread badge on workspace icons

- **Step:** 8
- **Repo:** rim-ui
- **Severity:** major
- **Did:** Read every conversation; switched windows and workspaces.
- **Expected:** The Inbox badge disappears once everything is read, and each workspace icon in
  the left rail shows its own unread badge like Direct messages does.
- **Saw:** The badge stays or comes back; workspace icons never show one.
- **Evidence:**
- **Notes:** The app tells the server a conversation was read only when the "New messages"
  divider is dismissed, which exists only after a watermark was set. Counts loaded from the
  server have no watermark, so the server is never told and every refetch restores the badge.
  Workspace badges: counts are fetched for the open workspace and the direct space only.

## Questions and suggestions

Not defects: questions raised and UX suggestions made during the run.

| # | Area | Repo | Item | Status |
|---|---|---|---|---|

## Steps passed

Steps run without a finding, so the gaps in the run are visible.

## Not run

Steps skipped and why.
