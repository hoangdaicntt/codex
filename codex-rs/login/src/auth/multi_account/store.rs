use std::fs::OpenOptions;
use std::io::Read;
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::path::PathBuf;

use chrono::DateTime;
use chrono::Utc;
use codex_config::types::AuthCredentialsStoreMode;
use codex_protocol::account::PlanType as AccountPlanType;
use codex_protocol::protocol::RateLimitSnapshot;
use serde::Deserialize;
use serde::Serialize;

use crate::auth::AuthDotJson;
use crate::auth::load_auth_dot_json;
use crate::auth::save_auth;

use super::metadata::AccountMetadata;
use super::selection;
use super::selection::SelectionReason;

const ACCOUNTS_JSON_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(transparent)]
pub struct AccountId(pub String);

impl AccountId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for AccountId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for AccountId {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

impl std::fmt::Display for AccountId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountsIndex {
    pub version: u32,
    pub active_account_id: Option<AccountId>,
    pub accounts: Vec<StoredAccount>,
}

impl Default for AccountsIndex {
    fn default() -> Self {
        Self {
            version: ACCOUNTS_JSON_VERSION,
            active_account_id: None,
            accounts: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredAccount {
    pub account_id: AccountId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_type: Option<AccountPlanType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    #[serde(default = "Utc::now")]
    pub created_at: DateTime<Utc>,
    pub last_used_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_limit_state: Option<StoredLimitState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_auth_failure: Option<StoredAuthFailureState>,
    pub auth: AuthDotJson,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum StoredAuthFailureKind {
    RefreshFailed,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredAuthFailureState {
    pub kind: StoredAuthFailureKind,
    pub recorded_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum StoredLimitKind {
    NearLimit,
    Exhausted,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredLimitState {
    pub kind: StoredLimitKind,
    pub recorded_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<RateLimitSnapshot>,
}

impl StoredLimitState {
    pub fn is_active(&self, now: DateTime<Utc>) -> bool {
        self.resets_at.is_none_or(|resets_at| resets_at > now)
    }
}

#[derive(Clone, Debug)]
pub struct AccountsStore {
    codex_home: PathBuf,
}

impl AccountsStore {
    pub fn new(codex_home: PathBuf) -> Self {
        Self { codex_home }
    }

    pub fn path(&self) -> PathBuf {
        self.codex_home.join("accounts.json")
    }

    pub fn load(&self) -> std::io::Result<AccountsIndex> {
        let path = self.path();
        let mut file = match std::fs::File::open(path) {
            Ok(file) => file,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Ok(AccountsIndex::default());
            }
            Err(err) => return Err(err),
        };
        let mut contents = String::new();
        file.read_to_string(&mut contents)?;
        serde_json::from_str(&contents).map_err(std::io::Error::other)
    }

    pub fn save(&self, index: &AccountsIndex) -> std::io::Result<()> {
        let path = self.path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temp_path = temp_path_for(&path);
        let json_data = serde_json::to_string_pretty(index).map_err(std::io::Error::other)?;
        let mut options = OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            options.mode(0o600);
        }
        {
            let mut file = options.open(&temp_path)?;
            file.write_all(json_data.as_bytes())?;
            file.flush()?;
        }
        std::fs::rename(temp_path, path)?;
        Ok(())
    }

    pub fn import_active_auth_if_missing(
        &self,
        auth_credentials_store_mode: AuthCredentialsStoreMode,
    ) -> std::io::Result<AccountsIndex> {
        let mut index = self.load()?;
        let Some(auth) = load_auth_dot_json(&self.codex_home, auth_credentials_store_mode)? else {
            return Ok(index);
        };
        if let Some(metadata) = AccountMetadata::from_auth(&auth)
            && !index
                .accounts
                .iter()
                .any(|account| account.account_id.as_str() == metadata.account_id)
        {
            upsert_auth(&mut index, auth, Utc::now())?;
            self.save(&index)?;
        }
        Ok(index)
    }

    pub fn upsert_active_auth(&self, auth: AuthDotJson) -> std::io::Result<AccountId> {
        let mut index = self.load()?;
        let account_id = upsert_auth(&mut index, auth, Utc::now())?;
        self.save(&index)?;
        Ok(account_id)
    }

    pub fn switch_active_account(
        &self,
        account_id: &AccountId,
        auth_credentials_store_mode: AuthCredentialsStoreMode,
    ) -> std::io::Result<bool> {
        let mut index = self.load()?;
        let Some(account) = index
            .accounts
            .iter_mut()
            .find(|account| &account.account_id == account_id)
        else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("account {account_id} was not found"),
            ));
        };
        account.last_used_at = Utc::now();
        let auth = account.auth.clone();
        let changed = index.active_account_id.as_ref() != Some(account_id);
        index.active_account_id = Some(account_id.clone());
        self.save(&index)?;
        save_active_auth_json(&self.codex_home, &auth, auth_credentials_store_mode)?;
        Ok(changed)
    }

    pub fn sync_active_auth_json(
        &self,
        auth_credentials_store_mode: AuthCredentialsStoreMode,
    ) -> std::io::Result<bool> {
        let index = self.load()?;
        let Some(active_account_id) = index.active_account_id.as_ref() else {
            return Ok(false);
        };
        let Some(account) = index
            .accounts
            .iter()
            .find(|account| &account.account_id == active_account_id)
        else {
            return Ok(false);
        };
        save_active_auth_json(&self.codex_home, &account.auth, auth_credentials_store_mode)?;
        Ok(true)
    }

    pub fn active_limit_kind(
        &self,
        now: DateTime<Utc>,
    ) -> std::io::Result<Option<StoredLimitKind>> {
        let index = self.load()?;
        let Some(active_account_id) = index.active_account_id.as_ref() else {
            return Ok(None);
        };
        Ok(index
            .accounts
            .iter()
            .find(|account| &account.account_id == active_account_id)
            .and_then(|account| account.last_limit_state.as_ref())
            .filter(|state| state.is_active(now))
            .map(|state| state.kind))
    }

    pub fn mark_active_from_snapshot(
        &self,
        snapshot: RateLimitSnapshot,
    ) -> std::io::Result<Option<StoredLimitKind>> {
        let mut index = self.load()?;
        let Some(active_account_id) = index.active_account_id.clone() else {
            return Ok(None);
        };
        let Some(account) = index
            .accounts
            .iter_mut()
            .find(|account| account.account_id == active_account_id)
        else {
            return Ok(None);
        };
        let now = Utc::now();
        let Some(limit_state) = selection::limit_state_from_snapshot(snapshot, now) else {
            account.last_limit_state = None;
            self.save(&index)?;
            return Ok(None);
        };
        let kind = limit_state.kind;
        account.last_limit_state = Some(limit_state);
        self.save(&index)?;
        Ok(Some(kind))
    }

    pub fn mark_active_exhausted(&self, resets_at: Option<DateTime<Utc>>) -> std::io::Result<()> {
        let mut index = self.load()?;
        let Some(active_account_id) = index.active_account_id.clone() else {
            return Ok(());
        };
        let Some(account) = index
            .accounts
            .iter_mut()
            .find(|account| account.account_id == active_account_id)
        else {
            return Ok(());
        };
        account.last_limit_state = Some(StoredLimitState {
            kind: StoredLimitKind::Exhausted,
            recorded_at: Utc::now(),
            resets_at,
            snapshot: None,
        });
        self.save(&index)
    }

    pub fn mark_active_exhausted_from_snapshot(
        &self,
        snapshot: RateLimitSnapshot,
        fallback_resets_at: Option<DateTime<Utc>>,
    ) -> std::io::Result<()> {
        let mut index = self.load()?;
        let Some(active_account_id) = index.active_account_id.clone() else {
            return Ok(());
        };
        let Some(account) = index
            .accounts
            .iter_mut()
            .find(|account| account.account_id == active_account_id)
        else {
            return Ok(());
        };
        account.last_limit_state = Some(StoredLimitState {
            kind: StoredLimitKind::Exhausted,
            recorded_at: Utc::now(),
            resets_at: selection::snapshot_reset_time(&snapshot).or(fallback_resets_at),
            snapshot: Some(snapshot),
        });
        self.save(&index)
    }

    pub fn mark_active_refresh_failed(&self, message: Option<String>) -> std::io::Result<()> {
        let mut index = self.load()?;
        let Some(active_account_id) = index.active_account_id.clone() else {
            return Ok(());
        };
        let Some(account) = index
            .accounts
            .iter_mut()
            .find(|account| account.account_id == active_account_id)
        else {
            return Ok(());
        };
        account.last_auth_failure = Some(StoredAuthFailureState {
            kind: StoredAuthFailureKind::RefreshFailed,
            recorded_at: Utc::now(),
            message,
        });
        self.save(&index)
    }

    pub fn next_available_account(
        &self,
        _reason: SelectionReason,
        forced_workspace_ids: Option<&[String]>,
    ) -> std::io::Result<Option<AccountId>> {
        let index = self.load()?;
        let Some(active_account_id) = index.active_account_id.as_ref() else {
            return Ok(None);
        };
        Ok(selection::select_next_available_account(
            &index,
            active_account_id,
            forced_workspace_ids,
            Utc::now(),
        ))
    }
}

fn upsert_auth(
    index: &mut AccountsIndex,
    auth: AuthDotJson,
    now: DateTime<Utc>,
) -> std::io::Result<AccountId> {
    let metadata = AccountMetadata::from_auth(&auth).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "multi-account store only supports managed ChatGPT auth",
        )
    })?;
    let account_id = AccountId(metadata.account_id);

