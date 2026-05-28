# Feature: Indexed Account Picker For CLI And TUI

## Overview

Update multi-account controls so saved ChatGPT accounts are shown as an indexed list and can be selected by index instead of account id. The same compact account display should be used by CLI and TUI:

```text
1. user@example.com (5H 12%, Week 44%) - 2 hours ago | 10:30 28/05/2026
```

CLI changes:

- `codex account` is the only account-management command.
- `codex account` prints the indexed list and, when running in an interactive terminal, lets the user type/select an index and press Enter to switch.
- Remove the `list`, `switch`, and `sync` account subcommands from the public CLI.

TUI changes:

- Add `/accounts` slash command.
- `/accounts` opens an account picker similar in spirit to `/fast`, `/status`, and other in-app menus.
- The picker lists accounts in the shared display format and switches the active account when the highlighted row is confirmed with Enter.

## Current Project Survey

- `codex-rs/login/src/auth/multi_account/store.rs` stores `$CODEX_HOME/accounts.json` with account id, email, plan, workspace, last used time, limit state, refresh failure state, and full `AuthDotJson`.
- `codex-rs/login/src/auth/manager.rs` already exposes:
  - `list_accounts()`
  - `switch_account(account_id)`
  - `sync_active_auth_json()`
  - active account helpers used by CLI/core.
- `codex-rs/cli/src/main.rs` currently implements:
  - `codex account list`
  - `codex account switch <account-id>`
  - `codex account sync`
- TUI slash commands are registered in `codex-rs/tui/src/slash_command.rs`.
- TUI slash dispatch happens in `codex-rs/tui/src/chatwidget/slash_dispatch.rs`.
- TUI already has reusable menu/picker patterns for model, permissions, statusline/title setup, goal menu, and other command surfaces.
- Rate-limit display helpers exist in `codex-rs/tui/src/status/rate_limits.rs`; they already know how to render 5-hour and weekly-like windows from `RateLimitSnapshot`.

## Requirements

- The visible account list format must be:

  ```text
  Index. Email(5H percent, Week percent) - Last_used | CreatedAt
  ```

  Example:

  ```text
  1. a@example.com (5H 12%, Week 44%) - 2 hours ago | 10:30 28/05/2026
  2. b@example.com (5H -, Week -) - never | 08:15 27/05/2026
  ```

- Use 1-based indexes.
- Mark the active account without breaking the requested format. Proposed default:

  ```text
  * 1. user@example.com (5H 12%, Week 44%) - 2 hours ago | 10:30 28/05/2026
    2. other@example.com (5H -, Week -) - never | 08:15 27/05/2026
  ```

- `codex account` should reject invalid selected indexes with a clear error.
- `codex account` should not block non-interactive scripts. It should only prompt for index selection when stdin and stdout are terminals.
- TUI `/accounts` should list accounts and switch on Enter.
- Switching account must continue to rewrite `$CODEX_HOME/auth.json`, reload auth cache, and keep conversation/session state intact.
- Do not add logout-account management in this change.

## Data And Formatting Decisions

- Keep `accounts.json` compatible with the current schema.
- Reuse existing `last_used_at` for `Last used`.
- Add `createdAt` to `accounts.json` with backwards-compatible serde defaults for existing stores.
- Preserve existing account creation timestamps on upsert.
- Use `last_used_at` for `Last_used`.
- `Last_used` must render as time ago, such as `2 hours ago`; if unavailable, show `never`.
- `CreatedAt` must render as `HH:mm DD/MM/YYYY`.
- For percent display:
  - `5H` maps to the stored snapshot window whose duration/label corresponds to the primary short window when available.
  - `Week` maps to the stored snapshot window whose duration/label corresponds to the weekly/secondary window when available.
  - If the server does not provide exact 5-hour/7-day durations, still map primary/secondary to `5H`/`Week`.
  - If no matching window exists, show `-`.
  - Percent values should be rounded consistently, e.g. `12%`, `44%`.
- Email fallback should be `unknown` or `-` when unavailable. Proposed default: `-`.

## Architecture

- Add a shared account display model in `codex-login` multi-account module or a small CLI/TUI-local helper if reuse from TUI would create an awkward dependency.
- Preferred low-churn route:
  - Add public methods to `AuthManager`:
    - `list_accounts_with_active()` or return an index wrapper including active id.
    - `switch_account_by_index(index: usize)`.
  - Keep account id switching internally for core auto-failover.
  - CLI and TUI convert user-facing indexes to account ids through the same helper.
