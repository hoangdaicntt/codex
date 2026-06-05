use base64::Engine;
use chrono::Utc;
use codex_app_server_protocol::AuthMode;
use codex_config::types::AuthCredentialsStoreMode;
use codex_protocol::auth::KnownPlan;
use codex_protocol::auth::PlanType as AuthPlanType;
use codex_protocol::protocol::RateLimitReachedType;
use codex_protocol::protocol::RateLimitSnapshot;
use codex_protocol::protocol::RateLimitWindow;
use pretty_assertions::assert_eq;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use tempfile::tempdir;

use crate::auth::AuthDotJson;
use crate::auth::load_auth_dot_json;
use crate::auth::multi_account::AccountId;
use crate::auth::multi_account::AccountLimitSnapshots;
use crate::auth::multi_account::AccountsIndex;
use crate::auth::multi_account::AccountsStore;
use crate::auth::multi_account::LimitClassification;
use crate::auth::multi_account::ResolvedStoredAccountAuth;
use crate::auth::multi_account::account_display_label;
use crate::auth::multi_account::account_id_at_index;
use crate::auth::multi_account::classify_rate_limit_snapshot;
use crate::auth::multi_account::display_rows;
use crate::auth::save_auth;
use crate::token_data::IdTokenInfo;
use crate::token_data::TokenData;

#[test]
fn upsert_active_auth_writes_auth_snapshots_only() -> anyhow::Result<()> {
    let codex_home = tempdir()?;
    let store = AccountsStore::new(codex_home.path().to_path_buf());

    let account_id = store.upsert_active_auth(chatgpt_auth("account-a", "a@example.com"))?;

    assert_eq!(account_id, AccountId::from("account-a"));
    let index = store.load()?;
    assert_eq!(index.accounts[0].email.as_deref(), Some("a@example.com"));

    let raw_json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(store.path())?)?;
    let account = &raw_json["accounts"][0];
    assert!(account.get("auth").is_some());
    assert!(account.get("email").is_none());
    assert!(account.get("lastRateLimits").is_none());
    assert!(account.get("lastAuthFailure").is_none());
    Ok(())
}

