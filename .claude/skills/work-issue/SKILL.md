---
name: work-issue
description: Use when picking up a single GitHub issue — implements, self-reviews under triage rules, opens PR to main, self-merges if clean. Default unit of work for solo+AI development.
---

# Work Issue

One issue → one branch → one PR → merge. The default flow. **Trunk-based: PRs target `main`, no release branch.** Branch name: `issue-<N>-<slug>`. Merge with squash + delete-branch.

**Usage:** `/work-issue <issue-number>`

This skill assumes you already know how to drive git and `gh`. It only spells out the decisions specific to this project — where to find the bar for "done," how to triage findings, what gates a self-merge, and the rules that exist because we got burned before. Execute the ordinary mechanics yourself.

## Self-configure first

Read the project guide (`AGENTS.md` / `CLAUDE.md` / `GEMINI.md`) in full and pull out:
- **Shippability Bar** — the definition of "done." Everything below operates against it.
- **Critical Constraints** — the non-negotiables.
- **Running Tests** — the exact command.
- **Triage Rules** — blocker / significant / nit.

If the guide defines no Shippability Bar, stop and tell the user. Don't invent one — a made-up bar is worse than none, because it looks authoritative.

## The work-in-flight lock

Other sessions — interactive or GHA-driven — may be working this repo at the same time. The `work:in-progress` label is the lock that keeps two of them off the same issue. It's the agreed state-model primitive, not an ad-hoc convention (see `docs/automation/agent-automation-design.md` §6 / §12). Treat it as mandatory:

- **Respect the circuit breaker first.** If this issue carries `pause:agents` (or the repo's pause tracking issue does), stop — all agent automation is paused.
- **Refuse a contended issue.** If the issue already has `work:in-progress`, another session is on it — **stop and tell the user**, don't race it. Take over only if the user confirms it's stale (a crashed session left the label stuck).
- **Claim before you branch.** `gh issue edit <N> --add-label work:in-progress`, then post a one-line claim comment naming your branch (`issue-<N>-<slug>`) so a colliding session has the context to work around you. That's the moment the issue becomes yours.
- **See what else is in flight.** `gh issue list --label work:in-progress` lists every issue another session holds. If your change touches code one of those issues also touches, coordinate or pick a different issue — don't step on an in-flight change.
- **Release on every exit path** (merge, leave-open, escalate, bail — see the flow). A label left stuck after the session ends blocks the next session for nothing.

## The flow

1. **Read the issue *and its comments*.** Bodies go stale; the live scope lives in the comments. If scope is still ambiguous after reading both, ask before branching — a clarifying question is far cheaper than rework.

   Then **claim the issue** per the lock above: honor `pause:agents`, refuse if `work:in-progress` is already set, otherwise add the label and post the claim comment. Do this *before* you branch.

2. **Start from green.** Run the test command on `main` before you touch anything. If it's red before you've done a thing, stop and tell the user — never build on a broken baseline, you'll never know which red is yours.

3. **Implement on a branch, staying inside the issue's scope.** If the work starts pulling in things the issue didn't ask for, stop and ask rather than quietly growing the PR — scope creep is invisible to the reviewer until it's too late to undo.

4. **Self-review against the Shippability Bar**, then run one capped Opus pass (see below).

5. **Self-merge only if it's clean** (see gate below), then close the issue with a one-line verification comment noting what you actually tested. **Release the lock** as you go: on self-merge the closed issue no longer holds the repo, so drop `work:in-progress`. If you leave the PR open for a human, swap `work:in-progress` → `work:ready-for-review`. If you escalate or bail mid-flight, swap → `work:blocked` (escalated) or just remove `work:in-progress` (abandoned with no progress). Never leave `work:in-progress` set on an issue you've stopped working.

### Never touch version numbers

Do not edit the version in `Cargo.toml` / `package.json` / `pyproject.toml`, and do not touch version-coupled stamps — the driver PWA `CACHE_NAME` or `?v=` asset stamps. Renumbering is exclusively `cut-release`'s job. An issue PR that hardcodes a version desyncs the next release (this rule exists because PR #284 did exactly that). Leave every version string alone.

## Triage: the calibration that matters

Classify every finding — your own and Opus's — as one of:

- **blocker** — correctness, security, data-loss, a broken contract, or a missing test on the critical path.
- **significant** — meaningfully affects maintainability or correctness in edge cases. Worth fixing in *this* PR.
- **nit** — style, taste, micro-optimization, a refactor you'd enjoy. Not fixed, not tracked.

Fix blockers and significants inline. Mention nits in a `## Notes` section of the PR only if they cluster into a pattern worth a human's attention. **Never file a nit as a GitHub issue** — the backlog is for work, not for robot homework.

## Opus review: one pass, hard cap of two

Spawn Opus (Agent tool, `model: "opus"`):

> Review `git diff main...HEAD`. Apply the Shippability Bar and Triage Rules from `<project guide filename>`. Classify every finding as blocker / significant / nit with file:line citations. Don't enumerate nits — only flag a nit if nits cluster into a pattern.

Then triage the response yourself — **Opus output is input, not orders.** Fix blockers and run it once more. If iteration 2 still surfaces blockers, stop and escalate to the user; three iterations means the change needs human eyes, not another loop.

## Self-merge gate

Squash-merge and close the issue **only if all** hold:
- blockers = 0
- significants = 0, *addressed in this PR* (not deferred)
- CI green
- local test suite green

PR title: `<type>(<area>): <headline> (#<N>)`, body closes the issue. If any condition fails, leave the PR open, swap the lock to `work:ready-for-review`, and tell the user — never self-merge with a deferred significant.

## Common mistakes

| Mistake | Fix |
|---|---|
| Filing nits as GitHub issues | Nits go in PR `## Notes` or get discarded. The backlog is not a landfill. |
| Looping Opus past 2 iterations | Stop at iteration 2. Still finding blockers → human review. |
| Self-merging with deferred significants | Fix in-PR, or leave the PR open. |
| Treating "Opus found something" as "must fix" | Triage it. Opus output is input. |
| Inventing a Shippability Bar | It lives in the project guide. No bar there → stop. |
| Growing PR scope mid-flight | Scope expands → stop and ask. |
| Branching from a red baseline | Tests green on main *before* you branch. |
| Racing an issue another session holds | `work:in-progress` already set → stop. Take over only if the user confirms it's stale. |
| Forgetting to claim before branching | Add `work:in-progress` + claim comment *before* the branch, or two sessions collide. |
| Leaving `work:in-progress` stuck after you stop | Release on every exit — merge drops it, leave-open → `work:ready-for-review`, escalate → `work:blocked`, bail → remove. |
| Ignoring `pause:agents` | It's the circuit breaker — present on the issue or repo means stop, full stop. |
| Bumping a version or cache stamp | That's `cut-release`'s job. Leave `Cargo.toml`, `package.json`, PWA `CACHE_NAME`/`?v=` alone. |
