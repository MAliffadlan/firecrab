//! Renders and runs the Firecracker microVM: `firecracker.json` generation,
//! launching the VM's shim (`crate::vm_shim`), readiness polling, and the
//! exit monitor that records guest-initiated state transitions.

use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, watch};
use uuid::Uuid;

use crate::console::ConsoleBroker;
use crate::model::{MacAddr, VmRecord, VmState};
use crate::state::{AppState, RuntimeConfig};
use crate::vm_shim::client::{SessionEvent, ShimConnectError, ShimControl, ShimSession};
use crate::vm_shim::protocol::ExitStatus;
use crate::vm_shim::server::ShimConfig;

/// Delay between readiness probe attempts while waiting for the API socket.
const READY_POLL_INTERVAL: Duration = Duration::from_millis(20);

/// Failure modes for rendering config, launching, or stopping a VM.
#[derive(Debug, Error)]
pub enum FirecrackerError {
    /// Couldn't create the VM's own directory.
    #[error("failed to create VM directory {path}: {source}")]
    CreateDirectory {
        /// The directory that couldn't be created.
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    /// Couldn't serialize the config to JSON.
    #[error("failed to serialize Firecracker config for {path}: {source}")]
    Serialize {
        /// The config file path this was destined for.
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    /// Couldn't write the rendered config to disk.
    #[error("failed to write Firecracker config {path}: {source}")]
    Write {
        /// The config file path that failed to write.
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    /// Couldn't start the VM's shim process.
    #[error("failed to start the VM shim {program}: {source}")]
    Spawn {
        /// The program that failed to spawn.
        program: PathBuf,
        #[source]
        source: io::Error,
    },
    /// The shim exited before accepting a connection — typically because it
    /// could not start Firecracker at all.
    #[error(
        "the VM shim exited before it accepted a connection{}",
        reason.as_deref().map(|reason| format!(": {reason}")).unwrap_or_default()
    )]
    ShimExited {
        /// What the shim recorded in `shim.err`, if anything.
        reason: Option<String>,
    },
    /// The shim never became reachable, or refused the handshake.
    #[error(transparent)]
    Shim(#[from] ShimConnectError),
    /// Firecracker exited before its API socket answered.
    #[error("Firecracker exited before its API socket became ready")]
    ExitedBeforeReady,
    /// The API socket never answered within the readiness timeout.
    #[error("Firecracker API socket {path} did not become ready within {timeout:?}")]
    NotReady {
        /// The API socket path that never became ready.
        path: PathBuf,
        /// The timeout that was exceeded.
        timeout: Duration,
    },
}

/// The JSON body Firecracker's `--config-file` expects.
#[derive(Debug, Serialize)]
pub struct FirecrackerConfig {
    /// Kernel image path and boot args.
    #[serde(rename = "boot-source")]
    boot_source: BootSource,
    /// Block devices, always exactly the one root drive today.
    drives: Vec<Drive>,
    /// The VM's TAP interface, absent until TAP automation assigns one.
    #[serde(rename = "network-interfaces", skip_serializing_if = "Vec::is_empty")]
    network_interfaces: Vec<NetworkInterface>,
    /// vCPU/memory sizing.
    #[serde(rename = "machine-config")]
    machine_config: MachineConfig,
}

/// `boot-source` section of [`FirecrackerConfig`].
#[derive(Debug, Serialize)]
struct BootSource {
    /// Absolute path to the kernel image.
    kernel_image_path: PathBuf,
    /// Absolute path to the initrd image, if this template's kernel needs
    /// one (e.g. a distro kernel whose virtio_blk/ext4 are modules rather
    /// than builtin).
    #[serde(skip_serializing_if = "Option::is_none")]
    initrd_path: Option<PathBuf>,
    /// Kernel command line.
    boot_args: String,
}

/// One entry in [`FirecrackerConfig`]'s `drives` list.
#[derive(Debug, Serialize)]
struct Drive {
    /// Firecracker-side drive identifier.
    drive_id: String,
    /// Absolute host path to the backing file.
    path_on_host: PathBuf,
    /// Whether this drive is mounted as the VM's root filesystem.
    is_root_device: bool,
    /// Whether the drive is exposed read-only to the guest.
    is_read_only: bool,
}

/// `machine-config` section of [`FirecrackerConfig`].
#[derive(Debug, Serialize)]
struct MachineConfig {
    /// vCPU count.
    vcpu_count: u8,
    /// RAM in MiB.
    mem_size_mib: u32,
}

/// One entry in [`FirecrackerConfig`]'s `network-interfaces` list.
#[derive(Debug, Serialize)]
struct NetworkInterface {
    /// Firecracker-side interface identifier.
    iface_id: String,
    /// MAC address presented to the guest.
    guest_mac: String,
    /// Host-side TAP device name this interface is backed by.
    host_dev_name: String,
}

/// The VM's TAP attachment: its host TAP device name (from
/// [`crate::network::tap_name`]) and the guest MAC its IPAM lease pins.
#[derive(Debug, Clone)]
pub struct VmNetwork {
    /// Host-side TAP device name.
    pub tap_name: String,
    /// MAC address the guest's `eth0` presents.
    pub guest_mac: MacAddr,
}

impl FirecrackerConfig {
    /// Builds the config for `vm`'s single root drive at `rootfs_path`, and
    /// its network interface if TAP automation has attached one.
    pub fn for_vm(
        vm: &VmRecord,
        kernel_image_path: &Path,
        initrd_path: Option<&Path>,
        boot_args: &str,
        rootfs_path: &Path,
        network: Option<&VmNetwork>,
    ) -> Self {
        Self {
            boot_source: BootSource {
                kernel_image_path: kernel_image_path.to_owned(),
                initrd_path: initrd_path.map(Path::to_owned),
                boot_args: boot_args.to_owned(),
            },
            drives: vec![Drive {
                drive_id: "rootfs".to_owned(),
                path_on_host: rootfs_path.to_owned(),
                is_root_device: true,
                is_read_only: false,
            }],
            network_interfaces: network
                .map(|network| {
                    vec![NetworkInterface {
                        iface_id: "eth0".to_owned(),
                        guest_mac: network.guest_mac.to_string(),
                        host_dev_name: network.tap_name.clone(),
                    }]
                })
                .unwrap_or_default(),
            machine_config: MachineConfig {
                vcpu_count: vm.cpu,
                mem_size_mib: vm.ram,
            },
        }
    }
}

/// Renders the VM's Firecracker config into this start's runtime directory.
/// The root drive points at the active generation file from `prepare_rootfs`.
pub fn write_config(
    runtime: &crate::artifacts::HostRuntimePaths,
    rootfs_path: &Path,
    vm: &VmRecord,
    kernel_image_path: &Path,
    initrd_path: Option<&Path>,
    boot_args: &str,
    network: Option<&VmNetwork>,
) -> Result<PathBuf, FirecrackerError> {
    fs::create_dir_all(&runtime.dir).map_err(|source| FirecrackerError::CreateDirectory {
        path: runtime.dir.clone(),
        source,
    })?;

    let path = runtime.config.clone();
    let config = FirecrackerConfig::for_vm(
        vm,
        kernel_image_path,
        initrd_path,
        boot_args,
        rootfs_path,
        network,
    );
    let json =
        serde_json::to_vec_pretty(&config).map_err(|source| FirecrackerError::Serialize {
            path: path.clone(),
            source,
        })?;
    fs::write(&path, json).map_err(|source| FirecrackerError::Write {
        path: path.clone(),
        source,
    })?;
    Ok(path)
}

/// Resolves the Firecracker binary path: `FIRECRAB_FIRECRACKER_BIN` if set,
/// otherwise `firecracker` looked up on `PATH`.
pub fn default_firecracker_binary() -> PathBuf {
    env::var_os("FIRECRAB_FIRECRACKER_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("firecracker"))
}

/// Path to this start's Firecracker API Unix socket.
#[cfg(test)]
pub fn api_sock_path(runtime: &crate::artifacts::HostRuntimePaths) -> PathBuf {
    runtime.api_socket.clone()
}

/// Path to this start's tee'd guest console log file.
pub fn console_log_path(runtime: &crate::artifacts::HostRuntimePaths) -> PathBuf {
    runtime.console_log.clone()
}

/// How the API starts a VM's shim (`crate::vm_shim`).
#[derive(Debug, Clone)]
pub(crate) enum ShimLauncher {
    /// Runs `program vm-shim …` as a child process. Until startup
    /// reconciliation exists (#123), the shim is still tied to the API: the
    /// handle's drop kills it and the API's own death SIGTERMs it.
    Process {
        /// This binary.
        program: PathBuf,
    },
    /// Runs the shim as a task inside this process. Tests use it because a
    /// test binary cannot exec itself as `vm-shim`.
    #[cfg(test)]
    InProcess,
}

impl ShimLauncher {
    /// Re-executes this binary. Resolved once at startup: after a
    /// self-update replaces the file, `/proc/self/exe` names a deleted inode
    /// while the original path names the new binary, whose shim the
    /// protocol handshake then accepts or refuses on its version.
    pub(crate) fn this_binary() -> Self {
        Self::Process {
            program: env::current_exe().unwrap_or_else(|_| PathBuf::from("firecrab-api")),
        }
    }
}

/// How a VM's session with its shim ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VmExit {
    /// Firecracker exited and the shim reported how.
    Exited(ExitStatus),
    /// The shim went away without reporting an exit.
    Lost,
}

impl VmExit {
    fn clean(self) -> bool {
        matches!(self, VmExit::Exited(status) if status.clean())
    }
}

/// A running shim this API launched.
#[derive(Debug)]
enum ShimHandle {
    Process(Child),
    #[cfg(test)]
    Task(tokio::task::JoinHandle<io::Result<ExitStatus>>),
}

impl ShimHandle {
    /// Resolves once the shim itself has exited. Awaited at most once.
    async fn wait(&mut self) {
        match self {
            ShimHandle::Process(child) => {
                let _ = child.wait().await;
            }
            #[cfg(test)]
            ShimHandle::Task(handle) => {
                let _ = handle.await;
            }
        }
    }
}

impl Drop for ShimHandle {
    fn drop(&mut self) {
        // A `Process` child is killed by `kill_on_drop`; an in-process shim
        // has to be cancelled, which drops (and so kills) its Firecracker.
        #[cfg(test)]
        if let ShimHandle::Task(handle) = self {
            handle.abort();
        }
    }
}

/// A started VM that has not been handed to [`register_and_watch`] yet.
/// Dropping it kills the VM, so an early return during startup never leaks
/// a running guest.
#[derive(Debug)]
pub struct FirecrackerProcess {
    /// `None` once registered: the exit monitor owns the shim from then on.
    shim: Option<ShimHandle>,
    vmm_pid: u32,
    control: ShimControl,
    console: Arc<ConsoleBroker>,
    exit: watch::Receiver<Option<VmExit>>,
    api_sock: PathBuf,
}

impl FirecrackerProcess {
    /// Firecracker's OS process id.
    pub fn pid(&self) -> Option<u32> {
        Some(self.vmm_pid)
    }

    /// This VM's serial console broker, for watching its boot output (e.g.
    /// the network-readiness sentinel line) before it's registered with
    /// [`register_and_watch`].
    pub fn console(&self) -> &ConsoleBroker {
        self.console.as_ref()
    }

    /// Kills the VM and waits (up to `grace` per step) for Firecracker and
    /// its shim to be gone, for start paths that fail after launch.
    pub async fn abort(self, grace: Duration) {
        self.control.kill();
        self.shut_down(grace).await;
    }

    async fn shut_down(mut self, grace: Duration) {
        let mut exit = self.exit.clone();
        let _ = tokio::time::timeout(grace, exit.wait_for(Option::is_some)).await;
        if let Some(mut shim) = self.shim.take() {
            let _ = tokio::time::timeout(grace, shim.wait()).await;
        }
    }
}

impl Drop for FirecrackerProcess {
    fn drop(&mut self) {
        if self.shim.is_some() {
            self.control.kill();
        }
    }
}

/// Map entry for a live VM: Firecracker's process id, a channel that
/// resolves once the exit monitor has recorded the terminal state, the
/// console broker for anyone attaching a terminal to this VM's ttyS0, and
/// the shim control used to stop it.
#[derive(Debug, Clone)]
pub struct VmProcess {
    /// Firecracker's OS process id.
    pub pid: u32,
    /// Resolves to `true` once the exit monitor has recorded the final state.
    pub exited: watch::Receiver<bool>,
    /// Broker for anyone attaching a terminal to this VM's ttyS0.
    pub console: Arc<ConsoleBroker>,
    /// Stop/kill requests for this VM's shim.
    pub control: ShimControl,
}

/// Sends `SIGKILL` to `pid`. The last resort when a VM's shim does not
/// confirm that Firecracker exited; tests also use it to kill a guest behind
/// the API's back.
pub fn sigkill(pid: u32) {
    // SAFETY: sending a signal is memory-safe; pid races only misdeliver signals.
    unsafe {
        libc::kill(pid as i32, libc::SIGKILL);
    }
}

/// Makes the shim stop its VM if the API that launched it disappears: the
/// parent-death signal is SIGTERM, which the shim turns into SIGTERM, then
/// SIGKILL, for Firecracker. Startup reconciliation (#123) has to exist
/// before a VM may outlive the API; until then, a VM nothing tracks would
/// keep a TAP, lease, and nft policy that look owned by a dead API.
///
/// The parent-PID check handles the tiny race where the API exits between the
/// fork and the prctl call: a re-parented child then refuses to start rather
/// than becoming an untracked shim.
fn terminate_with_parent(command: &mut Command) {
    // SAFETY: reads this process's PID before the child is forked.
    let parent_pid = unsafe { libc::getpid() };
    // SAFETY: `pre_exec` runs the closure only in the child between fork and
    // exec. The closure uses only Linux process-control syscalls and reports
    // failure back through `spawn`.
    unsafe {
        command.pre_exec(move || {
            // SAFETY: both libc calls are async-signal-safe process-control
            // syscalls and this is the only code run in the post-fork child.
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) != 0 {
                return Err(io::Error::last_os_error());
            }
            if libc::getppid() != parent_pid {
                return Err(io::Error::from_raw_os_error(libc::ESRCH));
            }
            Ok(())
        });
    }
}

/// Registers the process in the state map and spawns the exit monitor.
///
/// The monitor is the only writer of guest-initiated terminal states: a
/// clean exit lands on `stopped`, a crash or a lost shim on `error`, and an
/// exit while the record is `stopping` always lands on `stopped` so the stop
/// API and the monitor never fight over the result.
pub fn register_and_watch(state: &AppState, id: Uuid, mut process: FirecrackerProcess) {
    let (exited_tx, exited_rx) = watch::channel(false);
    let pid = process.vmm_pid;
    let shim = process.shim.take();
    let mut exit = process.exit.clone();
    let api_sock = process.api_sock.clone();
    // Drop any leftover samples from a previous generation before this PID
    // starts reporting (exit monitors only clear when they still own the map).
    state
        .process_metrics
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clear(id);
    state
        .processes
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(
            id,
            VmProcess {
                pid,
                exited: exited_rx,
                console: Arc::clone(&process.console),
                control: process.control.clone(),
            },
        );

    let state = state.clone();
    let watched_pid = pid;
    tokio::spawn(async move {
        let outcome = match exit.wait_for(Option::is_some).await {
            Ok(outcome) => outcome.unwrap_or(VmExit::Lost),
            Err(_) => VmExit::Lost,
        };
        if let Some(mut shim) = shim {
            // The shim exits right after reporting; reap it so a `Process`
            // shim never lingers as a zombie.
            let _ = tokio::time::timeout(state.runtime.stop_grace, shim.wait()).await;
        }
        let _ = fs::remove_file(&api_sock);

        // Only tear down map/metrics entries for *this* process. After a
        // quick stop→start the new generation may already be registered under
        // the same VM id; a late exit monitor must not remove it or wipe the
        // new guest-agent samples (the root cause of empty Resource usage
        // after restart).
        let still_ours = {
            let mut processes = state
                .processes
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            match processes.get(&id) {
                Some(current) if current.pid == watched_pid => {
                    processes.remove(&id);
                    true
                }
                // Newer generation already registered, or this id is no longer
                // in the map (stop/start race). Do not claim ownership — a late
                // monitor must not wipe the new process's metrics or flip state.
                Some(_) | None => false,
            }
        };
        if still_ours {
            state
                .process_metrics
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clear(id);
        }

        let clean_exit = outcome.clean();
        let updated = {
            let mut vms = state
                .vms
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            match vms.get_mut(&id) {
                Some(vm)
                    if still_ours
                        && matches!(
                            vm.state,
                            VmState::Starting | VmState::Running | VmState::Stopping
                        ) =>
                {
                    vm.state = if vm.state == VmState::Stopping || clean_exit {
                        VmState::Stopped
                    } else {
                        VmState::Error
                    };
                    vm.startup_step = None;
                    Some(vm.clone())
                }
                _ => None,
            }
        };

        if let Some(record) = updated {
            tracing::info!(
                vm_id = %id,
                clean_exit,
                ?outcome,
                state = ?record.state,
                "vm process exited"
            );
            // Only stop_vm's own SIGTERM path tears the network down today;
            // a guest-initiated poweroff or an external kill lands here
            // instead and would otherwise leave the TAP/policy orphaned.
            crate::handlers::vms::teardown_vm_network(&state, id).await;
            let store = state.store.clone();
            match tokio::task::spawn_blocking(move || store.update(&record)).await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    tracing::error!(vm_id = %id, %error, "failed to persist exit state");
                }
                Err(error) => {
                    tracing::error!(vm_id = %id, %error, "exit state persistence task failed");
                }
            }
        }

        let _ = exited_tx.send(true);
    });
}

