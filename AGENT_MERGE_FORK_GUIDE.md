# Agent Merge Fork Guide

This fork carries local multi-account ChatGPT auth work on top of upstream Codex.
When merging or rebasing from the upstream main branch, preserve that behavior
unless the user explicitly asks to remove it.

## Protected Local Feature

Local commit to understand before merging:

```text
d1029dac3a Add multi-account ChatGPT auth switching
```

The feature adds:

- `$CODEX_HOME/accounts.json` as a file-backed store for multiple managed
  ChatGPT OAuth accounts.
- `$CODEX_HOME/auth.json` remains the active-account compatibility file.
- `codex accounts` lists saved accounts and, in an interactive terminal, lets
  the user switch by 1-based index.
- TUI `/accounts` opens an account picker and switches without closing the
  current session.
- Rate-limit snapshots are saved per account. Near-limit or exhausted accounts
  are skipped when selecting the next available account.
- Core switches to another eligible account for the next request after a
  near-limit snapshot or a usage-limit error. It does not retry the failed turn
  automatically.

## Files To Preserve

These files are the main fork-owned implementation. If upstream does not have
equivalent multi-account support, prefer keeping the fork version and adapting
imports or surrounding APIs around it:

```text
codex-rs/login/src/auth/multi_account/mod.rs
codex-rs/login/src/auth/multi_account/store.rs
codex-rs/login/src/auth/multi_account/selection.rs
codex-rs/login/src/auth/multi_account/display.rs
codex-rs/login/src/auth/multi_account/metadata.rs
codex-rs/login/src/auth/multi_account/tests.rs
codex-rs/tui/src/chatwidget/accounts.rs
plans/multi-account-auth-failover.md
plans/indexed-account-picker-cli-tui.md
plans/accounts-command-and-switching-policy.md
```

These files are upstream-owned integration points with local hooks. During
conflicts, merge carefully instead of blindly taking either side:

```text
codex-rs/login/src/auth/mod.rs
codex-rs/login/src/auth/manager.rs
codex-rs/login/src/server.rs
codex-rs/cli/src/main.rs
codex-rs/core/src/session/turn.rs
codex-rs/core/src/compact.rs
codex-rs/tui/src/app_event.rs
codex-rs/tui/src/app/event_dispatch.rs
codex-rs/tui/src/chatwidget.rs
codex-rs/tui/src/chatwidget/slash_dispatch.rs
codex-rs/tui/src/slash_command.rs
codex-rs/core/tests/common/test_codex.rs
codex-rs/core/tests/suite/client.rs
codex-rs/core/tests/suite/guardian_review.rs
```

## Recommended Workflow

Start from a clean branch or a branch with only intentional local changes:

```bash
git status --short
git fetch upstream
git checkout multi-auth-squashed
git merge upstream/main
```

If the branch should be linear, use rebase instead:

```bash
git fetch upstream
git checkout multi-auth-squashed
git rebase upstream/main
```

Do not use `git reset --hard`, `git checkout -- .`, or bulk conflict
resolution commands unless the user explicitly asks. This repository often has
local work in progress.

## Conflict Resolution Rules

For `codex-rs/login/src/auth/multi_account/*`:

- Keep the fork module unless upstream introduced a more complete equivalent.
- Preserve the JSON shape: `version`, `activeAccountId`, `accounts`,
  `createdAt`, `lastUsedAt`, `lastLimitState`, `lastAuthFailure`, and embedded
  `auth`.
- Keep Unix writes restrictive (`0600`) and temp-file-plus-rename saves.
- Keep `AuthDotJson` support limited to managed ChatGPT auth for the account
  pool.

For `codex-rs/login/src/auth/manager.rs`:

- Preserve `AccountsStore::import_active_auth_if_missing(...)` during manager
  initialization.
- Preserve account APIs:
  - `active_account_id`
  - `list_accounts`
  - `list_account_display_rows`
  - `switch_account`
  - `switch_account_by_index`
  - `mark_active_account_limited`
  - `mark_active_account_exhausted`
  - `mark_active_account_exhausted_from_snapshot`
  - `switch_to_next_available_account`
  - `switch_if_active_account_limited`
