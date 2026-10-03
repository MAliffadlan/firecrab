//! Startup reconciliation (#123): what each VM the database calls active
//! really is, decided from the evidence a previous API run left behind.
//!
//! | Record     | Evidence                          | Outcome                         |
//! |------------|-----------------------------------|---------------------------------|
//! | `running`  | shim answers                      | re-adopted                      |
//! | `stopping` | shim answers                      | re-adopted, stop finished       |
//! | `starting` | shim answers                      | killed, `error`                 |
//! | any        | shim it cannot attach to          | unit stopped, `stopped`         |
//! |            | ... and the unit stop fails       | `error`, network kept           |
//! | any        | `exit.json`: clean or requested   | `stopped`                       |
//! | any        | `exit.json`: crash                | `error`                         |
//! | any        | neither                           | `stopped` (gone)                |
//!
//! Every VM not re-adopted has its unit stopped (a no-op where none runs)
//! and loses its TAP and firewall policy; the host networks and the
//! re-adopted VMs' policies and TAPs are then re-applied.

use std::fs;
use std::path::Path;
use std::time::Duration;

use uuid::Uuid;

use crate::artifacts::{HostRuntimePaths, VmArtifactPaths};
use crate::firecracker::{self, FirecrackerProcess};
use crate::model::{VmRecord, VmState};
use crate::state::AppState;
use crate::vm_shim::client::{self, SessionEvent, ShimConnectError, ShimSession};
use crate::vm_shim::protocol::ExitReport;

/// How long a shim that accepted the connection has to send its greeting.
const ATTACH_TIMEOUT: Duration = Duration::from_secs(2);

/// Prefix and suffix of the systemd units the helper starts for VMs.
const UNIT_PREFIX: &str = "firecrab-vm-";
const UNIT_SUFFIX: &str = ".service";

/// What reconciliation decided for each VM the database called active.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct ReconcileReport {
    /// Running or stopping VMs whose shim answered, tracked again.
    pub adopted: Vec<Uuid>,
    /// Starts the restart interrupted: killed and recorded as `error`.
    pub interrupted: Vec<Uuid>,
    /// VMs that exited while the API was down, with the state recorded.
    pub ended: Vec<(Uuid, VmState)>,
    /// VMs with neither a shim nor an exit record, recorded as `stopped`.
    pub gone: Vec<Uuid>,
    /// VMs whose shim answered but cannot be controlled (another protocol
    /// version, another VM, an unreadable greeting): stopped through the
    /// helper, or recorded `error` when that fails.
    pub mismatched: Vec<Uuid>,
    /// Re-adopted VMs whose TAP could not be re-attached.
    pub network_mismatches: Vec<Uuid>,
    /// `firecrab-vm-*` units still loaded that run no re-adopted VM.
    /// Reported, never stopped.
    pub orphan_units: Vec<String>,
}

enum Evidence {
    Live(ShimSession),
    Mismatch(ShimConnectError),
    Exited(ExitReport),
    Gone,
}

