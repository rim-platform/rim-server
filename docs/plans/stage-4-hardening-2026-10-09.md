# Stage 4 hardening (plan, 2026-10-09)

_Written the evening of 2026-10-09 from section 6 of `docs/security/crypto-review-brief-2026-10.md`,
the brief for the outside review. The items below are the ones we fix before Stage 5 builds on
them; the rest stay questions for the reviewer. Numbers are the brief's._

## Two PRs

| # | Repo | Fixes | Size |
|---|---|---|---|
| H-1 | rim-ui | **1** a local encryption pin (a conversation this device holds group state or an `e2ee_conversation` row for, or ever saw `encryptedAt` on, stays encrypted; a server answering `null` gives the "encryption reset" state, never a plaintext send; desktop and web). **3** clients refuse certificates whose `keySeq` precedes a recovery (the server's rule, on the desktop and in the browser). **4** a DM is pinned to its two principals (a pending add of a third is refused and shown); a group shows "X was added" / "X was removed" rows from Commits. **5** a row whose author has no principal id is "could not be verified"; the server's `senderDeviceId` must equal the decrypted sender's; the timeline shows the MLS sender. **6** a Welcome for a (conversation, incarnation) the device holds state for is refused unless its epoch is above the held one or the device is awaiting a re-add. **10** key package `not_after` is set an hour short of 90 days. **11** key-log heads are pinned per principal on the device; a shorter or forked log is refused ("identity changed in a way RIM cannot verify"). **2** a signed revocation is verified, not just present, and a device once seen revoked stays revoked on this device. **20** a Commit with proposals other than Add and Remove, or a lying summary, is refused and the device asks for a re-add instead of merging. **22** the desktop verifies Ed25519 keys as the server does (canonical encoding, `identityKey != recoveryKey`, unknown fields refused). **18** the vault fails per record, not as a whole. | L |
| H-2 | rim-server | **7** the franking key lives in `RIM_FRANKING_KEY_FILE` (default `<RIM_DATA_DIR>/franking-key.json`) with key ids, a `franking-key rotate` CLI and a verification window for the previous key; a report's commitment must equal the message row's stored one when the row exists. **9** re-add requests are fresh: `issuedAt` within 10 minutes, nonce recorded per (device, incarnation) and refused on reuse, `lastEpoch` bound to the device's last accepted epoch. **10** the server accepts `not_after` up to 90 days plus 5 minutes. **14** `verify_login` checks challenge and audience before answering revocation. **15** a server that binds to a non-loopback address with neither `RIM_KEY_LOGIN_AUDIENCES` nor `RIM_PUBLIC_BASE_URL` refuses to start. **23** `zeroize` in `rim_identity` for link secrets and derived keys; the franking key as `Zeroizing`. **26** anything S4-5 left of "a report needs no message row". **27** parser text out of `VALIDATION` messages. Spec text updated where a rule changes. | M |

H-1 starts at once (it does not overlap S4-5's client work); H-2 after S4-5 merges (both change the
franking code). Same rules as every PR (`docs/plans/stage-5-2026-10-09.md`, "Rules for every PR").

## Left to the reviewer

8 (rollback of local state), 12 (the webview as signer), 13 (local-process plugins), 16, 17, 19,
21, 24, 25, and the parts of 2 and 4 that need a protocol change (a signed device set in the
profile card; member-signed adds).
