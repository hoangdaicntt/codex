# Feature: Multi-account auth simplification and upstream cleanup

## Overview

Clean up the fork's multi-account implementation by comparing it against `upstream/main` and keeping only the minimum code needed for safe manual account switching. The target design is:

- `auth.json` remains the active runtime credential.
- `accounts.json` is a passive list of saved `AuthDotJson` snapshots plus an active account id.
- `AuthManager` is the only place that refreshes managed ChatGPT OAuth tokens.
- `/accounts` and `codex accounts` list saved accounts and fetch account limits automatically using the currently stored access tokens only. They never refresh inactive account tokens.
- Switching accounts flushes the current cached active auth before replacing it, so a token refreshed in memory cannot be lost.
- Automatic failover is kept, but only after a real exhausted-limit error or auth/refresh failure. Proactive failover when remaining limit drops below a threshold is removed.
- Code that was only needed for inactive rate-limit refresh, stored rate-limit state, stored auth-failure state, or proactive near-limit failover is removed.

This plan is for confirmation before implementation.

## Current Project Survey

The current branch is `sync/upstream-main-20260604`, tracking `origin/sync/upstream-main-20260604`. After fetching, `upstream/main` is at `6a6a5f925e` and the fork branch is at `ac20c02ad7`.

`git diff --shortstat upstream/main...HEAD` reports:

- `75 files changed`
- `5165 insertions`
- `6237 deletions`

Major fork-only areas compared with upstream:

- GitHub workflow replacement and fork build workflow:
  - Added `.github/workflows/build.yml`
  - Added `.github/workflows/release-artifacts.yml`
  - Deleted many upstream workflow files
  - Added `.codex/skills/github-hosted-test-build`
- Multi-account auth and UI:
  - Added `codex-rs/login/src/auth/multi_account/*`
  - Modified `codex-rs/login/src/auth/manager.rs`
  - Modified `codex-rs/login/src/server.rs`
  - Added `codex-rs/tui/src/chatwidget/accounts.rs`
  - Modified TUI app events, slash commands, interaction handling, and bottom-pane list selection
  - Modified CLI for account listing/switching
- Automatic failover and rate-limit state:
  - Modified `codex-rs/core/src/session/turn.rs`
  - Added tests in `codex-rs/core/tests/suite/client.rs`
- Auth-change plumbing:
  - Modified `codex-rs/core/src/client.rs`
  - Modified app-server client/session to expose in-process `AuthManager`
- Other fork differences outside this auth cleanup:
  - Update-check default changed from true to false
  - stream idle timeout changed from 300s to 60s
  - Python SDK workflow simplification
  - Docker build script and fork merge guide

## Upstream Comparison Findings

### Upstream does not have multi-account modules

`upstream/main` contains:

- `codex-rs/login/src/auth/manager.rs`
- `codex-rs/login/src/auth/mod.rs`
- `codex-rs/login/src/auth/storage.rs`

It does not contain:

- `codex-rs/login/src/auth/multi_account/display.rs`
- `metadata.rs`
- `selection.rs`
- `store.rs`
- `token_refresh.rs`
- `tests.rs`

Implication: every multi-account module is fork-owned. We should keep only the pieces needed for manual account storage/switching and remove behavior that no longer matches the simplified design.

### Current fork code that is risky or likely unnecessary

- `multi_account/token_refresh.rs`
  - Refreshes stale stored accounts directly from `accounts.json`.
  - Can use a rotated refresh token outside `AuthManager`.
  - Marks permanent refresh failures while loading `/accounts`.
  - Should be removed or reduced to non-refresh helpers.

- `multi_account/selection.rs`
  - Drives automatic selection based on near-limit, exhausted, or auth-failure states.
  - Depends on metadata that the simplified design does not need.
  - Should be removed or replaced with a much smaller next-account helper that does not inspect persisted limit/failure state.

- `StoredLimitState`, `StoredRateLimitSnapshot`, `StoredAuthFailureState`
  - Make `accounts.json` more than a passive auth snapshot list.
  - Create additional load-save mutation paths.
  - Should be removed from the auth store for the simplified design.

- `core/src/session/turn.rs` fork additions
  - Switch accounts automatically on rate-limit or refresh failure.
  - Relies on stored limit/failure state.
  - Should keep retry-after-exhausted-limit and retry-after-refresh-failure, but remove proactive before-request switching based on stored near-limit state.

