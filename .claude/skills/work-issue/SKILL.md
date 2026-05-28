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

## The flow

1. **Read the issue *and its comments*.** Bodies go stale; the live scope lives in the comments. If scope is still ambiguous after reading both, ask before branching — a clarifying question is far cheaper than rework.

2. **Start from green.** Run the test command on `main` before you touch anything. If it's red before you've done a thing, stop and tell the user — never build on a broken baseline, you'll never know which red is yours.

3. **Implement on a branch, staying inside the issue's scope.** If the work starts pulling in things the issue didn't ask for, stop and ask rather than quietly growing the PR — scope creep is invisible to the reviewer until it's too late to undo.

4. **Self-review against the Shippability Bar**, then run one capped Opus pass (see below).

5. **Self-merge only if it's clean** (see gate below), then close the issue with a one-line verification comment noting what you actually tested.

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

PR title: `<type>(<area>): <headline> (#<N>)`, body closes the issue. If any condition fails, leave the PR open and tell the user — never self-merge with a deferred significant.

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
| Bumping a version or cache stamp | That's `cut-release`'s job. Leave `Cargo.toml`, `package.json`, PWA `CACHE_NAME`/`?v=` alone. |
