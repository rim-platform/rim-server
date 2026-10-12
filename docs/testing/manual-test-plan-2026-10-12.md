# Manual test plan: a stranger's first week (2026-10-12)

_The second hands-on run, written for the public launch. The first plan
(`manual-test-plan-2026-10-10.md`) covered keys, plugins, encryption and two servers; this one
covers what changed since and what a stranger does with a public instance: arrive from the
internet, sign up, be limited, be banned. Record every remark in a findings file like
`manual-test-findings-2026-10-11.md`, questions and suggestions included._

The plan assumes rim-server master with the rate limits, the IP deny list, the instance notice
and the direct-space changes merged, and a rim-ui master built after its schema regeneration.
Where a step needs something not yet merged, it says so.

## 0. Setup (45 min)

Two machines, or one machine and one VM: the server must not be `localhost` for the client,
because that is the path nobody has tested.

1. **Server on the VM**, from `deploy/`: `cp .env.example .env`, set `RIM_DOMAIN` to a name
   that resolves to the VM (a `/etc/hosts` entry on the client machine is enough), a database
   password, and the testing-run controls:

   ```sh
   RIM_FEATURES_DISABLED=attachments,custom_emoji,workspace_plugins,link_previews
   RIM_RATE_REQUESTS_PER_MIN_PER_IP=600
   RIM_RATE_MUTATIONS_PER_MIN_PER_SESSION=120
   RIM_RATE_MESSAGES_PER_MIN_PER_USER=30
   RIM_RATE_MESSAGES_BURST=10
   RIM_RATE_WS_CONNECTIONS_PER_USER=4
   RIM_AUDIT_RETENTION_DAYS=90
   RIM_INSTANCE_NOTICE="Testing instance: data can be wiped without notice."
   RIM_TERMS_URL=https://<domain>/terms
   ```

   Until the first release tag exists there are no published images: add
   `-f deploy/compose.dev.yml` and have a rim-ui checkout next to rim-server. Start with
   `docker compose -f deploy/compose.yml -f deploy/compose.dev.yml up -d --build`, then
   `docker compose ... exec rim /app/rim-server invite create` for your first link.
2. **Packaged desktop app on the client machine**: `npm run tauri build` in rim-ui and install
   the bundle. Not `tauri dev`: the packaged app has the `tauri://localhost` origin, which is the
   point.
3. **Web client**: the browser on the client machine, at `https://<domain>`.
4. **A second person**, or a second browser profile, for the moderation and DM steps.
5. Open the server log in a terminal: `docker compose ... logs -f rim`.

## 1. Arrival (20 min)

| # | Do | Expect |
|---|---|---|
| 1.1 | Open `https://<domain>` in the browser. | The notice from `RIM_INSTANCE_NOTICE` is on the sign-in screen (needs the notice PR). |
| 1.2 | Open the invite link. | The sign-up screen shows the notice and a checkbox linking to the terms URL; sign-up without the checkbox is refused with a message that names the terms. |
| 1.3 | Sign up with the checkbox. | The shell opens. Direct messages has no badge yet. |
| 1.4 | Open the packaged desktop app, point it at `https://<domain>`, create a key, sign in. | Sign-in works. Then send one message in any channel. **This is the CSRF test**: if the message is refused or the log shows `csrf validation failed`, record it as a blocker with the exact message. |
| 1.5 | In the desktop app, open About. | Server version, API version, the feature list showing attachments, custom emoji, workspace plugins and link previews as disabled, and the notice. |
| 1.6 | Paste a link with a preview-able page into a message. | No preview card appears and the log shows no outbound fetch. |
| 1.7 | Try to attach a file. | The attach control is absent or disabled, with the reason "unavailable on this instance", not an error after the fact. |

## 2. Direct messages and the direct space (15 min)

| # | Do | Expect |
|---|---|---|
| 2.1 | From the second account, send a DM to the first before the first has ever opened Direct messages. | The first account's Direct messages entry shows a badge with the count at once, and after a sign-out and sign-in the badge is there before opening the view. |
| 2.2 | Open Direct messages, read the DM. | The badge clears. A desktop notification arrived while a workspace was open, not only with Direct messages open. |
| 2.3 | In the direct space, open Settings. | No Pipelines, State or Emoji tabs. Keybinds and device plugins remain. |
| 2.4 | With the first account's palette open in the direct space, list commands. | Only the commands a guest may run there: create a group, invite, join; no create channel, archive or kick. |

## 3. Who sees which commands (15 min)

