//! Onboarding inputs. Account API tokens are transient and redacted in Debug.
use serde::{Deserialize, Serialize};

use crate::pipe::Config;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ApiToken(String);

impl std::fmt::Debug for ApiToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[REDACTED]")
    }
}

impl ApiToken {
    pub fn value(&self) -> &str {
        self.0.trim()
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        let value = self.value();
        if !(32..=1024).contains(&value.len())
            || !value
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_'))
        {
            return Err("Enter a Cloudflare API token, not a tunnel token or installation command");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AccessRequest {
    pub api_token: ApiToken,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    pub id: String,
    #[serde(default)]
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Zone {
    pub id: String,
    pub name: String,
    pub status: String,
    pub account: Account,
}

pub fn identifier(value: &str) -> bool {
    value.len() == 32 && value.bytes().all(|c| c.is_ascii_hexdigit())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConnectRequest {
    pub api_token: ApiToken,
    pub zone_id: String,
    pub hostname: String,
    pub config: Config,
}

impl ConnectRequest {
    pub fn validate(&self) -> Result<(), String> {
        self.api_token.validate().map_err(str::to_owned)?;
        if !identifier(&self.zone_id) {
            return Err("Invalid Cloudflare domain identifier".into());
        }
        self.config.validate().map_err(|e| e.to_string())?;
        if self.config.tunnel.is_some() {
            return Err("Disconnect the configured tunnel before creating a new connection".into());
        }
        if !self
            .config
            .routes
            .iter()
            .any(|route| route.host == self.hostname)
        {
            return Err("Add an app route for the public hostname before connecting".into());
        }
        if serde_json::to_vec(self).map_or(true, |bytes| bytes.len() > 48 * 1024) {
            return Err("Cloudflare request exceeds 48 KiB".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn api_tokens_are_redacted_and_commands_are_rejected() {
        let secret = "scoped-api-token-01234567890123456789";
        let request: AccessRequest =
            serde_json::from_value(serde_json::json!({"apiToken":secret})).unwrap();
        request.api_token.validate().unwrap();
        assert!(!format!("{request:?}").contains(secret));
        let invalid: AccessRequest = serde_json::from_value(
            serde_json::json!({"apiToken":"cloudflared tunnel run --token secret"}),
        )
        .unwrap();
        assert!(invalid.api_token.validate().is_err());
    }
}