/// Launches the VM's shim, attaches to it, and waits until Firecracker's API
/// socket answers. Any failure kills the VM before the error returns, so a
/// failed start never leaks a running guest.
pub(crate) async fn spawn_vm(
    config: &RuntimeConfig,
    runtime: &crate::artifacts::HostRuntimePaths,
    id: Uuid,
    enable_pci: bool,
    process_metrics: Arc<Mutex<crate::process_metrics::ProcessMetricsTracker>>,
) -> Result<FirecrackerProcess, FirecrackerError> {
    fs::create_dir_all(&runtime.dir).map_err(|source| FirecrackerError::CreateDirectory {
        path: runtime.dir.clone(),
        source,
    })?;

    let mut shim = launch_shim(
        &config.shim,
        ShimConfig {
            vm_id: id,
            runtime: runtime.clone(),
            firecracker: config.firecracker_binary.clone(),
            enable_pci,
            stop_grace: config.stop_grace,
        },
    )
    .await?;
    // A shim that cannot start Firecracker exits without ever accepting;
    // racing its exit keeps that from costing the whole ready timeout.
    let session = tokio::select! {
        session = crate::vm_shim::client::connect(&runtime.shim_socket, config.ready_timeout) => session?,
        () = shim.wait() => {
            let reason = fs::read_to_string(&runtime.shim_error)
                .ok()
                .map(|reason| reason.trim().to_owned())
                .filter(|reason| !reason.is_empty());
            return Err(FirecrackerError::ShimExited { reason });
        }
    };
    let ShimSession {
        vmm_pid,
        control,
        events,
    } = session;

    let console = Arc::new(ConsoleBroker::new());
    console.attach_control(control.clone());
    let (exit_tx, exit) = watch::channel(None);
    spawn_event_pump(id, events, Arc::clone(&console), process_metrics, exit_tx);
    let process = FirecrackerProcess {
        shim: Some(shim),
        vmm_pid,
        control,
        console,
        exit,
        api_sock: runtime.api_socket.clone(),
    };

    let mut exited = process.exit.clone();
    let ready = tokio::select! {
        ready = wait_ready(&runtime.api_socket, config.ready_timeout) => ready,
        _ = exited.wait_for(Option::is_some) => Err(FirecrackerError::ExitedBeforeReady),
    };
    if let Err(error) = ready {
        process.abort(config.stop_grace).await;
        return Err(error);
    }
    Ok(process)
}

