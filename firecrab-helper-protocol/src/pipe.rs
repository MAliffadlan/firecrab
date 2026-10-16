use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

pub const STATUS_SOCKET: &str = "/run/firecrab-ingress/status.sock";
pub const TUNNEL_UNIT: &str = "firecrab-tunnel.service";

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// First data-plane slice: HTTP on loopback, with public TLS at Cloudflare.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Config {
    #[serde(default = "default_listen")]
    pub listen: SocketAddr,
    pub routes: Vec<Route>,
    pub tunnel: Option<Tunnel>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Route {
    pub host: String,
    #[serde(default = "default_path")]
    pub path_prefix: String,
    pub upstreams: Vec<SocketAddr>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Tunnel {
    pub id: Uuid,
    /// Source credentials, copied into a root-only systemd credential file.
    pub credentials_file: PathBuf,
}

fn default_listen() -> SocketAddr {
    "127.0.0.1:8080".parse().unwrap()
}

fn default_path() -> String {
    "/".into()
}

fn hostname(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 253
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
        })
}

fn path(path: &str) -> bool {
    path.starts_with('/')
        && path.bytes().all(|c| {
            c.is_ascii_graphic()
                && !matches!(
                    c,
                    b'*' | b'?' | b'#' | b'\\' | b'{' | b'}' | b'%' | b'\"' | b'\'' | b';' | b'$'
                )
        })
}

impl Config {
    pub fn credentials(&self) -> Result<Option<Value>> {
        let Some(tunnel) = &self.tunnel else {
            return Ok(None);
        };
        let value: Value = serde_json::from_slice(&std::fs::read(&tunnel.credentials_file)?)?;
        self.validate_credentials(&value)?;
        Ok(Some(value))
    }

    pub fn validate_credentials(&self, value: &Value) -> Result<()> {
        let tunnel = self
            .tunnel
            .as_ref()
            .ok_or("provide tunnel configuration with credentials")?;
        let id = value
            .get("TunnelID")
            .and_then(Value::as_str)
            .and_then(|id| Uuid::parse_str(id).ok());
        if id != Some(tunnel.id)
            || ["AccountTag", "TunnelSecret"].iter().any(|key| {
                value
                    .get(key)
                    .and_then(Value::as_str)
                    .is_none_or(str::is_empty)
            })
        {
            return Err("tunnel credentials must contain AccountTag, TunnelSecret, and the matching TunnelID".into());
        }
        Ok(())
    }

    pub fn read(file: &Path) -> Result<Self> {
        let mut config: Self = serde_json::from_slice(&std::fs::read(file)?)?;
        if let Some(tunnel) = &mut config.tunnel
            && tunnel.credentials_file.is_relative()
        {
            tunnel.credentials_file = file
                .parent()
                .unwrap_or(Path::new("."))
                .join(&tunnel.credentials_file);
        }
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        if serde_json::to_vec(self)?.len() > 48 * 1024 {
            return Err("pipe manifest exceeds 48 KiB".into());
        }
        if !self.listen.ip().is_loopback() || self.listen.port() < 1024 {
            return Err("listen must be a loopback IP and an unprivileged port (>= 1024); terminate public TLS at Cloudflare".into());
        }
        let mut matches = BTreeSet::new();
        for route in &self.routes {
            if !hostname(&route.host) {
                return Err(format!(
                    "invalid route host: {} (use a lowercase DNS name)",
                    route.host
                )
                .into());
            }
            if !path(&route.path_prefix) {
                return Err(format!("{}: paths must be literal absolute paths", route.host).into());
            }
            if !matches.insert((&route.host, route.path_prefix.trim_end_matches('/'))) {
                return Err(
                    format!("duplicate route: {} {}", route.host, route.path_prefix).into(),
                );
            }
            if route.upstreams.is_empty()
                || route.upstreams.iter().any(|address| {
                    address.port() == 0
                        || address.ip().is_unspecified()
                        || address.ip().is_multicast()
                        || *address == self.listen
                })
            {
                return Err(format!("{}: provide nonzero upstream IP:port addresses distinct from the ingress listener", route.host).into());
            }
            let unique: BTreeSet<_> = route.upstreams.iter().collect();
            if unique.len() != route.upstreams.len() {
                return Err(format!("{}: duplicate upstream", route.host).into());
            }
        }
        if self
            .tunnel
            .as_ref()
            .is_some_and(|tunnel| tunnel.id.is_nil())
        {
            return Err("tunnel.id must not be the nil UUID".into());
        }
        Ok(())
    }

    pub fn generation(&self) -> String {
        let bytes =
            serde_json::to_vec(&(self.listen, &self.routes)).expect("serializable pipe routes");
        format!("{:x}", Sha256::digest(bytes))
    }

