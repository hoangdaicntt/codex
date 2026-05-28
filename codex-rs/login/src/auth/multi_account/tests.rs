use base64::Engine;
use chrono::Duration;
use chrono::TimeZone;
use chrono::Utc;
use codex_app_server_protocol::AuthMode;
use codex_config::types::AuthCredentialsStoreMode;
use codex_protocol::auth::KnownPlan;
use codex_protocol::auth::PlanType as AuthPlanType;
use codex_protocol::protocol::CreditsSnapshot;
use codex_protocol::protocol::RateLimitReachedType;
use codex_protocol::protocol::RateLimitSnapshot;
use codex_protocol::protocol::RateLimitWindow;
use pretty_assertions::assert_eq;
use tempfile::tempdir;

use crate::auth::AuthDotJson;
use crate::auth::load_auth_dot_json;
use crate::auth::multi_account::AccountId;
use crate::auth::multi_account::AccountsIndex;
use crate::auth::multi_account::AccountsStore;
use crate::auth::multi_account::LimitClassification;
use crate::auth::multi_account::SelectionReason;
use crate::auth::multi_account::StoredLimitKind;
use crate::auth::multi_account::StoredLimitState;
use crate::auth::multi_account::account_id_at_index;
use crate::auth::multi_account::classify_rate_limit_snapshot;
use crate::auth::multi_account::display_rows;
use crate::auth::save_auth;
use crate::token_data::IdTokenInfo;
use crate::token_data::TokenData;

#[test]
fn upsert_active_auth_writes_single_accounts_json() -> anyhow::Result<()> {
    let codex_home = tempdir()?;
    let store = AccountsStore::new(codex_home.path().to_path_buf());

    let account_id = store.upsert_active_auth(chatgpt_auth("account-a", "a@example.com"))?;

    assert_eq!(account_id, AccountId::from("account-a"));
    let index = store.load()?;
    let expected = AccountsIndex {
        version: 1,
        active_account_id: Some(AccountId::from("account-a")),
        accounts: vec![index.accounts[0].clone()],
    };
    assert_eq!(index, expected);
    assert_eq!(index.accounts[0].email.as_deref(), Some("a@example.com"));
    assert!(store.path().exists());
    Ok(())
}

#[test]
fn accounts_json_created_at_is_backwards_compatible() -> anyhow::Result<()> {
    let legacy_json = serde_json::json!({
        "version": 1,
        "activeAccountId": "account-a",
        "accounts": [
            {
                "accountId": "account-a",
                "email": "a@example.com",
                "planType": "plus",
                "lastUsedAt": "2026-05-28T00:00:00Z",
                "auth": chatgpt_auth("account-a", "a@example.com")
            }
        ]
    });

    let index: AccountsIndex = serde_json::from_value(legacy_json)?;

    assert_eq!(index.accounts.len(), 1);
    Ok(())
}

#[test]
fn upsert_preserves_existing_created_at() -> anyhow::Result<()> {
    let codex_home = tempdir()?;
    let store = AccountsStore::new(codex_home.path().to_path_buf());
    store.upsert_active_auth(chatgpt_auth("account-a", "a@example.com"))?;
    let mut index = store.load()?;
    let created_at = Utc.with_ymd_and_hms(2026, 5, 27, 3, 4, 0).unwrap();
    index.accounts[0].created_at = created_at;
    store.save(&index)?;

    store.upsert_active_auth(chatgpt_auth("account-a", "a@example.com"))?;

    assert_eq!(store.load()?.accounts[0].created_at, created_at);
    Ok(())
}

#[test]
fn account_id_at_index_uses_one_based_indexes() -> anyhow::Result<()> {
    let codex_home = tempdir()?;
    let store = AccountsStore::new(codex_home.path().to_path_buf());
    store.upsert_active_auth(chatgpt_auth("account-a", "a@example.com"))?;
    store.upsert_active_auth(chatgpt_auth("account-b", "b@example.com"))?;
    let accounts = store.load()?.accounts;

    assert_eq!(
        account_id_at_index(&accounts, 2)?,
        AccountId::from("account-b")
    );
    assert!(account_id_at_index(&accounts, 0).is_err());
    assert!(account_id_at_index(&accounts, 3).is_err());
    Ok(())
}

#[test]
fn import_active_auth_migrates_auth_json_once() -> anyhow::Result<()> {
    let codex_home = tempdir()?;
    let auth = chatgpt_auth("account-a", "a@example.com");
    save_auth(codex_home.path(), &auth, AuthCredentialsStoreMode::File)?;
    let store = AccountsStore::new(codex_home.path().to_path_buf());

    let first = store.import_active_auth_if_missing(AuthCredentialsStoreMode::File)?;
    let second = store.import_active_auth_if_missing(AuthCredentialsStoreMode::File)?;

    assert_eq!(first, second);
    assert_eq!(second.accounts.len(), 1);
    assert_eq!(
        second.active_account_id.as_ref().map(AccountId::as_str),
        Some("account-a")
    );
    Ok(())
}