async fn launch_shim(
    launcher: &ShimLauncher,
    config: ShimConfig,
) -> Result<ShimHandle, FirecrackerError> {
    match launcher {
        ShimLauncher::Process { program } => {
            let mut command = Command::new(program);
            command
                .arg0(crate::vm_shim::PROCESS_NAME)
                .arg(crate::vm_shim::SUBCOMMAND)
                .args(crate::vm_shim::command_args(&config))
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .kill_on_drop(true);
            terminate_with_parent(&mut command);
            // `execve` of a program that was just written can fail with
            // `ETXTBSY` while another test's forked child still holds a
            // write descriptor on it; the installed binary never changes
            // under a running API, so the retry is test-only.
            const BUSY_ATTEMPTS: u32 = 8;
            let mut attempt = 0;
            loop {
                match command.spawn() {
                    Ok(child) => return Ok(ShimHandle::Process(child)),
                    Err(source)
                        if cfg!(test)
                            && source.kind() == io::ErrorKind::ExecutableFileBusy
                            && attempt < BUSY_ATTEMPTS =>
                    {
                        attempt += 1;
                        tokio::time::sleep(Duration::from_millis(10 * u64::from(attempt))).await;
                    }
                    Err(source) => {
                        return Err(FirecrackerError::Spawn {
                            program: program.clone(),
                            source,
                        });
                    }
                }
            }
        }
        #[cfg(test)]
        ShimLauncher::InProcess => Ok(ShimHandle::Task(tokio::spawn(
            crate::vm_shim::server::serve(config, std::future::pending()),
        ))),
    }
}