- CLI:
  - Replace the `AccountSubcommand` enum with a single `codex account` command handler.
  - Update parser tests.
  - Implement interactive prompt after the list when terminal is interactive.
  - Remove `codex account list`, `codex account switch`, and `codex account sync` from help and parsing.
- TUI:
  - Add `SlashCommand::Accounts`.
  - Add dispatch path in `chatwidget/slash_dispatch.rs`.
  - Add a focused account picker module instead of growing `chatwidget.rs`.
  - Use existing popup/menu state patterns for keyboard navigation and Enter selection.
  - On selection, call the existing auth manager switch path and surface a short confirmation/error message in history.

## Tasks

- [x] Task 1: Add indexed account selection helpers.
  - Add a helper that loads accounts in store order and resolves a 1-based index to account id.
  - Return clear errors for zero, out-of-range, or empty account list.
  - Keep existing account-id based switching for internal failover.
  - Add unit tests in `codex-login`.

- [x] Task 2: Add shared account list display formatting.
  - Add backwards-compatible `createdAt` persistence to stored accounts.
  - Format each row as `Index. Email(5H x%, Week y%) - Last_used | CreatedAt`.
  - Include active marker while preserving scan-friendly rows.
  - Extract `5H`/`Week` percentages from `last_limit_state.snapshot`.
  - Render `Last_used` as time ago and `CreatedAt` as `HH:mm DD/MM/YYYY`.
  - Add tests for missing email, missing limits, active marker, time-ago, and created-at formatting.

- [x] Task 3: Update CLI account commands.
  - Replace `codex account list/switch/sync` with only `codex account`.
  - Update `codex account` output to the new compact list format.
  - When interactive, prompt for an index after list and switch on Enter.
  - Avoid prompting when stdin/stdout are not terminals.
  - Update CLI parse tests and add behavior tests where practical.

- [x] Task 4: Add TUI `/accounts` picker.
  - Register `SlashCommand::Accounts` with description.
  - Add dispatch handling that opens the picker.
  - Build a focused picker component or reuse an existing menu pattern.
  - Show the same account list format.
  - Support keyboard navigation and Enter selection.
  - Switch account via auth manager and refresh visible auth state.

- [x] Task 5: Add TUI coverage.
  - Add snapshot or component tests for `/accounts` list rendering.
  - Add interaction test for selecting an account with Enter.
  - Verify empty-account and switch-error states.

- [x] Task 6: Validation and commit.
  - Run `just fmt` from `codex-rs`.
  - Run `just fix -p codex-login -p codex-cli -p codex-tui`.
  - Run `just test -p codex-login`.
  - Run `just test -p codex-cli account_commands_parse`.
  - Run focused TUI tests for the new `/accounts` picker.
  - If snapshots change, review and accept intended `insta` updates.
  - Commit the completed code update.

## Validation

- CLI manual checks:

  ```bash
  codex account
  ```

- TUI manual checks:

  ```text
  /accounts
  ```

  Then move selection and press Enter.

- Expected behavior:
  - account indexes are stable for the current store order;
  - active account marker moves after switch;
  - `auth.json` reflects the selected account;
  - current session/conversation remains open;
- non-interactive `codex account` prints rows and exits.

Validation notes:

- `just fmt` was attempted in Docker. Rust `cargo fmt` completed, then the Python `uv` step failed because `openai-codex-cli-bin==0.131.0a4` has no Linux glibc wheel for the container platform.
- `just test -p codex-login` passed.
- `just test -p codex-cli account_commands_parse` passed.
- `just test -p codex-tui accounts` passed.
- `just test -p codex-tui certain_commands_are_available_during_task` passed.
- `just fix -p codex-login -p codex-cli -p codex-tui` passed.

## Confirmed Decisions

- Use `codex account` as the single CLI entry point; remove `list`, `switch`, and `sync`.
- In interactive terminals, `codex account` lists accounts and prompts for an index; empty Enter keeps the current account.
- Display labels are `5H` and `Week`.
- Map primary/secondary rate-limit windows to `5H`/`Week` even when exact durations are missing or different.
- Show account rows as `Index. Email(5H percent, Week percent) - Last_used | CreatedAt`.
- Render `Last_used` as time ago.
- Render `CreatedAt` as `HH:mm DD/MM/YYYY`.
