# Mattermost Playbooks / Agents V2 Teardown

_Doctor Strange, 2026-07-10. Resolves ROADMAP.md Open Decision #7 (the Track C scoping gate). Commissioned by Fury for Jakub._

## Question

Does Mattermost Playbooks + Agents V2 already cover RIM's planned Track C (incident wedge) closely enough to threaten it, and should Track C be re-scoped?

## Findings (current as of 2026, cited)

- **Playbooks = checklists + retrospectives + configurable metrics.** Runs are webhook-triggerable, so alert-ingestion-to-run exists. (github.com/mattermost/mattermost-plugin-playbooks; docs.mattermost.com/playbooks/notifications-and-updates.html)
- **No native on-call scheduling or escalation-policy engine.** Escalation is a threshold message to a "secondary SRE"; real rotations/overrides/policies are delegated to PagerDuty/Opsgenie via slash-command integrations. (mattermost.com/blog/playbooks-how-to-incident-resolution/; support.atlassian.com/opsgenie/docs/integrate-opsgenie-with-mattermost/)
- **Metrics are template-built** (duration/dollars — MTTR is DIY) and retrospective publishing is Enterprise-gated. Retros are manually authored, not AI-drafted. (docs.mattermost.com/playbooks/metrics-and-goals.html)
- **Agents V2**: DB-backed agent identities, MCP tool grants, access rules, grounded + cited answers. Agents do NOT execute autonomously — tool calls surface in-channel and the initiator approves. (mattermost.com/blog/introducing-mattermost-agents-v2)
- **Fusion gap**: no documentation shows Agents V2 executing Playbook steps. The primitives exist (MCP grants + webhook runs) but agent-drives-runbook is not a shipped, integrated feature.

## Gap table (RIM Track C vs Playbooks)

| Track C item | Mattermost state | Verdict |
|---|---|---|
| C1 on-call schedules | none native | **RIM differentiates** |
| C1 escalation policies | threshold-msg / external (PagerDuty) | **RIM differentiates** |
| C2 lifecycle-as-commands/pipelines | checklist runs, not a command fabric | **RIM differentiates** |
| C3 AI cited postmortem | manual retros, Enterprise-gated, no AI | RIM differentiates (modest) |
| C4 grounded cited recall | **Mattermost HAS it** | RIM catching up (parity) |
| C5 MTTA/MTTR + history | gated dashboard | parity, MM slightly ahead on polish |
| Alert ingestion | has it | **parity — RIM shipped 2026-07-09** |

## Verdict

The wedge holds, but NOT where the roadmap spreads its weight. Not on checklists (Mattermost owns), not on metrics/retros (parity/behind), not on grounded recall (catch-up). Two genuinely defensible gaps:

1. **On-call scheduling + escalation policies IN THE BOX (C1)** — Mattermost structurally punts to PagerDuty/Opsgenie. The single strongest differentiator; make it Track C's spearhead. This is the "Slack + incident.io in one self-hosted box" story.
2. **Lifecycle-as-commands fused with the BYO-model operator under confirmation-proof gating (C2 + Track D)** — Agents V2 shares the human-gate philosophy but agents don't drive playbook steps and Playbooks aren't a programmable command fabric. Leverages RIM's plugin + command differentiators; hard to retrofit onto a checklist model.

## Recommended re-scope (pending Jakub's ratification)

- Promote **C1 to the front** — the true structural gap.
- Keep **C2 tightly coupled to Track D** — the fusion is the moat, not lifecycle commands alone.
- De-emphasize C3/C5 as differentiators; ship for completeness/parity, not as the story.
- Frame C4 (grounded recall) explicitly as catch-up to Agents V2 parity — defense, not wedge.

## Uncertainties

(a) Exact free-tier Playbooks limits / whether the metrics dashboard is Professional vs Enterprise; (b) whether Mattermost quietly shipped native on-call in 2026 — no evidence found, multiple current sources still point to PagerDuty/Opsgenie, but a negative can't be proven; (c) agent-driven runbooks — primitives exist, no integrated confirmation-gated feature documented; treat "they don't do it" as likely-but-not-certain.
