use super::*;
use crate::app_event::AccountRemoveResult;
use crate::app_event::AccountSwitchResult;
use crate::bottom_pane::SelectionRowDisplay;
use codex_backend_client::Client as BackendClient;
use codex_login::AuthManager;
use codex_login::auth::multi_account::AccountId;
use codex_login::auth::multi_account::AccountsIndex;
use codex_login::auth::multi_account::AccountsStore;
use codex_login::auth::multi_account::account_id_at_index;
use codex_login::auth::multi_account::display_rows;
use codex_login::auth::multi_account::preferred_account_limit_snapshot;
use codex_model_provider::BearerAuthProvider;
use std::sync::Arc;

const ACCOUNTS_SELECTION_VIEW_ID: &str = "accounts";

impl ChatWidget {
    pub(crate) fn open_accounts_picker(&mut self) {
        self.show_accounts_picker_loading(None);
        let config = self.config.clone();
        let tx = self.app_event_tx.clone();
        tokio::spawn(async move {
            let result = load_accounts_picker_index(config, tx.clone()).await;
            tx.send(AppEvent::AccountsPickerLoaded { result });
        });
        self.request_redraw();
    }

    pub(crate) fn update_accounts_picker_loading(&mut self, loaded: usize, total: usize) {
        self.show_accounts_picker_loading(Some((loaded, total)));
    }

    pub(crate) fn handle_accounts_picker_delete_key(&mut self, key_event: KeyEvent) -> bool {
        if !matches!(
            key_event,
            KeyEvent {
                code: KeyCode::Delete,
                kind: KeyEventKind::Press | KeyEventKind::Repeat,
                ..
            }
        ) {
            return false;
        }
        if self
            .bottom_pane
            .selected_item_is_enabled_for_active_view(ACCOUNTS_SELECTION_VIEW_ID)
            != Some(true)
        {
            return false;
        }
        let Some(selected_idx) = self
            .bottom_pane
            .selected_index_for_active_view(ACCOUNTS_SELECTION_VIEW_ID)
        else {
            return false;
        };
        let store = AccountsStore::new(self.config.codex_home.to_path_buf());
        let Ok(index) = store.load() else {
            return false;
        };
        let Some(account) = index.accounts.get(selected_idx) else {
            return false;
        };
        let tx = self.app_event_tx.clone();
        let account_id = account.account_id.clone();
        tx.send(AppEvent::AccountRemoveRequested { account_id });
        true
    }

    pub(crate) fn show_accounts_picker(&mut self, index: AccountsIndex) {
        self.show_accounts_picker_with_selection(index, None);
    }

    pub(crate) fn show_accounts_picker_with_selection(
        &mut self,
        index: AccountsIndex,
        selected_idx: Option<usize>,
    ) {
        let params = account_selection_params(
            index,
            selected_idx,
            accounts_picker_footer_hint(),
        );
        if params.items.is_empty() {
            self.dismiss_accounts_picker();
            return;
        }

        let replaced = self
            .bottom_pane
            .replace_selection_view_if_active(ACCOUNTS_SELECTION_VIEW_ID, params);
        if replaced {
            self.request_redraw();
        }
    }

    pub(crate) fn dismiss_accounts_picker(&mut self) -> bool {
        self.bottom_pane
            .dismiss_active_view_if_id(ACCOUNTS_SELECTION_VIEW_ID)
    }

    fn show_accounts_picker_loading(&mut self, progress: Option<(usize, usize)>) {
        let label = match progress {
            Some((loaded, total)) => format!("Loading saved accounts {loaded}/{total}..."),
            None => "Loading saved accounts...".to_string(),
        };
        let params = SelectionViewParams {
            view_id: Some(ACCOUNTS_SELECTION_VIEW_ID),
            title: Some("Accounts".to_string()),
            footer_hint: Some(Line::from("Press esc to go back")),
            items: vec![SelectionItem {
                name: label,
                is_disabled: true,
                ..Default::default()
            }],
            ..Default::default()
        };
        if !self
            .bottom_pane
            .replace_selection_view_if_active(ACCOUNTS_SELECTION_VIEW_ID, params)
        {
            self.bottom_pane.show_selection_view(accounts_loading_params(progress));
        }
        self.request_redraw();
    }
}