#[test]
fn switch_active_account_rewrites_auth_json() -> anyhow::Result<()> {
    let codex_home = tempdir()?;
    let store = AccountsStore::new(codex_home.path().to_path_buf());
    store.upsert_active_auth(chatgpt_auth("account-a", "a@example.com"))?;
    store.upsert_active_auth(chatgpt_auth("account-b", "b@example.com"))?;

    let changed = store.switch_active_account(
        &AccountId::from("account-a"),
        AuthCredentialsStoreMode::File,
    )?;

    let auth = load_auth_dot_json(codex_home.path(), AuthCredentialsStoreMode::File)?
        .expect("active auth should exist");
    assert!(changed);
    assert_eq!(
        auth.tokens.and_then(|tokens| tokens.account_id),
        Some("account-a".to_string())
    );
    Ok(())
}

#[test]
fn selection_skips_exhausted_accounts_until_reset() -> anyhow::Result<()> {
    let codex_home = tempdir()?;
    let store = AccountsStore::new(codex_home.path().to_path_buf());
    store.upsert_active_auth(chatgpt_auth("account-a", "a@example.com"))?;
    store.upsert_active_auth(chatgpt_auth("account-b", "b@example.com"))?;
    store.upsert_active_auth(chatgpt_auth("account-c", "c@example.com"))?;
    store.switch_active_account(
        &AccountId::from("account-a"),
        AuthCredentialsStoreMode::File,
    )?;
    store.switch_active_account(
        &AccountId::from("account-b"),
        AuthCredentialsStoreMode::File,
    )?;
    store.mark_active_exhausted(Some(Utc.with_ymd_and_hms(2099, 1, 1, 0, 0, 0).unwrap()))?;
    store.switch_active_account(
        &AccountId::from("account-a"),
        AuthCredentialsStoreMode::File,
    )?;

    let next = store.next_available_account(SelectionReason::ProactiveNearLimit, None)?;

    assert_eq!(next, Some(AccountId::from("account-c")));
    Ok(())
}

#[test]
fn selection_skips_near_limit_accounts_until_reset() -> anyhow::Result<()> {
    let codex_home = tempdir()?;
    let store = AccountsStore::new(codex_home.path().to_path_buf());
    store.upsert_active_auth(chatgpt_auth("account-a", "a@example.com"))?;
    store.upsert_active_auth(chatgpt_auth("account-b", "b@example.com"))?;
    store.upsert_active_auth(chatgpt_auth("account-c", "c@example.com"))?;
    store.switch_active_account(
        &AccountId::from("account-b"),
        AuthCredentialsStoreMode::File,
    )?;
    store.mark_active_from_snapshot(snapshot(Some(95.0), None, None, None))?;
    store.switch_active_account(
        &AccountId::from("account-a"),
        AuthCredentialsStoreMode::File,
    )?;

    let next = store.next_available_account(SelectionReason::ProactiveNearLimit, None)?;

    assert_eq!(next, Some(AccountId::from("account-c")));
    Ok(())
}

#[test]
fn selection_allows_accounts_after_limit_reset_expires() -> anyhow::Result<()> {
    let codex_home = tempdir()?;
    let store = AccountsStore::new(codex_home.path().to_path_buf());
    store.upsert_active_auth(chatgpt_auth("account-a", "a@example.com"))?;
    store.upsert_active_auth(chatgpt_auth("account-b", "b@example.com"))?;
    store.switch_active_account(
        &AccountId::from("account-b"),
        AuthCredentialsStoreMode::File,
    )?;
    store.mark_active_exhausted(Some(Utc::now() - Duration::minutes(1)))?;
    store.switch_active_account(
        &AccountId::from("account-a"),
        AuthCredentialsStoreMode::File,
    )?;

    let next = store.next_available_account(SelectionReason::UsageLimitReached, None)?;

    assert_eq!(next, Some(AccountId::from("account-b")));
    Ok(())
}

#[test]
fn selection_respects_forced_workspace_filter() -> anyhow::Result<()> {
    let codex_home = tempdir()?;
    let store = AccountsStore::new(codex_home.path().to_path_buf());
    store.upsert_active_auth(chatgpt_auth("account-a", "a@example.com"))?;
    store.upsert_active_auth(chatgpt_auth("account-b", "b@example.com"))?;
    store.upsert_active_auth(chatgpt_auth("account-c", "c@example.com"))?;
    store.switch_active_account(
        &AccountId::from("account-a"),
        AuthCredentialsStoreMode::File,
    )?;

    let forced_workspace_ids = vec!["account-c".to_string()];
    let next = store.next_available_account(
        SelectionReason::ProactiveNearLimit,
        Some(&forced_workspace_ids),
    )?;

    assert_eq!(next, Some(AccountId::from("account-c")));
    Ok(())
}

