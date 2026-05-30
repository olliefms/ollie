---
name: sprint-plan
description: Use ONLY for cross-cutting multi-issue work that genuinely needs to land atomically. Plans and executes a batched sprint on a feature branch, opens one PR to main. For single issues, use /work-issue instead.
---

# Sprint Plan

The exception, not the default. For solo+AI dev the default unit of work is one issue / one PR (`/work-issue`). Reach for this skill only when several issues are *genuinely interdependent* — they must land atomically or the system is broken in between. It plans and executes in one session and opens a single PR to `main`.

This skill leans on `/work-issue`'s rules for the parts they share — triage, the Opus cap, the version-number prohibition, the self-merge gate, and the **work-in-flight lock** (`/work-issue`'s "The work-in-flight lock"). It spells out only what's different about a batched sprint: the guardrail that decides whether a sprint is even justified, and the worktree + plan + subagent execution machinery. The one twist on the lock is fan-out: a sprint holds *every* in-scope issue, so claim and release them all, not just one.

## Step 0 — the batching guardrail (do this first, every time)

Before anything else, answer honestly:

> Do these issues *break the system* if any one lands without the others — or am I batching for ceremony?

If you can't name a concrete dependency — a shared schema migration, an atomic API contract change, a refactor that touches every caller at once — **exit this skill and recommend `/work-issue` per issue.** This guardrail is the whole reason the skill exists; almost everything that *feels* like a sprint is really three `/work-issue` runs wearing a trenchcoat.

Passes: schema migration + every callsite that depends on it · renaming a public API + every consumer · splitting a module + every import.

Fails: "three bug fixes all in the auth code" (→ `/work-issue` ×3) · "ship these together as v1.5" (that's release packaging — `/work-issue` then `/cut-release`).

State your call and wait for confirmation before proceeding.

## Self-configure

Read the project guide (`AGENTS.md` / `CLAUDE.md` / `GEMINI.md`) for the Shippability Bar, Critical Constraints, test command, and Triage Rules.

## The flow

1. **Isolate in a worktree.** Verify `main` is green first, then run the whole sprint inside an isolated worktree (`superpowers:using-git-worktrees`, or the `EnterWorktree` tool) — it keeps your main checkout untouched and lets parallel sessions coexist. Skip only if the user explicitly opts out.

2. **Scope, then claim every in-scope issue.** Read each in-scope issue *and its comments* (comments hold the live scope). Restate the scope and the dependency that justified batching, then wait for confirmation. Once confirmed, apply the work-in-flight lock to *all* of them: honor `pause:agents`, and if any issue already carries `work:in-progress`, stop and tell the user — a sprint can't proceed half-claimed. Otherwise `gh issue edit <N> --add-label work:in-progress` for each, with a claim comment naming the sprint branch (`feature/<slug>`) so a colliding session sees the whole batch is held. Check `gh issue list --label work:in-progress` for issues another session holds that overlap your sprint's blast radius.

3. **Write the plan** with `superpowers:writing-plans`, saved to `docs/superpowers/plans/YYYY-MM-DD-<slug>.md`. Make the inter-task dependencies explicit — that's the entire reason this isn't `/work-issue`.

4. **Review the plan** with one capped Opus pass: file paths exist, signatures consistent across tasks, error variants match real code, no placeholder steps. Same 2-iteration cap; escalate if blockers remain at iteration 2.

5. **Branch** `feature/<slug>`, commit the plan, push.

6. **Execute** with `superpowers:subagent-driven-development`. Hand each subagent its task, the Critical Constraints, and the branch name — and the version prohibition (below). After every subagent, check `git diff --stat HEAD` and commit manually if non-empty: **subagents silently skip commits occasionally**, and a lost commit mid-sprint is brutal to reconstruct. Keep tests green throughout; never proceed on red.

7. **Self-review, then one capped Opus pass** against the plan and the Shippability Bar. Triage exactly as `/work-issue` does (blocker / significant / nit; nits noted or discarded, never filed).

8. **Record lessons (optional).** If the sprint surfaced genuinely reusable lessons, add 2–4 to the project guide as a separate commit (**rule — why — how to apply**). Skip entirely if nothing worth keeping surfaced; don't pad.

9. **Open one PR** to `main`, title `<slug>: <headline>`, body summarizing each in-scope issue, a **Why batched** section naming the dependency, and `Closes #…` for every issue.

10. **Self-merge if clean** — same gate as `/work-issue` (blockers 0, significants 0 and addressed, CI green, tests green). Squash-merge, exit the worktree, close each issue with a verification comment. Closing the issues drops their locks. If the gate fails, leave the PR open, swap every in-scope issue's lock to `work:ready-for-review`, and tell the user — and on any abort, release the locks across the whole batch (`work:blocked` if escalating, remove if abandoned). Never leave a half-claimed sprint behind.

11. **Release decision.** Tell the user the sprint is merged and ask whether to `/cut-release` now or accumulate more first.

### Version numbers stay frozen

No subagent edits `Cargo.toml` / `package.json` / `pyproject.toml` versions or version-coupled stamps (PWA `CACHE_NAME`, `?v=` asset stamps). Versioning is `cut-release`'s job; a sprint that hardcodes a version desyncs the release.

## Common mistakes

| Mistake | Fix |
|---|---|
| Skipping the batching guardrail | Step 0 is the point of the skill. Most "sprints" are three `/work-issue` runs. |
| Treating this as the default | It's the exception. Default is `/work-issue`. |
| Running the sprint in the main checkout | Use a worktree — keeps main clean, lets sessions run in parallel. |
| Not checking `git diff --stat HEAD` after each subagent | Silent commit skip is a real failure mode. |
| Looping Opus past 2 iterations | Hard cap. Escalate at iteration 2. |
| Filing nits as backlog issues | Nits go in PR notes or get discarded. |
| Padding the lessons step | Only record genuinely reusable lessons. Skip if none. |
| Claiming only some of the batch | A sprint holds *every* in-scope issue — `work:in-progress` on all of them, or another session grabs an uncovered one. |
| Starting a sprint over a contended issue | Any in-scope issue already `work:in-progress` (or `pause:agents` set) → stop, don't proceed half-claimed. |
| Leaving locks stuck after abort | Release across the whole batch on every exit — merge drops them, leave-open → `work:ready-for-review`, escalate → `work:blocked`, abandon → remove. |
| Bumping a version or cache stamp | That's `cut-release`'s job. Leave manifests and PWA stamps alone. |
