//! Durable address ownership and HTTP/WebSocket forwarding for both supported source types.
mod proxy;

use axum::{
    Router,
    body::{Body, to_bytes},
    extract::{ConnectInfo, State},
    http::{Request, StatusCode},
    response::{IntoResponse, Response},
};
use firecrab_helper_protocol::firecrab_tunnel::subdomain;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    net::SocketAddr,
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const LEASE_SECONDS: i64 = 180;

/// One connection-service instance. SQLite serializes ownership and target updates.
#[derive(Clone)]
pub struct Service {
    trusted_cloudflare_proxy: bool,
    db: Arc<Mutex<Connection>>,
    domain: String,
    nginx_origins: Vec<String>,
    registrations: Arc<Mutex<HashMap<String, (i64, u32)>>>,
    client: reqwest::Client,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Target {
    kind: Kind,
    url: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum Kind {
    QuickTunnel,
    Nginx,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Publish {
    target: Option<Target>,
    enabled: bool,
}
#[derive(Debug)]
struct Binding {
    target: Option<Target>,
    enabled: bool,
    seen: i64,
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
fn error(status: StatusCode, code: &str) -> Response {
    (status, axum::Json(serde_json::json!({"error":code}))).into_response()
}
fn host(request: &Request<Body>) -> Option<&str> {
    request
        .headers()
        .get("host")?
        .to_str()
        .ok()?
        .split(':')
        .next()
}

impl Service {
    /// Open the durable registry and configure the explicitly trusted direct Nginx origins.
    pub fn open(
        path: &Path,
        domain: String,
        nginx_origins: Vec<String>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        if !firecrab_helper_protocol::pipe::valid_route_hostname(&domain) {
            return Err("invalid public domain".into());
        }
        let db = Connection::open(path)?;
        db.busy_timeout(Duration::from_secs(5))?;
        db.execute_batch(
            "PRAGMA journal_mode=WAL;
             CREATE TABLE IF NOT EXISTS addresses (
                 name TEXT PRIMARY KEY,
                 owner BLOB NOT NULL,
                 target TEXT,
                 enabled INTEGER NOT NULL,
                 seen INTEGER NOT NULL
             );",
        )?;
        let origins = nginx_origins
            .into_iter()
            .map(|value| {
                let url = reqwest::Url::parse(&value)?;
                if !matches!(url.scheme(), "https" | "http")
                    || !url.username().is_empty()
                    || url.password().is_some()
                    || url.path() != "/"
                    || url.query().is_some()
                    || url.fragment().is_some()
                {
                    return Err("Nginx allowlist entries must be plain HTTP(S) origins".into());
                }
                Ok(url.origin().ascii_serialization())
            })
            .collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?;
        Ok(Self {
            trusted_cloudflare_proxy: false,
            db: Arc::new(Mutex::new(db)),
            domain,
            nginx_origins: origins,
            registrations: Arc::new(Mutex::new(HashMap::new())),
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(5))
                .build()?,
        })
    }

    /// Route the control hostname separately from tenant application requests.
    pub fn router(self) -> Router {
        Router::new().fallback(dispatch).with_state(self)
    }

    /// Trust Cloudflare's client-IP header only on connections from a local connector.
    pub fn with_cloudflare_proxy(mut self) -> Self {
        self.trusted_cloudflare_proxy = true;
        self
    }

    fn binding(&self, name: &str) -> rusqlite::Result<Option<Binding>> {
        self.db
            .lock()
            .unwrap()
            .query_row(
                "SELECT target,enabled,seen FROM addresses WHERE name=?1",
                [name],
                |row| {
                    let target: Option<String> = row.get(0)?;
                    Ok(Binding {
                        target: target.and_then(|v| serde_json::from_str(&v).ok()),
                        enabled: row.get(1)?,
                        seen: row.get(2)?,
                    })
                },
            )
            .optional()
    }

    fn valid_target(&self, target: &Target) -> bool {
        let Ok(url) = reqwest::Url::parse(&target.url) else {
            return false;
        };
        if !url.username().is_empty()
            || url.password().is_some()
            || url.path() != "/"
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return false;
        }
        let quick = url
            .host_str()
            .and_then(|h| h.strip_suffix(".trycloudflare.com"))
            .is_some_and(|label| {
                !label.is_empty()
                    && label.len() <= 63
                    && !label.starts_with('-')
                    && !label.ends_with('-')
                    && label
                        .bytes()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
            });
        (url.scheme() == "https" && url.port().is_none() && quick)
            || (target.kind == Kind::Nginx
                && self
                    .nginx_origins
                    .contains(&url.origin().ascii_serialization()))
    }

    async fn control(&self, request: Request<Body>) -> Response {
        if request.uri().path() == "/health" && request.method() == "GET" {
            return StatusCode::NO_CONTENT.into_response();
        }
        let Some(name) = request
            .uri()
            .path()
            .strip_prefix("/v1/addresses/")
            .filter(|name| subdomain(name))
            .map(str::to_owned)
        else {
            return error(StatusCode::NOT_FOUND, "not_found");
        };
        if !matches!(request.method().as_str(), "PUT" | "DELETE") {
            return error(StatusCode::METHOD_NOT_ALLOWED, "method_not_allowed");
        }
        let Some(token) = request
            .headers()
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .filter(|v| v.len() == 64 && v.bytes().all(|b| b.is_ascii_hexdigit()))
        else {
            return error(StatusCode::UNAUTHORIZED, "ownership_token_required");
        };
        let digest = Sha256::digest(token.as_bytes()).to_vec();
        let deleting = request.method() == "DELETE";
        let socket_peer = request
            .extensions()
            .get::<ConnectInfo<SocketAddr>>()
            .map(|p| p.0.ip());
        let forwarded =
            if self.trusted_cloudflare_proxy && socket_peer.is_some_and(|ip| ip.is_loopback()) {
                request
                    .headers()
                    .get("cf-connecting-ip")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse::<std::net::IpAddr>().ok())
            } else {
                None
            };
        let peer = forwarded
            .or(socket_peer)
            .map(|ip| ip.to_string())
            .unwrap_or_else(|| "local".into());
        let publish = if deleting {
            None
        } else {
            let Ok(body) = to_bytes(request.into_body(), 4096).await else {
                return error(StatusCode::PAYLOAD_TOO_LARGE, "request_too_large");
            };
            let Ok(value) = serde_json::from_slice::<Publish>(&body) else {
                return error(StatusCode::BAD_REQUEST, "invalid_request");
            };
            if value
                .target
                .as_ref()
                .is_some_and(|target| !self.valid_target(target))
                || (value.enabled && value.target.is_none())
            {
                return error(StatusCode::BAD_REQUEST, "invalid_target");
            }
            Some(value)
        };
        let mut db = self.db.lock().unwrap();
        let Ok(transaction) = db.transaction() else {
            return error(StatusCode::SERVICE_UNAVAILABLE, "registry_unavailable");
        };
        let owner: Option<Vec<u8>> = match transaction
            .query_row("SELECT owner FROM addresses WHERE name=?1", [&name], |r| {
                r.get(0)
            })
            .optional()
        {
            Ok(v) => v,
            Err(_) => return error(StatusCode::SERVICE_UNAVAILABLE, "registry_unavailable"),
        };
        if let Some(owner) = &owner {
            let different = owner.len() != digest.len()
                || owner.iter().zip(&digest).fold(0u8, |v, (a, b)| v | (a ^ b)) != 0;
            if different {
                return error(StatusCode::CONFLICT, "subdomain_owned_by_another_host");
            }
        }
        if deleting {
            if transaction
                .execute("DELETE FROM addresses WHERE name=?1", [name])
                .is_err()
                || transaction.commit().is_err()
            {
                return error(StatusCode::SERVICE_UNAVAILABLE, "registry_unavailable");
            }
            return StatusCode::NO_CONTENT.into_response();
        }
        if owner.is_none() {
            let count: i64 = transaction
                .query_row("SELECT count(*) FROM addresses", [], |r| r.get(0))
                .unwrap_or(i64::MAX);
            if count >= 10000 {
                return error(StatusCode::SERVICE_UNAVAILABLE, "registry_full");
            }
            let mut limits = self.registrations.lock().unwrap();
            let hour = now() / 3600;
            limits.retain(|_, v| v.0 == hour);
            if limits.len() >= 10000 {
                return error(StatusCode::TOO_MANY_REQUESTS, "registration_rate_limit");
            }
            let quota = limits.entry(peer).or_insert((hour, 0));
            if quota.1 >= 16 {
                return error(StatusCode::TOO_MANY_REQUESTS, "registration_rate_limit");
            }
            quota.1 += 1;
        }
        let value = publish.unwrap();
        let target = value.target.map(|t| serde_json::to_string(&t).unwrap());
        if transaction
            .execute(
                "INSERT INTO addresses(name,owner,target,enabled,seen)
                 VALUES(?1,?2,?3,?4,?5) ON CONFLICT(name) DO UPDATE SET
                 target=excluded.target,enabled=excluded.enabled,seen=excluded.seen",
                params![name, digest, target, value.enabled, now()],
            )
            .is_err()
            || transaction.commit().is_err()
        {
            return error(StatusCode::SERVICE_UNAVAILABLE, "registry_unavailable");
        }
        (
            StatusCode::OK,
            axum::Json(serde_json::json!({
                "url": format!("https://{name}.{}", self.domain),
                "leaseSeconds": LEASE_SECONDS
            })),
        )
            .into_response()
    }
}

async fn dispatch(State(service): State<Service>, request: Request<Body>) -> Response {
    let hostname = host(&request).unwrap_or("").to_ascii_lowercase();
    if hostname == format!("tunnel.{}", service.domain) {
        return service.control(request).await;
    }
    let Some(name) = hostname
        .strip_suffix(&format!(".{}", service.domain))
        .filter(|v| subdomain(v))
    else {
        return error(StatusCode::NOT_FOUND, "unknown_address");
    };
    let binding = match service.binding(name) {
        Ok(Some(v)) => v,
        Ok(None) => return error(StatusCode::NOT_FOUND, "unknown_address"),
        Err(_) => return error(StatusCode::SERVICE_UNAVAILABLE, "registry_unavailable"),
    };
    if !binding.enabled || now() - binding.seen > LEASE_SECONDS {
        return error(StatusCode::SERVICE_UNAVAILABLE, "target_offline");
    }
    let Some(target) = binding.target else {
        return error(StatusCode::SERVICE_UNAVAILABLE, "target_offline");
    };
    if !service.valid_target(&target) {
        return error(StatusCode::SERVICE_UNAVAILABLE, "target_no_longer_allowed");
    }
    proxy::forward(service.client, request, &target.url, &hostname).await
}

#[cfg(test)]
mod tests;
