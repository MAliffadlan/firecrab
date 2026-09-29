# Contributing to firecrab

Thanks for helping improve firecrab.

This guide covers how to set up a development environment, what to change where, and how we review pull requests. For product concepts and operator docs, start with [`public-docs/`](public-docs/README.md).

## A note from the maintainer

<p align="center">
  <img src="assets/icons/contributors.png" alt="Contributors" width="120" />
</p>

**Contributions are welcome.**  
We publish as much information as we can so anyone can join in. Small work counts too — typo fixes, minor bug reports, and similar help are all appreciated. Final review and merge are done by SteelCrab.

**Security and stability come first.**  
This project is complex and aims for features that fit enterprise environments. We care more about security and stability than shipping features for their own sake.

**Please file install failures as Issues.**  
If `install.sh` fails partway through, do not assume it is only your machine. Open an Issue when you can. Environment details, logs, and where it stopped already help a lot.

**Treat each other with respect.**  
Be courteous with other contributors. Prefer positive language and a light emoji over harsh or negative wording. 🙏

**Overlapping work is integrated together.**  
When several people work on similar features, SteelCrab will coordinate the merge so the result is a shared contribution.

**It is okay if maintenance pauses.**  
If life makes it hard to keep a PR going, the maintainer may pick up the work, polish it, and land it. We understand personal circumstances. Showing up and contributing at all is already a big help — a stalled commit or PR does not make the effort meaningless.

## What firecrab is

