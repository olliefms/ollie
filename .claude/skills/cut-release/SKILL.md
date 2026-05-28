---
name: cut-release
description: Use when main has accumulated enough merged work to ship — bumps version, tags, and creates the GitHub release. Trunk-based; no release branch involved.
---

# Cut Release

Ship the current state of `main` as a tagged release. Assumes work landed via `/work-issue` PRs that were each reviewed at merge time. This skill owns three things only: the version bump, the tag, and the release notes.

**Usage:** `/cut-release` (or `/cut-release patch` / `/cut-release minor` to skip the bump-decision step).

You know how to run git, `gh release`, and the project's build. What follows is only the project-specific judgment: how to pick the bump, the version-coupled stamps unique to this repo, and the tag invariant that bites if you get it wrong.

## Self-configure first

Read the project guide (`AGENTS.md` / `CLAUDE.md` / `GEMINI.md`) for version increment rules, version file locations, and the test command.

## Preconditions

`main`, pulled, tests green. If `git log <last-tag>..HEAD` is empty there's nothing to ship — stop. If tests are red, stop and fix on main with a `/work-issue` flow first; you never tag a red tree.

## Decide the bump

If the user passed `patch` / `minor`, use it. Otherwise read the commits since the last tag and **propose** the bump with that commit list as your evidence, then wait for confirmation:

- **patch (x.y.Z)** — bug fixes only. No new API surface, no new features.
- **minor (x.Y.0)** — any new endpoint, feature, or user-visible capability.
- **major (X.0.0)** — propose only; require explicit user confirmation.

## Optional cross-PR review

Only worth it when the diff since the last tag spans multiple subsystems or several interacting `/work-issue` PRs — that's the one thing per-PR review can't have caught. Skip it for a one-PR release; those were already reviewed at merge. When you do run it, spawn Opus:

> Review the cumulative diff `git diff <last-tag>..HEAD` on main. Focus on *interactions between separate PRs* — semantic conflicts, redundant work, contract drift that per-PR review wouldn't catch. Apply the Shippability Bar and Triage Rules from `<project guide>`. Report blockers and significants only.

Triage as in `/work-issue`. Hard cap of 2 iterations; blockers still standing at iteration 2 → stop and escalate. Nits from this pass get noted or discarded, never filed.

## Bump the version — including this repo's hidden stamps

Update the manifest version (`Cargo.toml` `[package] version`, `package.json` `"version"`, or `pyproject.toml`), then refresh the lockfile so it matches (`cargo build`, `npm install`, etc.).

**Then update the version-coupled asset stamps**, which live outside the manifest and must match the release version. Issue and sprint PRs are explicitly told not to touch these, so setting them is *your* job. In this repo that's the driver PWA cache stamp — `CACHE_NAME = 'ollie-vX.Y.Z'` in `static/driver/sw.js` and the `?v=X.Y.Z` query stamps in `static/driver/*.html`. Grep the previous version string to find every occurrence; a missed stamp ships a stale service worker.

Commit the bump (`chore: bump to vX.Y.Z`) and push to main.

## Tag — the invariant that bites

**The tag must point at the bump commit.** Bump first, then tag — never the reverse. Push the tag as `refs/tags/vX.Y.Z`; without the `refs/tags/` prefix git can push a same-named branch instead. Verify `vX.Y.Z` and `main` resolve to the same commit before moving on.

## Release notes

Generate from `git log <last-tag>..HEAD`, grouped by type, referencing the issue numbers from PR titles. Create the GitHub release targeting `main`. Omit empty sections. Then report: version shipped, issues closed (by PR reference), and the release URL.

## Common mistakes

| Mistake | Fix |
|---|---|
| Tagging before bumping | Tag points at the bump commit — bump first, tag second. |
| Pushing the tag as `vX.Y.Z` | Use `refs/tags/vX.Y.Z`, or git may push a branch. |
| Forgetting the PWA cache stamps | Grep the old version string; `sw.js` + `?v=` stamps must match the manifest. |
| Cross-PR Opus on a one-PR release | Skip it — already reviewed at merge. |
| Looping Opus past 2 iterations | Escalate, same rule as `/work-issue`. |
| Tagging a red tree | Hard stop. Fix on main first. |
| Filing cross-PR nits as issues | Note or discard. No robot homework. |
