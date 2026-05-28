use chrono::DateTime;
use chrono::Local;
use chrono::Utc;

use super::store::AccountId;
use super::store::StoredAccount;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountDisplayRow {
    pub index: usize,
    pub account_id: AccountId,
    pub is_active: bool,
    pub line: String,
}

pub fn display_rows(
    accounts: &[StoredAccount],
    active_account_id: Option<&AccountId>,
    now: DateTime<Utc>,
) -> Vec<AccountDisplayRow> {
    accounts
        .iter()
        .enumerate()
        .map(|(offset, account)| display_row(offset + 1, account, active_account_id, now))
        .collect()
}

pub fn account_id_at_index(accounts: &[StoredAccount], index: usize) -> std::io::Result<AccountId> {
    if index == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "account index starts at 1",
        ));
    }

    let Some(account) = accounts.get(index - 1) else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("account index {index} was not found"),
        ));
    };

    Ok(account.account_id.clone())
}

fn display_row(
    index: usize,
    account: &StoredAccount,
    active_account_id: Option<&AccountId>,
    now: DateTime<Utc>,
) -> AccountDisplayRow {
    let is_active = active_account_id == Some(&account.account_id);
    let marker = if is_active { "* " } else { "  " };
    let email = account.email.as_deref().unwrap_or("-");
    let five_hour = limit_percent(account, LimitWindowKind::FiveHour);
    let week = limit_percent(account, LimitWindowKind::Week);
    let last_used = format_time_ago(account.last_used_at, now);
    let created_at = format_created_at(account.created_at);
    let line = format!(
        "{marker}{index}. {email} (5H {five_hour}, Week {week}) - {last_used} | {created_at}"
    );

    AccountDisplayRow {
        index,
        account_id: account.account_id.clone(),
        is_active,
        line,
    }
}

#[derive(Clone, Copy)]
enum LimitWindowKind {
    FiveHour,
    Week,
}

fn limit_percent(account: &StoredAccount, kind: LimitWindowKind) -> String {
    let Some(snapshot) = account
        .last_limit_state
        .as_ref()
        .and_then(|state| state.snapshot.as_ref())
    else {
        return "-".to_string();
    };

    let used_percent = match kind {
        LimitWindowKind::FiveHour => snapshot.primary.as_ref().map(|window| window.used_percent),
        LimitWindowKind::Week => snapshot
            .secondary
            .as_ref()
            .map(|window| window.used_percent),
    };

    used_percent
        .map(|percent| format!("{percent:.0}%"))
        .unwrap_or_else(|| "-".to_string())
}

fn format_time_ago(timestamp: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let seconds = now.signed_duration_since(timestamp).num_seconds().max(0);

    match seconds {
        0..=59 => "just now".to_string(),
        60..=3_599 => plural(seconds / 60, "minute"),
        3_600..=86_399 => plural(seconds / 3_600, "hour"),
        86_400..=2_591_999 => plural(seconds / 86_400, "day"),
        2_592_000..=31_535_999 => plural(seconds / 2_592_000, "month"),
        _ => plural(seconds / 31_536_000, "year"),
    }
}

fn plural(value: i64, unit: &str) -> String {
    let suffix = if value == 1 { "" } else { "s" };
    format!("{value} {unit}{suffix} ago")
}

fn format_created_at(timestamp: DateTime<Utc>) -> String {
    timestamp
        .with_timezone(&Local)
        .format("%H:%M %d/%m/%Y")
        .to_string()
}
