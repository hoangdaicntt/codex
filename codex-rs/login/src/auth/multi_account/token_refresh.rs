use chrono::Utc;
use codex_app_server_protocol::AuthMode;
use codex_protocol::protocol::RateLimitSnapshot;
use std::collections::HashMap;
use std::future::Future;
use tokio::task::JoinSet;

use crate::auth::AuthDotJson;
use crate::token_data::parse_jwt_expiration;

use super::store::AccountId;
use super::store::AccountsStore;

pub type AccountLimitSnapshots = HashMap<AccountId, RateLimitSnapshot>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedStoredAccountAuth {
    pub access_token: String,
    pub account_id: String,
    pub is_fedramp_account: bool,
}

impl AccountsStore {
    pub async fn refresh_account_limits_for_display<F, Fut, P>(
        &self,
        fetch_rate_limits: F,
        mut progress: P,
    ) -> AccountLimitSnapshots
    where
        F: Fn(ResolvedStoredAccountAuth) -> Fut + Clone + Send + 'static,
        Fut: Future<Output = Option<RateLimitSnapshot>> + Send + 'static,
        P: FnMut(usize, usize),
    {
        let Ok(index) = self.load() else {
            return AccountLimitSnapshots::new();
        };
        let total = index.accounts.len();
        let mut loaded = 0;
        let mut fetches = JoinSet::new();
        progress(0, total);

        for account in index.accounts {
            let Some(auth) = resolve_account_auth_for_display(&account.auth) else {
                loaded += 1;
                progress(loaded, total);
                continue;
            };
            let account_id = account.account_id;
            let fetch_rate_limits = fetch_rate_limits.clone();
            fetches.spawn(async move {
                let snapshot = fetch_rate_limits(auth).await;
                (account_id, snapshot)
            });
        }

        let mut snapshots = AccountLimitSnapshots::new();
        while let Some(result) = fetches.join_next().await {
            if let Ok((account_id, Some(snapshot))) = result {
                snapshots.insert(account_id, snapshot);
            }
            loaded += 1;
            progress(loaded, total);
        }
        snapshots
    }
}

pub fn resolve_account_auth_for_display(auth: &AuthDotJson) -> Option<ResolvedStoredAccountAuth> {
    if !is_managed_chatgpt_auth(auth) {
        return None;
    }
    let tokens = auth.tokens.as_ref()?;
    if parse_jwt_expiration(&tokens.access_token)
        .ok()
        .flatten()
        .is_some_and(|expires_at| expires_at <= Utc::now())
    {
        return None;
    }
    let account_id = tokens
        .account_id
        .clone()
        .or_else(|| tokens.id_token.chatgpt_account_id.clone())?;
    Some(ResolvedStoredAccountAuth {
        access_token: tokens.access_token.clone(),
        account_id,
        is_fedramp_account: tokens.id_token.chatgpt_account_is_fedramp,
    })
}

fn is_managed_chatgpt_auth(auth: &AuthDotJson) -> bool {
    matches!(auth.auth_mode, None | Some(AuthMode::Chatgpt)) && auth.tokens.is_some()
}
