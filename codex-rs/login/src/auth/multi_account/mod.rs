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
pub use selection::classify_rate_limit_snapshot;
pub use selection::preferred_account_limit_snapshot;
pub use token_refresh::AccountLimitSnapshots;
pub use store::AccountId;
pub use store::AccountsIndex;
pub use store::AccountsStore;
pub use store::RemoveAccountOutcome;
pub use store::StoredAccount;
pub use token_refresh::ResolvedStoredAccountAuth;

#[cfg(test)]
mod tests;
