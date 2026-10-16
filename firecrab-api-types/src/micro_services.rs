//! Desired service configuration. Deployment and live health are separate resources.

use std::collections::BTreeMap;

use crate::VmState;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceRuntime {
    #[default]
    Vm,
}

/// Body for POST /api/micro-services and full replacement by PUT /api/micro-services/{id}.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct MicroServiceSpec {
    /// Unique DNS label; also usable as the future `<name>.internal` address.
    pub name: String,
    /// OCI image reference (tag or sha256 digest). Resolved only during deployment.
    pub image: String,
    #[serde(default)]
    pub runtime: ServiceRuntime,
    pub port: u16,
    #[serde(default = "default_health_path")]
    pub health_path: String,
    #[serde(default)]
    pub domain: Option<String>,
    /// Guest RAM in MiB, using the same sizes as MicroVMs.
    pub ram: u32,
    pub cpu: u8,
    #[serde(default = "default_replicas")]
    pub replicas: u16,
    /// Seconds allowed for app startup.
    #[serde(default = "default_startup_timeout")]
    pub startup_timeout: u32,
    /// Seconds allowed for draining requests.
    #[serde(default = "default_drain_timeout")]
    pub drain_timeout: u32,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Environment variable name -> CrabSecrets UUID. Never contains secret values.
    #[serde(default)]
    pub secrets: BTreeMap<String, Uuid>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MicroServiceResponse {
    pub id: Uuid,
    #[serde(flatten)]
    pub spec: MicroServiceSpec,
}

/// The first replica uses host_port; the others use consecutive host ports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DeployMicroServiceRequest {
    #[serde(default)]
    pub micro_network_id: Option<Uuid>,
    pub host_port: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServicePhase {
    Queued,
    Importing,
    Starting,
    CheckingHealth,
    Running,
    Degraded,
    Stopping,
    Stopped,
    Failed,
}

impl ServicePhase {
    pub fn is_busy(self) -> bool {
        matches!(
            self,
            Self::Queued | Self::Importing | Self::Starting | Self::CheckingHealth | Self::Stopping
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceInstance {
    pub vm_id: Uuid,
    pub ipv4: String,
    pub host_port: u16,
    pub state: VmState,
    pub healthy: bool,
}

/// Durable deployment snapshot. Live reads refresh instance states and health.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MicroServiceDeployment {
    pub id: Uuid,
    pub service_id: Uuid,
    pub phase: ServicePhase,
    pub spec: MicroServiceSpec,
    pub request: DeployMicroServiceRequest,
    pub digest: Option<String>,
    pub instances: Vec<ServiceInstance>,
    pub log: Vec<String>,
    pub error: Option<String>,
}

fn default_health_path() -> String {
    "/health".to_owned()
}

fn default_replicas() -> u16 {
    1
}

fn default_startup_timeout() -> u32 {
    300
}

fn default_drain_timeout() -> u32 {
    30
}