    if let Some(account) = index
        .accounts
        .iter_mut()
        .find(|account| account.account_id == account_id)
    {
        account.email = metadata.email;
        account.plan_type = metadata.plan_type;
        account.workspace_id = metadata.workspace_id;
        account.last_used_at = now;
        account.last_auth_failure = None;
        account.auth = auth;
    } else {
        index.accounts.push(StoredAccount {
            account_id: account_id.clone(),
            email: metadata.email,
            plan_type: metadata.plan_type,
            workspace_id: metadata.workspace_id,
            created_at: now,
            last_used_at: now,
            last_limit_state: None,
            last_auth_failure: None,
            auth,
        });
    }

    index.version = ACCOUNTS_JSON_VERSION;
    index.active_account_id = Some(account_id.clone());
    Ok(account_id)
}

fn temp_path_for(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("accounts.json");
    path.with_file_name(format!("{file_name}.tmp.{}", std::process::id()))
}

fn save_active_auth_json(
    codex_home: &Path,
    auth: &AuthDotJson,
    auth_credentials_store_mode: AuthCredentialsStoreMode,
) -> std::io::Result<()> {
    save_auth(codex_home, auth, auth_credentials_store_mode)?;
    if !matches!(
        auth_credentials_store_mode,
        AuthCredentialsStoreMode::File | AuthCredentialsStoreMode::Ephemeral
    ) {
        save_auth(codex_home, auth, AuthCredentialsStoreMode::File)?;
    }
    Ok(())
}
