use codex_app_server_protocol::AuthMode;
use codex_protocol::account::PlanType as AccountPlanType;

use crate::auth::AuthDotJson;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountMetadata {
    pub account_id: String,
    pub email: Option<String>,
    pub plan_type: Option<AccountPlanType>,
    pub workspace_id: Option<String>,
}

impl AccountMetadata {
    pub fn from_auth(auth: &AuthDotJson) -> Option<Self> {
        if matches!(
            auth.auth_mode,
            Some(AuthMode::ApiKey | AuthMode::ChatgptAuthTokens | AuthMode::AgentIdentity)
        ) {
            return None;
        }

        let tokens = auth.tokens.as_ref()?;
        let account_id = tokens
            .account_id
            .as_ref()
            .or(tokens.id_token.chatgpt_account_id.as_ref())?
            .to_string();
        let plan_type = tokens
            .id_token
            .chatgpt_plan_type
            .clone()
            .map(AccountPlanType::from);

        Some(Self {
            account_id,
            email: tokens.id_token.email.clone(),
            plan_type,
            workspace_id: tokens.id_token.chatgpt_account_id.clone(),
        })
    }
}
