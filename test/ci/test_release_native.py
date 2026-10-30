"""Native build receipt tests use fake subprocesses, never a Rust compiler."""
import importlib.util
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("release_native", ROOT / ".github/scripts/release-native.py")
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class NativeRelease(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        for name in ("Cargo.lock", "Cargo.toml", ".cargo/config.toml", "rust-toolchain.toml"):
            path = self.root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("fixture input")
        self.output = self.root / MODULE.OUTPUT
        self.output.mkdir(parents=True)
        for name in MODULE.BINARIES:
            (self.output / name).write_bytes(name.encode())
        self.receipt = self.output / "BUILD.txt"
        self.receipt.write_text("stale successful build")
        self.enterContext(patch.object(MODULE, "__file__", str(self.root / ".github/scripts/release-native.py")))
        self.enterContext(patch.object(MODULE.os, "chdir"))
        self.enterContext(patch.dict(os.environ, {"RELEASE_VERSION": "0.1.0", "CFLAGS": "-DPRIVATE=secret"}, clear=True))
        self.enterContext(patch.object(MODULE.subprocess, "check_output",
                                      side_effect=["a" * 40 + "\n", "rustc fixture\nhost: x86_64-unknown-linux-gnu\n",
                                                   "a" * 40 + "\n"]))

    def test_build_uses_explicit_target_and_records_outputs_without_environment_values(self):
        with patch.object(MODULE.subprocess, "run") as run:
            MODULE.main()
        command = run.call_args.args[0]
        self.assertEqual(command[command.index("--target") + 1], MODULE.TARGET)
        self.assertEqual(command[command.index("--target-dir") + 1], "target/native")
        self.assertIn("storage-server", command)
        self.assertIn("storage-rescue", command)
        content = self.receipt.read_text()
        self.assertIn("native-target=x86_64-unknown-linux-gnu\n", content)
        self.assertIn("native-zxs-sha256=" + MODULE.digest(self.output / "zxs"), content)
        self.assertIn("native-zxs-rescue-sha256=" + MODULE.digest(self.output / "zxs-rescue"), content)
        self.assertNotIn("secret", content)
        self.assertNotIn("stale", content)

    def test_failed_build_removes_old_receipt(self):
        with patch.object(MODULE.subprocess, "run", side_effect=subprocess.CalledProcessError(1, "cargo")):
            with self.assertRaises(subprocess.CalledProcessError):
                MODULE.main()
        self.assertFalse(self.receipt.exists())

    def test_cargo_build_rustc_override_is_used_for_compiler_query(self):
        with patch.dict(os.environ, {"CARGO_BUILD_RUSTC": "/private/toolchain/rustc"}), \
                patch.object(MODULE.subprocess, "run"):
            MODULE.main()
        self.assertEqual(MODULE.subprocess.check_output.call_args_list[1].args[0],
                         ["/private/toolchain/rustc", "-vV"])
        self.assertNotIn("/private/toolchain", self.receipt.read_text())


if __name__ == "__main__":
    unittest.main()
