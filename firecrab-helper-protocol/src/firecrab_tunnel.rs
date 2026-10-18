//! Stable Firecrab addresses and local target selections. Ownership secrets stay on the host.
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Default production connection-service API, configured by the operator rather than dashboard users.
pub const ENDPOINT: &str = "https://tunnel.firecrab.dev";
/// Public suffix served by the Firecrab connection service.
pub const DOMAIN: &str = "firecrab.dev";
/// Maximum number of stable addresses on one managed host.
pub const MAX_ADDRESSES: usize = 16;

/// Reject reserved infrastructure names and require a single lowercase DNS label.
pub fn subdomain(value: &str) -> bool {
    (3..=48).contains(&value.len())
        && !value.starts_with('-')
        && !value.ends_with('-')
        && value
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
        && ![
            "www",
            "api",
            "admin",
            "tunnel",
            "connect",
            "docs",
            "status",
            "mail",
            "smtp",
            "support",
            "dashboard",
            "cdn",
            "assets",
            "auth",
            "login",
            "account",
            "accounts",
        ]
        .contains(&value)
}

/// Existing anonymous connector or saved Nginx hostname to publish.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Target {
    /// Reuse a connector selected from the current Quick Tunnel list.
    QuickTunnel {
        /// Stable connector identity.
        id: Uuid,
    },
    /// Publish all saved paths using an automatically managed connector.
    Nginx {
        /// Saved Nginx route hostname.
        host: String,
    },
}

/// One address change, applied to the helper's latest saved state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    /// Reserve or change an owned address.
    Save {
        /// Public DNS label.
        subdomain: String,
        /// Selected local resource.
        target: Target,
    },
    /// Remove the remote reservation and an exclusively owned connector.
    Delete {
        /// Public DNS label.
        subdomain: String,
    },
}
impl Request {
    /// Validate bounded input before privileged work.
    pub fn validate(&self) -> Result<(), String> {
        let name = match self {
            Self::Save { subdomain, .. } | Self::Delete { subdomain } => subdomain,
        };
        if !subdomain(name) {
            return Err(
                "Use 3–48 lowercase letters, digits or hyphens; infrastructure names are reserved"
                    .into(),
            );
        }
        if let Self::Save { target, .. } = self {
            match target {
                Target::QuickTunnel { id } if id.is_nil() => {
                    return Err("Select an existing Quick Tunnel".into());
                }
                Target::Nginx { host } if !super::pipe::valid_route_hostname(host) => {
                    return Err("Select an existing Nginx hostname".into());
                }
                _ => {}
            }
        }
        Ok(())
    }
}

/// Non-secret status returned through Pipe inspection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Address {
    /// Reserved DNS label.
    pub subdomain: String,
    /// Stable public HTTPS address.
    pub public_url: String,
    /// User-selected local target.
    pub target: Target,
    /// Current source connector.
    pub connector_id: Uuid,
    /// Whether the last service update succeeded with a live target.
    pub connected: bool,
    /// Sanitized failure or pending status.
    pub error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn names_and_target_intents_are_bounded_and_reject_registration_credentials() {
        for name in ["my-app", "abc", "api-service"] {
            assert!(subdomain(name));
        }
        for name in ["api", "www", "tunnel", "-app", "app-", "a.b", "UPPER", "ab"] {
            assert!(!subdomain(name));
        }
        let invalid = serde_json::json!({"action":"save","subdomain":"my-app","target":{"type":"nginx","host":"app.test"},"token":"secret"});
        assert!(serde_json::from_value::<Request>(invalid).is_err());
        for target in [
            Target::QuickTunnel {
                id: Uuid::from_u128(2),
            },
            Target::Nginx {
                host: "app.test".into(),
            },
        ] {
            Request::Save {
                subdomain: "my-app".into(),
                target,
            }
            .validate()
            .unwrap();
        }
        assert!(
            Request::Save {
                subdomain: "my-app".into(),
                target: Target::Nginx {
                    host: "bad;host".into()
                }
            }
            .validate()
            .is_err()
        );
    }
}
