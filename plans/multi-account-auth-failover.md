# Feature: Multi-Account Auth Failover

## Overview

Add a file-backed multi-account ChatGPT auth store in `$CODEX_HOME/accounts.json` while keeping `$CODEX_HOME/auth.json` as the active-account compatibility file. Codex should automatically select another available account for the next request when the current account is near or at its usage limit, without dropping the current conversation/session.

## Current Project Survey

- `codex-rs/login/src/auth/storage.rs` owns the current `AuthDotJson` schema and file/keyring/auto/ephemeral storage backends. Today it models one active credential.
- `codex-rs/login/src/auth/manager.rs` owns `CodexAuth`, `AuthManager`, load/reload, proactive refresh, 401 recovery, logout, and external auth integration.
- `codex-rs/login/src/server.rs` and `codex-rs/login/src/device_code_auth.rs` persist ChatGPT browser/device-code logins through `persist_tokens_async(...)`.
- `codex-rs/cli/src/login.rs` calls the public login/logout helpers directly.
- `codex-rs/codex-api/src/rate_limits.rs` already parses `RateLimitSnapshot` from response headers and websocket `codex.rate_limits` events.
- `codex-rs/codex-api/src/api_bridge.rs` maps HTTP `429` plus `usage_limit_reached` into `CodexErr::UsageLimitReached`, including parsed rate-limit metadata.
- `codex-rs/core/src/session/turn.rs` currently records rate-limit snapshots and returns `UsageLimitReached` immediately; it does not switch account or retry.
- `codex-rs/core/src/client.rs` resolves auth for each request loop through the provider/auth manager, so an `AuthManager` switch can affect subsequent requests without recreating the session.
- `codex-rs/app-server/src/request_processors/account_processor.rs` already owns account login/logout/read/rate-limit RPC handling and notifications.
- `codex-rs/tui/src/status/*`, `codex-rs/tui/src/chatwidget.rs`, and slash-command/menu code already display account/rate-limit status and provide popup/menu patterns.

## Assumptions

- Multi-account support applies only to managed ChatGPT OAuth accounts in the first implementation. API-key, external `chatgptAuthTokens`, and agent identity auth remain single-active credentials.
- `$CODEX_HOME/auth.json` remains the source of truth for legacy active-auth readers, but the new store is authoritative for multi-account operations.
- "Limit < 10%" means any relevant non-unlimited window has `used_percent >= 90.0`, or credits indicate no remaining credits.
- Account switching must not occur mid-stream. It applies before the next request. A turn that already hit `UsageLimitReached` still reports the limit error instead of retrying immediately with another account.
- Forced login/workspace restrictions must be enforced before selecting or switching to an account.
- The multi-account store is file-backed even when the active auth mode uses keyring. This keeps the first implementation simple and easy to inspect.
- Logout behavior stays as close as possible to current behavior; this feature does not prioritize new logout UX because the intended workflow is to keep accounts logged in.

## Architecture

Create a focused module under `codex-rs/login/src/auth/multi_account/` so most future rebases touch only small hook points.

Suggested files:

- `mod.rs`: public facade used by `manager.rs`, login persistence, logout, and CLI hooks.
- `store.rs`: read/write the single-file account store.
- `selection.rs`: active account, next-account selection, limit state, forced-workspace filtering.
- `metadata.rs`: account metadata types derived from `AuthDotJson` and `CodexAuth`.
- `tests.rs`: unit tests for storage layout, selection, logout, migration, and limit policy.

Data layout:

```text
$CODEX_HOME/
  auth.json                         # active account compatibility file
  accounts.json                     # file-backed multi-account store
```

Initial store shape:

```json
{
  "version": 1,
  "activeAccountId": "account-123",
  "accounts": [
    {
      "accountId": "account-123",
      "email": "user@example.com",
      "planType": "plus",
      "lastUsedAt": "2026-05-27T00:00:00Z",
      "lastLimitState": null,
      "auth": {
        "auth_mode": "chatgpt",
        "OPENAI_API_KEY": null,
        "tokens": {
          "id_token": "<jwt>",
          "access_token": "<jwt>",
          "refresh_token": "<token>",
          "account_id": "account-123"
        },
        "last_refresh": "2026-05-27T00:00:00Z"
      }
    }
  ]
}
```

