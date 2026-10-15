use std::path::PathBuf;

use clap::Subcommand;
use firecrab_api_types::{MicroServiceResponse, MicroServiceSpec};
use uuid::Uuid;

use crate::api_client::{ApiClient, ApiError};

#[derive(Debug, Subcommand)]
pub enum Command {
    /// List saved service configurations.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Read one service configuration.
    Get { id: Uuid },
    /// Save a service configuration from a JSON file (does not deploy it).
    Create {
        #[arg(long)]
        file: PathBuf,
    },
    /// Replace every configuration field from a JSON file.
    Update {
        id: Uuid,
        #[arg(long)]
        file: PathBuf,
    },
    /// Delete a saved service configuration.
    Delete { id: Uuid },
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Api(#[from] ApiError),
    #[error("could not read service configuration {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("invalid service configuration {path}: {source}")]
    Json {
        path: PathBuf,
        source: serde_json::Error,
    },
}

pub fn run(client: &ApiClient, command: Command) -> Result<(), Error> {
    match command {
        Command::List { json } => {
            let services: Vec<MicroServiceResponse> = client.get("/api/micro-services")?;
            if json {
                print_json(&services);
            } else {
                println!("ID\tNAME\tIMAGE\tPORT\tREPLICAS\tRAM MiB\tCPU\tDOMAIN");
                for service in services {
                    let spec = service.spec;
                    println!(
                        "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                        service.id,
                        spec.name,
                        spec.image,
                        spec.port,
                        spec.replicas,
                        spec.ram,
                        spec.cpu,
                        spec.domain.as_deref().unwrap_or("-")
                    );
                }
            }
        }
        Command::Get { id } => {
            let service: MicroServiceResponse = client.get(&format!("/api/micro-services/{id}"))?;
            print_json(&service);
        }
        Command::Create { file } => {
            let service: MicroServiceResponse =
                client.post("/api/micro-services", &read_spec(file)?)?;
            print_json(&service);
        }
        Command::Update { id, file } => {
            let service: MicroServiceResponse =
                client.put(&format!("/api/micro-services/{id}"), &read_spec(file)?)?;
            print_json(&service);
        }
        Command::Delete { id } => {
            client.delete(&format!("/api/micro-services/{id}"))?;
            print_json(&serde_json::json!({ "deleted": id }));
        }
    }
    Ok(())
}

fn read_spec(path: PathBuf) -> Result<MicroServiceSpec, Error> {
    let bytes = std::fs::read(&path).map_err(|source| Error::Read {
        path: path.clone(),
        source,
    })?;
    serde_json::from_slice(&bytes).map_err(|source| Error::Json { path, source })
}

fn print_json(value: &impl serde::Serialize) {
    println!(
        "{}",
        serde_json::to_string_pretty(value).expect("API response serializes")
    );
}
