use super::*;
use crate::app_event::AccountSwitchResult;
use crate::bottom_pane::SelectionRowDisplay;
use codex_backend_client::Client as BackendClient;
use codex_login::AuthManager;
use codex_login::RefreshTokenError;
use codex_login::auth::multi_account::AccountsIndex;
use codex_login::auth::multi_account::AccountsStore;
use codex_login::auth::multi_account::display_rows;
use codex_model_provider::BearerAuthProvider;
use codex_protocol::protocol::RateLimitSnapshot as CoreRateLimitSnapshot;
use std::sync::Arc;

impl ChatWidget {
    pub(crate) fn open_accounts_picker(&mut self) {
        let config = self.config.clone();
        let tx = self.app_event_tx.clone();
        tokio::spawn(async move {
            let result = load_accounts_picker_index(config).await;
            tx.send(AppEvent::AccountsPickerLoaded { result });
        });
    }

    pub(crate) fn show_accounts_picker(&mut self, index: AccountsIndex) {
        let params = account_selection_params(
            &self.config,
            index,
            self.bottom_pane.standard_popup_hint_line(),
        );
        if params.items.is_empty() {
            self.add_info_message("No saved ChatGPT accounts.".to_string(), /*hint*/ None);
            return;
        }

        self.bottom_pane.show_selection_view(params);
        self.request_redraw();
    }
}

async fn load_accounts_picker_index(config: Config) -> Result<AccountsIndex, String> {
    let store = AccountsStore::new(config.codex_home.to_path_buf());
    store
        .import_active_auth_if_missing(config.cli_auth_credentials_store_mode)
        .map_err(|err| format!("Failed to load saved accounts: {err}"))?;
    refresh_account_limits_for_display(&config, &store).await;
    store
        .load()
        .map_err(|err| format!("Failed to load saved accounts: {err}"))
}

async fn refresh_account_limits_for_display(config: &Config, store: &AccountsStore) {
    let Ok(index) = store.load() else {
        return;
    };
    for account in index.accounts {
        let auth = match store
            .resolve_account_auth_for_usage(
                &account.account_id,
                config.cli_auth_credentials_store_mode,
            )
            .await
        {
            Ok(Some(auth)) => auth,
            Ok(None) => continue,
            Err(RefreshTokenError::Permanent(err)) => {
                let _ =
                    store.mark_account_refresh_failed(&account.account_id, Some(err.to_string()));
                continue;
            }
            Err(RefreshTokenError::Transient(_)) => continue,
        };

        let Ok(client) = BackendClient::new(config.chatgpt_base_url.clone()) else {
            continue;
        };
        let client = client.with_auth_provider(Arc::new(BearerAuthProvider {
            token: Some(auth.access_token),
            account_id: Some(auth.account_id),
            is_fedramp_account: auth.is_fedramp_account,
        }));
        let Ok(snapshots) = client.get_rate_limits_many().await else {
            continue;
        };
        let Some(snapshot) = preferred_account_limit_snapshot(snapshots) else {
            continue;
        };
        let _ = store.mark_account_rate_limits(&account.account_id, snapshot);
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
    footer_hint: Line<'static>,
) -> SelectionViewParams {
    let rows = display_rows(
        &index.accounts,
        index.active_account_id.as_ref(),
        chrono::Utc::now(),
    );
    let initial_selected_idx = rows.iter().position(|row| row.is_active).or(Some(0));
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
        view_id: Some("accounts"),
        title: Some("Accounts".to_string()),
        subtitle: Some("Choose active ChatGPT account".to_string()),
        footer_hint: Some(footer_hint),
        items,
        initial_selected_idx,
        row_display: SelectionRowDisplay::Wrapped,
        ..Default::default()
    }
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

        let params = account_selection_params(&config, index, Line::from(""));

        assert_eq!(params.items.len(), 1);
        assert!(params.items[0].name.contains("* 1. a@example.com"));
        assert!(params.items[0].name.contains("5h -"));
        assert!(params.items[0].name.contains("Week -"));
        assert_eq!(params.items[0].description, None);
        assert_eq!(params.initial_selected_idx, Some(0));
    }
}
