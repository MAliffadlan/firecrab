use firecrab_api_types::{MicroServiceDeployment, MicroServiceResponse, MicroServiceSpec};
use rusqlite::{OptionalExtension, params};
use uuid::Uuid;

use super::{PersistenceError, Store};

pub(super) const CREATE_TABLE_SQL: &str = "CREATE TABLE IF NOT EXISTS micro_services (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    spec TEXT NOT NULL
) STRICT";

pub(super) const CREATE_DEPLOYMENTS_SQL: &str =
    "CREATE TABLE IF NOT EXISTS micro_service_deployments (
    service_id TEXT PRIMARY KEY,
    deployment TEXT NOT NULL
) STRICT";

impl Store {
    pub fn list_micro_services(&self) -> Result<Vec<MicroServiceResponse>, PersistenceError> {
        let conn = self.lock();
        let mut statement =
            conn.prepare("SELECT id, spec FROM micro_services ORDER BY name, id")?;
        let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.map(|row| {
            let (id, spec) = row?;
            decode(id, spec)
        })
        .collect()
    }

    pub fn micro_service(
        &self,
        id: Uuid,
    ) -> Result<Option<MicroServiceResponse>, PersistenceError> {
        let row: Option<(String, String)> = self
            .lock()
            .query_row(
                "SELECT id, spec FROM micro_services WHERE id = ?1",
                [id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        row.map(|(id, spec)| decode(id, spec)).transpose()
    }

    pub fn insert_micro_service(
        &self,
        service: &MicroServiceResponse,
    ) -> Result<(), PersistenceError> {
        let spec = encode(service)?;
        self.lock()
            .execute(
                "INSERT INTO micro_services (id, name, spec) VALUES (?1, ?2, ?3)",
                params![service.id.to_string(), service.spec.name, spec],
            )
            .map_err(|error| write_error(error, &service.spec.name))?;
        Ok(())
    }

    /// A single statement replaces all fields, so a failed edit leaves the old spec intact.
    pub fn update_micro_service(
        &self,
        service: &MicroServiceResponse,
    ) -> Result<bool, PersistenceError> {
        let spec = encode(service)?;
        let changed = self
            .lock()
            .execute(
                "UPDATE micro_services SET name = ?2, spec = ?3 WHERE id = ?1",
                params![service.id.to_string(), service.spec.name, spec],
            )
            .map_err(|error| write_error(error, &service.spec.name))?;
        Ok(changed != 0)
    }

    pub fn delete_micro_service(&self, id: Uuid) -> Result<bool, PersistenceError> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        tx.execute(
            "DELETE FROM micro_service_deployments WHERE service_id = ?1",
            [id.to_string()],
        )?;
        let changed = tx.execute("DELETE FROM micro_services WHERE id = ?1", [id.to_string()])?;
        tx.commit()?;
        Ok(changed != 0)
    }

    pub fn service_deployment(
        &self,
        id: Uuid,
    ) -> Result<Option<MicroServiceDeployment>, PersistenceError> {
        let text: Option<String> = self
            .lock()
            .query_row(
                "SELECT deployment FROM micro_service_deployments WHERE service_id = ?1",
                [id.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        text.map(|text| {
            serde_json::from_str(&text).map_err(|error| PersistenceError::CorruptRecord {
                id: id.to_string(),
                reason: error.to_string(),
            })
        })
        .transpose()
    }

    pub fn save_service_deployment(
        &self,
        deployment: &MicroServiceDeployment,
    ) -> Result<(), PersistenceError> {
        let text =
            serde_json::to_string(deployment).map_err(|error| PersistenceError::CorruptRecord {
                id: deployment.id.to_string(),
                reason: error.to_string(),
            })?;
        self.lock().execute(
            "INSERT INTO micro_service_deployments (service_id, deployment) VALUES (?1, ?2)
             ON CONFLICT(service_id) DO UPDATE SET deployment = excluded.deployment",
            params![deployment.service_id.to_string(), text],
        )?;
        Ok(())
    }
}

fn encode(service: &MicroServiceResponse) -> Result<String, PersistenceError> {
    serde_json::to_string(&service.spec).map_err(|error| PersistenceError::CorruptRecord {
        id: service.id.to_string(),
        reason: error.to_string(),
    })
}

fn decode(id: String, spec: String) -> Result<MicroServiceResponse, PersistenceError> {
    let parsed = Uuid::parse_str(&id).map_err(|error| PersistenceError::CorruptRecord {
        id: id.clone(),
        reason: error.to_string(),
    })?;
    let spec: MicroServiceSpec =
        serde_json::from_str(&spec).map_err(|error| PersistenceError::CorruptRecord {
            id,
            reason: error.to_string(),
        })?;
    Ok(MicroServiceResponse { id: parsed, spec })
}

fn write_error(error: rusqlite::Error, name: &str) -> PersistenceError {
    if let rusqlite::Error::SqliteFailure(code, _) = &error
        && code.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE
    {
        return PersistenceError::DuplicateMicroServiceName {
            name: name.to_owned(),
        };
    }
    PersistenceError::Database(error)
}