fn accounts_loading_params(progress: Option<(usize, usize)>) -> SelectionViewParams {
    let label = match progress {
        Some((loaded, total)) => format!("Loading saved accounts {loaded}/{total}..."),
        None => "Loading saved accounts...".to_string(),
    };
    SelectionViewParams {
        view_id: Some(ACCOUNTS_SELECTION_VIEW_ID),
        title: Some("Accounts".to_string()),
        footer_hint: Some(Line::from("Press esc to go back")),
        items: vec![SelectionItem {
            name: label,
            is_disabled: true,
            ..Default::default()
        }],
        ..Default::default()
    }
}

async fn load_accounts_picker_index(
    config: Config,
    tx: crate::app_event_sender::AppEventSender,
) -> Result<AccountsIndex, String> {
    let store = AccountsStore::new(config.codex_home.to_path_buf());
    store
        .import_active_auth_if_missing(config.cli_auth_credentials_store_mode)
        .map_err(|err| format!("Failed to load saved accounts: {err}"))?;
    refresh_account_limits_for_display(&config, &store, Some(&tx)).await;
    store
        .load()
        .map_err(|err| format!("Failed to load saved accounts: {err}"))
}

async fn refresh_account_limits_for_display(
    config: &Config,
    store: &AccountsStore,
    tx: Option<&crate::app_event_sender::AppEventSender>,
) {
    let chatgpt_base_url = config.chatgpt_base_url.clone();
    store
        .refresh_account_limits_for_display(
            config.cli_auth_credentials_store_mode,
            move |auth| {
                let chatgpt_base_url = chatgpt_base_url.clone();
                async move {
                    let Ok(client) = BackendClient::new(chatgpt_base_url) else {
                        return None;
                    };
                    let client = client.with_auth_provider(Arc::new(BearerAuthProvider {
                        token: Some(auth.access_token),
                        account_id: Some(auth.account_id),
                        is_fedramp_account: auth.is_fedramp_account,
                    }));
                    client
                        .get_rate_limits_many()
                        .await
                        .ok()
                        .and_then(preferred_account_limit_snapshot)
                }
            },
            |loaded, total| send_accounts_picker_progress(tx, loaded, total),
        )
        .await;
}

fn send_accounts_picker_progress(
    tx: Option<&crate::app_event_sender::AppEventSender>,
    loaded: usize,
    total: usize,
) {
    if let Some(tx) = tx {
        tx.send(AppEvent::AccountsPickerLoadProgress { loaded, total });
    }
}

fn account_selection_params(
    index: AccountsIndex,
    selected_idx: Option<usize>,
    footer_hint: Line<'static>,
) -> SelectionViewParams {
    let rows = display_rows(
        &index.accounts,
        index.active_account_id.as_ref(),
        chrono::Utc::now(),
    );
    let initial_selected_idx = selected_idx
        .filter(|selected_idx| *selected_idx < rows.len())
        .or_else(|| rows.iter().position(|row| row.is_active))
        .or(Some(0));
    let items = rows
        .into_iter()
        .zip(index.accounts)
        .map(|(row, account)| {
            let row_index = row.index;
            let actions: Vec<SelectionAction> = vec![Box::new(move |tx| {
                tx.send(AppEvent::AccountSwitchRequested { index: row_index });
            })];

            SelectionItem {
                name: row.line,
                actions,
                dismiss_on_select: true,
                search_value: account.email,
                ..Default::default()
            }
        })
        .collect();

    SelectionViewParams {
        view_id: Some(ACCOUNTS_SELECTION_VIEW_ID),
        title: Some("Accounts".to_string()),
        subtitle: Some("Choose active ChatGPT account".to_string()),
        footer_hint: Some(footer_hint),
        items,
        initial_selected_idx,
        row_display: SelectionRowDisplay::Wrapped,
        ..Default::default()
    }
}

