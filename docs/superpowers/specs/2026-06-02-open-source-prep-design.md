# Open-source prep for Ollie — design (issue #233)

**Date:** 2026-06-02
**Issue:** #233 — Open source the FOSS edition
**Status:** Approved design, pending implementation plan

## Goal

Get the Ollie repository **ready to flip public on launch day**. This spec covers the
preparation work only. The sequencing of the version cut / org move / flip-public is a
launch-day decision and is out of scope here.

## Decisions

- **License: AGPL-3.0.** Copyleft keeps every fork's improvements open and flowing back to
  the project.
- **Contributions: DCO sign-off, no CLA.** `Signed-off-by` on each commit; low friction
  and AGPL-compatible.
- **Scope: single-fleet self-hosting.** The project targets a single fleet running its own
  instance. Features outside that scope are not in scope for now and won't be merged into
  the core.
- **Full git history is publishable.** A `gitleaks` scan over the full history (412 commits
  across all refs) is clean — the only hits are `Bearer your-secret-key` placeholders in
  README example curl commands (false positives). No history rewrite for secrets is
  required. (See Workstream C for the separate content review of historical docs.)

## Workstreams

### A — Repo hygiene (gating)

- Add `.gitleaks.toml` with an allowlist for the README documentation placeholders;
  confirm a clean scan.
- Wire `gitleaks` into CI as an ongoing guard (a `.github/workflows` job) so no future
  secret can land.
- Verify `.gitignore` covers all secret-bearing patterns.
- Confirm `.env.example` contains only placeholders (it does) — but it still lists the
  removed `ADMIN_API_KEY`; that cleanup is handled in Workstream B alongside the README.

### B — OSS artifacts

- `LICENSE` — AGPL-3.0 verbatim.
- `CONTRIBUTING.md` — DCO sign-off instructions (`git commit -s`); point at `AGENTS.md`
  for technical setup; state that the project targets single-fleet self-hosting scope.
- `SECURITY.md` — **GitHub Private Vulnerability Reporting (PVR) only.** Point reporters at
  the repo's "Report a vulnerability" button; no email fallback (no security inbox exists
  yet). Enabling PVR itself is a Workstream D step.
- `CODE_OF_CONDUCT.md` — Contributor Covenant.
- `CODEOWNERS` — trivial while solo, but set up.
- GitHub repo `description` + `topics`.
- **README polish** — it is stale after #236: it still documents the removed admin API
  (`/api/v1/*`) and describes *four* API surfaces. Fix to three surfaces and drop the
  `ADMIN_API_KEY` reference (also remove it from `.env.example`).

### C — Internal-docs content review

Scope: the internal planning/spec docs under `docs/superpowers/plans/` +
`docs/superpowers/specs/` (~37 files), plus `docs/automation/agent-automation-design.md`.

- Review each for any content **not intended for public release** — third-party/partner
  names, anything that isn't general technical documentation, and any material whose
  publication would weaken the agent-automation defenses.
- **The review must cover git history, not just the working tree.** Anything sensitive in a
  historical commit means "publish full history" would still expose it, which would require
  a targeted history rewrite — so this workstream can feed back into Workstream A's
  no-rewrite conclusion.
- Disposition per item: keep public / relocate out of the repo / rewrite from history.
  Expectation: most are general technical docs and stay; relocate the rest.

### D — Agent-automation hardening for public exposure

- **Trust-tier handling** — external `new`-tier issues become real, not hypothetical
  (`.github/trust-list.yml`, `.github/workflows/triage.yml`, `.github/prompts/triage.md`).
- **Branch-protection ruleset** — apply once public.
- **Enable Private Vulnerability Reporting** on the repo (and re-enable after the org move)
  so the SECURITY.md "Report a vulnerability" channel actually exists.
- **Triage prompt-injection hardening (timeboxed floor + split):** the live triage agent
  acts on stranger-submitted issue/PR text the day the repo goes public, so a minimum floor
  must gate launch.
  - **Launch gate (timeboxed, ~1 focused session):** a single pass against the known
    vectors — instruction-override in issue/PR bodies, fake system-prompt blocks,
    data-exfil via tool call, prompt-leaking. Ship "good enough for a small public repo"
    and stop.
  - **Split-out follow-up (does NOT gate launch):** deeper, ongoing adversarial red-teaming
    becomes its own separate issue, run after/alongside launch.
- **DCO enforcement (deterministic, not the review agent):**
  - Add a deterministic DCO check — the DCO GitHub App (probot/dco) or a small GitHub
    Action — that fails if any commit in a PR lacks a valid `Signed-off-by` matching the
    commit author. The code-review agent is deliberately **not** used: DCO is a mechanical
    regex check, and an LLM gate would be non-deterministic, costlier, and prompt-injectable
    on a public repo.
  - Make the DCO check a **required status check** in the branch-protection ruleset — that
    is what actually blocks merge, for humans and automation alike.
  - **Fix our own automation to sign off:** a required DCO check blocks *everyone*,
    including the `work-issue` / `sprint-plan` agent flows that open and self-merge PRs.
    Update those skills and the `.github/workflows/claude*.yml` commit conventions to add
    `git commit -s` (`Signed-off-by`) so self-merges pass the gate.

## Out of scope

- Comms / announcement — launch-day.
- The version cut / org move / flip-public execution and its sequencing.
- Any feature beyond single-fleet self-hosting scope.

## Open flags (acknowledged)

- Workstream D's injection-hardening is resolved as a timeboxed launch floor plus a
  split-out, non-gating follow-up issue (see Workstream D).
- SECURITY.md is PVR-only for now.
- Commit history is authored under a personal identity/email and GitHub handle — not
  secrets, but a conscious "fine to be public" is assumed.