#[test]
fn legacy_accounts_json_metadata_is_accepted() -> anyhow::Result<()> {
    let legacy_json = serde_json::json!({
        "version": 1,
        "activeAccountId": "account-a",
        "accounts": [
            {
                "accountId": "account-a",
                "email": "legacy@example.com",
                "planType": "plus",
                "lastLimitState": {
                    "kind": "nearLimit",
                    "recordedAt": "2026-05-28T00:00:00Z"
                },
                "lastAuthFailure": {
                    "kind": "refreshFailed",
                    "recordedAt": "2026-05-28T00:00:00Z",
                    "message": "old failure"
                },
                "auth": chatgpt_auth("account-a", "a@example.com")
            }
        ]
    });

    let index: AccountsIndex = serde_json::from_value(legacy_json)?;

    assert_eq!(index.accounts.len(), 1);
    assert_eq!(index.accounts[0].email.as_deref(), Some("a@example.com"));
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
fn switch_active_account_updates_auth_json() -> anyhow::Result<()> {
    let codex_home = tempdir()?;
    let store = AccountsStore::new(codex_home.path().to_path_buf());
    store.upsert_active_auth(chatgpt_auth("account-a", "a@example.com"))?;
    store.upsert_active_auth(chatgpt_auth("account-b", "b@example.com"))?;

    let changed =
        store.switch_active_account(&AccountId::from("account-a"), AuthCredentialsStoreMode::File)?;

    assert!(changed);
    let auth = load_auth_dot_json(codex_home.path(), AuthCredentialsStoreMode::File)?
        .expect("auth.json should exist");
    assert_eq!(
        auth.tokens.and_then(|tokens| tokens.account_id),
        Some("account-a".to_string())
    );
    Ok(())
}

#[test]
fn next_saved_account_wraps_and_skips_failed_accounts() -> anyhow::Result<()> {
    let codex_home = tempdir()?;
    let store = AccountsStore::new(codex_home.path().to_path_buf());
    for account in ["a", "b", "c", "d"] {
        store.upsert_active_auth(chatgpt_auth(account, &format!("{account}@example.com")))?;
    }
    let mut index = store.load()?;
    index.active_account_id = Some(AccountId::from("c"));
    store.save(&index)?;

    assert_eq!(
        store.next_saved_account(&["c".to_string()], None)?,
        Some(AccountId::from("d"))
    );

    index.active_account_id = Some(AccountId::from("d"));
    store.save(&index)?;
    assert_eq!(
        store.next_saved_account(&["d".to_string()], None)?,
        Some(AccountId::from("a"))
    );
    assert_eq!(
        store.next_saved_account(
            &["d".to_string(), "a".to_string(), "b".to_string(), "c".to_string()],
            None
        )?,
        None
    );
    Ok(())
}

#[test]
fn account_helpers_use_one_based_indexes() -> anyhow::Result<()> {
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
    assert_eq!(
        account_display_label(&accounts, &AccountId::from("account-b")),
        Some("2. b@example.com".to_string())
    );
    Ok(())
}

#[test]
fn display_rows_show_unknown_limits_without_snapshots() -> anyhow::Result<()> {
    let codex_home = tempdir()?;
    let store = AccountsStore::new(codex_home.path().to_path_buf());
    store.upsert_active_auth(chatgpt_auth("account-a", "a@example.com"))?;
    let index = store.load()?;

    let rows = display_rows(
        &index.accounts,
        index.active_account_id.as_ref(),
        &AccountLimitSnapshots::new(),
        Utc::now(),
    );

    assert_eq!(rows.len(), 1);
    assert!(rows[0].line.contains("5h unknown"));
    assert!(rows[0].line.contains("Week unknown"));
    Ok(())
}

#[test]
fn display_rows_use_transient_limit_snapshots() -> anyhow::Result<()> {
    let codex_home = tempdir()?;
    let store = AccountsStore::new(codex_home.path().to_path_buf());
    store.upsert_active_auth(chatgpt_auth("account-a", "a@example.com"))?;
    let index = store.load()?;
    let mut snapshots = AccountLimitSnapshots::new();
    snapshots.insert(
        AccountId::from("account-a"),
        snapshot(Some(12.5), Some(44.0), None),
    );

    let rows = display_rows(
        &index.accounts,
        index.active_account_id.as_ref(),
        &snapshots,
        Utc::now(),
    );

    assert!(rows[0].line.contains("5h 88%/-"));
    assert!(rows[0].line.contains("Week 56%/-"));
    Ok(())
}

#[test]
fn classify_rate_limit_snapshot_distinguishes_near_limit_and_exhausted() {
    assert_eq!(
        classify_rate_limit_snapshot(&snapshot(Some(95.0), None, None)),
        LimitClassification::NearLimit { resets_at: None }
    );
    assert_eq!(
        classify_rate_limit_snapshot(&snapshot(
            Some(10.0),
            None,
            Some(RateLimitReachedType::RateLimitReached)
        )),
        LimitClassification::Exhausted { resets_at: None }
    );
}

#[tokio::test]
async fn refresh_account_limits_for_display_uses_access_tokens_without_writing_store(
) -> anyhow::Result<()> {
    let codex_home = tempdir()?;
    let store = AccountsStore::new(codex_home.path().to_path_buf());
    store.upsert_active_auth(chatgpt_auth("account-a", "a@example.com"))?;
    store.upsert_active_auth(expired_access_auth("account-b", "b@example.com"))?;
    let before = std::fs::read_to_string(store.path())?;
    let calls = Arc::new(AtomicUsize::new(0));
    let calls_for_fetch = Arc::clone(&calls);

    let snapshots = store
        .refresh_account_limits_for_display(
            move |auth: ResolvedStoredAccountAuth| {
                let calls_for_fetch = Arc::clone(&calls_for_fetch);
                async move {
                    calls_for_fetch.fetch_add(1, Ordering::SeqCst);
                    assert_eq!(auth.account_id, "account-a");
                    Some(snapshot(Some(25.0), None, None))
                }
            },
            |_, _| {},
        )
        .await;

    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(snapshots.contains_key(&AccountId::from("account-a")));
    assert_eq!(std::fs::read_to_string(store.path())?, before);
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
            access_token: jwt_with_exp(Utc::now() + chrono::Duration::hours(1)),
            refresh_token: format!("refresh-{account_id}"),
            account_id: Some(account_id.to_string()),
        }),
        last_refresh: Some(Utc::now()),
        agent_identity: None,
    }
}

fn expired_access_auth(account_id: &str, email: &str) -> AuthDotJson {
    let mut auth = chatgpt_auth(account_id, email);
    auth.tokens.as_mut().expect("tokens").access_token =
        jwt_with_exp(Utc::now() - chrono::Duration::hours(1));
    auth
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

fn jwt_with_exp(expires_at: chrono::DateTime<Utc>) -> String {
    let encode = |bytes: &[u8]| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
    let header_b64 = encode(br#"{"alg":"none","typ":"JWT"}"#);
    let payload_b64 = encode(
        serde_json::to_string(&serde_json::json!({
            "exp": expires_at.timestamp(),
        }))
        .expect("test payload should serialize")
        .as_bytes(),
    );
    format!("{header_b64}.{payload_b64}.sig")
}

fn snapshot(
    primary_used_percent: Option<f64>,
    secondary_used_percent: Option<f64>,
    rate_limit_reached_type: Option<RateLimitReachedType>,
) -> RateLimitSnapshot {
    RateLimitSnapshot {
        limit_id: None,
        limit_name: None,
        primary: primary_used_percent.map(window),
        secondary: secondary_used_percent.map(window),
        credits: None,
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