Core flow:

1. Login upserts the new account record into `accounts.json`, then writes the selected account to `auth.json`.
2. `AuthManager` loads the active account through the existing `auth.json` path for compatibility, but exposes multi-account operations through the new facade.
3. Rate-limit snapshots update the current account's limit metadata.
4. Before a request, or after recording a usage-limit error for the current account, `AuthManager` can switch to the next eligible account, rewrite `auth.json`, reload cache, and notify subscribers.
5. Existing sessions continue because the conversation state is separate from auth; subsequent model requests resolve auth again through `current_client_setup()`.

## Tasks

- [x] Task 1: Add multi-account storage module.
  - Create `codex-rs/login/src/auth/multi_account/`.
  - Define `AccountId`, `StoredAccount`, `AccountsIndex`, `StoredLimitState`, and store facade.
  - Store all account metadata and `AuthDotJson` payloads in a single `$CODEX_HOME/accounts.json` array.
  - Implement atomic-ish writes using temp file plus rename where local patterns allow.
  - Add migration helper that imports existing `auth.json` as the first account when it is managed ChatGPT auth.

- [x] Task 2: Integrate login persistence.
  - Update `persist_tokens_async(...)` in `server.rs` to save ChatGPT credentials into the multi-account store.
  - Keep writing active account to `auth.json`.
  - Update device-code login automatically through the shared persist path.
  - Update `login_with_api_key` and `login_with_access_token` only as needed to preserve single-active behavior and avoid accidentally adding unsupported modes to the account pool.
  - Revisit revoke behavior: do not revoke the previous account merely because a different account logged in.

- [x] Task 3: Extend `AuthManager` with account pool operations.
  - Add methods such as `active_account_id()`, `list_accounts()`, `switch_account(account_id)`, `switch_to_next_available_account(reason)`, `mark_active_account_limited(snapshot)`, and `sync_active_auth_json()`.
  - Reuse existing `reload()` after switch so existing `auth_change_receiver` notifications work.
  - Keep guarded account-id behavior for refresh/reload so cross-account switches are explicit.
  - Enforce `forced_chatgpt_workspace_id` in selection.

- [x] Task 4: Add limit policy.
  - Add a small policy type in the multi-account module that classifies `RateLimitSnapshot`.
  - Treat `used_percent >= 90.0` as near limit.
  - Treat `UsageLimitReached`, `has_credits == false`, and reached-type depletion/spend-cap variants as exhausted until reset.
  - Store `resets_at` so accounts can become eligible again after reset.
  - Prefer accounts in stable order after the active account; skip restricted, exhausted, invalid, or refresh-failed accounts.

- [x] Task 5: Hook automatic switch into turn execution.
  - In `ResponseEvent::RateLimits` handling, record the snapshot and mark the active account near-limited if applicable.
  - Before building a sampling request, ask `AuthManager` whether the active account should switch based on stored limit state.
  - On `CodexErr::UsageLimitReached`, mark the account exhausted and switch to the next eligible account for the following request, then return the original limit error for the current turn.
  - Do not retry the same prompt automatically after switching; this keeps the current turn behavior predictable.
  - Emit a user-visible event/log when auto-switch occurs so the next request's account change is understandable.

- [x] Task 6: Cover non-sampling request paths.
  - Review compact, memories, realtime, models, connectors, and app-server account rate-limit reads.
  - For the first pass, at minimum ensure they pick up the active account after `AuthManager` switch.
  - Decide whether `UsageLimitReached` on compact/realtime should switch and retry or surface normally.

- [x] Task 7: Keep app-server account behavior compatible.
  - Do not add app-server account list/switch/sync RPCs in the first implementation.
  - Ensure existing `account/read`, `account/logout`, and `account/rateLimits/read` continue to reflect the active account from `auth.json`/`AuthManager`.
  - Reuse existing `account/updated` notification only if active-account switches need to notify app-server clients.
  - Avoid schema changes unless implementation reveals an unavoidable compatibility gap.