/// Classifies every active VM from its runtime directory, records the
/// outcome, and re-verifies host networking. Runs once, before the API
/// serves, so nothing else changes VM state meanwhile.
pub(crate) async fn reconcile(state: &AppState) -> ReconcileReport {
    let mut report = ReconcileReport::default();
    let active: Vec<VmRecord> = state
        .vms
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .values()
        .filter(|vm| {
            matches!(
                vm.state,
                VmState::Starting | VmState::Running | VmState::Stopping
            )
        })
        .cloned()
        .collect();

    // Adopted VMs are registered only after the network is re-verified, so
    // an exit monitor's teardown can never run before a TAP re-attach.
    let mut adopted: Vec<(VmRecord, FirecrackerProcess)> = Vec::new();
    let mut released: Vec<Uuid> = Vec::new();
    for vm in active {
        let runtime = vm.last_runtime_id.map(|runtime_id| {
            VmArtifactPaths::for_vm(&state.vms_dir_for(&vm.storage_root), vm.id).runtime(runtime_id)
        });
        let evidence = match &runtime {
            Some(runtime) => gather(vm.id, runtime).await,
            None => Evidence::Gone,
        };
        let uncontrollable = matches!(evidence, Evidence::Mismatch(_));
        let outcome = match evidence {
            Evidence::Live(session) => match vm.state {
                VmState::Starting => {
                    stop_interrupted_start(session, state.runtime.stop_grace).await;
                    report.interrupted.push(vm.id);
                    VmState::Error
                }
                _ => {
                    let runtime = runtime.as_ref().expect("a live shim has a runtime");
                    let process =
                        firecracker::adopt(vm.id, runtime, session, state.process_metrics.clone());
                    report.adopted.push(vm.id);
                    adopted.push((vm, process));
                    continue;
                }
            },
            Evidence::Mismatch(error) => {
                tracing::warn!(vm_id = %vm.id, %error, "stopping a VM shim this API cannot control");
                report.mismatched.push(vm.id);
                VmState::Stopped
            }
            Evidence::Exited(exit) => {
                let outcome = if exit.stop_requested || exit.status.clean() {
                    VmState::Stopped
                } else {
                    VmState::Error
                };
                report.ended.push((vm.id, outcome));
                outcome
            }
            Evidence::Gone => {
                report.gone.push(vm.id);
                VmState::Stopped
            }
        };
        // Every VM not re-adopted loses its unit too: a no-op where none
        // runs, the backstop for one that started after we looked, and the
        // only way to stop a shim this API cannot attach to.
        let unit_stopped = match state
            .network
            .stop_vm_unit(vm.id, state.runtime.stop_grace)
            .await
        {
            Ok(()) => true,
            Err(error) => {
                tracing::warn!(vm_id = %vm.id, %error, "failed to stop the VM unit");
                false
            }
        };
        if uncontrollable && !unit_stopped {
            // It may well still be running: not `stopped`, and its TAP stays.
            record_state(state, vm.id, VmState::Error).await;
            continue;
        }
        record_state(state, vm.id, outcome).await;
        released.push(vm.id);
    }

    for &id in &released {
        crate::handlers::vms::teardown_vm_network(state, id).await;
    }
    if let Err(error) = crate::handlers::micro_networks::ensure_all_networks(state).await {
        tracing::warn!(error, "startup network resync failed");
    }
    for (vm, ..) in adopted
        .iter()
        .filter(|(vm, ..)| vm.state == VmState::Running)
    {
        if let Err(error) = state.network.create_tap(vm.id, vm.micro_network_id).await {
            tracing::warn!(vm_id = %vm.id, %error, "re-adopted VM's TAP could not be re-attached");
            report.network_mismatches.push(vm.id);
        }
    }
    for (vm, process) in adopted {
        firecracker::register_and_watch(state, vm.id, process);
        // The stop the previous run accepted is finished now, escalating like
        // the stop API; the exit monitor records `stopped` for an exit while
        // `stopping`.
        if vm.state == VmState::Stopping {
            let registered = state
                .processes
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .get(&vm.id)
                .cloned();
            if let Some(process) = registered {
                let grace = state.runtime.stop_grace;
                tokio::spawn(firecracker::stop_registered(vm.id, process, grace));
            }
        }
    }

    if let Some(units) = list_vm_units().await {
        report.orphan_units = unaccounted_units(units, &report.adopted);
    }
    for unit in &report.orphan_units {
        tracing::warn!(
            unit,
            "VM unit runs no VM this API tracks; leaving it running"
        );
    }

    tracing::info!(
        adopted = report.adopted.len(),
        interrupted = report.interrupted.len(),
        ended = report.ended.len(),
        gone = report.gone.len(),
        mismatched = report.mismatched.len(),
        network_mismatches = report.network_mismatches.len(),
        orphan_units = report.orphan_units.len(),
        "startup reconciliation finished"
    );
    report
}

/// What the previous run left for one VM: a shim still serving it, or the
/// exit record its shim wrote.
async fn gather(id: Uuid, runtime: &HostRuntimePaths) -> Evidence {
    // One plain connect first: a socket a killed shim left behind refuses
    // at once, where `client::connect` would retry for its whole timeout.
    if tokio::net::UnixStream::connect(&runtime.shim_socket)
        .await
        .is_ok()
    {
        match client::connect(&runtime.shim_socket, id, ATTACH_TIMEOUT).await {
            Ok(session) => return Evidence::Live(session),
            // Something serves this VM's socket, but not a shim this API can
            // control: another protocol version, another VM, or a greeting
            // it cannot read. Its exit record would be stale.
            Err(error) => return Evidence::Mismatch(error),
        }
    }
    match read_exit(&runtime.exit_status) {
        Some(exit) => Evidence::Exited(exit),
        None => Evidence::Gone,
    }
}

