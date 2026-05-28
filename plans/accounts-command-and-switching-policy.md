# Feature: Accounts Command And Rate Limit Display

## Overview

Refine the multi-account picker after testing the built binary at:

```bash
/home/hoangdai/PhpstormProjects/codex/docker-build-out/codex accounts
```

Updated scope:

- Rename the CLI command from `codex account` to `codex accounts`, matching TUI `/accounts`.
- Keep `/accounts` visually clean by removing per-row helper text like `Press Enter to use this account`.
- Improve rate-limit display so `(5H x%, Week y%)` appears whenever Codex has a usable snapshot.

Out of scope:

- Account switching config.
- Round-robin switching.
- Retry-current-request behavior.
- Error-based account failover policy changes.

## Current Project Survey

- CLI implementation lives in `codex-rs/cli/src/main.rs`.
  - The command now exposes `codex accounts`.
  - A hidden blocker handles singular `codex account` with a clear removal message so it is not treated as an interactive prompt.
  - It uses `AuthManager::list_account_display_rows()` and `switch_account_by_index(...)`.
- TUI picker lives in `codex-rs/tui/src/chatwidget/accounts.rs`.
  - `/accounts` remains registered in `codex-rs/tui/src/slash_command.rs`.
  - Account row item descriptions are removed; the standard selection footer remains.
- Account display helpers live in `codex-rs/login/src/auth/multi_account/display.rs`.
  - `display_rows(...)` renders `5H` from `last_limit_state.snapshot.primary`.
  - `display_rows(...)` renders `Week` from `last_limit_state.snapshot.secondary`.
- Account store lives in `codex-rs/login/src/auth/multi_account/store.rs`.
  - `StoredLimitState` supports `snapshot: Option<RateLimitSnapshot>`.
  - A usage-limit path can now persist exhausted state with the full `RateLimitSnapshot`.
- Usage-limit handling lives in `codex-rs/core/src/session/turn.rs`.
  - When `UsageLimitReached` includes rate-limit data, Codex stores that snapshot so later `codex accounts` and `/accounts` rows can show percentages.

## Requirements

- CLI command name is plural:

  ```bash
  codex accounts
  ```

- Singular command is removed:

  ```bash
  codex account
  ```

- TUI command remains:

  ```text
  /accounts
  ```

- TUI `/accounts` rows do not show `Press Enter to use this account`.

- Account row format remains:

  ```text
  1. a@example.com (5H 12%, Week 44%) - 2 hours ago | 10:30 28/05/2026
  ```

- Show `-` only when no percent data is available.
- When `UsageLimitReached` includes rate-limit snapshot data, persist that snapshot so later `codex accounts` and `/accounts` can display percentages.

## Tasks

- [x] Task 1: Rename CLI command to `accounts`.
  - Rename CLI structs/enums and dispatch.
  - Keep implementation behavior from current account picker.
  - Update parse tests.

- [x] Task 2: Remove `/accounts` row helper text.
  - Remove `SelectionItem.description` from account rows.
  - Update TUI test to assert no helper description.
  - Keep standard footer hints intact.

- [x] Task 3: Persist rate-limit snapshots on usage-limit errors.
  - Add store/auth-manager method to record exhausted state with snapshot.
  - Update `UsageLimitReached` handling to persist `e.rate_limits` when present.
  - Add test proving `lastLimitState.snapshot` is stored and display rows show percentages.

- [x] Task 4: Final validation.
  - Run Rust fmt.
  - Run `just test -p codex-cli account_commands_parse`.
  - Run `just test -p codex-tui account_selection_params_renders_account_rows`.
  - Run `just test -p codex-login mark_active_exhausted_from_snapshot_preserves_display_percentages`.
  - Run relevant `just fix -p ...` commands for touched crates.
  - Manually check the rebuilt binary command shape.

Validation note:

- `cargo fmt` ran via `just fmt`.
- Full `just fmt` did not complete because the Python `uv` step cannot install `openai-codex-cli-bin==0.131.0a4` on Linux glibc; this is outside the Rust files changed here.

## Validation

Manual checks after rebuild:

```bash
./docker-build-out/codex accounts
./docker-build-out/codex account
```

Expected:

- `codex accounts` lists accounts and allows index selection in an interactive terminal.
- `codex account` does not run the old command.
- Account rows use:

  ```text
  1. a@example.com (5H 12%, Week 44%) - 2 hours ago | 10:30 28/05/2026
  ```

TUI:

```text
/accounts
```

Expected:

- account rows appear without `Press Enter to use this account`;
- selecting a row switches account;
- active marker updates after reopening the picker.
