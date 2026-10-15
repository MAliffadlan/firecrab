# microManager development

Build the CLI on the host and the Linux API/helper inside managed Debian.
Run commands from the checkout root with the pinned Rust toolchain.
See [Contributing](../../CONTRIBUTING.md#run-from-source) for native Linux development.

## macOS

```sh
cargo build -p firecrab-cli --locked
scripts/build-micromanager-macos.sh target/debug/firecrab-micromanager-macos
./target/debug/firecrab service dev
./target/debug/firecrab service debug --logs --tail 100
curl -fsS http://127.0.0.1:5523/api/host
```

The helper build script applies the Virtualization entitlement and ad-hoc signature.
The checkout CLI finds its signed sibling helper.
`FIRECRAB_MICROMANAGER_HELPER` can select another helper explicitly.

To replace installed CLI/helper binaries, use `./target/debug/firecrab service reinstall`.
This reprovisions the OS and preserves managed data.
See [macOS setup](../getting-started/macos.md) for host requirements.

## Windows

Run in PowerShell:

```powershell
cargo build -p firecrab-cli --locked
.\target\debug\firecrab.exe service dev
.\target\debug\firecrab.exe service debug --logs --tail 100
Invoke-RestMethod http://127.0.0.1:5523/api/host
```

The CLI uses `tar.exe` and `wsl.exe` to transfer the snapshot and build script.
It normalizes CRLF shell scripts and builds for the Debian architecture.
Continue using the checkout executable to test CLI edits.
Windows `service reinstall` does not copy it into the user's installed binary path.
See [Windows setup and release limits](../getting-started/windows.md).

## Build and deploy

`service dev` installs microManager if missing, then starts its resident service.
It uploads a source snapshot, installs compiler prerequisites on the first run, and builds `firecrab-api` and `firecrab-helper` with `--locked`.
Sources and compiler caches persist under `/var/lib/firecrab/dev`.
Local configuration, `.env` files, nested checkouts, and build output are excluded.

After a successful build, deployment switches the two systemd executable overrides and checks guest and host API readiness.
A build failure leaves services running; a deployment failure restores previous executables.
Run `service dev` again after each edit; it does not watch files.
Guest service restarts interrupt API/helper requests, so stop workloads before changing their runtime.

| Command or option | Effect |
| --- | --- |
| `service dev --source PATH` | Select another checkout |
| `service dev --release` | Optimize the source build |
| `service dev --yes` | Skip pending-download confirmation |
| `service dev start` / `stop` / `restart` | Control guest API/helper without rebuilding |
| `service dev --restore` | Remove development overrides and restart installed release binaries |
| `service shell` | Open a root shell in managed Debian |

`dev start`, `stop`, and `restart` require a running management VM.
They preserve deployed binaries and overrides and cannot be combined with build/restore options.
`--restore` needs no checkout and cannot be combined with `--source`, `--release`, or `--yes`.
Overrides persist across `service stop` and `start` until restored.
`service start` resumes selected binaries; it does not build edited sources.

## Frontend

The installed dashboard remains its release build.
Run the frontend from source separately:

```sh
npm ci --prefix firecrab-frontend
npm run dev --prefix firecrab-frontend
```

Open `http://localhost:8080`.
Vite proxies `/api` and `/ws` to `127.0.0.1:5523` through the macOS SSH tunnel or WSL localhost forwarding.
Use `localhost` for the configured browser origin.

## Related

- [Architecture](../reference/architecture.md)
- [CI and runtime E2E](ci.md)
- [Repair QA](macos-repair.md)
