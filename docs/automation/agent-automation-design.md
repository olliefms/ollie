# Agent Automation Architecture

**Status:** Design — pre-implementation
**Scope:** Ollie repository (initial), Spanish Academy Live (second deployment), generalizable to SMB fractional CTO engagements
**Author:** Jim (ergophobe)
**Last revised:** May 2026

---

## 1. Purpose

This document specifies the architecture for an automated issue-to-PR workflow driven by Claude Code agents, deployed initially to the Ollie repository. The goal is to compress the solo developer's workflow from a multi-session `/sprint-plan` → `/sprint-execute` → `/sprint-finalize` loop into an event-driven pipeline where agents handle triage, implementation, and review, with human-in-the-loop checkpoints calibrated to risk.

The architecture is intentionally portable. Patterns codified here are intended to be reused — under different billing and identity contexts — when deployed to client repositories as part of fractional CTO engagements.

---

## 2. Problem Statement

Traditional SDLC assumes:

- Writing code is the scarce resource; reviewing it is cheap.
- "Done" means a reviewer has signed off on everything they noticed.
- Sprints batch work to amortize coordination overhead across a team.

None of these assumptions hold for solo + AI development:

- **Writing is near-free.** Agents generate code faster than a human reviews it. The bottleneck has inverted: review and governance are now the scarce resources.
- **Reviewers always find something.** AI reviewers optimize for finding issues, not for deciding whether issues matter. Treating every finding as a thing-to-fix produces an infinite loop and a backburner of "robot homework" that crowds out user-driven priorities.
- **Sprint batching has no coordination benefit when there's one operator.** It mainly adds ceremony.

The architecture below addresses each of these by (1) defining shippability once, in code, rather than relitigating per-session; (2) requiring the reviewer to triage findings as blocker / significant / nit and only loop on blockers; and (3) defaulting to single-issue, single-session, single-PR units of work, with batching reserved for genuine multi-issue dependencies.

---

## 3. Design Principles

Adapted from the Agent-Driven Development Lifecycle (ADLC) discourse, with consultancy framing stripped out:

1. **Bottleneck inversion.** Optimize for reviewer cost and governance overhead, not writer cost. Cheap to generate, expensive to verify.
2. **Observe is continuous, not terminal.** Don't try to gate everything at PR time. Use post-merge signals (revert rate, follow-up issues, production behavior) as the truth signal.
3. **Intent + guardrails replace step approval.** Shippability bar is defined once in `AGENTS.md` and applies to every session, not negotiated per-PR.
4. **Prompts and skills are infrastructure.** Versioned, tested, treated with the same rigor as production code.
5. **GitHub is the substrate.** No external services. State, audit trail, and dashboard all use GitHub primitives.

---

## 4. Architecture Overview

Three event-driven workflows, each calling `anthropics/claude-code-action@v1`, orchestrated entirely through GitHub Actions:

| Workflow | Trigger | Purpose |
|---|---|---|
| **Triage** | `issues: opened` | Validate, classify, route, or reject incoming issues |
| **Work** | `issues: labeled` (filtered to `triage:approved`) | Implement the issue and open a PR |
| **Review** | `pull_request: opened, synchronize` | Triage-based review, auto-merge if narrow-scope and clean |
| **Metrics** (optional) | `schedule: weekly` | Aggregate audit-trail metadata into a dashboard summary |

The system reads from and writes to a small set of GitHub-native primitives. There is no external database, no separate orchestrator, no parallel UI.

---

## 5. Execution Layer: GitHub Actions + OAuth

Decision: Use `anthropics/claude-code-action@v1` triggered by GitHub Actions, authenticated via `ANTHROPIC_API_KEY` (set up via `/install-github-app` in Claude Code).

**Why not Claude Code Routines:**

- 15 runs/day cap on Max would be exhausted by a single active day on Ollie alone, before scaling to multiple projects or clients.
- Research-preview status means features can be restructured without notice — wrong foundation for production.
- Identity blur: routine actions appear under the personal account, breaking attribution for audit purposes.
- No retry semantics; "green" status indicates infrastructure success only, not task success.
- Individual-account-bound; cannot legitimately be used to run automation on a client's behalf.

**Why not API key:**

- Max OAuth is functionally identical for single-user workloads and uses already-paid subscription quota.
- Switching between OAuth and direct API key is one line of YAML, so this is not a lock-in decision.

