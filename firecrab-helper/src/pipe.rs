//! The helper owns fixed pipe controls. The API never starts traffic processes.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use firecrab_helper_protocol::network::HelperFailure;
use firecrab_helper_protocol::pipe::{Operation, Snapshot};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

fn cli() -> Result<PathBuf, HelperFailure> {
    use std::os::unix::fs::MetadataExt;
    let executable = std::env::current_exe().map_err(failure)?;
    let sibling = executable
        .parent()
        .unwrap_or(Path::new("/usr/local/bin"))
        .join("firecrab");
    let prefix = std::env::var("PREFIX").unwrap_or_else(|_| "/usr/local".into());
    for path in [sibling, PathBuf::from(prefix).join("bin/firecrab")] {
        // Check the original path too: a mutable symlink must not select a root program.
        let protected = path.ancestors().all(|component| {
            std::fs::symlink_metadata(component).is_ok_and(|m| {
                m.uid() == 0
                    && (m.file_type().is_symlink()
                        || m.mode() & 0o022 == 0
                        || (m.is_dir() && m.mode() & 0o1000 != 0))
            })
        });
        if protected && let Ok(resolved) = crate::vm_unit::trusted_executable(&path, 0) {
            return Ok(resolved);
        }
    }
    Err(HelperFailure::Internal {
        detail: "install the matching root-owned Firecrab CLI beside the helper".into(),
    })
}

fn failure(error: impl std::fmt::Display) -> HelperFailure {
    HelperFailure::Internal {
        detail: error.to_string(),
    }
}

async fn execute(
    cli: &Path,
    action: &str,
    input: Option<Vec<u8>>,
) -> Result<Vec<u8>, HelperFailure> {
    let mut child = Command::new(cli)
        .args(["pipe", action])
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(failure)?;
    let task = async {
        if let Some(input) = input {
            child
                .stdin
                .take()
                .ok_or_else(|| failure("missing pipe stdin"))?
                .write_all(&input)
                .await
                .map_err(failure)?;
        }
        let output = child.wait_with_output().await.map_err(failure)?;
        if !output.status.success() {
            return Err(failure(String::from_utf8_lossy(&output.stderr)));
        }
        Ok(output.stdout)
    };
    tokio::time::timeout(Duration::from_secs(220), task)
        .await
        .map_err(|_| failure("pipe command timed out"))?
}

pub(super) async fn run(request: Operation) -> Result<Snapshot, HelperFailure> {
    let cli = cli()?;
    match request {
        Operation::Inspect => {}
        Operation::Disable => {
            execute(&cli, "disable", None).await?;
        }
        Operation::Apply { request } => {
            request
                .config
                .validate()
                .map_err(|error| HelperFailure::InvalidRequest {
                    detail: error.to_string(),
                })?;
            if let Some(credentials) = &request.credentials {
                request
                    .config
                    .validate_credentials(credentials)
                    .map_err(|error| HelperFailure::InvalidRequest {
                        detail: error.to_string(),
                    })?;
            }
            let bytes = serde_json::to_vec(&request).map_err(failure)?;
            if bytes.len() > 48 * 1024 {
                return Err(HelperFailure::InvalidRequest {
                    detail: "pipe request exceeds 48 KiB".into(),
                });
            }
            execute(&cli, "apply-managed", Some(bytes)).await?;
        }
    }
    serde_json::from_slice(&execute(&cli, "inspect", None).await?).map_err(failure)
}
