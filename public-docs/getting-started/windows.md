# microManager on Windows

microManager runs Firecrab in the managed `firecrab-debian` WSL2 distribution.
The dashboard and API are available at `http://127.0.0.1:5523/`.

## Requirements

- x86_64 or ARM64 Windows with Microsoft Store WSL2; `wsl --version` must work.
- Hardware and nested virtualization exposing `/dev/kvm` inside WSL2.
- The [Windows CLI](installation.md#windows).

Commands run from a normal PowerShell session.
ARM64 artifacts are pinned, but ARM64 installation has no end-to-end validation.
Recorded runtime results come from a Windows 11 x64 lab VM on Linux KVM.
See [validation history](../development/validation-history.md) for scope and limits.

## Install and start

```powershell
firecrab service doctor
firecrab service install
firecrab service status
Invoke-RestMethod http://127.0.0.1:5523/api/host
```

Require `doctor` to report `ready: true` for runtime tests.
`doctor --json` includes suggested fixes.
With no WSL distribution yet, KVM is unverified until installation imports Debian.
A cold WSL2 start can also take about 25 seconds to load `kvm_intel`.

Installation verifies pinned Debian 13 WSL rootfs, Firecracker v1.17.0, and Firecrab v0.2.2 downloads.
It imports only `firecrab-debian`, provisions systemd services, and boots a nested BusyBox workload.
The exact Debian kernel selected by the [release manifest](../reference/micromanager-manifest.md) boots the nested test.
A per-user scheduled task then keeps Debian running and checks the localhost API before saving the installed TOML.
`install` and `reinstall` migrate explicitly; updating the CLI alone preserves existing installations.
Interrupted downloads resume; failed provisioning is retried by the next install.

## Known limitation

The pinned Firecrab v0.2.2 guest bundle cannot start workload MicroVMs on stock WSL2.
Its helper requires nftables `bridge` support and conflicts with WSL's loopback DNS listener.
The API, images, and networks still work.

The current source helper uses `netdev` rules and keeps dnsmasq off loopback.
Deploy it with [service dev](../development/micromanager.md#windows) to run workloads.
`service dev --restore` selects the installed binaries and restores their release limitations.
Source lab passes do not establish stock-release or physical-Windows compatibility.

## Service commands

| Command | Effect |
| --- | --- |
| `firecrab service start` / `stop` | Enable and start / disable and stop the managed distribution |
| `firecrab service status` | Check the task, guest units, and Windows API |
| `firecrab service validate` | Recheck downloads, distribution, provisioning, and task state |
| `firecrab service shell` | Open a root guest shell; `run` is an alias |
| `firecrab service shell -- COMMAND` | Run one guest command |
| `firecrab service repair` | Rebuild the task and restart installed guest services |
| `firecrab service reinstall` | Clear installation state and reprovision from TOML; retain data |
| `firecrab service uninstall` | Remove the task; retain distribution and data |
| `firecrab service uninstall --purge` | Also unregister the distribution and delete managed data |

The scheduled task `\Firecrab\microManager` holds WSL open with `wslg.exe -- sleep infinity`.
It starts at logon and, by default, retries every minute without creating duplicate holders.
The installed `micromanager.toml` accepts `runtime.auto_restart` to disable periodic relaunch.
See [runtime settings](../reference/micromanager-manifest.md#runtime-settings).
The SSH settings apply only to macOS; Windows continues to use WSL.
`stop` leaves the task disabled until the next `start`.
Stopping the distribution stops its workload MicroVMs.

## Networking and data

WSL localhost forwarding exposes the guest's loopback API on Windows without SSH.
The source helper also relays TCP forwards to the same Windows localhost ports.
UDP has no Windows localhost relay.
A TCP port occupied by another process fails VM startup.

Every WSL2 distribution shares the kernel, `/dev`, and network namespace.
Other distributions can see Firecrab's bridges and nftables tables.
Never create `/dev/kvm` by hand; a regular file there blocks the real device for all distributions until `wsl --shutdown`.

Managed files default to `%LOCALAPPDATA%\Firecrab\micromanager`.
Use `FIRECRAB_MICROMANAGER_HOME` for an isolated checkout or test.

| Path under the managed root | Content |
| --- | --- |
| `micromanager.toml` | Installed release inputs and editable runtime settings |
| `downloads\` | Verified release artifacts |
| `distro\ext4.vhdx` | Debian and Firecrab data, including `/var/lib/firecrab` |
| `provision\` | Guest setup scripts |
| `runtime\` | Markers, logs, and task XML |

## Diagnose and repair

```powershell
firecrab service debug --logs --tail 100
firecrab service debug --json
firecrab service repair
```

`debug` is read-only and does not start a stopped distribution.
It reports host checks, task registration, provisioning, guest services, and the localhost API.
Guest checks are unverified while WSL is stopped.
`--tail` requires `--logs` and accepts 1–1000 lines.
Inspect redacted log excerpts before sharing them.

Repair checks the disk, provisioning marker, distribution registration, and task prerequisites before stopping.
It recreates the task/XML, reloads systemd, clears failed units, and restarts the installed helper and API.
It preserves the WSL disk, binaries, data, and development overrides.
It downloads nothing and restarts only `firecrab-debian`.
Windows repair does not perform offline OS filesystem recovery.
Readiness has a five-minute deadline; any failed stage returns nonzero.
An incomplete installation requires `service reinstall`.

Inspect `runtime\guest-provision.log`, `runtime\provision.phase`, and `runtime\provision.failed` for installation failures.

## Related

- [Source development](../development/micromanager.md#windows)
- [Windows runtime E2E](../development/ci.md#windows-runtime-e2e)
- [Recorded validation results](../development/validation-history.md)
- [microManager on macOS](macos.md)
