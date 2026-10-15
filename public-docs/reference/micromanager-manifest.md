# microManager release manifest

[packaging/micromanager.toml](../../packaging/micromanager.toml) defines Debian, Firecrab, Firecracker, kernels, platform images, URLs, and checksums.
The CLI embeds it at build time.
Maintainers change the manifest and build a new CLI to publish different installation inputs.
macOS uses ARM64 assets; Windows selects x86_64 or ARM64.

## Command behavior

| Command | Manifest behavior |
| --- | --- |
| First `service install` | Install from the bundled TOML |
| `service install` on a legacy or changed installation | Migrate to the bundled TOML |
| `service install` with a completed matching manifest | Reuse the installation |
| `service reinstall` | Clear registration, old scripts, installed TOML, and completion records; provision again |
| `start`, `stop`, `status`, `dev`, `repair` | Use existing assets and provisioning state |
| `service uninstall` | Remove registration; retain managed data |
| `service uninstall --purge` | Also remove disks, keys, downloads, scripts, and TOML; Windows unregisters Debian |

Updating the CLI alone does not migrate existing installations.
`service dev` installs only when the management VM is missing.
An installation without `<managed-home>/micromanager.toml` uses legacy pins for validation and offline repair.

## Install and migrate

```sh
firecrab service install
# Force reprovisioning when needed:
firecrab service reinstall
```

Both commands validate the bundled TOML and artifact checksums.
The guest reports its manifest fingerprint after provisioning and the nested workload check.
The CLI saves `<managed-home>/micromanager.toml` only after the resident service and localhost API are healthy.
Failures do not publish a new installed manifest.
This installed copy records release inputs for validation/repair; it is not a user configuration override.

### macOS

Migration replaces the Debian OS and boot files and preserves the data disk and management SSH client keys.
It clears the cached SSH server fingerprint because the new OS creates new server keys.
A changed manifest reprovisions the OS; `reinstall` always rebuilds it.
The selected Debian kernel and matching initrd become `system/Image` and `system/initrd.img`.
Installation checks both nested Firecracker and resident boot with that kernel before saving the manifest.

### Windows

Migration reprovisions the existing WSL distribution without unregistering or reimporting it.
The rootfs archive is used only for a missing distribution.
Reinstall clears task registration, scripts, and completion records first.
TOML provisioning removes Firecrab development overrides so installed release binaries start.
User overrides and distribution data remain.
The selected Debian kernel boots the nested test; Microsoft's shared WSL kernel stays unchanged.
Other WSL distributions stay unchanged.

## Change release inputs

Update versions, HTTPS URLs, upstream checksums, and each architecture's Debian 13 kernel together.
Unsupported schemas, unknown fields, unsafe filenames, and malformed checksums fail before downloading or stopping.
An unavailable pinned kernel fails provisioning; no alternative kernel is selected.
Publish an updated manifest when mirrors no longer provide the package.

## Related

- [macOS setup](../getting-started/macos.md)
- [Windows setup](../getting-started/windows.md)
- [Source development](../development/micromanager.md)
