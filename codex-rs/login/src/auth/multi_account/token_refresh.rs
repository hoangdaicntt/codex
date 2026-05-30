use chrono::Utc;
use codex_app_server_protocol::AuthMode;
use codex_config::types::AuthCredentialsStoreMode;
use codex_protocol::auth::RefreshTokenFailedError;
use codex_protocol::auth::RefreshTokenFailedReason;
use codex_protocol::protocol::RateLimitSnapshot;
use reqwest::StatusCode;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use std::future::Future;

use crate::auth::AuthDotJson;
use crate::auth::CLIENT_ID;
use crate::auth::REFRESH_TOKEN_URL_OVERRIDE_ENV_VAR;
use crate::auth::RefreshTokenError;
use crate::auth::default_client::create_client;
use crate::auth::util::try_parse_error_message;
use crate::token_data::TokenData;
use crate::token_data::parse_chatgpt_jwt_claims;
use crate::token_data::parse_jwt_expiration;

use super::store::AccountId;
use super::store::AccountsStore;

const TOKEN_REFRESH_INTERVAL_DAYS: i64 = 8;
const REFRESH_TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
const REFRESH_TOKEN_EXPIRED_MESSAGE: &str = "Your access token could not be refreshed because your refresh token has expired. Please log out and sign in again.";
const REFRESH_TOKEN_REUSED_MESSAGE: &str = "Your access token could not be refreshed because your refresh token was already used. Please log out and sign in again.";
const REFRESH_TOKEN_INVALIDATED_MESSAGE: &str =
    "Your access token could not be refreshed because your refresh token was revoked. Please log out and sign in again.";
const REFRESH_TOKEN_UNKNOWN_MESSAGE: &str =
    "Your access token could not be refreshed. Please log out and sign in again.";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedStoredAccountAuth {
    pub access_token: String,
    pub account_id: String,
    pub is_fedramp_account: bool,
}

impl AccountsStore {
    pub async fn refresh_account_limits_for_display<F, Fut, P>(
        &self,
        auth_credentials_store_mode: AuthCredentialsStoreMode,
        mut fetch_rate_limits: F,
        mut progress: P,
    ) where
        F: FnMut(ResolvedStoredAccountAuth) -> Fut,
        Fut: Future<Output = Option<RateLimitSnapshot>>,
        P: FnMut(usize, usize),
    {
        let Ok(index) = self.load() else {
            return;
        };
        let total = index.accounts.len();
        progress(0, total);
        for (offset, account) in index.accounts.into_iter().enumerate() {
            let auth = match self
                .resolve_account_auth_for_usage(&account.account_id, auth_credentials_store_mode)
                .await
            {
                Ok(Some(auth)) => auth,
                Ok(None) => {
                    progress(offset + 1, total);
                    continue;
                }
                Err(RefreshTokenError::Permanent(err)) => {
                    let _ = self
                        .mark_account_refresh_failed(&account.account_id, Some(err.to_string()));
                    progress(offset + 1, total);
                    continue;
                }
                Err(RefreshTokenError::Transient(_)) => {
                    progress(offset + 1, total);
                    continue;
                }
            };

            if let Some(snapshot) = fetch_rate_limits(auth).await {
                let _ = self.mark_account_rate_limits(&account.account_id, snapshot);
            }
            progress(offset + 1, total);
        }
    }

    pub async fn resolve_account_auth_for_usage(
        &self,
        account_id: &AccountId,
        auth_credentials_store_mode: AuthCredentialsStoreMode,
    ) -> Result<Option<ResolvedStoredAccountAuth>, RefreshTokenError> {
        let Some(mut auth) = self
            .load()
            .map_err(|err| RefreshTokenError::Transient(std::io::Error::other(err)))?
            .accounts
            .into_iter()
            .find(|account| &account.account_id == account_id)
            .map(|account| account.auth)
        else {
            return Ok(None);
        };

        if !is_managed_chatgpt_auth(&auth) {
            return Ok(None);
        }

        if stored_auth_is_stale(&auth) {
            auth = refresh_stored_chatgpt_auth(auth).await?;
            self.update_account_auth(account_id, auth.clone(), auth_credentials_store_mode)
                .map_err(|err| RefreshTokenError::Transient(std::io::Error::other(err)))?;
        }

        Ok(resolved_auth_headers(&auth))
    }
}

pub fn stored_auth_is_stale(auth: &AuthDotJson) -> bool {
    let Some(tokens) = auth.tokens.as_ref() else {
        return false;
    };
    if let Ok(Some(expires_at)) = parse_jwt_expiration(&tokens.access_token) {
        return expires_at <= Utc::now();
    }
    let Some(last_refresh) = auth.last_refresh else {
        return false;
    };
    last_refresh < Utc::now() - chrono::Duration::days(TOKEN_REFRESH_INTERVAL_DAYS)
}

fn is_managed_chatgpt_auth(auth: &AuthDotJson) -> bool {
    matches!(auth.auth_mode, None | Some(AuthMode::Chatgpt)) && auth.tokens.is_some()
}

