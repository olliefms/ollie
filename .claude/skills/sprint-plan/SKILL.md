---
name: sprint-plan
description: Use ONLY for cross-cutting multi-issue work that genuinely needs to land atomically. Plans and executes a batched sprint on a feature branch, opens one PR to main. For single issues, use /work-issue instead.
---

# Sprint Plan

## Overview

The exception, not the default. For solo+AI dev, the default unit of work is one issue / one PR (`/work-issue`). Use this skill only when multiple issues are *genuinely interdependent* — they must land atomically or the system is broken in between.

Plans + executes in one session. Opens one PR to main when done.

## Step 0 — Batching guardrail (MANDATORY)

Before doing anything else, answer this honestly:

> Are these issues genuinely interdependent — does the system break if any one of them lands without the others — or am I batching for ceremony?

If you cannot point to a concrete dependency (shared schema migration, atomic API contract change, shared refactor that touches every caller), **exit this skill and recommend `/work-issue` per issue.**

Examples that pass the guardrail:
- Schema migration + every callsite update that depends on it
- Renaming a public API + every consumer at once
- Splitting a module + every import update

Examples that fail:
- "These three bug fixes are all in the auth code" → run `/work-issue` three times
- "I want to ship these together as v1.5" → that's release packaging, not atomicity. Use `/work-issue` then `/cut-release` when ready.

Tell the user your call. Wait for confirmation before proceeding.

## Self-Configure

Read the project guide (`AGENTS.md` / `CLAUDE.md` / `GEMINI.md`) for Shippability Bar, Critical Constraints, Running Tests, and Triage Rules.

## Steps

### 1. Verify state and create worktree

Worktrees are the default — they keep the current checkout untouched and allow multiple sessions to run in parallel without stepping on each other.

```bash
git checkout main && git pull
```

Run the test command on main. Must be green.

Then create an isolated worktree for the sprint via the `superpowers:using-git-worktrees` skill (or `EnterWorktree` tool if available). All subsequent steps run inside that worktree's directory. Skip the worktree only if the user explicitly opts out for this run.

### 2. Scope

For each issue in scope:

```bash
gh issue view <N> --comments
```

Read bodies AND comments — comments hold the current scope. Note the dependency that justified batching.

Present the scope summary to the user and wait for confirmation.

### 3. Write the plan

**REQUIRED SUB-SKILL:** Use `superpowers:writing-plans`. Save to `docs/superpowers/plans/YYYY-MM-DD-<slug>.md`.

The plan should make the inter-task dependencies explicit — that's the whole reason this isn't `/work-issue`.

### 4. Opus plan review (one iteration, capped)

Spawn Opus (Agent tool, `model: "opus"`):

> Review `<plan path>`. Verify: file paths exist, method signatures consistent across tasks, error variants match existing code, no placeholder steps. Apply Shippability Bar and Triage Rules from `<project guide>`. Report blockers/significants only.

Fix blockers inline. If iteration 2 still finds blockers, stop and escalate.

### 5. Feature branch

Inside the worktree from Step 1:

```bash
git checkout -b feature/<slug>
git push -u origin feature/<slug>
git add docs/superpowers/plans/<plan-file>
git commit -m "chore: plan for <slug>"
git push
```

(If the worktree was created on a fresh branch already, just push and commit the plan.)

### 6. Execute

**REQUIRED SUB-SKILL:** Use `superpowers:subagent-driven-development` for task execution. Pass each subagent:
- The specific task from the plan
- The Critical Constraints from the project guide
- The branch name
- **Do not bump version numbers.** No edits to `Cargo.toml` / `package.json` / `pyproject.toml` versions or version-coupled stamps (PWA `CACHE_NAME`, `?v=` asset stamps). Versioning is `cut-release`'s job — a sprint that hardcodes a version desyncs the release.

After every subagent:

```bash
git diff --stat HEAD
```

If non-empty, commit manually — subagents silently skip commits occasionally.

Run the test command. Never proceed with red tests.

### 7. Self-review with triage

Same rules as `/work-issue` step 5:

- **blockers** — correctness, security, data-loss, broken contract
- **significants** — meaningful, address in-PR
- **nits** — note in PR description or discard, never file as issues

Review your own diff:

```bash
git diff main...HEAD
```

### 8. Opus code review (one iteration, capped)

Spawn Opus:

> Review `git diff main...HEAD` against the plan at `<plan path>` and the Shippability Bar from `<project guide>`. Classify findings as blocker/significant/nit. Report blockers and significants only.

Same 2-iteration cap. Escalate at iteration 2 if blockers remain.

### 9. Lessons (optional)

If the sprint surfaced reusable lessons, add them to the project guide as a separate commit. 2–4 max. Format: **rule** — why — how to apply. Skip this step if nothing surfaced worth recording.

### 10. Open PR

```bash
gh pr create --base main --head feature/<slug> \
  --title "<slug>: <headline>" \
  --body "$(cat <<'EOF'
## Summary
- [bullet per in-scope issue with #number]

## Why batched
[Concrete dependency that prevented per-issue PRs]

## Test plan
- [ ] Project test suite passes
- [ ] [manual verification]

## Notes
[nits or patterns; omit if none]

Closes #<N>, #<N>, #<N>
EOF
)"
```

### 11. Self-merge if clean

Same conditions as `/work-issue`: blockers=0, significants=0, CI green, tests green.

```bash
gh pr merge <PR#> --squash --delete-branch
```

Then exit the worktree (`ExitWorktree` if used, or remove the worktree with `git worktree remove <path>`) and return to the main checkout:

```bash
git checkout main && git pull
```

Close each in-scope issue with a verification comment.

If conditions not met, leave PR open and tell the user.

### 12. Release decision

Tell the user the sprint is merged and ask whether to `/cut-release` now or accumulate more work first.

---

## Common Mistakes

| Mistake | Fix |
|---|---|
| Skipping the batching guardrail | Step 0 is mandatory. Most "sprints" are actually three `/work-issue` runs. |
| Treating this skill as default | It's the exception. Default is `/work-issue`. |
| Filing Opus nits as backlog issues | Nits go in PR notes or get discarded. |
| Looping Opus past 2 iterations | Hard cap. Escalate at iteration 2. |
| Forgetting to check `git diff --stat HEAD` after each subagent | Silent commit skip is a real failure mode |
| Skipping the lessons step "to be safe" | Only add lessons if something genuinely reusable surfaced. Skip if not. |
| Running the sprint directly in the main checkout | Worktrees are the default. They keep main untouched and let multiple sessions run in parallel without interfering. |
| Bumping a version number or cache stamp | Versioning is `cut-release`'s job. Leave `Cargo.toml`, `package.json`, PWA `CACHE_NAME`/`?v=` stamps, etc. untouched in sprint PRs. |
