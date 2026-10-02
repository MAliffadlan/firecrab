#!/usr/bin/env python3
"""Exercise the guest build/deploy transaction without a VM, root, or network."""
import os
from pathlib import Path
import shlex
import socket
import subprocess
import tarfile
import tempfile
import unittest

GUEST_SCRIPT = Path(__file__).parent / "micromanager/dev-macos-guest.sh"
STUB = r'''#!/bin/bash
set -eu
name=${0##*/}
case "$name" in
  uname) echo aarch64 ;;
  cc|pkg-config|cmake|journalctl|sleep) exit 0 ;;
  flock) [ "${LOCK_FAIL:-0}" = 0 ] ;;
  curl) [ "${HEALTH_FAIL:-0}" = 0 ] ;;
  rustup)
    [ "$1" = run ] || exit 0
    [ "${BUILD_FAIL:-0}" = 0 ] || exit 42
    test "$(cat "$TEST_ROOT/cache/source/firecrab-api/src/main.rs")" = 'edited local source'
    profile=debug
    for arg in "$@"; do [ "$arg" != --release ] || profile=release; done
    mkdir -p "$CARGO_TARGET_DIR/aarch64-unknown-linux-gnu/$profile"
    for unit in firecrab-api firecrab-net-helper; do
      printf '#!/bin/bash\necho built-from-source\n' >"$CARGO_TARGET_DIR/aarch64-unknown-linux-gnu/$profile/$unit"
    done
    ;;
  systemctl)
    echo "$*" >>"$TEST_ROOT/events"
    if [ "$*" = 'start firecrab-api' ] && [ "${START_FAIL:-0}" = 1 ] \
      && [ -f "$TEST_ROOT/units/firecrab-api.service.d/90-firecrab-dev.conf" ]; then exit 43; fi
    ;;
  *) echo "unexpected stub: $name" >&2; exit 1 ;;
esac
'''


