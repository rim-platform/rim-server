> **Status:** Accepted 2026-10-09 by Jakub. Implementation follows Stages 3 to 5 in `ROADMAP.md`.
> **Do not edit** once implementation lands — historical record. For changes, write a new ADR.

# Post-quantum cryptography: hybrid, never pure

**Date:** 2026-10-09
**Decided by:** Jakub
**Extends:** `decentralization-2026-10-08.md` (key identity, MLS, signed server-to-server API).

## Context

Every asymmetric primitive RIM uses or plans to use is classical: Ed25519 for the key log,
device certificates, revocations, profile cards and login proofs (`docs/specs/key-identity-v1.md`),
X25519 inside MLS for DMs and private groups (Stage 4), and HTTP Message Signatures for the
server-to-server API (Stage 5; the algorithm is not fixed yet, Ed25519 was the working
assumption). A large enough quantum computer breaks all of them.

The two kinds of break differ in urgency. Ciphertext recorded today can be decrypted once such
a computer exists ("harvest now, decrypt later"), so key agreement is exposed from the day a
message is sent. A forged signature only helps an attacker while the forged key is still
trusted, so signatures can move later.

## Decision

**RIM's asymmetric cryptography moves to hybrid post-quantum: a classical algorithm and a NIST
post-quantum algorithm together, so an attacker has to break both. RIM never uses a
post-quantum algorithm alone.**

| Where | Hybrid scheme | When |
|---|---|---|
| MLS key agreement (DMs, private groups) | X25519 + ML-KEM-768 (FIPS 203) | Stage 4, from its first PR if the chosen MLS library has the suite, otherwise its first follow-up |
| Identity, recovery and device keys: key log, device certificates, revocations, profile cards, login proofs | Ed25519 + ML-DSA-44 (FIPS 204); a signature is valid only when both parts verify | After Stage 4 encryption ships; the formats are made ready in Stage 3 |
| MLS credential signatures | Ed25519 + ML-DSA-44 suite once the library ships one | After identity keys go hybrid |
| Server-to-server API (Stage 5) | Ed25519 + ML-DSA-44 server keys | Stage 5 or later; server keys already rotate |

## Consequences

- **Stage 3 formats must be algorithm-agile before key identity reaches users.** As merged in
  rim-server #30, the `rimp1_` principal id is a hash of the inception payload bytes, so it does
  not depend on the key algorithm, and statements carry `"v": 1` and versioned context strings.
  But keys and signatures carry no algorithm identifier (decoders accept exactly a 32-byte key
  and a 64-byte signature), only statement version 1 is accepted, payload caps are 4 KiB and
  16 KiB, and `device_key` has a unique btree index on the raw public key. A Stage 3 follow-up
  PR adds a tagged key and signature encoding, statement version 2 beside v1 (a v2 rotation may
  move a v1 log to a hybrid key; the id derivation stays), larger caps, and a key-hash column in
  place of the raw-key index. Clients store keys and signatures as opaque strings meanwhile.
- **Sizes grow.** An ML-DSA-44 signature is 2,420 bytes and its public key 1,312 bytes, against
  64 and 32 for Ed25519, so key logs, profile cards and every signed server-to-server request
  grow roughly forty times. ML-KEM-768 adds 1,184 bytes per encapsulation key and 1,088 bytes
  per ciphertext to MLS key packages and Welcome messages.
- **Library maturity is a Stage 4 risk.** Hybrid MLS cipher suites are not standardized yet:
  draft-ietf-mls-pq-ciphersuites (ML-KEM-768 + X25519 among others) is an Internet-Draft with
  placeholder code points and no IANA assignment, so suite ids and details may change and other
  MLS implementations may not interoperate; RIM only needs its own clients to agree. OpenMLS has
  carried an X-Wing suite (also X25519 + ML-KEM-768, from the separate individual draft
  draft-mahy-mls-xwing) on the unassigned code point 0x004D through Cryspen's libcrux provider
  since April 2024; X-Wing combines the two keys differently from the working-group draft, so
  starting on it may mean a suite migration later. Candidate Rust crates: libcrux (hax proofs
  cover the Rust source of its ML-KEM and ML-DSA, not intrinsics or the compiled code; an
  unverified SHA-3 fallback gave wrong results on some ARM64 platforms until November 2025),
  RustCrypto `ml-kem` and `ml-dsa` (pure Rust; `ml-dsa` fixed a signing timing leak and a
  signature malleability bug in 2026), all pre-1.0, and aws-lc-rs (1.x, bindings to AWS-LC's C
  code; which of its post-quantum APIs are stable is checked at selection). The crate is chosen
  at the start of Stage 4, kept behind one crypto seam, and tested against NIST known-answer
  vectors.
- **Existing Ed25519 identities rotate to hybrid with a rotation signed by the old Ed25519
  key.** Once Ed25519 can be forged, an attacker could forge a fork of an old log for a server
  that has never seen the real one. This is the same class as the limit accepted when rim-server
  #30 merged (a stolen old identity key can claim the id first on a server that has not seen the
  log; the recovery key takes it back), which `key-identity-v1.md` does not describe yet. The
  planned mitigation: clients and relays pin the newest head they have seen, and a later cutoff
  refuses Ed25519-only rotations.

## Rejected alternatives

- **Post-quantum only (ML-KEM, ML-DSA alone):** the algorithms and their implementations are
  young; a flaw in either would leave RIM with no classical fallback. Hybrid costs under a
  hundred bytes per key, signature or ciphertext over pure post-quantum.
- **Stay classical until quantum computers arrive:** recorded DMs could be decrypted
  retroactively, and moving a deployed identity format later is far harder than reserving room
  now.
