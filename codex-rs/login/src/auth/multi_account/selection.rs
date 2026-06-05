use chrono::DateTime;
use chrono::Utc;
use codex_protocol::protocol::RateLimitReachedType;
use codex_protocol::protocol::RateLimitSnapshot;
use codex_protocol::protocol::RateLimitWindow;

const NEAR_LIMIT_USED_PERCENT: f64 = 90.0;

#[derive(Clone, Debug, PartialEq)]
pub enum LimitClassification {
    Available,
    NearLimit { resets_at: Option<DateTime<Utc>> },
    Exhausted { resets_at: Option<DateTime<Utc>> },
}

pub fn classify_rate_limit_snapshot(snapshot: &RateLimitSnapshot) -> LimitClassification {
    if snapshot
        .rate_limit_reached_type
        .is_some_and(reached_type_is_exhausted)
    {
        return LimitClassification::Exhausted {
            resets_at: snapshot_reset_time(snapshot),
        };
    }

    if window_is_near_limit(snapshot.primary.as_ref()) {
        return LimitClassification::NearLimit {
            resets_at: snapshot_reset_time(snapshot),
        };
    }

    LimitClassification::Available
}

pub fn preferred_account_limit_snapshot(
    snapshots: Vec<RateLimitSnapshot>,
) -> Option<RateLimitSnapshot> {
    snapshots
        .iter()
        .find(|snapshot| snapshot.limit_id.as_deref() == Some("codex"))
        .cloned()
        .or_else(|| snapshots.into_iter().next())
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
