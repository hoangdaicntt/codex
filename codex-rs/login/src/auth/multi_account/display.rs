use chrono::DateTime;
use chrono::Local;
use chrono::Utc;
use codex_protocol::protocol::RateLimitWindow;

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
    let last_used = format_compact_elapsed(account.last_used_at, now);
    let created_at = format_created_at(account.created_at);
    let detail = if account.last_auth_failure.is_some() {
        format!("   auth failed · used {last_used} · {created_at}")
    } else {
        let five_hour = limit_display(account, LimitWindowKind::FiveHour, now);
        let week = limit_display(account, LimitWindowKind::Week, now);
        format!("   5h {five_hour} · Week {week} · used {last_used} · {created_at}")
    };
    let line = format!("{marker}{index}. {email}\n{detail}");

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

fn limit_display(account: &StoredAccount, kind: LimitWindowKind, now: DateTime<Utc>) -> String {
    let Some(snapshot) = account
        .last_rate_limits
        .as_ref()
        .map(|stored| &stored.snapshot)
        .or_else(|| {
            account
                .last_limit_state
                .as_ref()
                .and_then(|state| state.snapshot.as_ref())
        })
    else {
        return "-".to_string();
    };

    let window = match kind {
        LimitWindowKind::FiveHour => snapshot.primary.as_ref(),
        LimitWindowKind::Week => snapshot.secondary.as_ref(),
    };

    window
        .map(|window| format_window(window, now))
        .unwrap_or_else(|| "-".to_string())
}

fn format_window(window: &RateLimitWindow, now: DateTime<Utc>) -> String {
    let percent = format!("{:.0}%", window.used_percent);
    let reset = window
        .resets_at
        .and_then(|timestamp| DateTime::from_timestamp(timestamp, 0))
        .map(|timestamp| format_reset_time(timestamp, now))
        .unwrap_or_else(|| "-".to_string());
    format!("{percent}/{reset}")
}

fn format_reset_time(timestamp: DateTime<Utc>, now: DateTime<Utc>) -> String {
    if timestamp <= now {
        return "now".to_string();
    }
    format_compact_duration(timestamp.signed_duration_since(now).num_seconds())
}

fn format_compact_elapsed(timestamp: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let seconds = now.signed_duration_since(timestamp).num_seconds().max(0);
    format_compact_duration(seconds)
}

fn format_compact_duration(seconds: i64) -> String {
    match seconds {
        0..=59 => "now".to_string(),
        60..=3_599 => format!("{}m", seconds / 60),
        3_600..=86_399 => {
            let hours = seconds / 3_600;
            let minutes = (seconds % 3_600) / 60;
            if minutes == 0 {
                format!("{hours}h")
            } else {
                format!("{hours}h{minutes}m")
            }
        }
        86_400..=604_799 => format!("{}d", seconds / 86_400),
        604_800..=31_535_999 => format!("{}w", seconds / 604_800),
        _ => format!("{}y", seconds / 31_536_000),
    }
}

fn format_created_at(timestamp: DateTime<Utc>) -> String {
    timestamp
        .with_timezone(&Local)
        .format("%d/%m %H:%M")
        .to_string()
}
