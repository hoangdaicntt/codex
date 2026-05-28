# Feature: Account limit display refresh

## Overview
Improve the multi-account picker and CLI account selector so switching messages include the selected account email, and account rows show fresh 5h/week usage with reset times for each saved ChatGPT account.
Also change the default startup update-check behavior so `check_for_update_on_startup` defaults to `false`.

## Current Project Survey
- Account rows are rendered by `codex-rs/login/src/auth/multi_account/display.rs`.
- `codex accounts` uses `AuthManager::list_account_display_rows()` and `switch_account_by_index()` in `codex-rs/cli/src/main.rs`.
- `/accounts` uses `AccountsStore::import_active_auth_if_missing()` and `display_rows()` in `codex-rs/tui/src/chatwidget/accounts.rs`.
- `accounts.json` stores `StoredAccount.auth`, `last_used_at`, `created_at`, and `last_limit_state`.
- Current row limit values read only from `last_limit_state.snapshot`; this is why normal accounts show `-`, because available accounts clear `last_limit_state`.
- The backend usage endpoint is already wrapped by `codex-backend-client::Client::get_rate_limits_many()`, which calls `/wham/usage` for ChatGPT backend URLs and parses `RateLimitSnapshot`.
- `codex-backend-client` depends on `codex-login`, so `codex-login` must not depend on `codex-backend-client`. Limit refresh orchestration should live in callers that can depend on both, or use a new lower-level helper with careful dependency direction.
- `check_for_update_on_startup` currently defaults to `true` in `codex-rs/core/src/config/mod.rs` via `unwrap_or(true)`, and the generated config schema documents that default.

## Dependencies
- Prefer adding `codex-backend-client` as a dependency to `codex-cli` and `codex-tui`, since both already depend on `codex-login`.
- If `Cargo.toml` changes, run `just bazel-lock-update` and `just bazel-lock-check` from the repo root.
- No new external crates are needed.
- Changing `check_for_update_on_startup` touches `ConfigToml`/config schema behavior; run `just write-config-schema` from `codex-rs` after implementation.

## Architecture
- Keep `accounts.json` as the source of saved account auth data and active account state.
- Add a separate cached current limit field to `StoredAccount`, for example:
  - `last_rate_limits: Option<StoredRateLimitSnapshot>`
  - `StoredRateLimitSnapshot { recorded_at: DateTime<Utc>, snapshot: RateLimitSnapshot }`
- Keep `last_limit_state` only for selection/failover semantics, so available accounts do not look limited just because they have a current usage snapshot.
- Put the new stored-account auth and token-refresh orchestration under `codex-rs/login/src/auth/multi_account/` so future merges from upstream are easier. Prefer intentionally duplicated multi-account-specific helper code over refactoring/moving existing `manager.rs` internals.
- Expose `codex-login` helpers from the multi-account module that operate on a stored account auth snapshot without switching active account:
  - Build a usable `CodexAuth` from `StoredAccount.auth`.
  - If the stored `access_token` is expired or stale, refresh it with the stored `refresh_token` using multi-account-local code adapted from `manager.rs`.
  - Persist refreshed tokens back to that account in `accounts.json`.
  - Sync `auth.json` only when the refreshed account is also the active account.
  - Mark permanent refresh failures on that account so it is skipped by auto-selection and shown with cached/empty limits.
- In CLI/TUI callers, fetch current limits for saved accounts before rendering rows:
  - Load/import account index.
  - For each account with managed ChatGPT auth, refresh stale/expired stored tokens first, then build `CodexAuth` from the refreshed `AuthDotJson`.
  - Create `codex_backend_client::Client::from_auth(config.chatgpt_base_url.clone(), &auth)`.
  - Call `get_rate_limits_many()`.
  - Prefer the snapshot whose `limit_id == Some("codex")`; otherwise use the first snapshot.
  - Persist that snapshot to the matching `StoredAccount.last_rate_limits`.
  - Continue rendering even if one account fails; mark/record auth failures only for permanent refresh failures, not generic network errors.
- Keep backend usage fetching out of `codex-login` because `codex-backend-client` already depends on `codex-login`; instead, `multi_account` should prepare/refresh the per-account `CodexAuth`, and CLI/TUI should pass it to `codex-backend-client`.
- Update display formatting to a compact two-line row:
  - `{marker}{index}. {email}`
  - `   5h {percent_5h}/{reset_in_5h} · Week {percent_week}/{reset_in_week} · used {last_used_short} · {created_short}`
  - Example: `* 1. user@example.com` then `   5h 72%/1h12m · Week 43%/3d · used 2h · 28/05 14:20`
  - Fallbacks: `5h -`, `Week -`, and `auth failed · used 1w · 20/05 11:30` when token refresh permanently fails.
- For TUI, change `/accounts` picker away from `SelectionRowDisplay::SingleLine` so multi-line row text is rendered correctly.
- For switch messages, return/display selected account metadata:
  - CLI: `Switched active account to {index}. {email}.`
  - TUI: same message in `AccountSwitchResult.message`.

## Tasks
- [ ] Task 1: Add stored current-limit cache
  - Add optional `last_rate_limits` or equivalent to `StoredAccount`.
  - Add `AccountsStore::mark_account_rate_limits(account_id, snapshot)` for arbitrary accounts.
  - Keep `mark_active_from_snapshot()` behavior unchanged for failover decisions.
  - Add/update tests for serialization defaults and current-limit caching.

