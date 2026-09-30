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

firecrab is a single-host microVM manager built on Firecracker.
It consists of an API, a network helper, a CLI, and a dashboard.
The API keeps host privileges minimal; the helper handles network operations that need them.

## Prerequisites

- **Common:** The repository's Rust toolchain, Node.js 22 or later, and npm
- **Linux:** Guest runtime tests require /dev/kvm and network tools.
- **macOS:** Uses microManager; runtime validation requires nested virtualization.
- **Windows:** Uses microManager in WSL2; runtime validation requires nested virtualization.

A full installation is unnecessary for unit tests and frontend builds.

## Run from source

Run these commands from the repository root.

**Linux:** Run each command in a separate terminal.

```sh
# Terminal 1: network helper
./scripts/dev-net-helper.sh

# Terminal 2: API
cargo run -p firecrab-api

# Terminal 3: dashboard
npm run dev --prefix firecrab-frontend
```

**macOS:**

```sh
cargo build -p firecrab-cli --locked
scripts/build-micromanager-macos.sh target/debug/firecrab-micromanager-macos
./target/debug/firecrab service install
```

**Windows PowerShell:**

```powershell
cargo build -p firecrab-cli --locked
.\target\debug\firecrab.exe service install
```

On a host where the service is already installed, use service start instead of service install.

## Tests

Run the common checks:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked
npm ci --prefix firecrab-frontend
npm run lint --prefix firecrab-frontend
npm run build --prefix firecrab-frontend
python3 scripts/check-doc-links.py
```

Run browser E2E checks without guest boot:

```sh
npm ci --prefix firecrab-e2e
npm run install-browsers --prefix firecrab-e2e
FIRECRAB_E2E_SKIP_GUEST_BOOT=1 npm test --prefix firecrab-e2e
```

For platform scenarios, manual steps, expected results, and cleanup, see the [English TEST guide](public-docs/TEST.md) or [Korean TEST guide](public-docs/TEST.ko.md).

## Commits

Use a short subject that describes one change. For example:

```text
fix(api): …
docs: …
ci: …
```

## Pull requests

Keep each [pull request](https://github.com/SteelCrab/firecrab/pulls) focused. Explain what changed and why, link related issues, and report test results.
Record applicable TEST items as PASS, FAILED, or WARNING.

## Issues

Report bugs and installation failures in an [issue](https://github.com/SteelCrab/firecrab/issues) with reproduction steps, environment details, and logs.
Prefer one problem per issue. Report sensitive security problems privately to the maintainers.

## CI

The [CI workflow](https://github.com/SteelCrab/firecrab/blob/main/.github/workflows/ci.yml) runs Rust, frontend, documentation, installer, and available automated scenario checks.

GitHub-hosted macOS and Windows CI cannot provide nested virtualization, so direct microVM runtime validation requires manual steps.
Depending on the contribution, contributors may post their manual test results in a PR comment.
If you have a better way to validate these changes, propose it in an issue so we can improve the process together.

## License

Contributions are covered by the project's [Apache License, Version 2.0](./LICENSE).
