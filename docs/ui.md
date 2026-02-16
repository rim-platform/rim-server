# Desktop UI Notes

This document captures the intended approach for the desktop client UI. It is not a final design;
it exists to keep implementation decisions consistent while the backend (auth + GraphQL) is being
built.

## Goals

- Fast startup and low resource usage.
- Keyboard-first workflows (command palette, macros, keybindings).
- Plugin-first UX (panels/actions) without destabilizing the host.

## Stack

- Desktop shell: Tauri v2.
- UI: React + Vite + TypeScript.

## Deployment Model

- Same-origin behind a reverse proxy (e.g. nginx).
- Auth: cookie-based sessions.
- CSRF: required for state-changing requests (GraphQL mutations and REST writes).

## Layout (v1)

- Side rail contains icons for built-in tools and enabled plugin panels.
- A single Tool Panel region is shown at a time (no multi-dock layout in v1).
- Chat area contains:
  - server/workspace selector
  - conversation list
  - message view + composer

## Performance Requirements

- Virtualize long lists (messages, threads, conversation lists where needed).
- Lazy-load plugin panel UIs and heavy routes.
- Avoid heavyweight UI frameworks/kits; keep dependencies small.

## Plugin UI Host Assumptions

- Plugin UI runs in a sandboxed surface (isolated WebView/iframe-like boundary).
- Host provides a narrow RPC bridge and capability enforcement.
- Plugins contribute commands/actions/panels via a manifest; no direct host DOM access.

## Keybindings

- Single-shortcut bindings only (v1).
- Users bind keys to commands (built-in, plugin commands, and macro run commands).
- Plugins cannot auto-bind keys.
