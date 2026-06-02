# Contributing to Ollie

Thanks for your interest in contributing.

## Scope

Ollie targets **single-fleet self-hosting** — one fleet running its own instance.
Contributions that fit that scope are welcome. Features outside it will not be merged
into the core.

## Developer Certificate of Origin (DCO)

All commits must be signed off. Signing off certifies that you wrote the patch or
otherwise have the right to submit it under the project's license, per the
[Developer Certificate of Origin](https://developercertificate.org/).

Add a sign-off to each commit:

    git commit -s -m "your message"

This appends a trailer to the commit message:

    Signed-off-by: Your Name <your@email>

The name and email must match your commit author identity. A CI check enforces this on
every pull request — commits without a valid sign-off block the merge. If you forget,
amend the most recent commit with:

    git commit --amend -s --no-edit

## Development setup

Build, test, and architecture details live in [AGENTS.md](AGENTS.md).

Before pushing, run:

    cargo test
    cargo clippy
    cargo build

## Pull requests

- Keep each PR focused on one change.
- Use conventional-commit prefixes: `feat:`, `fix:`, `refactor:`, `test:`, `chore:`.
- A PR description should say what changed and how you verified it.