/// Feeds the shim's console stream to the metrics tracker and the console
/// broker, and publishes how the session ended.
fn spawn_event_pump(
    id: Uuid,
    mut events: mpsc::UnboundedReceiver<SessionEvent>,
    console: Arc<ConsoleBroker>,
    process_metrics: Arc<Mutex<crate::process_metrics::ProcessMetricsTracker>>,
    exit: watch::Sender<Option<VmExit>>,
) {
    tokio::spawn(async move {
        let outcome = loop {
            match events.recv().await {
                Some(SessionEvent::Output(chunk)) => {
                    // Metrics read the raw stream (FIRECRAB_USAGE lines);
                    // the broker filters those out for viewers.
                    process_metrics
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .ingest_console(id, &chunk);
                    console.push_output(&chunk);
                }
                Some(SessionEvent::Exited(status)) => break VmExit::Exited(status),
                Some(SessionEvent::Lost) | None => break VmExit::Lost,
            }
        };
        let _ = exit.send(Some(outcome));
    });
}

/// Polls the API socket with a minimal HTTP request until it answers or
/// `timeout` elapses.
async fn wait_ready(api_sock: &Path, timeout: Duration) -> Result<(), FirecrackerError> {
    let probe = async {
        loop {
            if let Ok(mut stream) = UnixStream::connect(api_sock).await
                && stream
                    .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n")
                    .await
                    .is_ok()
            {
                let mut buffer = [0_u8; 32];
                if let Ok(read) = stream.read(&mut buffer).await
                    && read > 0
                {
                    return;
                }
            }
            tokio::time::sleep(READY_POLL_INTERVAL).await;
        }
    };

    tokio::time::timeout(timeout, probe)
        .await
        .map_err(|_| FirecrackerError::NotReady {
            path: api_sock.to_owned(),
            timeout,
        })
}