#[test]
fn display_rows_use_compact_account_format() -> anyhow::Result<()> {
    let codex_home = tempdir()?;
    let store = AccountsStore::new(codex_home.path().to_path_buf());
    store.upsert_active_auth(chatgpt_auth("account-a", "a@example.com"))?;
    let mut index = store.load()?;
    index.accounts[0].created_at = Utc.with_ymd_and_hms(2026, 5, 28, 3, 30, 0).unwrap();
    index.accounts[0].last_used_at = Utc.with_ymd_and_hms(2026, 5, 28, 8, 0, 0).unwrap();
    index.accounts[0].last_limit_state = Some(StoredLimitState {
        kind: StoredLimitKind::NearLimit,
        recorded_at: Utc.with_ymd_and_hms(2026, 5, 28, 8, 0, 0).unwrap(),
        resets_at: None,
        snapshot: Some(snapshot(Some(12.0), Some(44.0), None, None)),
    });
    store.save(&index)?;
    let now = Utc.with_ymd_and_hms(2026, 5, 28, 10, 0, 0).unwrap();

    let rows = display_rows(&index.accounts, index.active_account_id.as_ref(), now);

    assert_eq!(rows.len(), 1);
    assert!(rows[0].is_active);
    assert!(rows[0].line.starts_with("* 1. a@example.com "));
    assert!(rows[0].line.contains("(5H 12%, Week 44%)"));
    assert!(rows[0].line.contains("- 2 hours ago | "));
    assert!(rows[0].line.ends_with("28/05/2026"));
    Ok(())
}

#[test]
fn selection_skips_refresh_failed_accounts() -> anyhow::Result<()> {
    let codex_home = tempdir()?;
    let store = AccountsStore::new(codex_home.path().to_path_buf());
    store.upsert_active_auth(chatgpt_auth("account-a", "a@example.com"))?;
    store.upsert_active_auth(chatgpt_auth("account-b", "b@example.com"))?;
    store.upsert_active_auth(chatgpt_auth("account-c", "c@example.com"))?;
    store.switch_active_account(
        &AccountId::from("account-b"),
        AuthCredentialsStoreMode::File,
    )?;
    store.mark_active_refresh_failed(Some("refresh token expired".to_string()))?;
    store.switch_active_account(
        &AccountId::from("account-a"),
        AuthCredentialsStoreMode::File,
    )?;

    let next = store.next_available_account(SelectionReason::ProactiveNearLimit, None)?;

    assert_eq!(next, Some(AccountId::from("account-c")));
    Ok(())
}

#[test]
fn upsert_clears_refresh_failure_state() -> anyhow::Result<()> {
    let codex_home = tempdir()?;
    let store = AccountsStore::new(codex_home.path().to_path_buf());
    let auth = chatgpt_auth("account-a", "a@example.com");
    store.upsert_active_auth(auth.clone())?;
    store.mark_active_refresh_failed(Some("refresh token expired".to_string()))?;

    store.upsert_active_auth(auth)?;

    let index = store.load()?;
    assert_eq!(index.accounts[0].last_auth_failure, None);
    Ok(())
}

#[test]
fn selection_skips_invalid_stored_auth() -> anyhow::Result<()> {
    let codex_home = tempdir()?;
    let store = AccountsStore::new(codex_home.path().to_path_buf());
    store.upsert_active_auth(chatgpt_auth("account-a", "a@example.com"))?;
    store.upsert_active_auth(chatgpt_auth("account-b", "b@example.com"))?;
    store.upsert_active_auth(chatgpt_auth("account-c", "c@example.com"))?;
    let mut index = store.load()?;
    let account_b = index
        .accounts
        .iter_mut()
        .find(|account| account.account_id == AccountId::from("account-b"))
        .expect("account-b should exist");
    account_b.auth.tokens = None;
    store.save(&index)?;
    store.switch_active_account(
        &AccountId::from("account-a"),
        AuthCredentialsStoreMode::File,
    )?;

    let next = store.next_available_account(SelectionReason::ProactiveNearLimit, None)?;

    assert_eq!(next, Some(AccountId::from("account-c")));
    Ok(())
}