- `/accounts` rate-limit refresh in TUI/CLI
  - Fetches account limits for all saved accounts and may refresh stored tokens.
  - Should be kept for display, but changed to use existing access tokens only. It must not refresh OAuth tokens, write refreshed auth, or mark auth failed.
  - If a saved access token is expired or unusable, skip that account's limit fetch and still render the account row.

### Current fork code likely worth keeping

- `AuthManager` persisting refreshed active auth into `accounts.json`.
  - Needed so saved account snapshots stay current after active refresh.
  - Must be adjusted to the minimal schema.

- `AuthManager` auth-change watch and model-client websocket reset.
  - Useful after manual account switch to prevent stale websocket/session auth.
  - Keep if tests show it remains necessary.

- In-process app-server exposing the same `AuthManager` to TUI.
  - Useful so `/accounts` switch uses the runtime manager instead of a second manager.
  - Keep unless replaced with a cleaner app-server RPC.

- TUI `/accounts` picker UI and CLI account command.
  - Keep only for list, switch, remove, and display.
  - Keep automatic all-account rate-limit display, but remove OAuth refresh and auth-failure display behaviors.

- Fork GitHub hosted test workflow and skill.
  - Keep because validation will use `$github-hosted-test-build`.

## Dependencies

- No new dependencies.
- Do not install local Rust or extra tooling.
- Do not run `cargo test` directly.
- Prefer GitHub-hosted validation from `.github/workflows/build.yml` after implementation.

## Target Architecture

### Minimal stored schema

Use a compact `accounts.json` shape:

```json
{
  "version": 1,
  "activeAccountId": "account-id",
  "accounts": [
    {
      "auth": {
        "auth_mode": "chatgpt",
        "tokens": {
          "id_token": "...",
          "access_token": "...",
          "refresh_token": "...",
          "account_id": "account-id"
        },
        "last_refresh": "..."
      }
    }
  ]
}
```

Implementation detail can still use an internal `StoredAccount` with a cached `account_id()` accessor derived from `auth`; however, the persisted file should not need duplicated email, plan, workspace, limit, or failure fields.

Backwards compatibility:

- Existing `accounts.json` files with `accountId`, `email`, `planType`, `lastLimitState`, `lastRateLimits`, or `lastAuthFailure` must still deserialize.
- On save, write the simplified shape.
- If an old account lacks a usable account id in `auth.tokens.account_id` or token claims, skip it or surface a clear load error in `/accounts`.

### Active refresh flow

Only `AuthManager` refreshes tokens:

1. Acquire the existing `refresh_lock`.
2. Perform guarded reload.
3. Call the OAuth refresh endpoint only if storage is unchanged.
4. Persist refreshed auth to the active store.
5. Upsert the refreshed active auth into `accounts.json`.
6. Reload cached auth.
7. Notify auth-change watchers.

### Safe switch flow

`AuthManager::switch_account()` should become the single runtime switch API:

1. Acquire the same lock used for refresh.
2. Flush the current cached active managed ChatGPT auth into `accounts.json`.
3. Load the target saved auth from `accounts.json`.
4. Set `activeAccountId` in `accounts.json`.
5. Write the target auth to active storage / `auth.json`.
6. Reload `AuthManager`.
7. Verify cached account id equals the target id.
8. Return success only after the cache is updated.

This directly addresses the case where cache has a newer token than disk at switch time.

### `/accounts` flow

`/accounts` should:

1. Load saved account auth snapshots.
2. Derive display metadata from token claims.
3. Show active marker.
4. Support switch and remove.
5. Never refresh OAuth tokens.
6. Never mark refresh failures.
7. Automatically fetch all-account rate limits using current saved access tokens only.
8. Skip limit fetch for accounts whose saved access token is expired, missing, or otherwise unusable.
9. Store only display-cache limit data if needed; never write auth token changes from this path.

### Automatic failover flow

Automatic failover is kept, but only for real failures observed during a request:

1. If the active account receives `UsageLimitReached` / exhausted-limit response, switch to the next saved account.
2. If active managed auth fails to refresh permanently during a request, switch to the next saved account.
3. The switch must call the same safe `AuthManager::switch_account()` path as manual `/accounts`.
4. Next account selection is circular by saved array order. For `[a, b, c, d]`, if `c` is active then next is `d`; if `d` is active then next is `a`.
5. Retry is bounded to at most `accounts.len() - 1` switches for a single user turn, so Codex can try every other saved account once but cannot loop forever.
6. Do not switch merely because a rate-limit snapshot says the account has less than a threshold remaining.
7. Do not store limit/failure state in `accounts.json` to drive a future switch.