/// Stops an unregistered VM: SIGTERM through the shim, SIGKILL after
/// `grace`, then waits for the shim to exit. Registered VMs are stopped
/// through their [`VmProcess::control`] instead, since the exit monitor owns
/// them.
#[cfg(test)]
pub async fn stop_vm(process: FirecrackerProcess, grace: Duration) -> Result<(), FirecrackerError> {
    process.control.terminate();
    let mut exit = process.exit.clone();
    if tokio::time::timeout(grace, exit.wait_for(Option::is_some))
        .await
        .is_err()
    {
        process.control.kill();
    }
    process.shut_down(grace).await;
    Ok(())
}

/// Test-only helpers for standing in a fake Firecracker binary (a small
/// Python script) so process-spawning tests don't need the real one.
#[cfg(test)]
pub(crate) mod test_support {
    use std::fs;
    use std::path::{Path, PathBuf};

    /// Every fake writes "{api_sock}.pid" so tests can probe the process after
    /// the FirecrackerProcess handle is consumed.
    pub const FAKE_PRELUDE: &str = r#"#!/usr/bin/env python3
import os, signal, socket, sys, time
sock_path = sys.argv[sys.argv.index("--api-sock") + 1]
open(sock_path + ".pid", "w").write(str(os.getpid()))
"#;

    /// Answers the readiness probe forever, like a running guest.
    pub const SERVE_LOOP: &str = r#"
print("booted", flush=True)
print("FIRECRAB_NETWORK_READY 172.30.0.5", flush=True)
srv = socket.socket(socket.AF_UNIX)
srv.bind(sock_path)
srv.listen(1)
while True:
    conn, _ = srv.accept()
    conn.recv(1024)
    conn.sendall(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}")
    conn.close()
"#;

    /// Serves the readiness probe once, then exits like a guest poweroff.
    pub const SERVE_ONCE_THEN_EXIT: &str = r#"
print("FIRECRAB_NETWORK_READY 172.30.0.5", flush=True)
srv = socket.socket(socket.AF_UNIX)
srv.bind(sock_path)
srv.listen(1)
conn, _ = srv.accept()
conn.recv(1024)
conn.sendall(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}")
conn.close()
sys.exit(0)
"#;