- [ ] Task 2: Add stored account auth resolution and refresh inside `multi_account`
  - Add a new module such as `codex-rs/login/src/auth/multi_account/token_refresh.rs` or `auth_resolution.rs`.
  - Add a `codex-login` helper on `AccountsStore` or a new multi-account service type to build `CodexAuth` from a stored `AuthDotJson` without switching active account.
  - Duplicate/adapt the small refresh-token request and error-classification flow from `manager.rs` instead of moving private helpers out of `manager.rs`.
  - Keep duplicated constants/request shape aligned with `manager.rs`: refresh endpoint, client id, `grant_type = "refresh_token"`, and permanent failure classification.
  - Add tests that make drift obvious if upstream refresh semantics change.
  - When refresh succeeds, update only that account's `auth` in `accounts.json`; also sync `auth.json` if and only if the account is currently active.
  - When refresh fails permanently (`refresh_token_expired`, `refresh_token_reused`, `refresh_token_invalidated`), record `last_auth_failure` for that account and skip its usage fetch.
  - When refresh fails transiently, keep the stored auth unchanged and keep showing cached limits or `-`.
  - Preserve managed ChatGPT-only behavior; return a clear error/skip for API key, external tokens, and agent identity accounts.
  - Ensure token refresh behavior is deliberate: never overwrite active `auth.json` while fetching limits for a non-active account.

- [ ] Task 3: Refresh account limits before rendering
  - Add `codex-backend-client` dependencies to `codex-cli` and `codex-tui`.
  - Implement a small refresh helper in CLI/TUI shared code if possible; otherwise keep duplicated glue minimal and scoped.
  - Fetch `/wham/usage` for each stored account using that account's stored token/account id.
  - Persist successful snapshots into `accounts.json`.
  - On failures, keep cached values and show `-` where no cache exists.

- [ ] Task 4: Update row formatting
  - Render percent values as rounded whole percentages, falling back to `-`.
  - Render reset values from `RateLimitWindow.resets_at` as compact relative durations (`1h12m`, `3d`), falling back to `-`.
  - Render last-used as compact relative time (`2h`, `3d`, `1w`) and created time as `DD/MM HH:mm`.
  - Update row text to the selected two-line layout:
    - `{marker}{index}. {email}`
    - `   5h {percent}/{reset} · Week {percent}/{reset} · used {last_used} · {created_at}`
  - Render permanent auth failure rows as:
    - `{marker}{index}. {email}`
    - `   auth failed · used {last_used} · {created_at}`
  - Update account display helper tests for 5h/week percent and reset text.

- [ ] Task 5: Update switch success messages
  - Change `switch_account_by_index()` or add a new method to return selected account display metadata.
  - Update CLI message from `Switched active account to 1.` to `Switched active account to 1. xxx@xxx.com.`
  - Update TUI `/accounts` action message the same way.
  - Add tests for both CLI parsing/message behavior where practical and TUI account picker action metadata.

- [ ] Task 6: TUI rendering and snapshots
  - Change `/accounts` picker row display to support multi-line rows.
  - Add or update TUI snapshot coverage for `/accounts` rows showing percent/reset values.
  - Verify no row text overlaps or truncates badly in the selection view.

- [ ] Task 7: Default startup update checks to disabled
  - Change `codex-rs/core/src/config/mod.rs` so missing `check_for_update_on_startup` resolves to `false`.
  - Update the config field doc/schema description to say it defaults to `false`.
  - Run `just write-config-schema` from `codex-rs` to update `core/config.schema.json`.
  - Add or update config tests covering the default value.

## Validation
- Run `just fmt` in `codex-rs`.
- Run `just fix -p codex-login`, `just fix -p codex-cli`, and `just fix -p codex-tui` if those crates are changed.
- Run `just test -p codex-login`.
- Run `just test -p codex-cli`.
- Run `just test -p codex-tui`; review and accept any intentional `insta` snapshots.
- Run the relevant config test after changing `check_for_update_on_startup`; if the change is in `codex-core`, run `just test -p codex-core` or a narrower core config test if available.
- If `Cargo.toml` dependencies change, run `just bazel-lock-update` and `just bazel-lock-check`.
- Manual checks:
  - `codex accounts` lists each saved account in the two-line format with email, 5h/week percent/reset, compact last-used time, and compact created date.
  - Selecting an index prints `Switched active account to {index}. {email}.`
  - `/accounts` shows the same two-line data and switching message.

## Open Questions And Assumptions
- Assumption: refresh limits when opening `codex accounts` or `/accounts`; no background refresh is needed.
- Assumption: if a single account limit fetch fails transiently, keep showing the picker with cached data or `-`.
- Assumption: if a stored account's refresh token is permanently invalid, mark that account with `last_auth_failure`, skip current-limit refresh for it, and keep it out of auto-switch selection.
- Assumption: reset time should be relative and compact. If exact absolute time is preferred, use local time formatting instead.
- Assumption: fetching limits for inactive accounts should not switch `auth.json`; it should use stored account auth directly.
- Assumption: new per-account auth refresh logic should live in `auth/multi_account` and may duplicate a small amount of `manager.rs` refresh logic to avoid touching upstream-heavy code.
- Decision to confirm: whether `codex accounts` should perform network calls every time, or support a later `--no-refresh` option if the command becomes slow with many accounts.