fn read_exit(path: &Path) -> Option<ExitReport> {
    let bytes = fs::read(path).ok()?;
    serde_json::from_slice(&bytes)
        .inspect_err(|error| {
            tracing::warn!(path = %path.display(), %error, "unreadable VM exit record");
        })
        .ok()
}

/// Kills a VM whose start the restart cut short — its readiness and network
/// checks never finished — and waits for its shim to confirm the exit.
async fn stop_interrupted_start(mut session: ShimSession, grace: Duration) {
    session.control.kill();
    let _ = tokio::time::timeout(grace, async {
        while let Some(event) = session.events.recv().await {
            if matches!(event, SessionEvent::Exited(_) | SessionEvent::Lost) {
                return;
            }
        }
    })
    .await;
}

async fn record_state(state: &AppState, id: Uuid, outcome: VmState) {
    let updated = {
        let mut vms = state
            .vms
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        vms.get_mut(&id).map(|vm| {
            vm.state = outcome;
            vm.startup_step = None;
            vm.clone()
        })
    };
    let Some(record) = updated else { return };
    let store = state.store.clone();
    match tokio::task::spawn_blocking(move || store.update(&record)).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            tracing::error!(vm_id = %id, %error, "failed to persist reconciled state")
        }
        Err(error) => {
            tracing::error!(vm_id = %id, %error, "reconciled state persistence task failed")
        }
    }
}

/// Every loaded `firecrab-vm-*` unit, or `None` where systemd is not there
/// to ask (a host without it runs no VM units).
async fn list_vm_units() -> Option<Vec<String>> {
    let output = tokio::process::Command::new("systemctl")
        .args([
            "list-units",
            "--all",
            "--plain",
            "--no-legend",
            "--type=service",
        ])
        .arg(format!("{UNIT_PREFIX}*"))
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|line| line.split_whitespace().next())
            .filter(|unit| unit.starts_with(UNIT_PREFIX))
            .map(str::to_owned)
            .collect(),
    )
}

/// Units that run no re-adopted VM: after every other active VM's unit was
/// stopped, these belong to no record, a record that is not active, or a
/// stop that failed.
fn unaccounted_units(units: Vec<String>, adopted: &[Uuid]) -> Vec<String> {
    units
        .into_iter()
        .filter(|unit| vm_id_from_unit(unit).is_none_or(|id| !adopted.contains(&id)))
        .collect()
}