    /// Writes an executable fake Firecracker script combining the prelude
    /// with `body`.
    ///
    /// The bytes are fsync'd and renamed into place so `execve` does not see
    /// a writer still attached. Without that, concurrent `cargo test --release`
    /// runs hit `ETXTBSY` (`ExecutableFileBusy`) on this sandbox.
    pub fn fake_firecracker(directory: &Path, body: &str) -> PathBuf {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;

        fs::create_dir_all(directory).unwrap();
        let path = directory.join("fake-firecracker");
        let tmp = directory.join("fake-firecracker.tmp");
        {
            let mut file = fs::File::create(&tmp).unwrap();
            file.write_all(format!("{FAKE_PRELUDE}{body}").as_bytes())
                .unwrap();
            file.sync_all().unwrap();
        }
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o755)).unwrap();
        fs::rename(&tmp, &path).unwrap();
        path
    }

    /// A tempdir under `/tmp`: Unix socket paths are capped near 108 bytes,
    /// so this avoids a deeply nested `TMPDIR`.
    pub fn short_tempdir() -> tempfile::TempDir {
        tempfile::tempdir_in("/tmp").unwrap()
    }

    /// Runs a fake Firecracker, tolerating `ETXTBSY` the way
    /// [`super::spawn_firecracker`] does.
    ///
    /// `execve` refuses a file that any process still holds open for writing,
    /// and a concurrent test's forked child inherits exactly such a descriptor
    /// for the moment between its `fork` and its `exec`. `rename` cannot help,
    /// because the descriptor follows the inode.
    pub fn exec_tolerating_busy(path: &Path, api_sock: &Path) -> std::process::ExitStatus {
        const BUSY_ATTEMPTS: u32 = 8;
        let mut attempt = 0;
        loop {
            match std::process::Command::new(path)
                .arg("--api-sock")
                .arg(api_sock)
                .status()
            {
                Ok(status) => return status,
                Err(source)
                    if source.kind() == std::io::ErrorKind::ExecutableFileBusy
                        && attempt < BUSY_ATTEMPTS =>
                {
                    attempt += 1;
                    std::thread::sleep(std::time::Duration::from_millis(10 * u64::from(attempt)));
                }
                Err(source) => panic!("fake-firecracker must be executable: {source}"),
            }
        }
    }

    /// Whether a process with `pid` still exists.
    pub fn process_alive(pid: i32) -> bool {
        // SAFETY: signal 0 only probes for existence.
        unsafe { libc::kill(pid, 0) == 0 }
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;
    use uuid::Uuid;

    use super::*;
    use crate::model::VmState;
    use core::assert_matches;

    fn record(cpu: u8, ram: u32) -> VmRecord {
        VmRecord {
            id: Uuid::new_v4(),
            name: "test-vm".to_owned(),
            purpose: crate::model::VmPurpose::Instance,
            state: VmState::Created,
            template: "ubuntu-26.04".to_owned(),
            template_version: "ubuntu-26.04-v1".to_owned(),
            template_kernel_sha256: "kernel".to_owned(),
            template_rootfs_sha256: "rootfs".to_owned(),
            template_boot_args_sha256: "args".to_owned(),
            cpu,
            ram,
            disk_gb: 2,
            egress_policy: Default::default(),
            micro_network_id: Uuid::from_u128(1),
            storage_root: "default".to_owned(),
            disk_generation: None,
            last_runtime_id: None,
            startup_step: None,
            startup_timeline: Vec::new(),
            env: Default::default(),
        }
    }

    fn runtime_for(vms_root: &Path, vm_id: Uuid) -> crate::artifacts::HostRuntimePaths {
        let paths = crate::artifacts::VmArtifactPaths::for_vm(vms_root, vm_id);
        paths.create_runtime(Uuid::new_v4()).unwrap()
    }

    fn test_config(binary: &Path, ready_timeout: Duration) -> RuntimeConfig {
        RuntimeConfig {
            vms_dir: PathBuf::from("/nonexistent"),
            firecracker_binary: binary.to_owned(),
            ready_timeout,
            stop_grace: Duration::from_secs(5),
            network_ready_timeout: Duration::from_secs(5),
            shim: ShimLauncher::InProcess,
        }
    }

    fn test_metrics() -> Arc<Mutex<crate::process_metrics::ProcessMetricsTracker>> {
        Arc::new(Mutex::new(
            crate::process_metrics::ProcessMetricsTracker::default(),
        ))
    }

    #[test]
    fn config_reflects_requested_resources() {
        let directory = tempdir().unwrap();
        let vms_dir = directory.path().join("vms");
        let vm = record(3, 768);
        let runtime = runtime_for(&vms_dir, vm.id);
        let rootfs = Path::new("/tmp/rootfs.ext4");

        let path = write_config(
            &runtime,
            rootfs,
            &vm,
            Path::new("/images/vmlinux"),
            None,
            "console=ttyS0",
            None,
        )
        .unwrap();

        assert_eq!(path, runtime.config);
        let config: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(config["machine-config"]["vcpu_count"], 3);
        assert_eq!(config["machine-config"]["mem_size_mib"], 768);
    }

    #[test]
    fn config_wires_boot_source_and_root_drive() {
        let directory = tempdir().unwrap();
        let vms_dir = directory.path().join("vms");
        let vm = record(1, 512);
        let runtime = runtime_for(&vms_dir, vm.id);
        let rootfs = Path::new("/tmp/guest-rootfs.ext4");

        let path = write_config(
            &runtime,
            rootfs,
            &vm,
            Path::new("/images/vmlinux"),
            None,
            "console=ttyS0",
            None,
        )
        .unwrap();

        let config: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(
            config["boot-source"]["kernel_image_path"],
            "/images/vmlinux"
        );
        assert_eq!(config["boot-source"]["boot_args"], "console=ttyS0");

        let drive = &config["drives"][0];
        assert_eq!(drive["drive_id"], "rootfs");
        assert_eq!(drive["is_root_device"], true);
        assert_eq!(drive["is_read_only"], false);
        assert_eq!(drive["path_on_host"], rootfs.to_str().unwrap());
    }

    #[test]
    fn initrd_path_is_omitted_when_the_template_has_none() {
        let directory = tempdir().unwrap();
        let vms_dir = directory.path().join("vms");
        let vm = record(1, 512);
        let runtime = runtime_for(&vms_dir, vm.id);

        let path = write_config(
            &runtime,
            Path::new("/tmp/rootfs.ext4"),
            &vm,
            Path::new("/images/vmlinux"),
            None,
            "console=ttyS0",
            None,
        )
        .unwrap();

        let config: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert!(config["boot-source"].get("initrd_path").is_none());
    }

    #[test]
    fn initrd_path_is_wired_through_when_the_template_has_one() {
        let directory = tempdir().unwrap();
        let vms_dir = directory.path().join("vms");
        let vm = record(1, 512);
        let runtime = runtime_for(&vms_dir, vm.id);

        let path = write_config(
            &runtime,
            Path::new("/tmp/rootfs.ext4"),
            &vm,
            Path::new("/images/vmlinux"),
            Some(Path::new("/images/initrd")),
            "console=ttyS0",
            None,
        )
        .unwrap();

        let config: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(config["boot-source"]["initrd_path"], "/images/initrd");
    }

    #[test]
    fn rewriting_config_overwrites_previous_content() {
        let directory = tempdir().unwrap();
        let vms_dir = directory.path().join("vms");
        let mut vm = record(1, 512);
        let runtime = runtime_for(&vms_dir, vm.id);

        write_config(
            &runtime,
            Path::new("/tmp/rootfs.ext4"),
            &vm,
            Path::new("/images/vmlinux"),
            None,
            "console=ttyS0",
            None,
        )
        .unwrap();
        vm.cpu = 2;
        vm.ram = 1024;
        let path = write_config(
            &runtime,
            Path::new("/tmp/rootfs.ext4"),
            &vm,
            Path::new("/images/vmlinux"),
            None,
            "console=ttyS0",
            None,
        )
        .unwrap();

        let config: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(config["machine-config"]["vcpu_count"], 2);
        assert_eq!(config["machine-config"]["mem_size_mib"], 1024);
    }

    use super::test_support::{
        SERVE_LOOP, exec_tolerating_busy, fake_firecracker, process_alive, short_tempdir,
    };

    /// Guards the `fake_firecracker` write: the +x bit is set and the body is
    /// complete, so a fresh fake runs.
    ///
    /// `ETXTBSY` is tolerated the same way [`spawn_firecracker`] tolerates it,
    /// because it is not something the helper can prevent: another test's
    /// forked child can still hold a write descriptor on the file we just
    /// wrote, and `rename` leaves the inode — and therefore that descriptor —
    /// untouched.
    #[test]
    fn fake_firecracker_script_can_be_execd_immediately() {
        let directory = short_tempdir();
        let sock = directory.path().join("sock");
        let path = fake_firecracker(directory.path(), "sys.exit(0)\n");
        let status = exec_tolerating_busy(&path, &sock);
        assert!(
            status.success(),
            "fresh fake-firecracker must be executable"
        );
    }

    /// Exercises the `ETXTBSY` retry instead of assuming it: an open write
    /// descriptor makes `execve` fail for exactly as long as it lives, which
    /// is the race a parallel run hits by accident.
    #[test]
    fn a_busy_fake_firecracker_is_retried_until_the_writer_closes_it() {
        let directory = short_tempdir();
        let sock = directory.path().join("sock");
        let path = fake_firecracker(directory.path(), "sys.exit(0)\n");

        let writer = fs::OpenOptions::new().write(true).open(&path).unwrap();
        match std::process::Command::new(&path).status() {
            Err(error) if error.kind() == io::ErrorKind::ExecutableFileBusy => {}
            other => panic!(
                "an open write descriptor must make execve fail with ETXTBSY, \
                 otherwise this test proves nothing: {other:?}"
            ),
        }
        let releaser = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(60));
            drop(writer);
        });

        let status = exec_tolerating_busy(&path, &sock);
        releaser.join().unwrap();
        assert!(
            status.success(),
            "the retry must outlast a transient ETXTBSY"
        );
    }

    fn fake_pid(runtime: &crate::artifacts::HostRuntimePaths) -> i32 {
        let pid_file = format!("{}.pid", api_sock_path(runtime).display());
        fs::read_to_string(pid_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap()
    }

    #[tokio::test]
    async fn spawn_reaches_readiness_and_stop_terminates_the_process() {
        let directory = short_tempdir();
        let vms_dir = directory.path().join("vms");
        let binary = fake_firecracker(
            directory.path(),
            &format!("signal.signal(signal.SIGTERM, lambda *_: sys.exit(0)){SERVE_LOOP}"),
        );
        let id = Uuid::new_v4();
        let mut runtime = runtime_for(&vms_dir, id);
        fs::write(&runtime.config, "{}").unwrap();

        let process = spawn_vm(
            &test_config(&binary, Duration::from_secs(5)),
            &runtime,
            id,
            false,
            test_metrics(),
        )
        .await
        .unwrap();
        let pid = process.pid().unwrap() as i32;
        assert!(process_alive(pid));

        // The broker must see the same bytes the shim tees to the log file.
        // They travel shim → socket → broker, so give them a moment.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let (backlog, _receiver) = process.console.subscribe();
            if String::from_utf8_lossy(&backlog).contains("booted") {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the console broker never saw the guest's boot output"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        stop_vm(process, Duration::from_secs(5)).await.unwrap();

        assert!(!process_alive(pid));
        let console = fs::read_to_string(console_log_path(&runtime)).unwrap();
        assert!(console.contains("booted"));
        let _ = &mut runtime;
    }

    #[tokio::test]
    async fn readiness_timeout_cleans_up_the_process() {
        let directory = short_tempdir();
        let vms_dir = directory.path().join("vms");
        let binary = fake_firecracker(directory.path(), "while True:\n    time.sleep(60)\n");
        let id = Uuid::new_v4();
        let runtime = runtime_for(&vms_dir, id);
        fs::write(&runtime.config, "{}").unwrap();

        let error = spawn_vm(
            &test_config(&binary, Duration::from_millis(500)),
            &runtime,
            id,
            false,
            test_metrics(),
        )
        .await
        .unwrap_err();

        assert_matches!(error, FirecrackerError::NotReady { .. });
        assert!(!process_alive(fake_pid(&runtime)));
    }

    #[tokio::test]
    async fn a_vmm_that_dies_before_readiness_fails_the_start_fast() {
        let directory = short_tempdir();
        let vms_dir = directory.path().join("vms");
        let binary = fake_firecracker(directory.path(), "sys.exit(7)\n");
        let id = Uuid::new_v4();
        let runtime = runtime_for(&vms_dir, id);
        fs::write(&runtime.config, "{}").unwrap();

        let started = std::time::Instant::now();
        let result = spawn_vm(
            &test_config(&binary, Duration::from_secs(5)),
            &runtime,
            id,
            false,
            test_metrics(),
        )
        .await;

        assert_matches!(
            result,
            Err(FirecrackerError::ShimExited { .. } | FirecrackerError::ExitedBeforeReady)
        );
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "an exited VMM must fail the start without waiting out the ready timeout, took {:?}",
            started.elapsed()
        );
    }

    /// The most common install error must stay visible in the startup
    /// timeline, not only in the API's journal.
    #[tokio::test]
    async fn a_missing_firecracker_binary_is_named_in_the_start_error() {
        let directory = short_tempdir();
        let id = Uuid::new_v4();
        let runtime = runtime_for(&directory.path().join("vms"), id);
        fs::write(&runtime.config, "{}").unwrap();
        let missing = directory.path().join("no-such-firecracker");

        let error = spawn_vm(
            &test_config(&missing, Duration::from_secs(5)),
            &runtime,
            id,
            false,
            test_metrics(),
        )
        .await
        .unwrap_err();

        let message = error.to_string();
        assert!(message.contains("no-such-firecracker"), "{message}");
        assert!(message.contains("No such file"), "{message}");
    }

    #[test]
    fn this_binary_launches_the_running_executable() {
        let ShimLauncher::Process { program } = ShimLauncher::this_binary() else {
            panic!("this_binary must launch a process");
        };
        assert_eq!(program, env::current_exe().unwrap());
    }

    #[tokio::test]
    async fn the_process_launcher_runs_the_program_as_a_vm_shim() {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;

        let directory = short_tempdir();
        let argv_file = directory.path().join("argv");
        // Stands in for this binary: records its arguments and exits the way
        // a shim that cannot start Firecracker would.
        let program = directory.path().join("fake-firecrab-api");
        {
            let mut file = fs::File::create(&program).unwrap();
            write!(
                file,
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nexit 1\n",
                argv_file.display()
            )
            .unwrap();
            file.sync_all().unwrap();
        }
        fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
        let id = Uuid::new_v4();
        let runtime = runtime_for(&directory.path().join("vms"), id);
        let mut config = test_config(Path::new("/opt/firecracker"), Duration::from_secs(5));
        config.shim = ShimLauncher::Process { program };

        let started = std::time::Instant::now();
        let result = spawn_vm(&config, &runtime, id, true, test_metrics()).await;

        assert_matches!(result, Err(FirecrackerError::ShimExited { .. }));
        assert!(started.elapsed() < Duration::from_secs(3));
        let argv: Vec<std::ffi::OsString> = fs::read_to_string(&argv_file)
            .unwrap()
            .lines()
            .map(std::ffi::OsString::from)
            .collect();
        assert_eq!(argv[0], crate::vm_shim::SUBCOMMAND);
        let parsed = crate::vm_shim::parse_args(argv[1..].to_vec()).unwrap();
        assert_eq!(parsed.vm_id, id);
        assert_eq!(parsed.runtime, runtime);
        assert_eq!(parsed.firecracker, Path::new("/opt/firecracker"));
        assert!(parsed.enable_pci);
        assert_eq!(parsed.stop_grace, config.stop_grace);
    }

    #[tokio::test]
    async fn stop_escalates_to_sigkill_when_sigterm_is_ignored() {
        let directory = short_tempdir();
        let vms_dir = directory.path().join("vms");
        let binary = fake_firecracker(
            directory.path(),
            &format!("signal.signal(signal.SIGTERM, signal.SIG_IGN){SERVE_LOOP}"),
        );
        let id = Uuid::new_v4();
        let runtime = runtime_for(&vms_dir, id);
        fs::write(&runtime.config, "{}").unwrap();

        let process = spawn_vm(
            &test_config(&binary, Duration::from_secs(5)),
            &runtime,
            id,
            false,
            test_metrics(),
        )
        .await
        .unwrap();
        let pid = process.pid().unwrap() as i32;
        let started = std::time::Instant::now();

        stop_vm(process, Duration::from_millis(200)).await.unwrap();

        assert!(started.elapsed() >= Duration::from_millis(200));
        assert!(!process_alive(pid));
    }

    #[tokio::test]
    async fn spawn_enables_pci_only_when_the_template_requests_it() {
        let directory = short_tempdir();
        let vms_dir = directory.path().join("vms");
        let binary = fake_firecracker(
            directory.path(),
            &format!(
                "if '--enable-pci' not in sys.argv:\n    raise SystemExit('missing --enable-pci')\n{SERVE_LOOP}"
            ),
        );
        let id = Uuid::new_v4();
        let runtime = runtime_for(&vms_dir, id);
        fs::write(&runtime.config, "{}").unwrap();

        let process = spawn_vm(
            &test_config(&binary, Duration::from_secs(5)),
            &runtime,
            id,
            true,
            test_metrics(),
        )
        .await
        .unwrap();
        stop_vm(process, Duration::from_secs(5)).await.unwrap();
    }
}
