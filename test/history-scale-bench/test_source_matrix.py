"""Source selection cannot silently omit a comparison or execute an old source."""

import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "tools/history-scale-bench"))
from controller import BASELINE, PROFILES
import run


class SourceMatrixTests(unittest.TestCase):
    def arguments(self, root, *extra):
        frozen = "d3eefd289e53a2803703870fe4250d08cadff9b5"
        plan = root / "plan.json"
        plan.write_text(json.dumps({"schema_version": 1, "baseline_sha": BASELINE,
                                   "reviewed_candidate_sha": frozen, "frozen_candidate_sha": frozen,
                                   "ten_thousand_review": None, "profiles": list(PROFILES)}))
        binary, proof = root / "binary", root / "provenance.json"
        binary.touch()
        proof.touch()
        argv = ["run.py", "--plan", str(plan), "--profile", "boundaries",
                "--candidate-source", str(root), "--candidate-binary", str(binary),
                "--candidate-provenance", str(proof), "--build-run-id", "123",
                "--build-carrier-sha", "a" * 40, "--output-root", str(root / "output"), *extra]
        with patch.object(sys, "argv", argv):
            return run.arguments()

    def test_explicit_current_only_selects_exactly_one_frozen_source(self):
        with tempfile.TemporaryDirectory() as directory:
            args = self.arguments(Path(directory), "--candidate-only")
            self.assertEqual(args.revisions, {"candidate": "d3eefd289e53a2803703870fe4250d08cadff9b5"})
            self.assertTrue(args.scale_authorized)
            self.assertEqual(args.sizes, [255, 256, 257, 1023, 1024, 1025])

    def test_omitted_comparison_requires_explicit_mode_and_cannot_add_old_inputs(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for extra in ((), ("--candidate-only", "--baseline-source", str(root)),
                          ("--candidate-only", "--profile", "1000"),
                          ("--candidate-only", "--profile", "10000")):
                with self.subTest(extra=extra), self.assertRaises(SystemExit):
                    self.arguments(root, *extra)
