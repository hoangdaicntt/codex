use chrono::DateTime;
use chrono::Utc;
use codex_protocol::protocol::CreditsSnapshot;
use codex_protocol::protocol::RateLimitReachedType;
use codex_protocol::protocol::RateLimitSnapshot;
use codex_protocol::protocol::RateLimitWindow;

use super::metadata::AccountMetadata;
use super::store::AccountId;
use super::store::AccountsIndex;
use super::store::StoredAccount;
use super::store::StoredLimitKind;
use super::store::StoredLimitState;

const NEAR_LIMIT_USED_PERCENT: f64 = 90.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectionReason {
    ProactiveNearLimit,
    UsageLimitReached,
    Manual,
}

#[derive(Clone, Debug, PartialEq)]
pub enum LimitClassification {
    Available,
    NearLimit { resets_at: Option<DateTime<Utc>> },
    Exhausted { resets_at: Option<DateTime<Utc>> },
}

pub fn classify_rate_limit_snapshot(snapshot: &RateLimitSnapshot) -> LimitClassification {
    if snapshot.credits.as_ref().is_some_and(credits_are_exhausted)
        || snapshot
            .rate_limit_reached_type
            .is_some_and(reached_type_is_exhausted)
    {
        return LimitClassification::Exhausted {
            resets_at: snapshot_reset_time(snapshot),
        };
    }

    if window_is_near_limit(snapshot.primary.as_ref())
        || window_is_near_limit(snapshot.secondary.as_ref())
    {
        return LimitClassification::NearLimit {
            resets_at: snapshot_reset_time(snapshot),
        };
    }

    LimitClassification::Available
}

pub(crate) fn limit_state_from_snapshot(
    snapshot: RateLimitSnapshot,
    recorded_at: DateTime<Utc>,
) -> Option<StoredLimitState> {
    let classification = classify_rate_limit_snapshot(&snapshot);
    match classification {
        LimitClassification::Available => None,
        LimitClassification::NearLimit { resets_at } => Some(StoredLimitState {
            kind: StoredLimitKind::NearLimit,
            recorded_at,
            resets_at,
            snapshot: Some(snapshot),
        }),
        LimitClassification::Exhausted { resets_at } => Some(StoredLimitState {
            kind: StoredLimitKind::Exhausted,
            recorded_at,
            resets_at,
            snapshot: Some(snapshot),
        }),
    }
}

pub(crate) fn select_next_available_account(
    index: &AccountsIndex,
    active_account_id: &AccountId,
    forced_workspace_ids: Option<&[String]>,
    now: DateTime<Utc>,
) -> Option<AccountId> {
    let active_index = index
        .accounts
        .iter()
        .position(|account| &account.account_id == active_account_id)?;
    index
        .accounts
        .iter()
        .cycle()
        .skip(active_index + 1)
        .take(index.accounts.len().saturating_sub(1))
        .find(|account| account_is_eligible(account, forced_workspace_ids, now))
        .map(|account| account.account_id.clone())
}

fn account_is_eligible(
    account: &StoredAccount,
    forced_workspace_ids: Option<&[String]>,
    now: DateTime<Utc>,
) -> bool {
    if let Some(expected) = forced_workspace_ids
        && !account
            .workspace_id
            .as_ref()
            .is_some_and(|workspace_id| expected.contains(workspace_id))
    {
        return false;
    }

    if AccountMetadata::from_auth(&account.auth).is_none() {
        return false;
    }

    if account.last_auth_failure.is_some() {
        return false;
    }

    if account
        .last_limit_state
        .as_ref()
        .is_some_and(|state| state.is_active(now))
    {
        return false;
    }

    true
}

fn credits_are_exhausted(credits: &CreditsSnapshot) -> bool {
    !credits.unlimited && !credits.has_credits
}

fn reached_type_is_exhausted(reached_type: RateLimitReachedType) -> bool {
    match reached_type {
        RateLimitReachedType::RateLimitReached
        | RateLimitReachedType::WorkspaceOwnerCreditsDepleted
        | RateLimitReachedType::WorkspaceMemberCreditsDepleted
        | RateLimitReachedType::WorkspaceOwnerUsageLimitReached
        | RateLimitReachedType::WorkspaceMemberUsageLimitReached => true,
    }
}

fn window_is_near_limit(window: Option<&RateLimitWindow>) -> bool {
    window.is_some_and(|window| window.used_percent >= NEAR_LIMIT_USED_PERCENT)
}

pub(crate) fn snapshot_reset_time(snapshot: &RateLimitSnapshot) -> Option<DateTime<Utc>> {
    [snapshot.primary.as_ref(), snapshot.secondary.as_ref()]
        .into_iter()
        .flatten()
        .filter_map(|window| window.resets_at)
        .max()
        .and_then(|timestamp| DateTime::from_timestamp(timestamp, 0))
}
