use std::fs::{self, File};
use std::io;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::lifecycle::Layout;
use crate::micromanager::report;

const GUEST_SCRIPT: &str = include_str!("../../../../scripts/micromanager/dev-macos-guest.sh");
// Include the workspace manifests and compile-time resources, not the whole
// checkout: local configs, credentials, nested checkouts and build output stay home.
const SOURCE_INPUTS: &[&str] = &[
    "Cargo.toml",
    "Cargo.lock",
    "rust-toolchain.toml",
    "firecrab-api",
    "firecrab-api-types",
    "firecrab-cli",
    "firecrab-helper-protocol",
    "firecrab-net-helper",
    "scripts/firecracker-menual",
    "scripts/micromanager/dev-macos-guest.sh",
    "assets/firecrab-motd",
    "packaging/m2images.json",
];

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid Firecrab checkout {path}: {detail}")]
    Checkout { path: PathBuf, detail: String },
    #[error("could not {action}: {source}")]
    Io {
        action: &'static str,
        #[source]
        source: io::Error,
    },
    #[error("{0} failed; see the command output above")]
    Command(&'static str),
    #[error("management VM SSH is not ready; inspect `firecrab service debug --logs`")]
    GuestNotReady,
    #[error(
        "management VM SSH at {ip} is not ready: {detail}; inspect `firecrab service debug --logs` before retrying (source upload was not attempted)"
    )]
    GuestSshUnavailable { ip: IpAddr, detail: String },
    #[error(
        "guest services are ready but the localhost API tunnel is unavailable; inspect `firecrab service debug --logs`"
    )]
    LocalApiUnavailable,
}

pub struct Checkout {
    root: PathBuf,
    channel: String,
    archive: tempfile::NamedTempFile,
}

impl Checkout {
    pub fn prepare(path: &Path) -> Result<Self, Error> {
        let root =
            fs::canonicalize(path).map_err(|error| checkout_error(path, error.to_string()))?;
        let channel = validate_checkout(&root)?;
        let archive = tempfile::NamedTempFile::new()
            .map_err(|source| io_error("create source archive", source))?;
        let status = Command::new("/usr/bin/tar")
            .args(["--no-xattrs", "-cf"])
            .arg(archive.path())
            .arg("-C")
            .arg(&root)
            .args([
                "--exclude=.git",
                "--exclude=target",
                "--exclude=node_modules",
                "--exclude=.DS_Store",
                "--exclude=.env",
                "--exclude=.env.*",
            ])
            .args(SOURCE_INPUTS)
            .env("COPYFILE_DISABLE", "1")
            .status()
            .map_err(|source| io_error("archive checkout sources", source))?;
        if !status.success() {
            return Err(Error::Command("source archive"));
        }
        Ok(Self {
            root,
            channel,
            archive,
        })
    }
}

fn validate_checkout(root: &Path) -> Result<String, Error> {
    for input in SOURCE_INPUTS {
        let path = root.join(input);
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| checkout_error(root, format!("{input}: {error}")))?;
        if metadata.is_symlink() || (!metadata.is_file() && !metadata.is_dir()) {
            return Err(checkout_error(
                root,
                format!("{input} must be a regular file or directory"),
            ));
        }
    }
    let manifest = read_toml(root, "Cargo.toml")?;
    let members = manifest
        .get("workspace")
        .and_then(|workspace| workspace.get("members"))
        .and_then(toml::Value::as_array)
        .ok_or_else(|| checkout_error(root, "workspace members are missing"))?;
    // Cargo resolves every member's manifest even when only two packages build.
    for required in SOURCE_INPUTS
        .iter()
        .filter(|input| input.starts_with("firecrab-"))
    {
        if !members
            .iter()
            .any(|member| member.as_str() == Some(required))
        {
            return Err(checkout_error(
                root,
                format!("workspace member {required} is missing"),
            ));
        }
    }
    if members.len() != 5 {
        return Err(checkout_error(
            root,
            "development archive supports the five Firecrab workspace members",
        ));
    }
    let toolchain = read_toml(root, "rust-toolchain.toml")?;
    let channel = toolchain
        .get("toolchain")
        .and_then(|toolchain| toolchain.get("channel"))
        .and_then(toml::Value::as_str)
        .filter(|channel| {
            !channel.is_empty()
                && channel
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-'))
        })
        .ok_or_else(|| checkout_error(root, "Rust toolchain channel is missing or invalid"))?;
    Ok(channel.to_string())
}

fn read_toml(root: &Path, name: &str) -> Result<toml::Value, Error> {
    let text = fs::read_to_string(root.join(name))
        .map_err(|error| checkout_error(root, format!("{name}: {error}")))?;
    toml::from_str(&text).map_err(|error| checkout_error(root, format!("{name}: {error}")))
}

