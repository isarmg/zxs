"""Isolation and identity contract for the real Debian Samba acceptance gate."""
import importlib.util
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("system_samba_gate", ROOT / ".github/scripts/check-system-samba.py")
GATE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(GATE)


class SystemSambaGate(unittest.TestCase):
    def test_host_without_explicit_isolation_is_rejected_before_actions(self):
        with patch.dict(os.environ, {}, clear=True), patch.object(GATE.subprocess, "check_output") as execute:
            with self.assertRaisesRegex(RuntimeError, "isolated Debian fixture"):
                GATE.run(None)
            execute.assert_not_called()

    def test_phase_markers_publish_complete_expected_bytes(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            GATE.publish_marker(root, "uploaded")
            self.assertEqual((root / "uploaded").read_bytes(), b"ok\n")
            self.assertFalse((root / "uploaded.new").exists())

    def test_debian_source_and_abi_are_exact_not_upstream_fallback(self):
        target = json.loads((ROOT / ".github/samba-debian-target.json").read_text())
        self.assertEqual((target["release"], target["architecture"], target["vfs_interface"]), ("13", "amd64", 50))
        build = (ROOT / "src/smb-vfs/build.rs").read_text()
        self.assertIn("samba (" + target["package_version"] + ")", build)
        self.assertIn("SMB_VFS_INTERFACE_VERSION 50", build)
        sdk = (ROOT / ".github/scripts/smb-sdk.sh").read_text()
        self.assertIn("make -f debian/rules configure", sdk)
        self.assertIn("apt-get source --download-only --only-source", sdk)

    def test_runtime_is_network_isolated_and_no_service_autostart(self):
        workflow = (ROOT / ".github/workflows/smb-sdk.yml").read_text()
        self.assertIn("--network none", workflow)
        self.assertIn("ZXS_ISOLATED_SAMBA_TEST=1", workflow)
        image = (ROOT / ".github/containers/debian13-runtime.Dockerfile").read_text()
        self.assertIn("policy-rc.d", image)
        self.assertIn("exit 101", image)
        self.assertNotIn("build-essential", image)
        self.assertNotIn("samba-dev", image)
        script = (ROOT / ".github/scripts/check-system-samba.py").read_text()
        self.assertIn('"NT_STATUS_ACCESS_DENIED" not in denial', script)
        self.assertIn('"--client-protection=encrypt"', script)


if __name__ == "__main__":
    unittest.main()