**Portability note:** For client deployments, this workflow architecture is identical; only the secret name and identity change. Clients deploy with their own API key on their own GitHub org with their own GitHub App identity for the bot.

**Bot identity:** A dedicated GitHub App (e.g., `ollie-agent[bot]`) is created so agent commits and comments are visibly attributable to the automation rather than the human operator. This is essential for audit clarity and is solved structurally by GHA in a way that Routines cannot match.

---

## 6. State Model: Labels

All workflow state is expressed as GitHub labels. Labels are queryable, free, and trigger workflows on change.

**Label families** (exact taxonomy to be finalized in implementation):

- `triage:*` — pending, approved, rejected, needs-clarification, flagged-human
- `work:*` — queued, in-progress, blocked
- `review:*` — auto-approved, blockers-found, significants-pending, human-required
- `trust:*` — mirrors the trust-list tier of the issue author (for routing logic)
- `pause:agents` — circuit breaker; presence of this label on a repo or issue disables agent processing

Transitions are agent- or human-driven. The workflows read these labels to decide whether to act.

---

## 7. Trust Tiers

A repo-committed `.github/trust-list.yml` defines tiers and the autonomy each unlocks:

| Tier | Examples | Triage Treatment | Auto-Work Eligible |
|---|---|---|---|
| `maintainer` | Operator, partner | Presumed good faith, light validation | Yes |
| `trusted` | Known recurring contributors | Standard validation | Yes (narrow scope) |
| `contributor` | Returning external contributors | Full validation | No — human approves |
| `new` | First-time accounts | Heightened scrutiny, prompt-injection hardening | No |
| `blocked` | Known bad actors | Auto-rejected, flagged for human review | No |

Trust list edits are themselves PRs, making the trust graph auditable. The list is small — these are not enterprise RBAC tiers.

**Note:** Auto-rejection alerts to GitHub for suspected malicious accounts are explicitly out of scope due to false-positive blast radius. Suspicious accounts are flagged for human review, never auto-reported.

---

## 8. Audit Trail: Structured Comments

Every agent decision posts a comment to the relevant issue or PR that is human-readable on top and machine-parseable below:

```markdown
## Triage: APPROVED

**Trust tier:** maintainer
**Classification:** simple-bug
**Confidence:** high
**Reasoning:** Clear repro, scope <50 LOC, no security-sensitive paths touched.
**Next action:** Queued for /work-issue.

<!-- AGENT_META
{
  "phase": "triage",
  "decision": "approve",
  "tier": "maintainer",
  "class": "simple-bug",
  "confidence": "high",
  "model": "claude-opus-4-7",
  "run_url": "https://github.com/.../actions/runs/...",
  "ts": "2026-05-19T14:23:00Z"
}
-->
```

The HTML-commented JSON block is the structured data layer; the prose above is the audit trail for a human reading the issue. The `run_url` provides a one-click forensics path to the GHA run logs.

**Comment prefix convention:** All agent-authored comments begin with a visible marker (e.g., `🤖 [agent:triage]`) so human-vs-agent provenance is obvious at a glance, supplementing the GitHub App identity.

---

## 9. Dashboard: GitHub Projects v2

One Project per repo (or one shared across active repos):

- **Kanban view:** Triage Queue → Approved → In Work → PR Open → Merged → Released
- **Table view:** Triage backlog with custom fields (class, confidence, age)
- **Chart view:** Approval / rejection / auto-merge / post-merge-revert rates over time

The weekly metrics workflow aggregates `AGENT_META` blocks from comments and writes a summary issue tagged `metrics:weekly`, or appends to a tracked markdown file in `docs/metrics/`. No external dashboard service.

---

## 10. Shippability Bar (in AGENTS.md)

The standard for "done" is defined once in the repo's `AGENTS.md` and applies to all sessions, agent-driven or interactive:

- **Blockers** (must fix before merge): correctness, security, data-loss, broken contracts, missing tests on critical paths.
- **Significants** (fix this PR if reasonable, otherwise file follow-up): real bugs in non-critical paths, missing tests on non-critical paths, regressions in covered behavior.
- **Nits** (note in PR description, do not block, do not file new issues): style, taste, micro-optimizations, refactor opportunities.