firecrab is a **single-host**, self-installed microVM manager on [Firecracker](https://firecracker-microvm.github.io/). Contributors usually work on:

| Area | Location |
| --- | --- |
| REST API, VM lifecycle, images, SQLite | `firecrab-api/` |
| Shared request/response types | `firecrab-api-types/` |
| Unix-socket protocol between API and helper | `firecrab-helper-protocol/` |
| Privileged host networking (bridge, TAP, nft, dnsmasq) | `firecrab-net-helper/` |
| Browser dashboard | `firecrab-frontend/` |
| Host installer and doctor | `install.sh`, `scripts/` |
| Published English docs | `public-docs/` |

Keep host privileges small: the API stays unprivileged; only `firecrab-net-helper` owns network capabilities.

## Prerequisites

- **Linux** with `/dev/kvm` for the direct host workflow below (unit tests mostly do not need it); macOS and Windows use microManager as described below.
- **Rust** matching [`rust-toolchain.toml`](rust-toolchain.toml) (currently 1.96.0 with `clippy`, `rustfmt`, `llvm-tools`)
- **Node.js 22+** and npm (dashboard)
- Common host tools for full local runs: `ip`, `nft`, `dnsmasq`, `mkfs.ext4`, Firecracker (or use `./install.sh`)

You do not need a full install for pure Rust unit tests or frontend lint/build.

## Develop from source

On Linux, run the API from the **repository root** so relative paths (`data/`, `images/`) resolve correctly. Use three processes:

```sh
# Terminal 1 — privileged network helper
./scripts/dev-net-helper.sh
# or:
# cargo build -p firecrab-net-helper
# sudo -u root -g "$(id -gn)" FIRECRAB_NET_HELPER_ALLOWED_UID="$(id -u)" \
#   ./target/debug/firecrab-net-helper

# Terminal 2 — API
cargo run -p firecrab-api

# Terminal 3 — Vite dashboard → http://localhost:8080/
npm install --prefix firecrab-frontend
npm run dev --prefix firecrab-frontend
```

Production-like (API serves the built SPA on `http://127.0.0.1:5523/`):

```sh
npm run build --prefix firecrab-frontend
FIRECRAB_STATIC_ROOT="$PWD/firecrab-frontend/dist" cargo run -p firecrab-api
```

More dashboard notes: [public-docs/dashboard.md](public-docs/dashboard.md).

### microManager checkout workflow (macOS and Windows)

Build the host CLI from the checkout. On macOS, also build and sign its native helper:

```sh
# macOS, from the repository root
cargo build -p firecrab-cli --locked
scripts/build-micromanager-macos.sh target/debug/firecrab-micromanager-macos
./target/debug/firecrab service start
./target/debug/firecrab service debug --logs --tail 100
curl -fsS http://127.0.0.1:5523/api/host
```

```powershell
# Windows PowerShell, from the repository root
cargo build -p firecrab-cli --locked
.\target\debug\firecrab.exe service start
.\target\debug\firecrab.exe service debug --logs --tail 100
Invoke-RestMethod http://127.0.0.1:5523/api/host
```

`service start` requires an installed microManager service. On a new host, run the checkout CLI's `service install` instead; it provisions and starts the managed guest and API. Later, `cargo run -p firecrab-cli -- service start` builds and runs the same start command. `service debug` only observes state and never starts a stopped guest. Use `service stop` to shut it down. See the [macOS](public-docs/micromanager-macos.md#develop-from-a-checkout) and [Windows](public-docs/micromanager-windows.md#develop-from-a-checkout) guides for installation and console details.

These commands build the host CLI and start the Firecrab API already installed inside the managed guest. Editing `firecrab-api/` in the checkout does not change that guest binary.

### Build and install a local release

Build the required host binaries and the dashboard from the current checkout:

```sh
./scripts/ci-prepare-install-payload.sh
```

The script builds `firecrab-api`, `firecrab-net-helper`, and `firecrab` in the
Cargo release profile, then creates `firecrab-frontend/dist` with npm.

Install the prepared files from the repository root:

```sh
./install.sh --bin-dir target/release
```

Do not prefix `install.sh` with `sudo`.
The installer requests privileges only for host operations that need them.
When `firecrab-frontend/dist` exists, the installer detects it automatically,
so `--dashboard-dir` is unnecessary.

After installation, open the management dashboard at `http://127.0.0.1:5523/`.

## Checks before you open a PR

Run what you can locally. CI will re-run the same gates on every pull request.
Use the [English test checklist](public-docs/TEST.md) or [Korean test checklist](public-docs/TEST.ko.md) to choose applicable cases, record PASS/FAILED/WARNING, and confirm cleanup before opening a PR.

### Rust workspace

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked
```

Optional, closer to CI coverage:

```sh
cargo llvm-cov --workspace --locked --lcov --output-path lcov.info
```

### Frontend

```sh
npm ci --prefix firecrab-frontend
npm run lint --prefix firecrab-frontend
npm run build --prefix firecrab-frontend
```

### OCI import browser E2E

Isolated Playwright suite in `firecrab-e2e/`. It is not part of `cargo test --workspace`.

```sh
npm ci --prefix firecrab-e2e
npm run install-browsers --prefix firecrab-e2e
FIRECRAB_E2E_SKIP_GUEST_BOOT=1 npm test --prefix firecrab-e2e
```

That covers inspect → import against a local registry fixture (no Docker Hub).
The guest-boot half needs KVM, Firecracker, and `./scripts/dev-net-helper.sh`; see [firecrab-e2e/README.md](firecrab-e2e/README.md).

### Docs and installer scripts

```sh
python3 scripts/check-doc-links.py
python3 scripts/check-changelog.py
shellcheck install.sh scripts/firecrab-release.sh
bash scripts/test-firecrab-release.sh
bash scripts/test-install-cli.sh
```

`check-doc-links.py` enforces published docs rules: English by default (with the Korean test checklist as an explicit exception), max **300 lines** per `public-docs/**/*.md` file except `api.md` and the two complete test checklists, valid relative links, and no stale `docs/` paths in tracked sources.

`check-changelog.py` requires root [`CHANGELOG.md`](CHANGELOG.md) to document the workspace version with **Added**, **Changed**, **Deprecated**, **Fixed**, and **Improved**. A `v*` tag builds the GitHub Release body with `scripts/write-release-notes.py` (install URL, that changelog section, then contributor icons).

## Issues

Use the **Task** issue template (`.github/ISSUE_TEMPLATE/task.md`) when opening work items:

```text
## Summary
## Motivation
## Scope (MVP)
## Acceptance
## Notes
```

Keep bodies short (see [#55](https://github.com/SteelCrab/firecrab/issues/55), [#56](https://github.com/SteelCrab/firecrab/issues/56), [#59](https://github.com/SteelCrab/firecrab/issues/59)). Prefer one concern per issue.

## Pull requests

1. **Fork** (or use a branch on the main repo if you have write access).
2. Prefer a **focused branch** and a **focused PR** — one concern per change when practical.
3. Describe **what** changed and **why**. Link issues if any.
4. Include tests for bug fixes and new API behavior when it is practical without nested KVM.
   OCI import UI changes should keep `FIRECRAB_E2E_SKIP_GUEST_BOOT=1 npm test --prefix firecrab-e2e` green.
   Use the [test checklist](public-docs/TEST.md) to report which applicable cases passed, failed, or were skipped.
5. Keep the default security model in mind (see below).

### Commit messages

Prefer short, conventional subjects used on `main`:

```text
feat(api): …
fix(net-helper): …
feat(frontend): …
docs: …
ci: …
chore: …
```

One logical change per commit is nice; a tidy PR history is more important than perfect splitting.

### What CI runs

| Job | On every PR | Notes |
| --- | --- | --- |
| Rust fmt, clippy, test + coverage | yes | Workspace-wide |
| rustdoc + `check-doc-links.py` + `check-changelog.py` | yes | Doc links, public-docs shape, and changelog sections |
| Installer shellcheck + install/uninstall smoke | yes | Guest boot skipped (`install.sh` never installs an image) |
| Frontend lint + build | yes | Node 22 |
| Multi-distro installer deps | yes | Debian, Fedora, Arch, openSUSE containers |
| microManager macOS and Windows builds | yes | Hosted runners: build, unit tests, `doctor --json`; no nested virtualization, so no runtime E2E |
| microManager PR report | yes | On PRs touching either platform, one comment with each job's steps and log tails, the commands to reproduce it, and the manual E2E commands |

## Documentation

| Kind | Where | Rules |
| --- | --- | --- |
| Published operator/developer docs | `public-docs/` | English by default; `TEST.ko.md` is the Korean checklist. Keep ≤300 lines except `api.md` and the complete test checklists; use relative links. |
| READMEs | `README.md`, `README.ko.md`, … | Keep install and develop paths accurate |
| Private project notes | local `docs/` | **Gitignored** — not for PRs or the remote |

Keep the bilingual test checklists in `public-docs/` and update both when cases change. Prefer editing `public-docs/` for anything users should see.

When you move or rename a public guide, update references in code comments, scripts, and READMEs — CI greps for `public-docs/…` paths.

## Security and scope notes

- **Default bind is loopback** (`127.0.0.1:5523`). The control plane is meant for a trusted host or a carefully reverse-proxied deployment. Do not assume auth or multi-tenant isolation is present.
- **No implicit “open to the LAN”** in contributions. If you expose the API, document the risk.
- Prefer extending the **helper protocol** over giving the API new host privileges.
- Avoid expanding into multi-host scheduling, full cloud IAM, or Jailer/VRF unless the change is explicitly agreed — those sit outside the current single-host MVP focus.

Report sensitive security issues privately to the maintainers rather than opening a public issue with exploit detail.

## License

By contributing, you agree that your contributions are licensed under the same [Apache License, Version 2.0](./LICENSE) as the rest of the project.

## Getting help

- Architecture: [public-docs/architecture.md](public-docs/architecture.md)
- API contracts: [public-docs/api.md](public-docs/api.md)
- Install and doctor: [public-docs/installation.md](public-docs/installation.md)
- Troubleshooting: [public-docs/troubleshooting.md](public-docs/troubleshooting.md)
- GitHub Issues and pull request discussion on [SteelCrab/firecrab](https://github.com/SteelCrab/firecrab)

Questions that unblock a PR are welcome in the PR itself.