async fn refresh_stored_chatgpt_auth(
    mut auth: AuthDotJson,
) -> Result<AuthDotJson, RefreshTokenError> {
    let refresh_token = auth
        .tokens
        .as_ref()
        .map(|tokens| tokens.refresh_token.clone())
        .filter(|token| !token.is_empty())
        .ok_or_else(|| {
            RefreshTokenError::Transient(std::io::Error::other(
                "Token data is not available.",
            ))
        })?;

    let refresh_response = request_stored_chatgpt_token_refresh(refresh_token).await?;
    apply_refresh_response(&mut auth, refresh_response)?;
    Ok(auth)
}

fn resolved_auth_headers(auth: &AuthDotJson) -> Option<ResolvedStoredAccountAuth> {
    let tokens = auth.tokens.as_ref()?;
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

fn apply_refresh_response(
    auth: &mut AuthDotJson,
    response: RefreshResponse,
) -> Result<(), RefreshTokenError> {
    let tokens = auth.tokens.get_or_insert_with(TokenData::default);
    if let Some(id_token) = response.id_token {
        tokens.id_token = parse_chatgpt_jwt_claims(&id_token)
            .map_err(|err| RefreshTokenError::Transient(std::io::Error::other(err)))?;
    }
    if let Some(access_token) = response.access_token {
        tokens.access_token = access_token;
    }
    if let Some(refresh_token) = response.refresh_token {
        tokens.refresh_token = refresh_token;
    }
    auth.last_refresh = Some(Utc::now());
    Ok(())
}

async fn request_stored_chatgpt_token_refresh(
    refresh_token: String,
) -> Result<RefreshResponse, RefreshTokenError> {
    let refresh_request = RefreshRequest {
        client_id: CLIENT_ID,
        grant_type: "refresh_token",
        refresh_token,
    };
    let endpoint = refresh_token_endpoint();
    let response = create_client()
        .post(endpoint.as_str())
        .header("Content-Type", "application/json")
        .json(&refresh_request)
        .send()
        .await
        .map_err(|err| RefreshTokenError::Transient(std::io::Error::other(err)))?;

    let status = response.status();
    if status.is_success() {
        return response
            .json::<RefreshResponse>()
            .await
            .map_err(|err| RefreshTokenError::Transient(std::io::Error::other(err)));
    }

    let body = response.text().await.unwrap_or_default();
    if status == StatusCode::UNAUTHORIZED {
        Err(RefreshTokenError::Permanent(classify_refresh_token_failure(
            &body,
        )))
    } else {
        let message = try_parse_error_message(&body);
        Err(RefreshTokenError::Transient(std::io::Error::other(
            format!("Failed to refresh token: {status}: {message}"),
        )))
    }
}

fn classify_refresh_token_failure(body: &str) -> RefreshTokenFailedError {
    let code = extract_refresh_token_error_code(body);
    let normalized_code = code.as_deref().map(str::to_ascii_lowercase);
    let reason = match normalized_code.as_deref() {
        Some("refresh_token_expired") => RefreshTokenFailedReason::Expired,
        Some("refresh_token_reused") => RefreshTokenFailedReason::Exhausted,
        Some("refresh_token_invalidated") => RefreshTokenFailedReason::Revoked,
        _ => RefreshTokenFailedReason::Other,
    };
    let message = match reason {
        RefreshTokenFailedReason::Expired => REFRESH_TOKEN_EXPIRED_MESSAGE.to_string(),
        RefreshTokenFailedReason::Exhausted => REFRESH_TOKEN_REUSED_MESSAGE.to_string(),
        RefreshTokenFailedReason::Revoked => REFRESH_TOKEN_INVALIDATED_MESSAGE.to_string(),
        RefreshTokenFailedReason::Other => REFRESH_TOKEN_UNKNOWN_MESSAGE.to_string(),
    };
    RefreshTokenFailedError::new(reason, message)
}

fn extract_refresh_token_error_code(body: &str) -> Option<String> {
    if body.trim().is_empty() {
        return None;
    }

    let Value::Object(map) = serde_json::from_str::<Value>(body).ok()? else {
        return None;
    };

    if let Some(error_value) = map.get("error") {
        match error_value {
            Value::Object(obj) => {
                if let Some(code) = obj.get("code").and_then(Value::as_str) {
                    return Some(code.to_string());
                }
            }
            Value::String(code) => {
                return Some(code.to_string());
            }
            _ => {}
        }
    }

    map.get("code").and_then(Value::as_str).map(str::to_string)
}

#[derive(Serialize)]
struct RefreshRequest {
    client_id: &'static str,
    grant_type: &'static str,
    refresh_token: String,
}

#[derive(Deserialize)]
struct RefreshResponse {
    id_token: Option<String>,
    access_token: Option<String>,
    refresh_token: Option<String>,
}

fn refresh_token_endpoint() -> String {
    std::env::var(REFRESH_TOKEN_URL_OVERRIDE_ENV_VAR)
        .unwrap_or_else(|_| REFRESH_TOKEN_URL.to_string())
}