pub(crate) async fn switch_account_for_picker(
    config: Config,
    auth_manager: Arc<AuthManager>,
    index: usize,
) -> Result<AccountSwitchResult, String> {
    let store = AccountsStore::new(config.codex_home.to_path_buf());
    let accounts_index = store
        .load()
        .map_err(|err| format!("Failed to load saved accounts: {err}"))?;
    let account_id = account_id_at_index(&accounts_index.accounts, index)
        .map_err(|err| format!("Account switch failed: {err}"))?;
    let account = accounts_index
        .accounts
        .iter()
        .find(|account| account.account_id == account_id)
        .expect("account_id_at_index returned an existing account id");
    let email = account.email.clone();
    let switch_message_email = email.clone().unwrap_or_else(|| "-".to_string());
    let plan_type = account.plan_type;
    let status_account_display = Some(StatusAccountDisplay::ChatGpt {
        email,
        plan: plan_type.map(|plan_type| format!("{plan_type:?}")),
    });
    auth_manager
        .switch_account_by_index(index)
        .await
        .map_err(|err| format!("Account switch failed: {err}"))?;
    Ok(AccountSwitchResult {
        message: format!("Switched active account to {index}. {switch_message_email}."),
        status_account_display,
        plan_type,
        has_chatgpt_account: true,
    })
}

pub(crate) async fn remove_account_for_picker(
    config: Config,
    auth_manager: Arc<AuthManager>,
    account_id: AccountId,
) -> Result<AccountRemoveResult, String> {
    let store = AccountsStore::new(config.codex_home.to_path_buf());
    let before = store
        .load()
        .map_err(|err| format!("Failed to load saved accounts: {err}"))?;
    let removed_index = before
        .accounts
        .iter()
        .position(|account| account.account_id == account_id)
        .ok_or_else(|| format!("Account {account_id} was not found"))?;
    let outcome = store
        .remove_account(&account_id, config.cli_auth_credentials_store_mode)
        .map_err(|err| format!("Account remove failed: {err}"))?;
    auth_manager.reload().await;
    let accounts_index = store
        .load()
        .map_err(|err| format!("Failed to load saved accounts: {err}"))?;
    let selected_idx = if accounts_index.accounts.is_empty() {
        None
    } else {
        Some(removed_index.min(accounts_index.accounts.len() - 1))
    };
    let (status_account_display, plan_type, has_chatgpt_account) =
        if let Some(active_account) = outcome.new_active_account.as_ref() {
            let plan_type = active_account.plan_type;
            (
                Some(StatusAccountDisplay::ChatGpt {
                    email: active_account.email.clone(),
                    plan: plan_type.map(|plan_type| format!("{plan_type:?}")),
                }),
                plan_type,
                true,
            )
        } else {
            (None, None, false)
        };
    let removed_email = outcome
        .removed_account
        .email
        .as_deref()
        .unwrap_or("-");
    let message = match outcome.new_active_account.as_ref() {
        Some(active_account) if outcome.active_account_changed => {
            let active_email = active_account.email.as_deref().unwrap_or("-");
            format!("Removed account {removed_email}. Active account is now {active_email}.")
        }
        Some(_) => format!("Removed account {removed_email}."),
        None if outcome.active_account_changed => {
            format!("Removed account {removed_email}. No saved ChatGPT accounts.")
        }
        None => format!("Removed account {removed_email}."),
    };
    Ok(AccountRemoveResult {
        message,
        accounts_index,
        selected_idx,
        status_account_display,
        plan_type,
        has_chatgpt_account,
    })
}

fn accounts_picker_footer_hint() -> Line<'static> {
    Line::from("Press enter to confirm | del to remove | esc to go back")
}

#[cfg(test)]
mod tests {
    use base64::Engine;
    use chrono::TimeZone;
    use chrono::Utc;
    use codex_app_server_protocol::AuthMode;
    use codex_login::AuthDotJson;
    use codex_login::auth::multi_account::AccountId;
    use codex_login::auth::multi_account::AccountsStore;
    use codex_login::auth::multi_account::StoredAccount;
    use codex_login::token_data::IdTokenInfo;
    use codex_login::TokenData;
    use codex_model_provider::create_model_provider;
    use codex_model_provider::ModelProvider;
    use codex_protocol::auth::KnownPlan;
    use codex_protocol::auth::PlanType as AuthPlanType;
    use codex_protocol::account::PlanType;