    /// Standalone config: never includes or edits the host's nginx.conf.
    pub fn nginx(&self) -> String {
        let mut text = format!(
            "worker_processes auto;\npid nginx.pid;\nerror_log stderr notice;\nworker_shutdown_timeout 5m;\nevents {{ worker_connections 1024; }}\nhttp {{\n  access_log off;\n  server_tokens off;\n  client_body_temp_path body;\n  proxy_temp_path proxy;\n  map $http_upgrade $fc_connection {{ default upgrade; '' ''; }}\n  map $http_x_forwarded_proto $fc_proto {{ default $scheme; https https; http http; }}\n  server {{ listen unix:{STATUS_SOCKET}; location / {{ return 200 \"{}\"; }} }}\n",
            self.generation()
        );
        let mut hosts: BTreeMap<&str, Vec<(usize, &Route)>> = BTreeMap::new();
        for (index, route) in self.routes.iter().enumerate() {
            text.push_str(&format!(
                "  upstream fc_route_{index} {{\n    zone fc_route_{index} 64k;\n"
            ));
            for address in &route.upstreams {
                text.push_str(&format!(
                    "    server {address} max_fails=1 fail_timeout=30s;\n"
                ));
            }
            text.push_str("    keepalive 16;\n  }\n");
            hosts.entry(&route.host).or_default().push((index, route));
        }
        text.push_str(&format!(
            "  server {{ listen {} default_server; server_name _; return 404; }}\n",
            self.listen
        ));
        for (host, routes) in hosts {
            text.push_str(&format!(
                "  server {{ listen {}; server_name {host};\n",
                self.listen
            ));
            let has_fallback = routes.iter().any(|(_, route)| route.path_prefix == "/");
            for (index, route) in routes {
                let prefix = route.path_prefix.trim_end_matches('/');
                let locations = if prefix.is_empty() {
                    vec!["/".to_owned()]
                } else {
                    vec![format!("= \"{prefix}\""), format!("^~ \"{prefix}/\"")]
                };
                for location in locations {
                    text.push_str(&format!("    location {location} {{\n      proxy_pass http://fc_route_{index};\n      proxy_http_version 1.1;\n      proxy_set_header Host $host;\n      proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;\n      proxy_set_header X-Forwarded-Proto $fc_proto;\n      proxy_set_header Upgrade $http_upgrade;\n      proxy_set_header Connection $fc_connection;\n      proxy_connect_timeout 2s;\n      proxy_read_timeout 30s;\n      proxy_next_upstream error timeout http_500 http_502 http_503 http_504;\n      proxy_next_upstream_timeout 3s;\n    }}\n"));
                }
            }
            if !has_fallback {
                text.push_str("    location / { return 404; }\n");
            }
            text.push_str("  }\n");
        }
        text.push_str("}\n");
        text
    }

    pub fn tunnel_config(&self) -> Option<Value> {
        self.tunnel.as_ref().map(|tunnel| {
            let hosts: BTreeSet<_> = self.routes.iter().map(|route| &route.host).collect();
            let mut ingress: Vec<_> = hosts
                .into_iter()
                .map(|host| json!({"hostname": host, "service": format!("http://{}", self.listen)}))
                .collect();
            ingress.push(json!({"service": "http_status:404"}));
            json!({
                "tunnel": tunnel.id,
                "credentials-file": format!("/run/credentials/{TUNNEL_UNIT}/credentials.json"),
                "ingress": ingress
            })
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApplyRequest {
    pub config: Config,
    /// A supplied credential replaces the previous one; None retains it.
    pub credentials: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Operation {
    Inspect,
    Apply {
        request: ApplyRequest,
    },
    ConnectCloudflare {
        request: crate::cloudflare::ConnectRequest,
    },
    Disable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnitStatus {
    pub unit: String,
    pub load_state: String,
    pub active_state: String,
    pub sub_state: String,
    pub main_pid: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub config: Option<Config>,
    pub units: Vec<UnitStatus>,
    pub nginx_available: bool,
    pub cloudflared_available: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    fn config() -> Config {
        serde_json::from_value(json!({"routes":[{"host":"app.example.com","upstreams":["127.0.0.1:9000","127.0.0.1:9001"]},{"host":"app.example.com","pathPrefix":"/api","upstreams":["127.0.0.1:9002"]}]})).unwrap()
    }
    #[test]
    fn configuration_preserves_paths_and_load_balances() {
        let config = config();
        config.validate().unwrap();
        let nginx = config.nginx();
        assert!(nginx.contains("location = \"/api\""));
        assert!(nginx.contains("location ^~ \"/api/\""));
        assert!(nginx.contains("proxy_pass http://fc_route_1;"));
        assert!(nginx.contains("server 127.0.0.1:9001 max_fails=1"));
        assert!(nginx.contains("default_server; server_name _; return 404"));
    }
    #[test]
    fn rejects_injection_and_unintended_listeners() {
        for host in [
            "*.example.com",
            "app.example.com:443",
            "App.example.com",
            "app..com",
            "app;return 200;",
        ] {
            let mut config = config();
            config.routes[0].host = host.into();
            assert!(config.validate().is_err());
        }
        for prefix in [
            "api",
            "/api*",
            "/api?x",
            "/api%2fsecret",
            "/a\";return 200;",
            "/a$host",
        ] {
            let mut config = config();
            config.routes[0].path_prefix = prefix.into();
            assert!(config.validate().is_err());
        }
        for listen in ["0.0.0.0:8080", "127.0.0.1:80"] {
            let mut config = config();
            config.listen = listen.parse().unwrap();
            assert!(config.validate().is_err());
        }
    }
}