| # | Do | Expect |
|---|---|---|
| 3.1 | As the workspace owner, open the palette in a workspace. | Admin commands (invite member, kick, ban, set role, slow mode, archive) are listed. |
| 3.2 | As a plain member in the same workspace. | None of the admin commands; create channel is listed. |
| 3.3 | As a member of no workspace (second account before joining anything), open the palette. | Only the commands the client runs itself; nothing that needs a workspace. |
| 3.4 | Ask the AI agent (desktop, with a provider key) to "kick user X" as a plain member. | The agent does not have the command and says so; it does not attempt it and get refused. |

## 4. Being limited (20 min)

| # | Do | Expect |
|---|---|---|
| 4.1 | Set `RIM_RATE_MESSAGES_PER_MIN_PER_USER=5`, `RIM_RATE_MESSAGES_BURST=2`, restart `rim`. Send messages as fast as you can. | The third message in the same second is refused with a "slow down" message that names the wait; the composer keeps the text; after the wait it sends. |
| 4.2 | Set `RIM_RATE_REQUESTS_PER_MIN_PER_IP=20`, restart, and reload the web client a few times. | The client shows a rate-limited state rather than a blank page; `curl -i https://<domain>/api/sdl` in a loop gets 429 with `Retry-After`. Restore 600 afterwards. |
| 4.3 | Open the web client in five tabs. | The fifth tab's live updates fail with a "too many connections" message and the other four keep working. Close one; the fifth reconnects on reload. |
| 4.4 | `curl -s https://<domain>/api -H 'content-type: application/json' -d '[{"query":"{serverInfo{version}}"}, ... 21 copies ...]'` | 400 with `{"error":"batch_too_large"}`. |

## 5. Being banned (25 min)

| # | Do | Expect |
|---|---|---|
| 5.1 | As the owner, time out the second account for 5 minutes from the member menu. | The second account's composer says it is timed out until when; sending is refused with the time. |
| 5.2 | Kick, then ban the second account from the workspace. | It loses the workspace at once (open view closes or goes read-only), cannot rejoin by invite link, and the audit log page shows both actions with the actor. |
| 5.3 | Instance ban: `rim-server principal block <principal-id> --reason test`. | The second account's sessions end within seconds on desktop and web; sign-in is refused; `abuse-reports list` shows the reason. |
| 5.4 | IP block: `rim-server ip block <client machine ip>/32 --reason test --expires-in-days 1`. | Within 30 seconds every request from the client machine answers 403 `{"error":"blocked"}`; `/healthz` still answers; `ip list` shows the expiry. `ip unblock` restores access within 30 seconds. |
| 5.5 | Check the audit log page as owner. | It shows the retention line ("Entries are kept for 90 days") and the ban, timeout, block and unblock events. |

## 6. The user's own backups (20 min)

| # | Do | Expect |
|---|---|---|
| 6.1 | Desktop, Settings, History: start the reference backup service from `docs/specs/backup-endpoint-reference.py` on the client machine with a bearer token, add it as a destination. | The setup check passes; a first backup runs; the per-device last-backup status shows a time. |
| 6.2 | Add an S3-compatible destination (MinIO in Docker is enough) with a key limited to the bucket. | Same: check, backup, status. Remove the `s3:DeleteObject` right and run the check again: it reports the probe could not be removed and still passes. |
| 6.3 | Stop the service in 6.1, wait for the next backup. | The status shows the failure with the reason; nothing else breaks. |

## 7. Operator routine (15 min)

| # | Do | Expect |
|---|---|---|
| 7.1 | `deploy/backup.sh`, then `deploy/restore-drill.sh`. | Both succeed; the drill reports the restored server answering `/readyz`. |
| 7.2 | `curl https://<domain>/metrics` from outside. | Refused: metrics are on the internal listener only. From the VM: the counters for rate limit rejections and blocked requests have the values from sections 4 and 5. |
| 7.3 | `docker compose ... exec rim /app/rim-server audit purge --dry-run`. | Reports what 90 days would remove (nothing yet) and exits 0. |

## 8. Regressions worth a glance

- Everything in sections 1 to 3 of the 2026-10-10 plan still holds on the packaged app against
  the remote server: key sign-in, device linking by code to the web client, an encrypted DM,
  recovery with the kit.
- The desktop still opens offline on the second start with the server unreachable.
- Keyboard profiles and the Vim layer unchanged.

## 9. Record

Findings go to `manual-test-findings-2026-10-12.md` in the format of the previous file: one table
row per finding with severity and status, one section per finding with what you did, what you
saw, and what you expected. The packaged-app CSRF result (1.4) is the one answer this plan exists
for; record it even when it passes.