/// The VM a helper-started unit runs, from its name.
fn vm_id_from_unit(unit: &str) -> Option<Uuid> {
    let id = unit.strip_prefix(UNIT_PREFIX)?.strip_suffix(UNIT_SUFFIX)?;
    Uuid::try_parse(id).ok()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use uuid::Uuid;

    use super::*;
    use crate::artifacts::{HostRuntimePaths, VmArtifactPaths};
    use crate::firecracker::test_support::{SERVE_LOOP, fake_firecracker, short_tempdir};
    use crate::handlers::vms::test_support::{record, seed_vm, test_state_with_binary};
    use crate::model::VmState;
    use crate::state::AppState;
    use crate::vm_shim::server::{ShimConfig, serve};

    const HONOR_SIGTERM: &str = "signal.signal(signal.SIGTERM, lambda *_: sys.exit(0))\n";

    struct Host {
        _directory: tempfile::TempDir,
        state: AppState,
        helper_log: Arc<Mutex<Vec<&'static str>>>,
        firecracker: std::path::PathBuf,
    }

    async fn host() -> Host {
        host_failing(None).await
    }

    /// A host whose helper fails `fail_operation`.
    async fn host_failing(fail_operation: Option<&'static str>) -> Host {
        host_running(&format!("{HONOR_SIGTERM}{SERVE_LOOP}"), fail_operation).await
    }

    /// A host whose fake Firecracker runs `body`.
    async fn host_running(body: &str, fail_operation: Option<&'static str>) -> Host {
        let directory = short_tempdir();
        let firecracker = fake_firecracker(directory.path(), body);
        let state = test_state_with_binary(directory.path(), firecracker.clone()).await;
        let socket = directory.path().join("recording-helper.sock");
        let (_helper, helper_log) =
            crate::network::test_support::spawn_recording_helper(&socket, fail_operation);
        let state =
            state.with_test_network(crate::network::NetworkClient::with_socket_path(socket));
        Host {
            _directory: directory,
            state,
            helper_log,
            firecracker,
        }
    }

    /// Seeds `state` with an active VM whose last start used a fresh runtime
    /// directory, and returns that directory's paths.
    fn seed_active(host: &Host, name: &str, state: VmState) -> (Uuid, HostRuntimePaths) {
        let id = Uuid::new_v4();
        let runtime_id = Uuid::new_v4();
        let runtime = VmArtifactPaths::for_vm(&host.state.runtime.vms_dir, id)
            .create_runtime(runtime_id)
            .unwrap();
        fs::write(&runtime.config, "{}").unwrap();
        let mut vm = record(name, id);
        vm.state = state;
        vm.last_runtime_id = Some(runtime_id);
        seed_vm(&host.state, &vm);
        (id, runtime)
    }

    /// A shim that outlived the previous API run, as a systemd unit's would.
    async fn surviving_shim(host: &Host, id: Uuid, runtime: &HostRuntimePaths) {
        tokio::spawn(serve(
            ShimConfig {
                vm_id: id,
                runtime: runtime.clone(),
                firecracker: host.firecracker.clone(),
                enable_pci: false,
                stop_grace: Duration::from_secs(2),
            },
            std::future::pending(),
        ));
        // The fake VMM binds its API socket only after it installs its
        // SIGTERM handler; a stop sent before then would kill it outright.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while tokio::net::UnixStream::connect(&runtime.shim_socket)
            .await
            .is_err()
            || !runtime.api_socket.exists()
        {
            assert!(
                std::time::Instant::now() < deadline,
                "shim never served its VM"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    fn write_exit(runtime: &HostRuntimePaths, json: &str) {
        fs::write(&runtime.exit_status, json).unwrap();
    }

    fn memory_state(host: &Host, id: Uuid) -> Option<VmState> {
        host.state.vms.lock().unwrap().get(&id).map(|vm| vm.state)
    }

    fn db_state(host: &Host, id: Uuid) -> Option<VmState> {
        host.state
            .store
            .load_all()
            .unwrap()
            .get(&id)
            .map(|vm| vm.state)
    }

    async fn wait_for(host: &Host, id: Uuid, want: VmState) {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while memory_state(host, id) != Some(want) || db_state(host, id) != Some(want) {
            assert!(
                std::time::Instant::now() < deadline,
                "VM never reached {want:?}: memory {:?}, db {:?}",
                memory_state(host, id),
                db_state(host, id)
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    #[tokio::test]
    async fn a_running_vm_whose_shim_answers_is_adopted_and_its_network_reverified() {
        let host = host().await;
        let (id, runtime) = seed_active(&host, "survivor", VmState::Running);
        surviving_shim(&host, id, &runtime).await;

        let report = reconcile(&host.state).await;

        assert_eq!(report.adopted, vec![id]);
        assert_eq!(memory_state(&host, id), Some(VmState::Running));
        assert!(host.state.processes.lock().unwrap().contains_key(&id));
        let calls = host.helper_log.lock().unwrap().clone();
        assert!(calls.contains(&"ensure_firewall"), "{calls:?}");
        assert!(calls.contains(&"create_tap"), "{calls:?}");
        assert!(!calls.contains(&"delete_tap"), "{calls:?}");

        // The adopted VM is controllable: its shim relays the stop, and the
        // fake VMM's clean exit on SIGTERM lands on `stopped`.
        let control = host.state.processes.lock().unwrap()[&id].control.clone();
        control.terminate();
        wait_for(&host, id, VmState::Stopped).await;
    }

    #[tokio::test]
    async fn a_stop_from_outside_the_api_is_recorded_stopped() {
        // Firecracker dies of the SIGTERM, as a real one does when its unit
        // is stopped (`systemctl stop`, host shutdown) while the API runs.
        let host = host_running(SERVE_LOOP, None).await;
        let (id, runtime) = seed_active(&host, "stopped-by-systemd", VmState::Running);
        surviving_shim(&host, id, &runtime).await;
        reconcile(&host.state).await;

        let control = host.state.processes.lock().unwrap()[&id].control.clone();
        control.terminate();

        wait_for(&host, id, VmState::Stopped).await;
    }

    #[tokio::test]
    async fn a_stopping_vm_whose_shim_answers_finishes_its_stop() {
        let host = host().await;
        let (id, runtime) = seed_active(&host, "half-stopped", VmState::Stopping);
        surviving_shim(&host, id, &runtime).await;

        let report = reconcile(&host.state).await;

        assert_eq!(report.adopted, vec![id]);
        wait_for(&host, id, VmState::Stopped).await;
    }

    #[tokio::test]
    async fn a_stopping_vm_that_ignores_sigterm_is_killed_to_finish_its_stop() {
        let host = host_running(
            &format!("signal.signal(signal.SIGTERM, signal.SIG_IGN)\n{SERVE_LOOP}"),
            None,
        )
        .await;
        let (id, runtime) = seed_active(&host, "stubborn", VmState::Stopping);
        surviving_shim(&host, id, &runtime).await;

        reconcile(&host.state).await;

        wait_for(&host, id, VmState::Stopped).await;
    }

    #[tokio::test]
    async fn a_start_interrupted_by_the_restart_is_killed_and_recorded_as_an_error() {
        let host = host().await;
        let (id, runtime) = seed_active(&host, "half-started", VmState::Starting);
        surviving_shim(&host, id, &runtime).await;

        let report = reconcile(&host.state).await;

        assert_eq!(report.interrupted, vec![id]);
        assert_eq!(memory_state(&host, id), Some(VmState::Error));
        assert_eq!(db_state(&host, id), Some(VmState::Error));
        assert!(
            runtime.exit_status.exists(),
            "the interrupted VM must be stopped"
        );
        assert!(!host.state.processes.lock().unwrap().contains_key(&id));
    }

    #[tokio::test]
    async fn a_clean_exit_while_the_api_was_down_is_stopped_and_torn_down() {
        let host = host().await;
        let (id, runtime) = seed_active(&host, "powered-off", VmState::Running);
        write_exit(
            &runtime,
            r#"{"code":0,"signal":null,"stop_requested":false}"#,
        );

        let report = reconcile(&host.state).await;

        assert_eq!(report.ended, vec![(id, VmState::Stopped)]);
        assert_eq!(db_state(&host, id), Some(VmState::Stopped));
        let calls = host.helper_log.lock().unwrap().clone();
        assert!(calls.contains(&"remove_vm_policy"), "{calls:?}");
        assert!(calls.contains(&"delete_tap"), "{calls:?}");
    }

    #[tokio::test]
    async fn a_requested_stop_while_the_api_was_down_is_stopped() {
        let host = host().await;
        let (id, runtime) = seed_active(&host, "stopped-by-signal", VmState::Running);
        write_exit(
            &runtime,
            r#"{"code":null,"signal":15,"stop_requested":true}"#,
        );

        let report = reconcile(&host.state).await;

        assert_eq!(report.ended, vec![(id, VmState::Stopped)]);
    }

    #[tokio::test]
    async fn a_crash_while_the_api_was_down_is_an_error() {
        let host = host().await;
        let (id, runtime) = seed_active(&host, "crashed", VmState::Running);
        write_exit(
            &runtime,
            r#"{"code":null,"signal":9,"stop_requested":false}"#,
        );

        let report = reconcile(&host.state).await;

        assert_eq!(report.ended, vec![(id, VmState::Error)]);
        assert_eq!(db_state(&host, id), Some(VmState::Error));
    }

    #[tokio::test]
    async fn a_vm_with_no_shim_and_no_exit_record_is_gone() {
        let host = host().await;
        let (id, _runtime) = seed_active(&host, "vanished", VmState::Running);

        let report = reconcile(&host.state).await;

        assert_eq!(report.gone, vec![id]);
        assert_eq!(db_state(&host, id), Some(VmState::Stopped));
        let calls = host.helper_log.lock().unwrap().clone();
        assert!(calls.contains(&"delete_tap"), "{calls:?}");
        // A unit can exist without a socket yet (it had not bound when we
        // looked); a VM recorded stopped must not keep one.
        assert!(calls.contains(&"stop_vm_unit"), "{calls:?}");
    }

    #[tokio::test]
    async fn a_shim_from_another_protocol_version_is_stopped_through_the_helper() {
        let host = host().await;
        let (id, runtime) = seed_active(&host, "from-the-future", VmState::Running);
        greet_from_the_future(&runtime);

        let report = reconcile(&host.state).await;

        assert_eq!(report.mismatched, vec![id]);
        assert_eq!(db_state(&host, id), Some(VmState::Stopped));
        let calls = host.helper_log.lock().unwrap().clone();
        assert!(calls.contains(&"stop_vm_unit"), "{calls:?}");
    }

    /// A `shim.sock` that accepts and hangs up: a shim this API cannot
    /// attach to, though something is plainly serving the VM.
    fn hang_up_on_every_connection(runtime: &HostRuntimePaths) {
        let listener = tokio::net::UnixListener::bind(&runtime.shim_socket).unwrap();
        tokio::spawn(async move { while listener.accept().await.is_ok() {} });
    }

    /// A `shim.sock` greeting with a protocol version this API does not speak.
    fn greet_from_the_future(runtime: &HostRuntimePaths) {
        use crate::vm_shim::protocol::{PROTOCOL_VERSION, ShimEvent, write_event};

        let listener = tokio::net::UnixListener::bind(&runtime.shim_socket).unwrap();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let _ = write_event(
                    &mut stream,
                    &ShimEvent::Hello {
                        version: PROTOCOL_VERSION + 1,
                        vmm_pid: 1,
                        vm_id: None,
                    },
                )
                .await;
            }
        });
    }

    #[tokio::test]
    async fn a_shim_that_answers_but_cannot_be_attached_is_stopped_through_the_helper() {
        let host = host().await;
        let (id, runtime) = seed_active(&host, "unreadable", VmState::Running);
        hang_up_on_every_connection(&runtime);

        let report = reconcile(&host.state).await;

        assert_eq!(report.mismatched, vec![id]);
        assert_eq!(db_state(&host, id), Some(VmState::Stopped));
        let calls = host.helper_log.lock().unwrap().clone();
        assert!(calls.contains(&"stop_vm_unit"), "{calls:?}");
    }

    #[tokio::test]
    async fn a_shim_the_helper_cannot_stop_is_an_error_and_keeps_its_tap() {
        let host = host_failing(Some("stop_vm_unit")).await;
        let (id, runtime) = seed_active(&host, "unstoppable", VmState::Running);
        greet_from_the_future(&runtime);

        let report = reconcile(&host.state).await;

        assert_eq!(report.mismatched, vec![id]);
        assert_eq!(db_state(&host, id), Some(VmState::Error));
        let calls = host.helper_log.lock().unwrap().clone();
        assert!(
            !calls.contains(&"delete_tap"),
            "a VM that may still run keeps its TAP: {calls:?}"
        );
    }

    #[test]
    fn units_of_vms_that_were_not_readopted_are_reported() {
        let adopted = Uuid::from_u128(1);
        let stray = Uuid::from_u128(2);
        let units = vec![
            format!("firecrab-vm-{}.service", adopted.as_simple()),
            format!("firecrab-vm-{}.service", stray.as_simple()),
            "firecrab-vm-garbage.service".to_owned(),
        ];

        assert_eq!(
            unaccounted_units(units, &[adopted]),
            vec![
                format!("firecrab-vm-{}.service", stray.as_simple()),
                "firecrab-vm-garbage.service".to_owned(),
            ]
        );
    }

    #[test]
    fn unit_names_map_back_to_vm_ids() {
        let id = Uuid::from_u128(0xabcdef);
        assert_eq!(
            vm_id_from_unit(&format!("firecrab-vm-{}.service", id.as_simple())),
            Some(id)
        );
        assert_eq!(vm_id_from_unit("firecrab-api.service"), None);
        assert_eq!(vm_id_from_unit("firecrab-vm-nonsense.service"), None);
    }
}
