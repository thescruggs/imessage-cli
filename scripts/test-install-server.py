#!/usr/bin/env python3
"""Isolated regression tests for install-server.sh.

Every invocation runs in a temporary fake repository and HOME.  The mocked
commands only record their arguments, so this test never touches launchd,
builds Cargo projects, or the real user's files.
"""
from __future__ import annotations

import os
import plistlib
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path


SOURCE = Path(__file__).with_name("install-server.sh")

CURL_MOCK = '#!/bin/sh\nprintf "curl %s\\\\n" "$*" >> "$MOCK_EVENTS"\nout=; url=\nwhile [ "$#" -gt 0 ]; do case $1 in -o) out=$2; shift 2 ;; http*) url=$1; shift ;; *) shift ;; esac; done\nprintf "url %s\\\\n" "$url" >> "$MOCK_EVENTS"\ncp "$MOCK_TARBALL" "$out"'


class InstallerTests(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = Path(tempfile.mkdtemp(prefix="imsg installer "))
        self.repo = self.tmp / "repo & <fixture>"
        (self.repo / "scripts").mkdir(parents=True)
        (self.repo / "target/release").mkdir(parents=True)
        shutil.copy2(SOURCE, self.repo / "scripts/install-server.sh")
        for name in ("imsg", "imsg-server"):
            binary = self.repo / "target/release" / name
            binary.write_text("fake " + name)
            binary.chmod(0o755)
        self.home = self.tmp / "home & <xml>"
        self.home.mkdir()
        self.caller = self.tmp / "caller"
        self.caller.mkdir()
        self.bin = self.tmp / "mock-bin"
        self.bin.mkdir()
        self.events = self.tmp / "events.log"
        self.launch_state = self.tmp / "launchd-state"
        self._mock("id", '#!/bin/sh\nprintf "%s\\n" "${MOCK_UID:-501}"')
        self._mock("uname", '#!/bin/sh\nprintf "%s\\n" "Darwin"')
        self._mock("sleep", "#!/bin/sh\nexit 0")
        self._mock("open", '#!/bin/sh\nprintf "open %s\\n" "$*" >> "$MOCK_EVENTS"\nexit 0')
        self._mock("cargo", '#!/bin/sh\nprintf "cargo cwd=%s %s\\n" "$PWD" "$*" >> "$MOCK_EVENTS"\nexit "${MOCK_CARGO_STATUS:-0}"')
        self._mock("launchctl", '''#!/bin/sh
printf "launchctl %s\\n" "$*" >> "$MOCK_EVENTS"
case "$1" in
  bootstrap) [ "${MOCK_BOOTSTRAP_STATUS:-0}" -eq 0 ] && : > "$MOCK_LAUNCH_STATE"; exit "${MOCK_BOOTSTRAP_STATUS:-0}" ;;
  bootout|remove) rm -f "$MOCK_LAUNCH_STATE"; exit "${MOCK_LAUNCHCTL_STATUS:-0}" ;;
  print)
    case "$2" in
      */org.thescruggs.imsg-server) test -e "$MOCK_LAUNCH_STATE"; exit $? ;;
      *) exit "${MOCK_DOMAIN_STATUS:-0}" ;;
    esac ;;
  enable|kickstart|load|unload) exit "${MOCK_LAUNCHCTL_STATUS:-0}" ;;
esac
exit "${MOCK_LAUNCHCTL_STATUS:-0}"''')

    def tearDown(self) -> None:
        shutil.rmtree(self.tmp, ignore_errors=True)

    def _mock(self, name: str, body: str) -> None:
        path = self.bin / name
        path.write_text(body + "\n")
        path.chmod(0o755)

    def run_installer(self, *args: str, **extra: str) -> subprocess.CompletedProcess[str]:
        return self.run_script(self.repo / "scripts/install-server.sh", *args, **extra)

    def run_script(self, script: Path, *args: str, **extra: str) -> subprocess.CompletedProcess[str]:
        env = os.environ.copy()
        env.update({"HOME": str(self.home), "PATH": f"{self.bin}:{env['PATH']}",
                    "MOCK_EVENTS": str(self.events), "MOCK_LAUNCH_STATE": str(self.launch_state),
                    "MOCK_UID": "501"})
        env.pop("IMSG_BIND", None)
        env.update(extra)
        return subprocess.run([str(script), *args],
                              cwd=self.caller, env=env, text=True,
                              stdout=subprocess.PIPE, stderr=subprocess.PIPE)

    def plist(self) -> dict:
        return plistlib.loads((self.home / "Library/LaunchAgents/org.thescruggs.imsg-server.plist").read_bytes())

    def event_text(self) -> str:
        return self.events.read_text() if self.events.exists() else ""

    def make_package(self) -> Path:
        """Lay out an unpacked release package: binaries next to the installer, no source tree."""
        package = self.tmp / "package" / "imsg-0.0.0-macos"
        package.mkdir(parents=True)
        shutil.copy2(SOURCE, package / "install-server.sh")
        for name in ("imsg", "imsg-server"):
            binary = package / name
            binary.write_text("packaged " + name)
            binary.chmod(0o755)
        return package

    def write_token(self, token: str = "abc123TOKEN") -> None:
        config = self.home / ".config/imsg"
        config.mkdir(parents=True, exist_ok=True)
        (config / "server.toml").write_text(f'# imsg server config\ntoken = "{token}"\n')

    def test_default_bind_and_xml_escaping(self) -> None:
        result = self.run_installer("--no-build")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotIn("cargo ", self.event_text())
        data = self.plist()["Label"]
        self.assertEqual(data, "org.thescruggs.imsg-server")
        p = self.plist()
        args = p["ProgramArguments"]
        self.assertEqual(args[-2:], ["--bind", "0.0.0.0:8787"])
        self.assertEqual(args[0], str(self.home / ".local/bin/imsg-server"))
        self.assertEqual(p["RunAtLoad"], True)
        self.assertEqual(p["KeepAlive"], True)
        self.assertEqual(p["EnvironmentVariables"]["HOME"], str(self.home))
        self.assertIn("bootstrap gui/501", self.event_text())
        self.assertNotIn("bootstrap system", self.event_text())
        events = self.event_text()
        self.assertIn("enable gui/501/org.thescruggs.imsg-server", events)
        self.assertLess(events.index("enable gui/501/org.thescruggs.imsg-server"),
                        events.index("bootstrap gui/501"))

    def test_custom_bind_and_environment_override(self) -> None:
        result = self.run_installer("--bind", "[::1]:9123")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("--manifest-path", self.event_text())
        self.assertEqual(self.plist()["ProgramArguments"][-1], "[::1]:9123")
        result = self.run_installer("--no-build", IMSG_BIND="127.0.0.1:9999")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.plist()["ProgramArguments"][-1], "127.0.0.1:9999")

    def test_reinstall_bootstraps_existing_agent_and_preserves_binaries(self) -> None:
        self.assertEqual(self.run_installer("--no-build").returncode, 0)
        binary = self.home / ".local/bin/imsg-server"
        binary.write_text("user data")
        self.assertEqual(self.run_installer("--no-build").returncode, 0)
        events = self.event_text().splitlines()
        self.assertTrue(any("bootout gui/501" in e for e in events))
        self.assertGreaterEqual(sum("bootstrap gui/501" in e for e in events), 2)
        self.assertEqual(binary.read_text(), "fake imsg-server")

    def test_uninstall_removes_agent_preserves_data_and_binaries(self) -> None:
        self.assertEqual(self.run_installer("--no-build").returncode, 0)
        config = self.home / ".config/imsg/settings.toml"
        config.write_text("keep = true")
        result = self.run_installer("--uninstall")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse((self.home / "Library/LaunchAgents/org.thescruggs.imsg-server.plist").exists())
        self.assertTrue(config.exists())
        self.assertTrue((self.home / ".local/bin/imsg-server").exists())
        self.assertIn("bootout gui/501", self.event_text())

    def test_status_and_restart_use_gui_domain(self) -> None:
        self.assertEqual(self.run_installer("--no-build").returncode, 0)
        self.assertEqual(self.run_installer("--status").returncode, 0)
        self.assertEqual(self.run_installer("--restart").returncode, 0)
        events = self.event_text()
        self.assertIn("print gui/501/org.thescruggs.imsg-server", events)
        self.assertIn("kickstart -k gui/501/org.thescruggs.imsg-server", events)

    def test_install_without_gui_domain_defers_until_next_login(self) -> None:
        result = self.run_installer("--no-build", MOCK_DOMAIN_STATUS="1")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("next login", result.stdout)
        self.assertTrue((self.home / "Library/LaunchAgents/org.thescruggs.imsg-server.plist").exists())
        self.assertNotIn("bootstrap gui/501", self.event_text())

    def test_failures_and_invalid_arguments(self) -> None:
        help_result = self.run_installer("--help")
        self.assertEqual(help_result.returncode, 0)
        self.assertIn("--uninstall", help_result.stdout)
        self.assertIn("--status", help_result.stdout)
        self.assertNotEqual(self.run_installer("--bind").returncode, 0)
        self.assertNotEqual(self.run_installer("--bind", "--status").returncode, 0)
        self.assertNotEqual(self.run_installer("--bogus").returncode, 0)
        self.assertNotEqual(self.run_installer("--no-build", MOCK_UID="0").returncode, 0)
        self.assertNotEqual(self.run_installer(MOCK_CARGO_STATUS="7").returncode, 0)
        self.assertFalse((self.home / "Library/LaunchAgents").exists())

    def test_build_failure_keeps_existing_installation(self) -> None:
        self.assertEqual(self.run_installer("--no-build").returncode, 0)
        plist = self.home / "Library/LaunchAgents/org.thescruggs.imsg-server.plist"
        binary = self.home / ".local/bin/imsg-server"
        before_plist, before_binary = plist.read_bytes(), binary.read_bytes()
        result = self.run_installer(MOCK_CARGO_STATUS="7")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(plist.read_bytes(), before_plist)
        self.assertEqual(binary.read_bytes(), before_binary)

    def test_bootstrap_failure_is_reported(self) -> None:
        self.assertEqual(self.run_installer("--no-build").returncode, 0)
        self.launch_state.unlink()
        result = self.run_installer("--restart", MOCK_BOOTSTRAP_STATUS="7")
        self.assertNotEqual(result.returncode, 0)
        self.assertTrue((self.home / "Library/LaunchAgents/org.thescruggs.imsg-server.plist").exists())

    def test_helper_script_is_installed(self) -> None:
        self.assertEqual(self.run_installer("--no-build").returncode, 0)
        helper = self.home / ".local/bin/imsg-service"
        self.assertTrue(helper.exists())
        self.assertEqual(helper.read_bytes(), SOURCE.read_bytes())
        self.assertTrue(os.access(helper, os.X_OK))
        # The installed helper must keep working for service actions and for
        # re-registering itself (its binaries are already in place).
        self.assertEqual(self.run_script(helper, "--status").returncode, 0)
        self.assertEqual(self.run_script(helper, "--restart").returncode, 0)
        result = self.run_script(helper)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((self.home / ".local/bin/imsg-server").read_text(), "fake imsg-server")
        self.assertNotIn("cargo ", self.event_text())
        self.assertEqual(sorted(p.name for p in (self.home / ".local/bin").iterdir()),
                         ["imsg", "imsg-server", "imsg-service"])

    def test_package_mode_installs_without_building(self) -> None:
        package = self.make_package()
        result = self.run_script(package / "install-server.sh")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotIn("cargo ", self.event_text())
        self.assertEqual((self.home / ".local/bin/imsg-server").read_text(), "packaged imsg-server")
        self.assertEqual((self.home / ".local/bin/imsg").read_text(), "packaged imsg")
        self.assertIn("bootstrap gui/501", self.event_text())
        self.assertIn("NEXT STEPS", result.stdout)
        self.assertIn("Full Disk Access", result.stdout)
        self.assertNotIn("open ", self.event_text())

    def test_binaries_option_and_open_settings(self) -> None:
        package = self.make_package()
        result = self.run_installer("--binaries", str(package), "--open-settings")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotIn("cargo ", self.event_text())
        self.assertEqual((self.home / ".local/bin/imsg-server").read_text(), "packaged imsg-server")
        events = self.event_text()
        self.assertIn(f"open -R {self.home / '.local/bin/imsg-server'}", events)
        self.assertIn("open x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles", events)

    def test_token_url_and_open(self) -> None:
        self.assertNotEqual(self.run_installer("--token").returncode, 0)
        self.assertEqual(self.run_installer("--no-build").returncode, 0)
        self.write_token()
        result = self.run_installer("--token")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.strip(), "abc123TOKEN")
        result = self.run_installer("--url")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("http://localhost:8787/#token=abc123TOKEN", result.stdout)
        result = self.run_installer("--open")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("open http://localhost:8787/#token=abc123TOKEN", self.event_text())
        # A custom port must be reflected in the printed links.
        self.assertEqual(self.run_installer("--no-build", "--bind", "127.0.0.1:9000").returncode, 0)
        self.assertIn("http://localhost:9000/#token=abc123TOKEN", self.run_installer("--url").stdout)

    def test_install_prints_token_link_and_update_reminder(self) -> None:
        result = self.run_installer("--no-build")
        self.assertNotIn("forgets Full Disk Access", result.stdout)
        self.write_token("fromserver")
        result = self.run_installer("--no-build")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("http://localhost:8787/#token=fromserver", result.stdout)
        self.assertIn("forgets Full Disk Access", result.stdout)

    def test_bootstrap_installer_downloads_and_runs_package(self) -> None:
        package = self.make_package()
        tarball = self.tmp / "imsg-macos-universal.tar.gz"
        subprocess.run(["tar", "-czf", str(tarball), "-C", str(package.parent), package.name], check=True)
        self._mock("curl", CURL_MOCK)
        bootstrap = self.repo / "scripts/install.sh"
        shutil.copy2(SOURCE.with_name("install.sh"), bootstrap)
        result = self.run_script(bootstrap, MOCK_TARBALL=str(tarball), TMPDIR=str(self.tmp))
        self.assertEqual(result.returncode, 0, result.stderr)
        events = self.event_text()
        self.assertIn("url https://github.com/thescruggs/imessage-cli/releases/latest/download/imsg-macos-universal.tar.gz", events)
        self.assertEqual((self.home / ".local/bin/imsg-server").read_text(), "packaged imsg-server")
        self.assertIn("open x-apple.systempreferences", events)
        self.assertFalse(list(self.tmp.glob("imsg-install.*")), "temporary download directory was not removed")
        result = self.run_script(bootstrap, MOCK_TARBALL=str(tarball), TMPDIR=str(self.tmp),
                                 IMSG_VERSION="v9.9.9", IMSG_BIND="127.0.0.1:1234")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("url https://github.com/thescruggs/imessage-cli/releases/download/v9.9.9/imsg-macos-universal.tar.gz", self.event_text())
        self.assertEqual(self.plist()["ProgramArguments"][-1], "127.0.0.1:1234")


if __name__ == "__main__":
    unittest.main(verbosity=2)
