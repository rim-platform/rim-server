> **Status:** Accepted 2026-10-09 for Stage 4 (S4-0). Implementation followed the Stage 4 plan.
> **Do not edit** once implementation lands — historical record. For changes, write a new ADR.

# MLS library, providers and cipher suite for E2EE v1

**Date:** 2026-10-09
**Extends:** `decentralization-2026-10-08.md` (MLS for DMs and private groups),
`post-quantum-hybrid-2026-10-09.md` (X25519 + ML-KEM-768 key agreement, never post-quantum alone).
**Contract that uses it:** `docs/specs/e2ee-mls-v1.md`.

## Context

Stage 4 needs an MLS (RFC 9420) implementation in Rust that runs in the Tauri app, compiles to
WebAssembly for the web linked device, offers a hybrid X25519 + ML-KEM-768 cipher suite, and lets
RIM's own device keys anchor the credentials. The server never holds group state but must decode
MLS framing and validate key packages. Sources opened on 2026-10-09:

| Source | What it says |
|---|---|
| crates.io `openmls` | 0.9.0 on 2026-08-25, **0.9.1 on 2026-10-07** (with 0.8.2 the same day); 0.7.4 and 0.8.1 in February 2026 |
| crates.io `openmls_libcrux_crypto`, `openmls_rust_crypto` | 0.4.0 and 0.6.0, both 2026-08-25 (released with openmls 0.9.0) |
| `openmls/CHANGELOG.md` (main) | 0.9.0: "Added P-384, ML-DSA and additional PQ ciphersuites" behind the `draft-ietf-mls-pq-ciphersuites` feature; `UnsupportedCiphersuite` fails early; time-based deletion of past epoch secrets; `OwnPrivateMessage` / `OwnPendingCommit`; the `openmls::wasm` module removed; fixes GHSA-rrmv-c79f-cf5r and a treemath bound (#2186). 0.8.1: provider updates for GHSA-435g-fcv3-8j26 and GHSA-g433-pq76-6cmf (libcrux, hpke-rs). 0.8.0: GHSA-8x3w-qj7j-gqhf; `libcrux-provider` + `js` are enough for wasm |
| docs.rs `openmls` 0.9.1, `Ciphersuite` | `MLS_128_MLKEM768X25519_AES128GCM_SHA256_Ed25519 = 79` (0x004F), `…_AES256GCM_SHA384_Ed25519 = 78` (0x004E), `MLS_256_XWING_CHACHA20POLY1305_SHA256_Ed25519 = 77` (0x004D), `…_CHACHA20POLY1305_SHA384_MLDSA44 = 82` and pure ML-KEM suites, all behind `draft-ietf-mls-pq-ciphersuites` |
| `openmls/traits/src/types.rs` (main) | All four X25519 + ML-KEM-768 suites map to `HpkeKemType::XWingKemDraft6 = 0x647a`, 0x004D included |
| `openmls/libcrux_crypto/src/crypto.rs`, `openmls_rust_crypto/src/provider.rs` (main) | Both providers implement `XWingKemDraft6` behind the feature; `supports()` checks AEAD, signature and KEM separately; key package creation calls `supports()` (`openmls/src/key_packages/mod.rs`) |
| crates.io `hpke-rs-rust-crypto` 0.7/0.8, `x-wing` 0.1.1 (2026-10-01), `ml-kem` 0.3.2 (2026-05-10), `libcrux-ml-kem` 0.0.11 (2026-10-07) | The RustCrypto path implements X-Wing with RustCrypto `x-wing` ("draft 06") and `ml-kem`; the libcrux path with libcrux's ML-KEM |
| datatracker `draft-ietf-mls-pq-ciphersuites-06` (21 July 2026) | In WG Last Call; eleven suites with code points **TBD1 to TBD11**; TBD1 `MLS_128_MLKEM768X25519_AES128GCM_SHA256_Ed25519` uses HPKE KEM 0x647a |
| datatracker `draft-ietf-hpke-pq-05` (2026-07-06) | 0x647a is `MLKEM768-X25519` (Npk 1216, Nenc 1120, Nsk 32); asks IANA to replace the existing 0x647a entry |
| `draft-connolly-cfrg-xwing-kem-10` (March 2026) | X-Wing, the X25519 + ML-KEM-768 combiner behind 0x647a |
| book.openmls.tech, Introduction | Tested targets: x86_64 Linux and Windows, aarch64 macOS and Linux; **`wasm32-unknown-unknown` built on CI but not tested**; features `js`, `libcrux-provider`, `fork-resolution`, `sqlite-provider`; maintained by Phoenix R&D and CE Labs |
| crates.io/docs.rs `openmls_sqlite_storage` 0.3.0 | Depends on `rusqlite` 0.37 with `bundled` |
| crates.io `mls-rs-crypto-awslc` | 0.25.0 on 2026-04-21, "AWS-LC based CryptoProvider for mls-rs", with a `post-quantum` feature |
| github.com/mlswg/mls-implementations `test-vectors.md`; `openmls/test_vectors/` | The IETF vector families (tree math, crypto basics, secret tree, message protection, key schedule, PSKs, transcript hashes, welcome, tree operations and validation, TreeKEM, messages, passive client); OpenMLS keeps them in its repo |

## Decision

### 1. Library: OpenMLS 0.9.1, behind one seam

- **`openmls = "=0.9.1"`** with feature `draft-ietf-mls-pq-ciphersuites`, and `openmls_traits` of the
  same release. Exact pins: a minor OpenMLS release has changed storage encodings before (0.9.0
  restored the 0.7.1 serde tags), and group state written by one version must load in the next, so
  every upgrade is its own PR that runs the storage-stability and our own state round-trip tests.
- **The seam.** rim-ui's `rim_mls` crate (S4-2) is the only code that names OpenMLS types. It
  exposes RIM operations (create group, add/remove devices, encrypt, decrypt, process commit, join
  from Welcome) over bytes and RIM ids. The crypto provider and the storage provider are chosen
  inside it.
- **Storage.** RIM implements `openmls_traits::storage::StorageProvider` itself: on the desktop
  over the SQLCipher cache database (so decrypting a message, storing its plaintext and advancing
  the ratchet commit in one transaction), on the web over an in-memory map written through to
  IndexedDB. `openmls_sqlite_storage` 0.3.0 cannot be used: it links `rusqlite` 0.37 with
  `bundled`, and rim-ui links `rusqlite` 0.40 with `bundled-sqlcipher`; two `libsqlite3-sys`
  versions cannot be linked into one binary.

### 2. Crypto providers

| Where | Provider | Why |
|---|---|---|
| Clients (desktop, and the web in wasm) | **`openmls_libcrux_crypto` 0.4.0** with `draft-ietf-mls-pq-ciphersuites` (and `js` in wasm), composed with RIM storage into one `OpenMlsProvider` | ML-KEM through libcrux, whose ML-KEM source carries hax proofs; it is the path OpenMLS's authors use for their PQ suites |
| Server (key package validation, framing decode) | **`openmls_rust_crypto` 0.6.0** with `draft-ietf-mls-pq-ciphersuites` | Pure Rust, no SIMD paths; the server only verifies Ed25519 signatures and hashes key packages, it never runs HPKE |

The libcrux provider's `supported_ciphersuites()` list on main does not name 0x004F, although its
`supports()` accepts every component of it (AES-128-GCM, Ed25519, `XWingKemDraft6`, SHA-256) and
key package creation only calls `supports()`. S4-2's first test asserts that both providers
create, join and decrypt a 0x004F group; if libcrux refuses the suite, the client uses
`openmls_rust_crypto` 0.6.0 as well and a follow-up ADR records it. Either way S4-2 runs
cross-provider tests: a group created and driven with one provider is joined and read with the
other, so a divergence between two X-Wing and ML-KEM implementations fails a test.

### 3. Cipher suite: 0x004F now, the IANA code point later

**RIM ships `MLS_128_MLKEM768X25519_AES128GCM_SHA256_Ed25519`, OpenMLS code point `0x004F` (79),
in Stage 4's first PR.** It is TBD1 of `draft-ietf-mls-pq-ciphersuites-06`: KEM 0x647a
(MLKEM768-X25519, the X-Wing combiner of X25519 and ML-KEM-768), HKDF-SHA256, AES-128-GCM,
SHA-256, Ed25519 signatures. This satisfies the hybrid rule for key agreement.

- **Not 0x004D** (`MLS_256_XWING_CHACHA20POLY1305_SHA256_Ed25519`): an OpenMLS-only suite from the
  individual draft-mahy-mls-xwing that the working group draft does not carry, so it will never get
  an IANA code point. Its KEM is now the same X-Wing draft-06 as 0x004F, so it buys nothing.
- **Not 0x004E** (the AES-256-GCM / SHA-384 variant, TBD2): 128-bit classical security matches
  Ed25519 signatures, and SHA-256 / AES-128 keep key schedule and message overhead at the MTI
  suite's level. Either is acceptable; one suite keeps the test matrix small.
- **Not 0x0052** (TBD9, ML-DSA-44 signatures): it signs with ML-DSA-44 alone, which the hybrid rule
  forbids, and device keys are Ed25519 today.
- **Placeholder risk.** 0x004F is OpenMLS's choice for TBD1, not an IANA value. Only RIM clients
  need to agree, and every client pins the same OpenMLS release.

**Migration to the standardized suite.** When IANA assigns TBD1 (or the KEM changes before
publication), OpenMLS ships it under the new code point and RIM moves each conversation with a
**new group incarnation** (`e2ee-mls-v1.md`, "Incarnations"): a member device creates a fresh MLS
group under the new suite with every current device, the server retires the old group, and old
messages stay readable from the local cache and the archive. The server lists the suites it
accepts and the one it prefers; a client starts an incarnation only when every group device's key
packages offer the preferred suite. RFC 9420's ReInit proposal (§12.1.5) does the same inside the
protocol; RIM does not rely on it, because the new group still needs a Welcome for every device
and the incarnation path is also RIM's recovery path, so one mechanism serves both.

### 4. Credentials

- MLS **basic credentials** (credential type 1). The `identity` bytes are an `mls-key` statement
  (`{"payload": "...", "signature": "..."}`, the key-identity encoding) in which the device key
  certifies a separate per-device **MLS signature key**; the leaf's `signature_key` must equal it.
- Why not sign MLS messages with the device key itself: OpenMLS's `Signer` is synchronous, and the
  web client's device key is a non-extractable WebCrypto key whose `sign` is asynchronous, so the
  wasm build could not use it. A separate key also keeps the MLS signature algorithm (fixed by the
  suite) apart from the device key's algorithm (which becomes hybrid), and keeps the OS keyring
  out of every message send.
- Verification: replay `keyLog(principalId)` (`key-identity-v1.md`), check the device certificate at
  its `keySeq` and that the device is not revoked, check the `mls-key` statement under that device
  key and its principal and device ids, compare `mlsKey` with the leaf's signature key.
- **When identity and device keys go hybrid** (`ed25519-mldsa44`), the device key signs `mls-key`
  statements with a hybrid signature; the MLS signature key stays Ed25519 because the suite says so.
  MLS message authenticity stays classical until an MLS suite with a composite Ed25519 + ML-DSA-44
  signature exists; no draft defines one today (`post-quantum-hybrid-2026-10-09.md`, row "MLS
  credential signatures").

### 5. Known-answer tests

The crate tests run against published vectors, on the exact versions linked:

| Primitive or layer | Vectors | Through |
|---|---|---|
| MLS protocol layers | IETF `mls-implementations` vectors for suite 0x0001 (crypto basics, secret tree, message protection, key schedule, transcript hashes, welcome, tree validation, passive-client welcome and handling-commit): the IETF set has no PQ suite | OpenMLS's test-vector readers, re-run in RIM's CI on the pinned version |
| ML-KEM-768 | NIST ACVP FIPS 203 `ML-KEM-keyGen` and `ML-KEM-encapDecap` JSON (usnistgov/ACVP-Server) | `libcrux-ml-kem` and RustCrypto `ml-kem` as linked |
| X-Wing / MLKEM768-X25519 | Appendix vectors of draft-connolly-cfrg-xwing-kem-10 and draft-ietf-hpke-pq-05 | Each provider's HPKE: deterministic decapsulation of the vector's ciphertext with the vector's secret key; where an API cannot take the vector's randomness, the test names the property it checks instead |
| X25519 | RFC 7748 §5.2 and §6.1 | Provider HPKE `DhKem25519` |
| Ed25519 | RFC 8032 §7.1 tests 1 to 3 | Provider `verify_signature` / `sign` |
| HPKE base mode | RFC 9180 Appendix A.1 (0x0020, 0x0001, 0x0001) | Provider HPKE |
| AES-128-GCM, AES-256-GCM | Wycheproof `aes_gcm_test.json` | Provider AEAD; `aes-gcm` for attachments |
| HKDF-SHA256, HMAC-SHA256 | RFC 5869 A.1 to A.3, RFC 4231 | Provider KDF; RIM franking and attachments |
| RIM formats | `e2ee-mls-v1.md` Appendix A | `rim_mls`, `rim_identity`, the web client in Vitest |

"Nothing claimed secure without a test that would fail if it were not": each RIM-level rule in the
spec (credential chain, AAD checks, franking, attachment hash) has a negative test.

## Rejected alternatives

- **mls-rs (AWS Labs)** with `mls-rs-crypto-awslc` (0.25.0, 2026-04-21, a `post-quantum`
  feature): an RFC 9420 implementation whose post-quantum path runs on AWS-LC, a C library behind
  the aws-lc-rs bindings. That adds a C toolchain to the wasm build and an FFI boundary to the
  desktop. OpenMLS is the library the decentralization decision named and has both PQ paths in
  pure Rust.
- **The RustCrypto provider on clients as the default**: pure Rust and listing 0x004F explicitly,
  but RustCrypto `ml-kem` and `x-wing` are newer and unverified; kept as the server provider and the
  named fallback.
- **Classical suite 0x0001 first, hybrid later**: recorded traffic would be exposed to
  harvest-now-decrypt-later from Stage 4's first day, which the hybrid decision rules out when the
  library has the suite. It does.
- **Signing MLS with the device key**: see "Credentials".
- **`openmls_sqlite_storage`**: the `rusqlite` link conflict above.

## Consequences

- **Sizes.** A 0x004F key package is about 3.2 KB (two 1,216-byte X-Wing keys, a 382-byte
  credential, signatures); a parent node holds a 1,216-byte key, so a Welcome with the ratchet tree
  for 512 devices is under 2.5 MB. Limits in the spec follow from this.
- **wasm.** OpenMLS builds `wasm32-unknown-unknown` on CI but does not test it. S4-6 runs the crate's
  test suite in a headless browser (wasm-bindgen-test) as one of its gates and records the
  compressed `.wasm` size in its PR; libcrux's portable (non-SIMD) code paths run there.
- **FFI risk** is low: neither provider builds C for the primitives used; S4-2 confirms with
  `cargo tree -e build` and records the result. rim-ui already links SQLCipher's C.
- **The server depends on OpenMLS** for decoding and key package validation only, in one module
  behind a `rim_services` trait; it never creates or loads a group.
- **Upgrades** of OpenMLS or a provider are their own PRs, rerun every vector, and read the
  advisories listed for the release.
- **What an outside reviewer should look at first:** the credential chain and the `mls-key`
  statement; the commit summary checks on both sides; the one-commit-per-epoch locking; atomic
  decrypt-and-store; re-add requests and incarnations (what a hostile server can force); franking
  and the attachment construction (`e2ee-mls-v1.md`); the provider and storage seam in `rim_mls`.
