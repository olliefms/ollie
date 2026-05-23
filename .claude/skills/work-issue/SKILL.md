---
name: work-issue
description: Use when picking up a single GitHub issue — implements, self-reviews under triage rules, opens PR to main, self-merges if clean. Default unit of work for solo+AI development.
---

# Work Issue

## Overview

One issue → one branch → one PR → merge. The default flow. Trunk-based: PRs target `main`, no release branch.

**Usage:** `/work-issue <issue-number>`

## Self-Configure First

Find and read the project guide (`AGENTS.md`, `CLAUDE.md`, or `GEMINI.md`) in full. You need:
- **Shippability Bar** — the definition of "done." Skill operates against it.
- **Critical Constraints** — non-negotiable rules
- **Running Tests** — exact test command
- **Triage Rules** — blocker / significant / nit classification

If the project guide does not define a Shippability Bar, stop and tell the user. Do not invent one.

## Steps

### 1. Read the issue completely

```bash
gh issue view <N> --comments
```

Issue bodies go stale; comments hold the current scope. Read both. If scope is ambiguous, ask the user before branching — clarifying questions are cheaper than rework.

### 2. Verify clean starting state

```bash
git checkout main && git pull
```

Run the project's test command. If red before you touch anything, **stop and tell the user.** Do not implement against a broken baseline.

### 3. Branch

```bash
git checkout -b issue-<N>-<short-slug>
```

### 4. Implement

Edit, test, commit in small focused commits. Use the project's test command after every meaningful change. Never stack failures — fix before continuing.

If the work expands beyond the issue scope, stop and ask the user. Don't quietly grow the PR.

### 5. Self-review with triage

Before opening the PR, review your own diff against the Shippability Bar from the project guide:

```bash
git diff main...HEAD
```

Classify every finding as **blocker**, **significant**, or **nit**:

- **blocker** — correctness, security, data-loss, broken contract, missing critical-path test
- **significant** — meaningful issue that affects maintainability or correctness in edge cases; worth addressing in this PR
- **nit** — style, taste, micro-optimization, refactor opportunity; not addressed, not tracked

Fix all blockers and significants inline. Note nits in the PR description under a `## Notes` section if any are worth mentioning, then move on. **Never file a GitHub issue for a nit.**

### 6. Opus review (one iteration, capped)

Spawn Opus as a subagent (Agent tool, `model: "opus"`):

> Review the diff on branch `<branch>` vs main (`git diff main...HEAD`). Apply the Shippability Bar and Triage Rules from `<project guide filename>`. Classify every finding as blocker, significant, or nit. Report each with file:line citations. Do not enumerate nits — only mention nits if they cluster into a pattern worth noting.

Triage the response:

- **blockers** — fix inline, then run Opus once more (iteration 2). If iteration 2 still finds blockers, **stop and escalate to the user.** Do not loop further.
- **significants** — fix inline if < 30 min, otherwise stop and discuss with the user
- **nits** — note in PR description if a pattern, otherwise discard

Hard cap: **2 Opus iterations.** Three is a sign the change needs human eyes.

### 7. Final verification

```bash
# project test command — must be green
```

```bash
git log --oneline main..HEAD    # confirm commits are what you expect
```

### 8. Open PR

```bash
gh pr create --base main --head <branch> \
  --title "<type>(<area>): <headline> (#<N>)" \
  --body "$(cat <<'EOF'
Closes #<N>

## Summary
- [what changed and why, 1-3 bullets]

## Test plan
- [ ] Project test suite passes
- [ ] [manual verification if UI or integration]

## Notes
[nits or patterns worth mentioning; omit section if none]
EOF
)"
```

### 9. Self-merge if clean

Merge if **all** of:
- blockers = 0
- significants = 0 (addressed in PR, not deferred)
- CI green
- Local test suite green

```bash
gh pr merge <PR#> --squash --delete-branch
```

Then close the issue with a verification comment:

```bash
gh issue comment <N> --body "Implemented in #<PR#>. Verified: [what you tested]"
gh issue close <N> --reason completed
```

If self-merge conditions are not met, **leave the PR open and tell the user.** Do not self-merge with deferred significants.

### 10. Update main locally

```bash
git checkout main && git pull
```

---

## Common Mistakes

| Mistake | Fix |
|---|---|
| Filing nits as GitHub issues | Nits go in the PR `## Notes` section or get discarded. The backlog is not a landfill. |
| Looping Opus review past 2 iterations | Stop at iteration 2. If still finding blockers, the change needs human review. |
| Self-merging with deferred significants | Significants get fixed in-PR or the PR stays open for user review. |
| Treating "Opus found something" as "must fix" | Apply the triage rules. Opus output is input, not orders. |
| Skipping the project guide read | The Shippability Bar and Triage Rules live there. Don't invent them. |
| Growing the PR scope mid-flight | If scope expands, stop and ask. Don't grow the PR silently. |
| Branching from a red baseline | Verify tests green on main before branching. |
