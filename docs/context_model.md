# Context Model

This document defines the Phase 0 context model for RIM.

Context is one of the most important concepts in the product. It is what allows communication to
turn into action without forcing users to copy state manually between surfaces.

Examples:

- highlight text and create a YouTrack issue from it
- run a command against the currently selected message
- trigger a keybind that acts on the current thread
- run a plugin action against the entity selected in a panel

## Purpose

Context is the structured runtime state available to a command, keybind-triggered action, plugin,
or automation at invocation time.

In simpler terms, context is the currently active, selected, focused, or otherwise relevant state
around the user’s action.

## Core Rule

Context should be structured and typed, not just an ad-hoc bag of values.

This is important for:

- command availability checks
- context actions
- keybind behavior
- plugin interoperability
- future automation/event systems

## High-Level Model

The context model should have two layers:

### Core Context

Defined by RIM itself.

Examples:

- current principal
- current workspace/server
- current conversation
- current thread
- selected text
- selected message
- selected messages
- focused surface
- active panel

### Extension Context

Defined by plugins, panels, or integrations.

Examples:

- selected Kubernetes pod
- selected cluster
- selected issue
- selected repository path

## Phase 0 Decision

Plugin-defined context should stay scoped to the plugin or panel that owns it, instead of becoming
globally available everywhere.

This avoids uncontrolled namespace pollution and unclear coupling between unrelated extensions.

## Context Categories

### Identity Context

- current user
- current principal
- current workspace/server membership

### Workspace Context

- current workspace/server
- current conversation
- current thread

### Selection Context

- selected text
- selected message
- selected messages
- selected thread
- selected channel/conversation

## Phase 0 Decisions

- selected text should be first-class context from the beginning
- multiple selected messages should be first-class context from the beginning

These are important for workflow actions like issue creation, summarization, follow-up, and command
composition.

### UI Context

- focused surface
- active panel
- composer focused or not
- command palette open or not

### Navigation Context

- recent activity state
- current route or view
- active navigation target

### Call / Session Context

Likely later:

- active voice channel
- active call
- active screen-share session

These do not need to be fully designed in the first context version.

### Plugin / Panel Context

Plugin-owned contextual entities such as:

- selected pod
- selected issue
- selected repo
- selected deployment target

Plugin-defined context should be namespaced and scoped.

## Invocation-Time Context

Context should reflect the runtime state at the time a command runs.

It is not just static metadata or saved configuration. It should represent what the user is actually
looking at, selecting, or focusing when the action is invoked.

## Commands And Context

Commands may use both:

- implicit context
- explicit arguments

## Phase 0 Decision

Commands should be allowed to use both implicit context and explicit arguments.

This gives commands the flexibility to:

- work instantly when context is available
- still be used from the command palette with prompted inputs when context is missing

## Arguments vs Context

Arguments and context must remain separate concepts.

### Arguments

- explicitly provided by the caller or user
- stable input contract

### Context

- implicitly provided by the current runtime state
- depends on what is focused, selected, or active

Example:

- command: `integration.youtrack.create_issue`
- arguments:
  - project key
  - issue type
- context:
  - selected text
  - selected message
  - current conversation
  - current thread

## Required Context

Commands should declare required context.

Examples:

- `chat.reply_to_selected_message` requires selected message context
- `integration.youtrack.create_issue` may use selected text or selected message context
- `plugin.dev.example.k8s-tools.restart_pod` may require selected pod context

If required context is missing, the command must not run blindly.

## Missing Context Behavior

Recommended behavior:

- in context menus or keybind-driven surfaces, a command should be unavailable if required context is
  missing
- in the command palette, a command may still appear if it can prompt for the missing inputs

## Phase 0 Decision

The command palette should be allowed to show commands that are missing context if those commands
can ask for input instead.

## Context Availability Across Surfaces

The same context model should be usable across:

- command palette
- keybinds
- context actions
- panels
- plugin UI
- future automations

This is necessary so the product feels coherent rather than like disconnected feature sets.

## Context Mutability

Recommended rule:

- context is read-only by default

Commands and plugins should consume context, then mutate application state through commands/services,
not by mutating context directly.

This keeps context a view of state rather than the state engine itself.

## Plugin Context Rules

Plugins may publish context into the active scope of the plugin or panel that owns it.

Rules:

- plugin context should be namespaced
- plugin context should be scoped
- plugin context should not automatically become global shared context

This keeps extension behavior understandable and reduces accidental coupling.

## Minimal Core Context For The First Useful Version

Recommended initial core context set:

- `principal`
- `workspace`
- `conversation`
- `thread`
- `selected_text`
- `selected_message`
- `selected_messages`
- `focused_surface`
- `active_panel`

This is enough to support:

- message actions
- text-selection workflows
- panel-aware commands
- keybind contexts
- issue creation and similar workflow integrations

## Persistence And Lifetime

Most context is not something to persist directly as durable product data.

Recommended lifetime model:

- context is usually ephemeral and runtime-derived
- some navigation/session context may be held in client runtime state
- long-term product state belongs in domain models and services, not in context itself

## Non-Goals For Phase 0

The first context model should not try to solve:

- a fully reactive context graph
- cross-plugin context negotiation protocols
- complex context transformation pipelines
- distributed shared context sync semantics beyond existing product state
- final call/session context model

## Summary Of Decisions

- context is structured runtime state available at invocation time
- context is typed, not an ad-hoc bag of values
- commands may use both implicit context and explicit arguments
- plugin-defined context stays scoped to the plugin/panel that owns it
- selected text is first-class context from the beginning
- multiple selected messages are first-class context from the beginning
- command palette may show commands that are missing context if they can prompt for input
- context should be shared across command palette, keybinds, context actions, and plugin surfaces
- context is read-only by default
