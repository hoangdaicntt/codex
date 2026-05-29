---
name: github-hosted-test-build
description: Use when developing Codex features on a machine without Rust installed and the user wants GitHub-hosted runners to run tests or builds. Covers pushing feature branches, manually dispatching the Test and Build workflow for test-only validation, watching GitHub Actions with gh, creating PRs to main, and relying on merge-to-main to run test then Linux/macOS/Windows builds.
---

# GitHub Hosted Test Build

## Overview

Use GitHub Actions as the source of truth for Rust testing and release builds when the local machine cannot run Rust. Prefer this workflow over local `cargo`/`just` commands unless the user explicitly asks to run local tests.

The repository workflow is `.github/workflows/build.yml`:

- `workflow_dispatch` on any branch runs the `test` job.
- `pull_request` into `main` runs the `test` job.
- `push` to `main` runs `test`, then builds Linux, macOS, and Windows artifacts only if tests pass.

## Feature Branch Test Flow

For a feature branch that should be tested without building all OS artifacts:

1. Ensure the branch has a commit.
2. Push the branch to `origin`.
3. Dispatch the workflow on that branch.
4. Watch the run and report the result.

Commands:

```bash
git status --short --branch
git push -u origin "$(git branch --show-current)"
gh workflow run build.yml --ref "$(git branch --show-current)"
gh run list --branch "$(git branch --show-current)" --limit 5
gh run watch
```

If `gh run watch` cannot identify the intended run, get the run id from `gh run list` and watch it explicitly:

```bash
gh run watch <run-id>
```

## PR And Merge Flow

For code that is ready for review:

```bash
git push -u origin "$(git branch --show-current)"
gh pr create --base main --head "$(git branch --show-current)"
```

On the PR, expect tests only. After the PR is merged into `main`, expect the workflow to run tests again and then build all three OS artifacts.

## Build Artifacts

Build artifacts are produced only by a `push` to `main`, usually from merging a PR. The expected artifact names are:

- `codex-linux`
- `codex-macos`
- `codex-windows`

Use:

```bash
gh run list --branch main --limit 5
gh run view <run-id>
```

Download artifacts when requested:

```bash
gh run download <run-id>
```

## Operating Rules

- Do not install Rust locally just to test a feature when the user wants GitHub-hosted validation.
- Do not run `cargo test` directly for this repository.
- Do not dispatch workflow runs for dirty, uncommitted code; GitHub can only test pushed commits.
- Preserve unrelated untracked files such as local build outputs.
- If a GitHub-hosted test fails, inspect logs with `gh run view <run-id> --log-failed`, fix the branch, commit, push, and dispatch again.