**Loop termination:** Review iterates a maximum of 2 times. If iteration 2 still surfaces blockers, the PR is flagged `review:human-required` and the agent stops. No infinite loops on agent-generated nitpicks.

**No robot homework:** Review findings classified as nits do not become new GitHub issues. The operator may manually file follow-ups if a nit is worth tracking.

---

## 11. Phased Rollout

The architecture is enabled incrementally. Each phase validates the prior before unlocking the next.

**Phase 1: Triage-only.** Triage workflow runs on every new issue, classifies, posts decision comment, applies labels. No auto-work, no auto-merge. Human (operator) still drives implementation. Goal: validate triage accuracy, calibrate trust tiers, tune the shippability bar in `AGENTS.md`.

**Exit criteria:** ~30 days or ~50 issues triaged with acceptable false-approval and false-rejection rates.

**Phase 2: Add auto-work for trust-listed authors.** Work workflow enabled, but only fires on issues from `maintainer` or `trusted` tiers. PRs still require human merge. Review workflow runs in shadow mode (posts comments, does not merge). Goal: validate implementation quality and review-agent calibration against real PRs.

**Exit criteria:** ~50 PRs reviewed in shadow mode with the review agent's recommendations matching the operator's merge decision >90% of the time.

**Phase 3: Narrow-scope auto-merge.** Review workflow auto-merges PRs that meet all of: <50 LOC changed, no migrations, no dependency updates, no auth/secret/payment paths touched, all CI green, no blockers, from trust-listed author. Everything outside the auto-merge envelope continues to require human merge.

**Exit criteria:** Ongoing. Post-merge revert rate monitored; if it climbs, the auto-merge envelope tightens.

---

## 12. Operational Safeguards

- **Circuit breaker:** A `pause:agents` label, when applied at the repo level (via a tracking issue) or per-issue, disables agent processing. Workflows check for this label as their first step.
- **Per-account rate limit:** Issues from `new`-tier accounts are capped (e.g., 3/day) to limit prompt-injection or spam-attack blast radius.
- **Concurrency control:** GHA `concurrency` groups prevent two work workflows from colliding on overlapping code.
- **Recovery sweep:** A scheduled daily workflow re-triages issues stuck in `triage:pending` >24h and re-reviews PRs without a review comment, catching silent failures.
- **Prompt injection hardening:** Issue and PR text is treated as data, never as instruction. System prompts explicitly delimit user content. Findings that look like injection attempts are flagged, not actioned.

---

## 13. Open Items (to resolve in implementation)

1. **Final label taxonomy** — exact label names and the transition graph.
2. **Trust-list schema** — exact YAML structure and fields per tier.
3. **`AGENT_META` schema** — exact field set and versioning convention.
4. **Project board fields and views** — concrete configuration.
5. **Workflow YAML** — three files plus optional metrics.
6. **Routine prompts** — the per-workflow prompt text, written for self-contained autonomous execution.
7. **AGENTS.md updates** — the shippability bar content, integrated with any existing AGENTS.md.
8. **GitHub App identity** — one-time setup for the bot account.

These are the deliverables of the next implementation session.

---

## 14. Portability Notes

This architecture is designed to be reused across SMB fractional CTO engagements. When deployed to a client repo:

- **Execution layer:** Identical GHA workflows, with `anthropic_api_key` replacing `claude_code_oauth_token`. Client pays for and owns their inference billing.
- **Identity:** A new GitHub App is provisioned within the client's GitHub org, named for their context. All agent actions appear under the client's bot identity, not the operator's.
- **Trust list:** Initialized with the client's maintainers; tier definitions ported as-is.
- **AGENTS.md:** Shippability principles port verbatim; project-specific specifics layered on top.
- **Project board:** Template structure ported; populated with client's labels and milestones.
- **Operator role:** Configuration, calibration, periodic audit of metrics, evolution of the shippability bar. The operator does not run the client's automation; the client's infrastructure does.

The repeatable artifacts are this design document, the workflow YAML templates, the `AGENTS.md` template, the trust-list schema, and the Project board template. These constitute the fractional CTO playbook.

---

## 15. References

- ADLC discourse: AWS `awslabs/aidlc-workflows`, adlc.io, Anthropic Claude Code GitHub Actions docs
- `anthropics/claude-code-action@v1`
- Prior conversation context: design rationale and decision history captured in operator's chat archive (May 2026)