- Keep `switch_account` writing `auth.json`, then calling `reload()`.
- Preserve refresh safety: reload only when account IDs match, and record
  permanent refresh failures on the active account when appropriate.
- Preserve forced workspace filtering when selecting the next account.

For `codex-rs/login/src/server.rs`:

- Login must upsert successful managed ChatGPT auth into `accounts.json`.
- Login must still write the chosen active account to `auth.json`.
- Do not revoke a previous saved account merely because a different account
  logged in.

For `codex-rs/cli/src/main.rs`:

- Keep the public command as `codex accounts`.
- Keep singular `codex account` as a hidden removed-command blocker with a clear
  error pointing to `codex accounts`.
- Preserve non-interactive behavior: print accounts and exit when stdin or
  stdout is not a terminal.
- Preserve interactive behavior: prompt for a 1-based index; empty Enter keeps
  the current account.

For `codex-rs/tui/*`:

- Preserve `SlashCommand::Accounts` with command string `/accounts`.
- Preserve `/accounts` availability during running tasks and side
  conversations.
- Keep account picker code in `chatwidget/accounts.rs`; avoid growing
  `chatwidget.rs` with the picker implementation.
- Preserve `AppEvent::AccountSwitchFinished` and the event-dispatch path that
  updates visible account state after a switch.

For `codex-rs/core/src/session/turn.rs` and `compact.rs`:

- On `ResponseEvent::RateLimits`, keep recording the snapshot into the active
  account via `auth_manager.mark_active_account_limited(...)`.
- Before the next sampling request, keep
  `switch_if_active_account_limited()`.
- On `CodexErr::UsageLimitReached`, mark the active account exhausted. If the
  error has a rate-limit snapshot, store that snapshot too.
- Do not retry the current failed turn automatically after switching.
- If a switch happens, recreate the model client session before the next request
  so auth changes are picked up.

## Quick Behavior Checks

After resolving conflicts, inspect the account command shape:

```bash
cd codex-rs
just test -p codex-cli account_commands_parse
```

Check multi-account storage and selection:

```bash
cd codex-rs
just test -p codex-login multi_account
just test -p codex-login mark_active_exhausted_from_snapshot_preserves_display_percentages
```

Check core failover behavior:

```bash
cd codex-rs
just test -p codex-core -E 'test(rate_limit_event_switches_account_for_next_request) | test(usage_limit_error_switches_account_for_next_request_without_retry)'
```

Check TUI account picker behavior:

```bash
cd codex-rs
just test -p codex-tui accounts
just test -p codex-tui certain_commands_are_available_during_task
```

Finally format Rust changes:

```bash
cd codex-rs
just fmt
```

If Rust code changed in the merge, run scoped fix commands for touched crates,
for example:

```bash
cd codex-rs
just fix -p codex-login -p codex-cli -p codex-tui
```

Do not run `cargo test` directly; this repo expects `just test`.

## Manual Smoke Test

With a built fork binary:

```bash
codex accounts
```

Expected:

- Saved ChatGPT accounts are displayed as indexed rows.
- The active account is marked with `*`.
- Rows include `5H` and `Week` percentages when snapshots are available.
- In an interactive terminal, entering an index switches the active account.

In the TUI:

```text
/accounts
```

Expected:

- A picker opens with the same account row format.
- Selecting a row switches the active account.
- The session remains open.

## Common Mistakes

- Do not rename the public command back to `codex account`; the current fork uses
  `codex accounts`.
- Do not remove `$CODEX_HOME/auth.json` writes. It is still the compatibility
  active-auth file.
- Do not store API-key, agent-identity, or external token auth as saved ChatGPT
  accounts unless a later feature explicitly designs that behavior.
- Do not drop `lastLimitState.snapshot`; the account list uses it to display
  percentages.
- Do not retry the same turn automatically after a usage-limit switch. The
  intended behavior is "switch for the next request".
- Do not add app-server v1 API surface for account switching during merge
  cleanup.
