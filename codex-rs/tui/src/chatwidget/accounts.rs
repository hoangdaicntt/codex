use super::*;
use crate::app_event::AccountRemoveResult;
use crate::app_event::AccountSwitchResult;
use crate::bottom_pane::SelectionRowDisplay;
use codex_backend_client::Client as BackendClient;
use codex_login::AuthManager;
use codex_login::RefreshTokenError;
use codex_login::auth::multi_account::AccountId;
use codex_login::auth::multi_account::AccountsIndex;
use codex_login::auth::multi_account::AccountsStore;
use codex_login::auth::multi_account::display_rows;
use codex_model_provider::BearerAuthProvider;
use codex_protocol::protocol::RateLimitSnapshot as CoreRateLimitSnapshot;
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
        let config = self.config.clone();
        let account_id = account.account_id.clone();
        let tx = self.app_event_tx.clone();
        tokio::spawn(async move {
            let result = remove_account_for_picker(config, account_id).await;
            tx.send(AppEvent::AccountRemoveFinished { result });
        });
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
            &self.config,
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
    let Ok(index) = store.load() else {
        return;
    };
    let total = index.accounts.len();
    if let Some(tx) = tx {
        tx.send(AppEvent::AccountsPickerLoadProgress { loaded: 0, total });
    }
    for (offset, account) in index.accounts.into_iter().enumerate() {
        let auth = match store
            .resolve_account_auth_for_usage(
                &account.account_id,
                config.cli_auth_credentials_store_mode,
            )
            .await
        {
            Ok(Some(auth)) => auth,
            Ok(None) => {
                send_accounts_picker_progress(tx, offset + 1, total);
                continue;
            }
            Err(RefreshTokenError::Permanent(err)) => {
                let _ =
                    store.mark_account_refresh_failed(&account.account_id, Some(err.to_string()));
                send_accounts_picker_progress(tx, offset + 1, total);
                continue;
            }
            Err(RefreshTokenError::Transient(_)) => {
                send_accounts_picker_progress(tx, offset + 1, total);
                continue;
            }
        };

        let Ok(client) = BackendClient::new(config.chatgpt_base_url.clone()) else {
            send_accounts_picker_progress(tx, offset + 1, total);
            continue;
        };
        let client = client.with_auth_provider(Arc::new(BearerAuthProvider {
            token: Some(auth.access_token),
            account_id: Some(auth.account_id),
            is_fedramp_account: auth.is_fedramp_account,
        }));
        let Ok(snapshots) = client.get_rate_limits_many().await else {
            send_accounts_picker_progress(tx, offset + 1, total);
            continue;
        };
        let Some(snapshot) = preferred_account_limit_snapshot(snapshots) else {
            send_accounts_picker_progress(tx, offset + 1, total);
            continue;
        };
        let _ = store.mark_account_rate_limits(&account.account_id, snapshot);
        send_accounts_picker_progress(tx, offset + 1, total);
    }
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

fn preferred_account_limit_snapshot(
    snapshots: Vec<CoreRateLimitSnapshot>,
) -> Option<CoreRateLimitSnapshot> {
    snapshots
        .iter()
        .find(|snapshot| snapshot.limit_id.as_deref() == Some("codex"))
        .cloned()
        .or_else(|| snapshots.into_iter().next())
}

fn account_selection_params(
    config: &Config,
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
            let config = config.clone();
            let email = account.email.clone();
            let switch_message_email = email.clone().unwrap_or_else(|| "-".to_string());
            let plan_type = account.plan_type;
            let status_account_display = Some(StatusAccountDisplay::ChatGpt {
                email,
                plan: plan_type.map(|plan_type| format!("{plan_type:?}")),
            });
            let actions: Vec<SelectionAction> = vec![Box::new(move |tx| {
                let tx = tx.clone();
                let config = config.clone();
                let status_account_display = status_account_display.clone();
                let switch_message_email = switch_message_email.clone();
                tokio::spawn(async move {
                    let result = match AuthManager::shared_from_config(
                        &config, /*enable_codex_api_key_env*/ false,
                    )
                    .await
                    .switch_account_by_index(row_index)
                    .await
                    {
                        Ok(_) => Ok(AccountSwitchResult {
                            message: format!(
                                "Switched active account to {row_index}. {switch_message_email}."
                            ),
                            status_account_display,
                            plan_type,
                            has_chatgpt_account: true,
                        }),
                        Err(err) => Err(format!("Account switch failed: {err}")),
                    };
                    tx.send(AppEvent::AccountSwitchFinished { result });
                });
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

async fn remove_account_for_picker(
    config: Config,
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
    AuthManager::shared_from_config(&config, /*enable_codex_api_key_env*/ false)
        .await
        .reload()
        .await;
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
    use chrono::TimeZone;
    use chrono::Utc;
    use codex_app_server_protocol::AuthMode;
    use codex_login::AuthDotJson;
    use codex_login::auth::multi_account::AccountId;
    use codex_login::auth::multi_account::StoredAccount;
    use codex_protocol::account::PlanType;

    use crate::legacy_core::config::ConfigBuilder;

    use super::*;

    #[tokio::test]
    async fn account_selection_params_renders_account_rows() {
        let codex_home = tempfile::tempdir().unwrap();
        let config = ConfigBuilder::default()
            .codex_home(codex_home.path().to_path_buf())
            .build()
            .await
            .expect("config");
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

        let params = account_selection_params(&config, index, None, accounts_picker_footer_hint());

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
}