class GuestDevelopmentTest(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="firecrab-dev-test-", dir="/private/tmp" if Path("/private/tmp").exists() else None)
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name).resolve()
        self.cache = self.root / "cache"
        self.binaries = self.root / "binaries"
        self.units = self.root / "units"
        self.commands = self.root / "commands"
        self.commands.mkdir()
        stub = self.commands / "stub"
        stub.write_text(STUB)
        stub.chmod(0o755)
        for name in ["uname", "cc", "pkg-config", "cmake", "journalctl", "sleep", "flock", "curl", "rustup", "systemctl"]:
            (self.commands / name).symlink_to(stub)
        # Linux mv -T replaces a symlink rather than following its directory.
        mv = self.commands / "mv"
        mv.write_text("#!/usr/bin/env python3\nimport os, sys\nos.replace(sys.argv[-2], sys.argv[-1])\n")
        mv.chmod(0o755)
        rustup = self.cache / "cargo/bin/rustup"
        rustup.parent.mkdir(parents=True)
        rustup.symlink_to(stub)
        self.sock = socket.socket(socket.AF_UNIX)
        self.sock.bind(str(self.root / "helper.sock"))
        self.addCleanup(self.sock.close)
        script = GUEST_SCRIPT.read_text()
        for original, replacement in [
            ("root=/var/lib/firecrab/dev", "root=" + shlex.quote(str(self.cache))),
            ("bin_root=/usr/local/lib/firecrab-dev", "bin_root=" + shlex.quote(str(self.binaries))),
            ("unit_root=/etc/systemd/system", "unit_root=" + shlex.quote(str(self.units))),
            ("/run/firecrab/net-helper.sock", str(self.root / "helper.sock")),
        ]:
            script = script.replace(original, replacement)
        self.script = self.root / "guest.sh"
        self.script.write_text(script)
        source = self.root / "local/firecrab-api/src/main.rs"
        source.parent.mkdir(parents=True)
        source.write_text("edited local source")
        tools = self.root / "local/scripts/firecracker-menual"
        tools.mkdir(parents=True)
        for tool in ["extract-vmlinux", "extract-arm64-image"]:
            (tools / tool).write_text("#!/bin/sh\necho runtime-tool\n")
        self.archive = self.cache / "incoming-test.tar"
        with tarfile.open(self.archive, "w") as archive:
            archive.add(source, arcname="firecrab-api/src/main.rs")
            archive.add(tools, arcname="scripts/firecracker-menual")

    def run_guest(self, profile="debug", shell="bash", **flags):
        environment = os.environ.copy()
        environment.update(TEST_ROOT=str(self.root), PATH=str(self.commands) + ":" + environment["PATH"])
        environment.update({key: str(value) for key, value in flags.items()})
        return subprocess.run([shell, str(self.script), profile, "1.97.1", self.archive.name],
                              env=environment, text=True, capture_output=True, timeout=10)

    def override(self, unit="firecrab-api"):
        return self.units / (unit + ".service.d/90-firecrab-dev.conf")

    def test_debug_and_release_deploy_edited_source(self):
        for profile in ["debug", "release"]:
            with self.subTest(profile=profile):
                # Each upload is consumed by its build.
                if not self.archive.exists():
                    source = self.root / "local/firecrab-api/src/main.rs"
                    with tarfile.open(self.archive, "w") as archive:
                        archive.add(source, arcname="firecrab-api/src/main.rs")
                        archive.add(self.root / "local/scripts/firecracker-menual", arcname="scripts/firecracker-menual")
                result = self.run_guest(profile)
                self.assertEqual(result.returncode, 0, result.stderr + result.stdout)
                self.assertIn("ExecStart=" + str(self.binaries) + "/bin/firecrab-api", self.override().read_text())
                self.assertIn("built-from-source", (self.binaries / "bin/firecrab-net-helper").read_text())
                self.assertIn("runtime-tool", (self.binaries / "bin/extract-arm64-image").read_text())
                self.assertFalse(self.archive.exists())

    def test_debug_deploy_works_with_system_bash(self):
        result = self.run_guest(shell="/bin/bash")
        self.assertEqual(result.returncode, 0, result.stderr + result.stdout)
        self.assertTrue(self.override().is_file())
        self.assertIn("built-from-source", (self.binaries / "bin/firecrab-api").read_text())

    def test_build_failure_does_not_touch_services_or_previous_binaries(self):
        result = self.run_guest(BUILD_FAIL=1)
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((self.root / "events").exists())
        self.assertFalse(self.override().exists())
        self.assertFalse((self.binaries / "bin").exists())

    def test_failed_first_deployment_restores_release_services(self):
        result = self.run_guest(START_FAIL=1)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("[ROLLBACK]", result.stderr)
        self.assertFalse(self.override().exists())
        self.assertFalse((self.binaries / "bin").exists())
        self.assertTrue((self.root / "events").read_text().endswith("start firecrab-api\n"))

    def test_failed_update_restores_previous_development_executables(self):
        previous = self.binaries / "previous"
        previous.mkdir(parents=True)
        (self.binaries / "bin").symlink_to(previous)
        self.override().parent.mkdir(parents=True)
        self.override().write_text("previous override\n")
        result = self.run_guest(HEALTH_FAIL=1)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual((self.binaries / "bin").resolve(), previous.resolve())
        self.assertEqual(self.override().read_text(), "previous override\n")

    def test_restore_does_not_build_and_preserves_other_overrides(self):
        self.override().parent.mkdir(parents=True)
        self.override().write_text("development override\n")
        custom = self.override().parent / "custom.conf"
        custom.write_text("custom settings\n")
        result = self.run_guest("restore", BUILD_FAIL=1)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(self.override().exists())
        self.assertEqual(custom.read_text(), "custom settings\n")

    def test_restore_after_source_deployment_removes_development_pointer(self):
        result = self.run_guest()
        self.assertEqual(result.returncode, 0, result.stderr)
        result = self.run_guest("restore", BUILD_FAIL=1)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(self.override().exists())
        self.assertFalse((self.binaries / "bin").exists())

    def test_concurrent_build_is_rejected_before_mutating_services(self):
        result = self.run_guest(LOCK_FAIL=1)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("in progress", result.stderr)
        self.assertFalse((self.root / "events").exists())


if __name__ == "__main__":
    unittest.main()
