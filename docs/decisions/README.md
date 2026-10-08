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
