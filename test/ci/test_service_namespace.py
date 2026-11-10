"""Exercise namespace fixture planning without privileged host operations."""
from pathlib import Path
import importlib.util
import os
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("service_namespace", ROOT / "test/server/service_namespace.py")
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class ServiceNamespaceTests(unittest.TestCase):
    def test_exact_child_is_elevated_after_environment_reset(self):
        command = MODULE.child_command(Path("/tmp/fixture"), "mnt:[1]", "/repo/test-bin", "selected_test")
        self.assertEqual(command[:5], ["/usr/bin/sudo", "-n", "--", "/usr/bin/env", "-i"])
        self.assertIn("240s", command)
        start = command.index("/usr/bin/unshare")
        self.assertEqual(command[start:start + 6], ["/usr/bin/unshare", "--mount", "--propagation", "private", "--", "/usr/bin/python3"])
        self.assertEqual(command[-5:], ["--inside", "/tmp/fixture", "mnt:[1]", "/repo/test-bin", "selected_test"])

    def test_shell_gate_is_elevated_only_after_build_and_environment_reset(self):
        command = MODULE.child_command(Path("/tmp/fixture"), "mnt:[1]",
                                       "--command", "/usr/bin/bash", "/repo/gate.sh", "/repo/test-bin", "/repo/cli")
        self.assertEqual(command[:5], ["/usr/bin/sudo", "-n", "--", "/usr/bin/env", "-i"])
        self.assertEqual(command[-8:], ["--inside", "/tmp/fixture", "mnt:[1]",
                                       "--command", "/usr/bin/bash", "/repo/gate.sh", "/repo/test-bin", "/repo/cli"])

    def test_command_runs_only_after_private_root_is_prepared(self):
        events = []
        def executed(executable, command):
            events.append(("exec", executable, command))
            raise SystemExit(0)
        with patch.object(MODULE, "prepare_root", side_effect=lambda *args: events.append(("prepare", *args))), \
             patch.object(MODULE.os, "chroot", side_effect=lambda root: events.append(("chroot", root))), \
             patch.object(MODULE.os, "chdir", side_effect=lambda root: events.append(("chdir", root))), \
             patch.object(MODULE.os, "execv", side_effect=executed), \
             self.assertRaises(SystemExit):
            MODULE.main(["--inside", "/tmp/fixture", "mnt:[1]",
                         "--command", "/usr/bin/bash", "/repo/gate.sh", "/repo/test-bin", "/repo/cli"])
        self.assertEqual(events, [("prepare", Path("/tmp/fixture"), "mnt:[1]"),
                                  ("chroot", Path("/tmp/fixture")), ("chdir", "/"),
                                  ("exec", "/usr/bin/bash", ["/usr/bin/bash", "/repo/gate.sh", "/repo/test-bin", "/repo/cli"])])

    def test_command_failure_is_not_skipped_or_retried(self):
        with tempfile.TemporaryDirectory() as temporary, \
             patch.object(MODULE.tempfile, "TemporaryDirectory") as directory, \
             patch.object(MODULE.os, "readlink", return_value="mnt:[1]"), \
             patch.object(MODULE.subprocess, "run", return_value=SimpleNamespace(returncode=19)) as run:
            directory.return_value.__enter__.return_value = temporary
            self.assertEqual(MODULE.main(["--command", "/usr/bin/bash", "/repo/gate.sh"]), 19)
            run.assert_called_once_with(MODULE.child_command(Path(temporary), "mnt:[1]",
                                                             "--command", "/usr/bin/bash", "/repo/gate.sh"))

    def test_unchanged_namespace_is_rejected_before_any_mount(self):
        with patch.object(MODULE.os, "geteuid", return_value=0), \
             patch.object(MODULE.os, "readlink", return_value="mnt:[1]"), \
             patch.object(MODULE, "mount") as mount:
            with self.assertRaisesRegex(RuntimeError, "separate mount namespace"):
                MODULE.prepare_root(Path("/unused"), "mnt:[1]")
            mount.assert_not_called()

    def test_host_root_is_omitted_and_new_root_is_private(self):
        entries = [SimpleNamespace(name=name, is_symlink=lambda: False, is_dir=lambda: True)
                   for name in ["zxs", "tmp", "usr"]]
        with tempfile.TemporaryDirectory() as temporary, \
             patch.object(MODULE.os, "geteuid", return_value=0), \
             patch.object(MODULE.os, "readlink", return_value="mnt:[2]"), \
             patch.object(MODULE.os, "scandir") as scan, \
             patch.object(MODULE, "mount") as mount, \
             patch.object(MODULE, "bind") as bind:
            scan.return_value.__enter__.return_value = entries
            root = Path(temporary)
            MODULE.prepare_root(root, "mnt:[1]")
            mount.assert_called_once_with("--types", "tmpfs", "--options", "mode=755", "tmpfs", root)
            self.assertEqual([(c.args[0], c.kwargs) for c in bind.call_args_list],
                             [(Path("/tmp"), {"writable": True}), (Path("/usr"), {"writable": False})])
            self.assertTrue((root / "zxs").is_dir())
            self.assertEqual(os.stat(root / "zxs").st_mode & 0o777, 0o755)


if __name__ == "__main__":
    unittest.main()