    use crate::legacy_core::config::ConfigBuilder;

    use super::*;

    #[test]
    fn account_selection_params_renders_account_rows() {
        let account = StoredAccount {
            account_id: AccountId::from("account-a"),
            email: Some("a@example.com".to_string()),
            plan_type: Some(PlanType::Plus),
            workspace_id: None,
            created_at: Utc.with_ymd_and_hms(2026, 5, 28, 3, 30, 0).unwrap(),
            last_used_at: Utc::now(),
            last_limit_state: None,
            last_rate_limits: None,
            last_auth_failure: None,
            auth: AuthDotJson {
                auth_mode: Some(AuthMode::Chatgpt),
                openai_api_key: None,
                tokens: None,
                last_refresh: None,
                agent_identity: None,
            },
        };
        let index = AccountsIndex {
            version: 1,
            active_account_id: Some(AccountId::from("account-a")),
            accounts: vec![account],
        };

        let params = account_selection_params(index, None, accounts_picker_footer_hint());

        assert_eq!(params.items.len(), 1);
        assert!(params.items[0].name.contains("* 1. a@example.com"));
        assert!(params.items[0].name.contains("5h -"));
        assert!(params.items[0].name.contains("Week -"));
        assert!(params.items[0].name.contains("token exp -"));
        assert_eq!(
            params.footer_hint,
            Some(Line::from(
                "Press enter to confirm | del to remove | esc to go back"
            ))
        );
        assert_eq!(params.items[0].description, None);
        assert_eq!(params.initial_selected_idx, Some(0));
    }

    #[test]
    fn accounts_loading_params_renders_progress() {
        let params = accounts_loading_params(Some((1, 4)));

        assert_eq!(params.title, Some("Accounts".to_string()));
        assert_eq!(params.items.len(), 1);
        assert_eq!(params.items[0].name, "Loading saved accounts 1/4...");
        assert!(params.items[0].is_disabled);
    }

    #[tokio::test]
    async fn switch_account_for_picker_updates_passed_auth_manager() {
        let codex_home = tempfile::tempdir().unwrap();
        let config = ConfigBuilder::default()
            .codex_home(codex_home.path().to_path_buf())
            .build()
            .await
            .expect("config");
        let store = AccountsStore::new(config.codex_home.to_path_buf());
        store
            .upsert_active_auth(chatgpt_auth("account-a", "a@example.com"))
            .expect("upsert account a");
        store
            .upsert_active_auth(chatgpt_auth("account-b", "b@example.com"))
            .expect("upsert account b");
        let auth_manager =
            AuthManager::shared_from_config(&config, /*enable_codex_api_key_env*/ false).await;
        assert_eq!(
            auth_manager.active_account_id().as_deref(),
            Some("account-b")
        );

        let result = switch_account_for_picker(config.clone(), auth_manager.clone(), 1)
            .await
            .expect("switch account");

        assert_eq!(
            result.message,
            "Switched active account to 1. a@example.com."
        );
        assert_eq!(
            auth_manager.active_account_id().as_deref(),
            Some("account-a")
        );
    }

    #[tokio::test]
    async fn provider_with_session_auth_manager_uses_switched_account() {
        let codex_home = tempfile::tempdir().unwrap();
        let config = ConfigBuilder::default()
            .codex_home(codex_home.path().to_path_buf())
            .build()
            .await
            .expect("config");
        let store = AccountsStore::new(config.codex_home.to_path_buf());
        store
            .upsert_active_auth(chatgpt_auth("account-a", "a@example.com"))
            .expect("upsert account a");
        store
            .upsert_active_auth(chatgpt_auth("account-b", "b@example.com"))
            .expect("upsert account b");
        let auth_manager =
            AuthManager::shared_from_config(&config, /*enable_codex_api_key_env*/ false).await;
        let provider = create_model_provider(
            config.model_provider.clone(),
            Some(auth_manager.clone()),
        );

        switch_account_for_picker(config, auth_manager, 1)
            .await
            .expect("switch account");

        let auth = provider.auth().await.expect("provider auth");
        assert_eq!(auth.get_account_id().as_deref(), Some("account-a"));
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
}
