import hashlib
import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[3]
SPEC = importlib.util.spec_from_file_location("payload_provenance", ROOT / "tools/perf-check/payload-provenance.py")
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class PayloadProvenanceTests(unittest.TestCase):
    def test_manifest_binds_both_sources_fixtures_binaries_and_overlay(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            output = root / "evidence"
            output.mkdir()
            for label in ("baseline", "candidate"):
                source = root / label
                for name in MODULE.FIXTURES:
                    fixture = source / name
                    fixture.parent.mkdir(parents=True, exist_ok=True)
                    fixture.write_bytes(b"identical fixture")
                binary = root / f"{label}-test"
                binary.write_bytes(label.encode())
                (output / f"{label}-binary.txt").write_text(str(binary))

            def command(arguments, cwd):
                if arguments == ["git", "rev-parse", "HEAD"]:
                    return ("a" if cwd.name == "baseline" else "b").encode() * 40 + b"\n"
                return (arguments[0] + " pinned verbose version\n").encode()

            with patch.object(MODULE, "source_overlay", return_value=b"complete patch\n"), \
                 patch.object(MODULE, "command", side_effect=command):
                manifest = MODULE.record(root / "baseline", root / "candidate", output)
            self.assertEqual(json.loads((output / "provenance.json").read_text()), manifest)
            self.assertEqual((output / "baseline-overlay.patch").read_bytes(), b"complete patch\n")
            self.assertEqual(manifest["baseline_overlay"]["sha256"], hashlib.sha256(b"complete patch\n").hexdigest())
            for label, source in manifest["sources"].items():
                self.assertEqual(source["commit"], ("a" if label == "baseline" else "b") * 40)
                self.assertEqual(source["binary"]["sha256"], hashlib.sha256(label.encode()).hexdigest())
                self.assertEqual(set(source["toolchain"]), {"rustc", "cargo"})
                self.assertEqual(set(source["fixtures_sha256"]), set(MODULE.FIXTURES))
                self.assertEqual(set(source["fixtures_sha256"].values()), {hashlib.sha256(b"identical fixture").hexdigest()})

    def test_baseline_patch_includes_untracked_fixture_bytes(self):
        with patch.object(MODULE, "command", side_effect=[b"tracked patch\n", b"new fixture.rs\0"]), \
             patch.object(MODULE.subprocess, "run", return_value=subprocess.CompletedProcess([], 1, b"new file patch\n")) as run:
            self.assertEqual(MODULE.source_overlay(Path("baseline")), b"tracked patch\nnew file patch\n")
        self.assertEqual(run.call_args.args[0][-2:], ["/dev/null", "new fixture.rs"])

    def test_failed_overlay_read_is_not_reported_as_empty_patch(self):
        with patch.object(MODULE, "command", side_effect=[b"", b"fixture.rs\0"]), \
             patch.object(MODULE.subprocess, "run", return_value=subprocess.CompletedProcess([], 2, b"", b"failure")):
            with self.assertRaises(subprocess.CalledProcessError):
                MODULE.source_overlay(Path("baseline"))

    def test_measurement_job_restricts_dispatch_and_keeps_provenance_before_measurement(self):
        workflow = (ROOT / ".github/workflows/payload-measurement.yml").read_text()
        self.assertIn("  measure:\n    if: startsWith(github.ref, 'refs/heads/verify/payload-copy-')\n", workflow)
        self.assertIn("  workflow_dispatch:", workflow)
        self.assertIn('git worktree add --detach "$baseline" 9552cf36910b6b644d97d149f692e5c6e2cee841', workflow)
        self.assertNotIn("recovery/tests.rs", workflow)
        self.assertEqual(MODULE.FIXTURES, ("test/engine/unit/engine/payload_measurement.rs",))
        self.assertLess(workflow.index("name: Record source and binary provenance"), workflow.index("name: Compare untraced"))
        self.assertIn("path: payload-measurement/", workflow)
        self.assertIn("if: always()", workflow)


if __name__ == "__main__":
    unittest.main()
