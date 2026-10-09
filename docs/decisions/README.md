# Decisions

Sealed, historical design records. ADR-style.

## Convention

- Each file documents a design decision and its implementation outcome.
- Files in this directory are **frozen** once sealed (status header at top says "Do not edit").
- For changes to behavior described here, write a NEW ADR rather than editing the historical one.
- Active design work lives in `docs/design/`; once landed and reviewed, specs are sealed and moved here.
- A product-level decision accepted before implementation may be recorded here directly (status "Accepted"); it is sealed once its implementation lands.

## Index

- `registration-redesign-2026-04-28.md` — Multi-tenant registration onboarding (bootstrap CLI, instance invites, signup mutations, BOOTSTRAP/INSTANCE/WORKSPACE token types).
- `decentralization-2026-10-08.md` — Decentralization model: key identity, interchangeable relays, hosted workspaces, MLS for DMs and private groups, signed server-to-server HTTP API.
- `post-quantum-hybrid-2026-10-09.md` — Post-quantum cryptography goes hybrid (X25519 + ML-KEM-768 for MLS key agreement, Ed25519 + ML-DSA-44 for signatures), never pure; Stage 3 formats become algorithm-agile.
- `relay-proxied-workspaces-2026-10-09.md` — The app keeps one endpoint, its relay; the relay subscribes to workspace hosts with a user-signed delegation and forwards traffic as an untrusted courier; direct connection is the fallback; hosts accept any relay, allow-list optional.
- `mls-library-2026-10-09.md` — MLS library, providers and cipher suite for E2EE v1 (OpenMLS 0.9.1, libcrux on clients, suite 0x004F).
