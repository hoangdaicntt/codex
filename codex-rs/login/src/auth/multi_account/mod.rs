mod display;
mod metadata;
mod selection;
mod store;
mod token_refresh;

pub use display::AccountDisplayRow;
pub use display::account_display_label;
pub use display::account_id_at_index;
pub use display::display_rows;
pub use metadata::AccountMetadata;
pub use selection::LimitClassification;
pub use selection::SelectionReason;
pub use selection::classify_rate_limit_snapshot;
pub use selection::preferred_account_limit_snapshot;
pub use store::AccountId;
pub use store::AccountsIndex;
pub use store::AccountsStore;
pub use store::RemoveAccountOutcome;
pub use store::StoredAccount;
pub use store::StoredAuthFailureKind;
pub use store::StoredAuthFailureState;
pub use store::StoredLimitKind;
pub use store::StoredLimitState;
pub use store::StoredRateLimitSnapshot;
pub use token_refresh::ResolvedStoredAccountAuth;
pub use token_refresh::stored_auth_is_stale;

#[cfg(test)]
mod tests;