- [x] Task 8: Add minimal manual account controls.
  - Add a new CLI namespace: `codex account ...`.
  - Implement `codex account list`, `codex account switch <account-id>`, and optionally `codex account sync`.
  - Show active account, email, plan, account id/workspace, and latest limit status.
  - Provide actions: switch and sync/reload. Logout account management is not required for the first pass.
  - Add CLI tests.
  - Defer TUI account controls to a follow-up after CLI support lands.

- [x] Task 9: Keep logout compatible.
  - Preserve current logout behavior as much as possible.
  - If logout is touched, ensure it does not corrupt `accounts.json`.
  - Do not make logout-account management a core requirement for this feature.

- [x] Task 10: Tests and validation.
  - Unit-test single-file `accounts.json` layout, migration, selection order, limit expiry, and forced-workspace filtering.
  - Update login tests that currently assert overwrite semantics.
  - Add core/client or session tests for 429 `UsageLimitReached` marking the active account exhausted and switching for the next request without retrying the current turn.
  - Add rate-limit near-threshold tests from `ResponseEvent::RateLimits`.
  - Add CLI tests if account CLI commands are implemented.
  - Add app-server tests only if existing app-server account behavior needs compatibility updates.

## Validation

- Run from `codex-rs`:
  - `just fmt`
  - `just fix -p codex-login`
  - `just test -p codex-login`
  - `just test -p codex-core`
  - `just test -p codex-app-server` if account app-server behavior is touched
- If `ConfigToml` or app-server protocol schema changes, run the relevant schema writer.
- If Rust dependencies change, run `just bazel-lock-update` and `just bazel-lock-check`. This plan should not require new dependencies.

Validation notes:

- `cargo check -p codex-login -p codex-core -p codex-cli --tests` passed in Docker.
- `cargo fmt -- --config imports_granularity=Item` was run in Docker; stable rustfmt reports `imports_granularity` as nightly-only but still formatted the workspace.
- `just test -p codex-login` passed in Docker, including bench smoke.
- `just test -p codex-cli account_commands_parse` passed in Docker, including bench smoke.
- `just test -p codex-core -E 'test(rate_limit_event_switches_account_for_next_request) | test(usage_limit_error_switches_account_for_next_request_without_retry)'` passed in Docker, including bench smoke.
- A broader `just test -p codex-core` was attempted in Docker and reached unrelated environment failures around sandbox/MCP helper binaries, so focused core coverage was used for this feature.
- No app-server protocol/schema or Rust dependency changes were made.

## Risks And Edge Cases

- Refresh-token rotation across multiple processes can still race. The store should reload before refresh and avoid refreshing accounts that are not being selected.
- Switching accounts may affect workspace-scoped MCP/plugin/connector availability. Selection must respect forced workspace config and notify downstream account state consumers.
- Some requests may reuse websocket connections. Switching should force new auth on the next connection/request and avoid reusing a connection authenticated as the old account.
- The new multi-account store is file-backed by design, so it will contain credentials. The implementation must write it with restrictive permissions (`0600` on Unix), matching `auth.json`.
- Usage-limit snapshots are best-effort. If no snapshot arrives, switching can still happen after `UsageLimitReached`.
- API key and agent identity auth should not be silently mixed into the ChatGPT account pool unless explicitly designed.

## Closed Decisions

- Store all managed ChatGPT accounts in one file-backed `$CODEX_HOME/accounts.json` array.
- Keep `$CODEX_HOME/auth.json` as the active-account compatibility file.
- Switch proactively for future requests when remaining limit is below 10%, and also after `UsageLimitReached`/429.
- Do not retry the current failed turn after a switch; the next request uses the new account.
- Keep logout behavior close to existing behavior; account logout UX is not a priority.
- Add a new CLI namespace `codex account ...` for manual account controls.
- TUI account controls are deferred to a follow-up after CLI support lands.

## Remaining Open Decisions

- None for the first implementation plan.