## Tasks

- [ ] Task 1: Rebase or merge the implementation branch onto latest `upstream/main`.
  - Start from current fork branch or a new feature branch.
  - Bring in latest upstream changes before editing auth code.
  - Keep the current untracked plan file separate from implementation commits unless the user wants it committed.
  - Re-check diff against `upstream/main` after the update.

- [ ] Task 2: Define the simplified account store schema.
  - Replace stored account metadata fields with a minimal saved-auth entry.
  - Add derived helpers for account id, email, plan, workspace, and label.
  - Preserve backwards-compatible deserialization for the current fork schema.
  - Ensure saving writes only the simplified auth snapshot form.
  - Keep `accounts.json` permissions at `0600`.

- [ ] Task 3: Remove inactive account OAuth refresh code.
  - Delete or empty `multi_account/token_refresh.rs`.
  - Remove `refresh_stored_chatgpt_auth()`, `request_stored_chatgpt_token_refresh()`, and permanent failure marking from `/accounts`.
  - Keep or replace `refresh_account_limits_for_display()` as a limit-display helper that never calls the OAuth refresh endpoint.
  - Add access-token usability checks before fetching limits; expired or missing access tokens should skip fetch.
  - If the file becomes unused, remove the module export from `multi_account/mod.rs`.

- [ ] Task 4: Remove persisted limit/failure state from auth storage.
  - Remove `StoredLimitState`, `StoredRateLimitSnapshot`, and `StoredAuthFailureState` from the persisted auth store.
  - Remove `mark_account_rate_limits`, `mark_active_from_snapshot`, `mark_active_exhausted`, `mark_active_exhausted_from_snapshot`, and `mark_account_refresh_failed` from the auth store.
  - If `/accounts` needs limit values after async fetch, keep them as in-memory picker/CLI display data or a clearly non-auth cache that does not affect switching.
  - Remove display styling that depends on persisted auth-failure state.
  - Keep display focused on index, active marker, email/account id, and plan.

- [ ] Task 5: Simplify automatic failover to next-account-on-real-failure.
  - Remove proactive before-request switching based on stored near-limit state.
  - Remove `switch_if_active_account_limited()` and `switch_if_active_account_auth_failed()` if they depend on persisted state.
  - Add `AuthManager::switch_to_next_saved_account()` or equivalent helper that chooses the next account by circular array order only.
  - In `core/src/session/turn.rs`, keep retry after `UsageLimitReached` and `RefreshTokenFailed`, but call the simple next-account helper.
  - Track attempted/switch count for the current turn and cap switching at `accounts.len() - 1`.
  - Rewrite tests to assert switching only after exhausted-limit or auth-refresh failure, not when remaining limit is merely low.

- [ ] Task 6: Implement safe AuthManager-owned manual switching.
  - Add a private helper to flush cached active managed ChatGPT auth to `accounts.json`.
  - Update `AuthManager::switch_account()` to acquire the refresh lock, flush cache, switch active auth, reload, and verify.
  - Keep `switch_account_by_index()` as a thin wrapper.
  - Avoid letting `AccountsStore` write `auth.json` independently for runtime switch unless called inside `AuthManager` under lock.

- [ ] Task 7: Keep active refresh synchronized with saved accounts.
  - Update `refresh_and_persist_chatgpt_token()` for the simplified schema.
  - Ensure refreshed auth is upserted into the saved account entry before cache reload.
  - Ensure `persist_tokens_async()` on login still upserts the login auth into `accounts.json`.
  - Keep same-account revoke safety from the fork if still needed.

- [ ] Task 8: Simplify TUI `/accounts`.
  - Keep the slash command and picker.
  - Keep automatic saved-account rate-limit fetches, but ensure they use only saved access tokens and never refresh OAuth tokens.
  - Keep progress events if they still improve loading feedback.
  - Keep switch/remove events.
  - Ensure switch uses the in-process app-server `AuthManager` when available.
  - Verify remote app-server behavior: either disable switch for remote mode with a clear error or add an explicit app-server switch RPC in a later plan.