pub fn deploy(
    layout: &Layout,
    ip: IpAddr,
    checkout: Option<&Checkout>,
    release: bool,
) -> Result<(), Error> {
    // A guest marker is written before the host tunnel succeeds and can survive
    // a failed boot. Verify the live SSH connection before uploading the snapshot.
    let output = ssh_command(layout, ip, "true")
        .stdin(Stdio::null())
        .output()
        .map_err(|source| io_error("probe management SSH", source))?;
    if !output.status.success() {
        return Err(Error::GuestSshUnavailable {
            ip,
            detail: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }
    let archive_name = format!("incoming-{}.tar", uuid::Uuid::new_v4());
    let (profile, channel) = match checkout {
        Some(checkout) => {
            report!("[SOURCE] {}", checkout.root.display());
            let archive = checkout
                .archive
                .reopen()
                .map_err(|source| io_error("read source archive", source))?;
            run_ssh(
                layout,
                ip,
                &format!(
                    "install -d -m 0700 /var/lib/firecrab/dev && umask 077 && cat > /var/lib/firecrab/dev/{archive_name}"
                ),
                archive,
                "source upload",
            )?;
            (
                if release { "release" } else { "debug" },
                checkout.channel.as_str(),
            )
        }
        None => ("restore", "unused"),
    };
    // Send the embedded script independently of the snapshot so an installed CLI
    // can run from any checkout directory and restore without source files.
    let mut script =
        tempfile::tempfile().map_err(|source| io_error("create guest build script", source))?;
    use std::io::{Seek, Write};
    script
        .write_all(GUEST_SCRIPT.as_bytes())
        .and_then(|_| script.rewind())
        .map_err(|source| io_error("write guest build script", source))?;
    report!("[GUEST] {profile}: API + net-helper");
    run_ssh(
        layout,
        ip,
        &format!("bash -s -- {profile} {channel} {archive_name}"),
        script,
        "guest development build/deployment",
    )
}

fn run_ssh(
    layout: &Layout,
    ip: IpAddr,
    command: &str,
    stdin: File,
    action: &'static str,
) -> Result<(), Error> {
    let status = ssh_command(layout, ip, command)
        .stdin(Stdio::from(stdin))
        .status()
        .map_err(|source| io_error(action, source))?;
    if !status.success() {
        return Err(Error::Command(action));
    }
    Ok(())
}

fn ssh_command(layout: &Layout, ip: IpAddr, command: &str) -> Command {
    let runtime = layout.managed_home.join("runtime");
    let mut ssh = Command::new("/usr/bin/ssh");
    ssh.arg("-i")
        .arg(runtime.join("manager_ed25519"))
        .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=5"])
        .args(["-o", "StrictHostKeyChecking=yes", "-o", "UpdateHostKeys=no"])
        .args([
            "-o",
            "ServerAliveInterval=15",
            "-o",
            "ServerAliveCountMax=3",
        ])
        .arg("-o")
        .arg(format!(
            "UserKnownHostsFile={}",
            runtime.join("known_hosts").display()
        ))
        .arg(format!("root@{ip}"))
        .arg(command);
    ssh
}

fn checkout_error(path: &Path, detail: impl Into<String>) -> Error {
    Error::Checkout {
        path: path.to_owned(),
        detail: detail.into(),
    }
}

fn io_error(action: &'static str, source: io::Error) -> Error {
    Error::Io { action, source }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checkout_fixture() -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        for input in SOURCE_INPUTS {
            let path = directory.path().join(input);
            if input.starts_with("firecrab-") || *input == "scripts/firecracker-menual" {
                fs::create_dir_all(&path).unwrap();
            } else {
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(&path, "").unwrap();
            }
        }
        fs::write(directory.path().join("Cargo.toml"), r#"
[workspace]
members = ["firecrab-api", "firecrab-api-types", "firecrab-cli", "firecrab-helper-protocol", "firecrab-net-helper"]
"#).unwrap();
        fs::write(
            directory.path().join("rust-toolchain.toml"),
            "[toolchain]\nchannel = '1.97.1'\n",
        )
        .unwrap();
        directory
    }

    #[test]
    fn source_archive_contains_edited_sources_and_compile_time_resources_only() {
        let directory = checkout_fixture();
        let source = directory.path().join("firecrab-api/src/main.rs");
        fs::create_dir_all(source.parent().unwrap()).unwrap();
        fs::write(source, "edited local source").unwrap();
        for name in [
            "config.toml",
            "firecrab-api/.env",
            "firecrab-api/target/old-binary",
            "firecrab-api/.git/config",
        ] {
            let path = directory.path().join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, "must stay on the Mac").unwrap();
        }
        let checkout = Checkout::prepare(directory.path()).unwrap();
        let output = Command::new("/usr/bin/tar")
            .arg("-tf")
            .arg(checkout.archive.path())
            .output()
            .unwrap();
        assert!(output.status.success());
        let names = String::from_utf8(output.stdout).unwrap();
        assert!(names.lines().any(|name| name == "firecrab-api/src/main.rs"));
        assert!(names.lines().any(|name| name == "packaging/m2images.json"));
        assert!(names.lines().any(|name| name == "assets/firecrab-motd"));
        assert!(
            !names
                .lines()
                .any(|name| name == "config.toml" || name.starts_with("firecrab/"))
        );
        assert!(!names.lines().any(|name| {
            name.split('/')
                .any(|part| matches!(part, ".git" | "target" | "node_modules" | ".env"))
        }));
        let output = Command::new("/usr/bin/tar")
            .arg("-xOf")
            .arg(checkout.archive.path())
            .arg("firecrab-api/src/main.rs")
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, b"edited local source");
    }

    #[test]
    fn missing_checkout_fails_before_starting_a_vm() {
        let directory = tempfile::tempdir().unwrap();
        assert!(matches!(
            Checkout::prepare(directory.path()),
            Err(Error::Checkout { .. })
        ));
    }

    #[test]
    fn malformed_workspace_and_toolchain_are_errors_instead_of_panics() {
        let directory = checkout_fixture();
        fs::write(
            directory.path().join("Cargo.toml"),
            "[package]\nname = 'other'\n",
        )
        .unwrap();
        assert!(matches!(
            Checkout::prepare(directory.path()),
            Err(Error::Checkout { .. })
        ));
        let directory = checkout_fixture();
        fs::write(
            directory.path().join("rust-toolchain.toml"),
            "[toolchain]\nchannel = 'stable; echo injected'\n",
        )
        .unwrap();
        assert!(matches!(
            Checkout::prepare(directory.path()),
            Err(Error::Checkout { .. })
        ));
    }
}
