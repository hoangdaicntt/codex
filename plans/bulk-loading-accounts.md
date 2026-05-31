# Feature: Bulk loading for accounts commands

## Overview
Add visible loading for `codex accounts` and change saved-account limit refresh from one-account-at-a-time network loading to bulk loading. Keep the patch small and local so this fork remains easy to rebase from upstream.

## Current Project Survey
- Branch created for the work: `hoangdai/bulk-loading-accounts`.
- TUI `/accounts` is implemented in `codex-rs/tui/src/chatwidget/accounts.rs`.
  - It already opens a disabled selection row with `Loading saved accounts...`.
  - It receives `AccountsPickerLoadProgress` events and updates the row to `Loading saved accounts loaded/total...`.
- CLI `codex accounts` is implemented in `codex-rs/cli/src/main.rs`.
  - It imports active auth, refreshes account limits, then prints account rows.
  - It currently has no visible loading/progress while refreshing limits.
- Both TUI and CLI call the shared `AccountsStore::refresh_account_limits_for_display` helper in `codex-rs/login/src/auth/multi_account/token_refresh.rs`.
  - The helper currently loops through accounts sequentially.
  - Each account does auth resolution, then calls the caller-provided rate-limit fetcher, then writes the result.
- `AccountsStore` is cloneable, and `tokio` is already available in `codex-login`; no new dependency should be needed.

## Dependencies
- No host installs.
- No new Rust dependencies planned.
- Validation should use GitHub Actions via the `github-hosted-test-build` workflow after implementation is committed and pushed.

## Architecture
- Keep the shared behavior in `codex-login` so CLI and TUI benefit together.
- Use `tokio::task::JoinSet` in `AccountsStore::refresh_account_limits_for_display` to run rate-limit fetches concurrently.
- Avoid concurrent `accounts.json` writes:
  - Resolve stored account auth before spawning fetch tasks.
  - Fetch rate limits concurrently.
  - Apply `mark_account_rate_limits` from the parent task as each fetch completes.
- Keep progress callback semantics as `loaded, total`.
  - Invalid/unresolvable accounts count as loaded immediately.
  - Resolved accounts count as loaded when their fetch task completes.
- Update callers to pass cloneable, `'static` fetch closures by cloning `chatgpt_base_url` before the closure.
- Add CLI loading on stderr only when stdout/stderr are terminals, so scriptable stdout output remains clean.

## Tasks
- [ ] Update `codex-rs/login/src/auth/multi_account/token_refresh.rs`.
  - Change the generic bounds for `refresh_account_limits_for_display` so the fetch closure/future can be sent to spawned tasks.
  - Resolve account auths, spawn concurrent fetches with `JoinSet`, and write completed snapshots sequentially.
  - Preserve current error handling for missing auth, permanent refresh failures, and transient failures.
- [ ] Update TUI caller in `codex-rs/tui/src/chatwidget/accounts.rs`.
  - Adjust closure capture for the new bounds.
  - Keep existing loading view and progress text behavior.
- [ ] Update CLI caller in `codex-rs/cli/src/main.rs`.
  - Add an interactive stderr progress line for `codex accounts`.
  - Keep non-interactive stdout unchanged.
  - Adjust closure capture for the new bounds.
- [ ] Add focused tests.
  - Add a `codex-login` async test proving multiple rate-limit fetches can be in flight at once.
  - Keep or update existing TUI unit coverage for loading progress text.
  - Add CLI coverage only if there is an existing low-friction pattern for command stderr/progress; otherwise rely on the shared helper test plus manual/GitHub validation.
- [ ] Format and lint after implementation.
  - Run `just fmt` in `codex-rs` after Rust edits.
  - Run scoped `just fix -p codex-login`; if CLI/TUI edits trigger lint only there, use `just fix -p codex-cli` and/or `just fix -p codex-tui`.
- [ ] GitHub-hosted validation.
  - Commit the branch.
  - Push `hoangdai/bulk-loading-accounts`.
  - Dispatch `.github/workflows/build.yml` on the branch.
  - Watch the run and inspect failed logs if needed.

## Validation
- Local commands:
  - Do not run `cargo test` directly.
  - Do not install tools on the host.
  - If local Rust tooling is usable without installs, run only repo-approved commands like `just fmt` and scoped `just fix -p ...`.
- GitHub Actions:
  - Use `git push -u origin "$(git branch --show-current)"`.
  - Use `gh workflow run build.yml --ref "$(git branch --show-current)"`.
  - Use `gh run watch <run-id>` if needed.

## Notes
- The plan intentionally avoids changing app-server APIs, generated schemas, or `codex-core`.
- The likely behavioral change is that account rows appear faster when multiple saved accounts need rate-limit refreshes.
- If many accounts have stale ChatGPT tokens, token refresh itself may still happen before bulk rate-limit fetching. Fully parallel token refresh plus safe merged writes would be a larger change and is not planned unless requested.