#[test]
fn classifies_near_limit_and_exhausted_snapshots() {
    let near = snapshot(Some(90.0), None, None, None);
    let exhausted_by_credits = snapshot(
        Some(25.0),
        None,
        Some(CreditsSnapshot {
            has_credits: false,
            unlimited: false,
            balance: None,
        }),
        None,
    );
    let exhausted_by_reached_type = snapshot(
        None,
        None,
        None,
        Some(RateLimitReachedType::WorkspaceOwnerUsageLimitReached),
    );

    assert_eq!(
        classify_rate_limit_snapshot(&near),
        LimitClassification::NearLimit { resets_at: None }
    );
    assert_eq!(
        classify_rate_limit_snapshot(&exhausted_by_credits),
        LimitClassification::Exhausted { resets_at: None }
    );
    assert_eq!(
        classify_rate_limit_snapshot(&exhausted_by_reached_type),
        LimitClassification::Exhausted { resets_at: None }
    );
}

#[test]
fn mark_active_from_snapshot_stores_near_limit() -> anyhow::Result<()> {
    let codex_home = tempdir()?;
    let store = AccountsStore::new(codex_home.path().to_path_buf());
    store.upsert_active_auth(chatgpt_auth("account-a", "a@example.com"))?;

    let kind = store.mark_active_from_snapshot(snapshot(Some(95.0), None, None, None))?;

    let index = store.load()?;
    assert_eq!(kind, Some(StoredLimitKind::NearLimit));
    assert_eq!(
        index.accounts[0]
            .last_limit_state
            .as_ref()
            .map(|state| state.kind),
        Some(StoredLimitKind::NearLimit)
    );
    Ok(())
}

#[test]
fn mark_active_exhausted_from_snapshot_preserves_display_percentages() -> anyhow::Result<()> {
    let codex_home = tempdir()?;
    let store = AccountsStore::new(codex_home.path().to_path_buf());
    store.upsert_active_auth(chatgpt_auth("account-a", "a@example.com"))?;

    store
        .mark_active_exhausted_from_snapshot(snapshot(Some(12.0), Some(44.0), None, None), None)?;

    let index = store.load()?;
    let expected_state = StoredLimitState {
        kind: StoredLimitKind::Exhausted,
        recorded_at: index.accounts[0]
            .last_limit_state
            .as_ref()
            .expect("limit state")
            .recorded_at,
        resets_at: None,
        snapshot: Some(snapshot(Some(12.0), Some(44.0), None, None)),
    };
    assert_eq!(
        index.accounts[0].last_limit_state.as_ref(),
        Some(&expected_state)
    );

    let rows = display_rows(
        &index.accounts,
        index.active_account_id.as_ref(),
        Utc::now(),
    );
    assert!(rows[0].line.contains("(5H 12%, Week 44%)"));
    Ok(())
}

fn chatgpt_auth(account_id: &str, email: &str) -> AuthDotJson {
    let raw_jwt = jwt_for_account(account_id, email);
    AuthDotJson {
        auth_mode: Some(AuthMode::Chatgpt),
        openai_api_key: None,
        tokens: Some(TokenData {
            id_token: IdTokenInfo {
                email: Some(email.to_string()),
                chatgpt_plan_type: Some(AuthPlanType::Known(KnownPlan::Plus)),
                chatgpt_user_id: Some(format!("user-{account_id}")),
                chatgpt_account_id: Some(account_id.to_string()),
                chatgpt_account_is_fedramp: false,
                raw_jwt,
            },
            access_token: format!("access-{account_id}"),
            refresh_token: format!("refresh-{account_id}"),
            account_id: Some(account_id.to_string()),
        }),
        last_refresh: Some(Utc::now()),
        agent_identity: None,
    }
}

fn jwt_for_account(account_id: &str, email: &str) -> String {
    let encode = |bytes: &[u8]| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
    let header_b64 = encode(br#"{"alg":"none","typ":"JWT"}"#);
    let payload_b64 = encode(
        serde_json::to_string(&serde_json::json!({
            "email": email,
            "https://api.openai.com/auth": {
                "chatgpt_account_id": account_id,
                "chatgpt_plan_type": "plus",
                "chatgpt_user_id": format!("user-{account_id}"),
            }
        }))
        .expect("test payload should serialize")
        .as_bytes(),
    );
    format!("{header_b64}.{payload_b64}.sig")
}

fn snapshot(
    primary_used_percent: Option<f64>,
    secondary_used_percent: Option<f64>,
    credits: Option<CreditsSnapshot>,
    rate_limit_reached_type: Option<RateLimitReachedType>,
) -> RateLimitSnapshot {
    RateLimitSnapshot {
        limit_id: None,
        limit_name: None,
        primary: primary_used_percent.map(window),
        secondary: secondary_used_percent.map(window),
        credits,
        plan_type: None,
        rate_limit_reached_type,
    }
}

fn window(used_percent: f64) -> RateLimitWindow {
    RateLimitWindow {
        used_percent,
        window_minutes: None,
        resets_at: None,
    }
}
