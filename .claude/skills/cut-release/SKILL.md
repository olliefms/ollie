---
name: cut-release
description: Use when main has accumulated enough merged work to ship — bumps version, tags, and creates the GitHub release. Trunk-based; no release branch involved.
---

# Cut Release

## Overview

Ship the current state of `main` as a tagged release. Assumes work has landed on main via `/work-issue` PRs and each PR was already reviewed at merge time. This skill handles version bump, tag, and release notes.

**Usage:** `/cut-release` (optionally `/cut-release patch` or `/cut-release minor` to skip the bump-decision step)

## Self-Configure First

Read the project guide (`AGENTS.md` / `CLAUDE.md` / `GEMINI.md`) for:
- **Version increment rules** — patch vs minor criteria
- **Version file locations** — `Cargo.toml`, `package.json`, `pyproject.toml`, etc.
- **Running Tests** — exact command

## Steps

### 1. Verify state

```bash
git checkout main && git pull
gh release list --limit 1     # get last tag
git log --oneline <last-tag>..HEAD    # what's shipping
```

If `git log <last-tag>..HEAD` is empty, there's nothing to release — stop.

Run the test command. **Must be green.** If red, stop and tell the user.

### 2. Decide version bump

If the user passed `patch` or `minor` as an argument, use that. Otherwise read the commits since the last tag and propose:

- **patch (x.y.Z)** — bug fixes only, no new API surface, no new features
- **minor (x.Y.0)** — any new endpoint, feature, or user-visible capability
- **major (X.0.0)** — propose only; require explicit user confirmation

Present the bump decision with the commit list as evidence. Wait for user confirmation before proceeding unless they passed the argument.

### 3. Optional cross-PR review

If the diff since last tag touches multiple subsystems or includes several `/work-issue` PRs that interact, offer to run Opus against `<last-tag>..HEAD` to catch cross-PR semantic conflicts. Skip if the release is one small PR.

Spawn Opus with this brief:

> Review the cumulative diff `git diff <last-tag>..HEAD` on main. Focus on *interactions between separate PRs* — semantic conflicts, redundant work, or contract drift that per-PR review wouldn't catch. Apply the Shippability Bar and Triage Rules from `<project guide>`. Report blockers and significants only.

Triage as in `/work-issue`. Hard cap: 2 iterations. Blockers at iteration 2 → stop and escalate.

### 4. Bump version files

Update version in the project's manifest file(s). Common locations:

| File present | Field |
|---|---|
| `Cargo.toml` | `[package] version` |
| `package.json` | `"version"` |
| `pyproject.toml` | `[project] version` or `[tool.poetry] version` |

Run the lockfile update if applicable (`cargo build`, `npm install`, etc.) so the lockfile matches.

Commit:

```bash
git add <version files>
git commit -m "chore: bump to vX.Y.Z"
git push origin main
```

### 5. Tag

**Tag points to the bump commit on main.**

```bash
git tag vX.Y.Z
git push origin refs/tags/vX.Y.Z
```

The `refs/tags/` prefix avoids any ambiguity with branch names.

Verify:
```bash
git log --oneline -1 vX.Y.Z
git log --oneline -1 main
# must match
```

### 6. GitHub release

Generate notes from `<last-tag>..HEAD`:

```bash
git log <last-tag>..HEAD --oneline
```

Group commits by type. Reference issue numbers from PR titles.

```bash
gh release create vX.Y.Z --title "vX.Y.Z — <headline>" --target main --notes "$(cat <<'EOF'
## Bug Fixes
- [description] (#N)

## Enhancements
- [description] (#N)

## Infrastructure
- [description] (#N)
EOF
)"
```

Omit empty sections.

### 7. Done

Output a brief summary:
- Version shipped
- Issues closed (by PR reference)
- Release URL

---

## Common Mistakes

| Mistake | Fix |
|---|---|
| Tagging before bumping version | The tag should point to the bump commit — bump first, tag second |
| `git push origin vX.Y.Z` for the tag | Use `refs/tags/vX.Y.Z` — without the prefix, git may push a branch instead |
| Running cross-PR Opus review on a one-PR release | Skip it. Single PRs were already reviewed at merge. |
| Looping Opus past 2 iterations | Stop and escalate to the user, same rule as `/work-issue` |
| Releasing with red tests | Hard stop. Fix on main with a `/work-issue` flow before cutting. |
| Filing nits from cross-PR review as issues | Nits get noted in release prep or discarded. No robot homework. |
