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
use serde::Deserialize;
use serde::Deserializer;
use serde::Serialize;
use serde::Serializer;

use crate::auth::AuthDotJson;
use crate::auth::load_auth_dot_json;
use crate::auth::logout;
use crate::auth::save_auth;

use super::metadata::AccountMetadata;

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

#[derive(Clone, Debug, PartialEq)]
pub struct StoredAccount {
    pub account_id: AccountId,
    pub email: Option<String>,
    pub plan_type: Option<AccountPlanType>,
    pub workspace_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub last_used_at: DateTime<Utc>,
    pub auth: AuthDotJson,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredAccountWire {
    pub account_id: Option<AccountId>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub plan_type: Option<AccountPlanType>,
    #[serde(default)]
    pub workspace_id: Option<String>,
    #[serde(default)]
    pub created_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub last_used_at: Option<DateTime<Utc>>,
    pub auth: AuthDotJson,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StoredAccountSnapshot<'a> {
    auth: &'a AuthDotJson,
}

impl Serialize for StoredAccount {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        StoredAccountSnapshot { auth: &self.auth }.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for StoredAccount {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = StoredAccountWire::deserialize(deserializer)?;
        let now = Utc::now();
        let metadata = AccountMetadata::from_auth(&wire.auth);
        let account_id = metadata
            .as_ref()
            .map(|metadata| AccountId(metadata.account_id.clone()))
            .or(wire.account_id)
            .ok_or_else(|| serde::de::Error::custom("stored account is missing account id"))?;
        Ok(Self {
            account_id,
            email: metadata
                .as_ref()
                .and_then(|metadata| metadata.email.clone())
                .or(wire.email),
            plan_type: metadata
                .as_ref()
                .and_then(|metadata| metadata.plan_type)
                .or(wire.plan_type),
            workspace_id: metadata
                .as_ref()
                .and_then(|metadata| metadata.workspace_id.clone())
                .or(wire.workspace_id),
            created_at: wire.created_at.unwrap_or(now),
            last_used_at: wire.last_used_at.unwrap_or(now),
            auth: wire.auth,
        })
    }
}

#[derive(Clone, Debug)]
pub struct AccountsStore {
    codex_home: PathBuf,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RemoveAccountOutcome {
    pub removed_account: StoredAccount,
    pub new_active_account: Option<StoredAccount>,
    pub active_account_changed: bool,
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

    pub fn remove_account(
        &self,
        account_id: &AccountId,
        auth_credentials_store_mode: AuthCredentialsStoreMode,
    ) -> std::io::Result<RemoveAccountOutcome> {
        let mut index = self.load()?;
        let Some(removed_index) = index
            .accounts
            .iter()
            .position(|account| &account.account_id == account_id)
        else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("account {account_id} was not found"),
            ));
        };

        let removed_account = index.accounts.remove(removed_index);
        let removed_active = index.active_account_id.as_ref() == Some(account_id);
        let new_active_account = if removed_active {
            let next_index = if index.accounts.is_empty() {
                None
            } else {
                Some(removed_index.min(index.accounts.len() - 1))
            };
            next_index.map(|next_index| index.accounts[next_index].clone())
        } else {
            index
                .active_account_id
                .as_ref()
                .and_then(|active_account_id| {
                    index
                        .accounts
                        .iter()
                        .find(|account| &account.account_id == active_account_id)
                })
                .cloned()
        };

        if removed_active {
            index.active_account_id = new_active_account
                .as_ref()
                .map(|account| account.account_id.clone());
        }

        self.save(&index)?;
        if removed_active {
            if let Some(active_account) = new_active_account.as_ref() {
                save_active_auth_json(
                    &self.codex_home,
                    &active_account.auth,
                    auth_credentials_store_mode,
                )?;
            } else {
                logout(&self.codex_home, auth_credentials_store_mode)?;
                if !matches!(
                    auth_credentials_store_mode,
                    AuthCredentialsStoreMode::File | AuthCredentialsStoreMode::Ephemeral
                ) {
                    logout(&self.codex_home, AuthCredentialsStoreMode::File)?;
                }
            }
        }

        Ok(RemoveAccountOutcome {
            removed_account,
            new_active_account,
            active_account_changed: removed_active,
        })
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

    pub fn update_account_auth(
        &self,
        account_id: &AccountId,
        auth: AuthDotJson,
        auth_credentials_store_mode: AuthCredentialsStoreMode,
    ) -> std::io::Result<()> {
        let mut index = self.load()?;
        let active_account_id = index.active_account_id.clone();
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
        account.auth = auth.clone();
        self.save(&index)?;
        if active_account_id.as_ref() == Some(account_id) {
            save_active_auth_json(&self.codex_home, &auth, auth_credentials_store_mode)?;
        }
        Ok(())
    }

    pub fn next_saved_account(
        &self,
        skipped_account_ids: &[String],
        forced_workspace_ids: Option<&[String]>,
    ) -> std::io::Result<Option<AccountId>> {
        let index = self.load()?;
        let Some(active_account_id) = index.active_account_id.as_ref() else {
            return Ok(None);
        };
        let Some(active_index) = index
            .accounts
            .iter()
            .position(|account| &account.account_id == active_account_id)
        else {
            return Ok(None);
        };
        Ok(index
            .accounts
            .iter()
            .cycle()
            .skip(active_index + 1)
            .take(index.accounts.len().saturating_sub(1))
            .find(|account| {
                !skipped_account_ids
                    .iter()
                    .any(|skipped| skipped == account.account_id.as_str())
                    && forced_workspace_ids.is_none_or(|expected| {
                        account
                            .workspace_id
                            .as_ref()
                            .is_some_and(|workspace_id| expected.contains(workspace_id))
                    })
                    && AccountMetadata::from_auth(&account.auth).is_some()
            })
            .map(|account| account.account_id.clone()))
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
        account.auth = auth;
    } else {
        index.accounts.push(StoredAccount {
            account_id: account_id.clone(),
            email: metadata.email,
            plan_type: metadata.plan_type,
            workspace_id: metadata.workspace_id,
            created_at: now,
            last_used_at: now,
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