- [ ] Task 9: Simplify CLI accounts command.
  - Keep list and switch if still desired.
  - Keep automatic all-account rate-limit fetches, but ensure they use only saved access tokens and never refresh OAuth tokens.
  - Display derived metadata only.
  - Ensure switching goes through `AuthManager::switch_account_by_index()`.

- [ ] Task 10: Clean imports, modules, dependencies, and tests.
  - Remove unused `base64` dependency additions if only used by deleted tests.
  - Remove `multi_account/selection.rs` if unused.
  - Remove old plan files from the implementation branch if they are no longer useful, or keep only the final plan by user preference.
  - Run `rg` for deleted APIs and ensure no dead code remains.
  - Keep unrelated fork-specific workflow/config differences untouched unless the user explicitly asks to align them with upstream.

- [ ] Task 11: Add focused tests.
  - Login upserts a saved auth snapshot.
  - Active refresh updates both active auth storage and saved account snapshot.
  - `/accounts` listing fetches limits with current access tokens but does not call token refresh for expired access tokens.
  - Expired/missing saved access tokens skip limit fetch without marking auth failed.
  - Switching flushes newer cached active auth before replacing it.
  - Switching reloads and verifies target account id.
  - Automatic failover switches to the next saved account only after exhausted-limit or refresh-token failure.
  - Automatic failover wraps around from the last account to the first account.
  - A single turn performs at most `accounts.len() - 1` automatic switches.
  - A near-limit snapshot below the old threshold does not trigger automatic failover.
  - Removing active account chooses a next saved auth or logs out.
  - Backwards-compatible load migrates old full-metadata `accounts.json` to simplified save output.

- [ ] Task 12: Validate with GitHub-hosted tests.
  - Ensure all changes are committed on a feature branch.
  - Push the branch to `origin`.
  - Dispatch `.github/workflows/build.yml` on that branch.
  - Watch the run and inspect failures with `gh run view <run-id> --log-failed`.
  - Fix, commit, push, and re-dispatch until the workflow passes.

Commands from `$github-hosted-test-build`:

```bash
git status --short --branch
git push -u origin "$(git branch --show-current)"
gh workflow run build.yml --ref "$(git branch --show-current)"
gh run list --branch "$(git branch --show-current)" --limit 5
gh run watch <run-id>
```

## Validation

Before GitHub-hosted validation:

- Run `git diff --name-status upstream/main...HEAD` to confirm the cleanup reduced fork-only auth code.
- Run `rg` for removed symbols:
  - `refresh_account_limits_for_display`
  - `refresh_stored_chatgpt_auth`
  - `StoredLimitState`
  - `StoredAuthFailureState`
  - persisted near-limit switching helpers
- Run `just fmt` in `codex-rs` if local Rust tooling is available; do not install tooling just for this.
- Do not run `cargo test` directly.

GitHub-hosted source of truth:

- Dispatch `build.yml` via `gh workflow run`.
- The current fork workflow runs `just test -p codex-cli`.
- If auth changes need broader coverage, update the workflow or add a temporary workflow input/job in the implementation branch only after confirmation.

## Cleanup Rules

- Any code whose only purpose is inactive-account token refresh should be removed.
- Any code whose only purpose is storing rate-limit or auth-failure state inside `accounts.json` should be removed.
- Automatic `/accounts` and `codex accounts` limit fetch is allowed only when it uses the saved access token as-is.
- Any code required only by proactive near-limit failover should be removed.
- Simple next-account failover after exhausted-limit or auth-refresh failure should be kept.
- Fork workflow files and the GitHub-hosted test skill should remain because they are needed for validation.
- Unrelated fork settings, such as update-check defaults or stream timeout, should not be changed in this auth cleanup.

## Open Decisions For Confirmation

- Keep manual `/accounts` list/switch/remove: confirmed.
- Keep automatic failover only for exhausted-limit or auth/refresh failure; remove proactive `<10%` near-limit switching: pending confirmation.
- Keep automatic `/accounts` and `codex accounts` limit display using current saved access tokens only, with no refresh-token use: pending confirmation.
- Use circular next-account switching with max automatic switches per turn equal to `accounts.len() - 1`: pending confirmation.
- Keep `accounts.json` as simplified saved-auth snapshots with backwards-compatible migration: confirmed.
- Use `$github-hosted-test-build` for validation after implementation, while leaving workflow files as they are: confirmed.
- Do not touch unrelated fork-vs-upstream differences in workflows/config outside the auth cleanup: confirmed.

Implementation should not start until these defaults are confirmed.
