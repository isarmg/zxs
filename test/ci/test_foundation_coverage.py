"""Invariants of the generated foundation coverage map (no Cargo needed)."""
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
MAP = json.loads((ROOT / "test/ci/fixtures/foundation-test-coverage.json").read_text())
CATALOG = json.loads((ROOT / "compliance/rust-sources.json").read_text())
WORKSPACES = json.loads((ROOT / "tools/rust-sources/workspaces.json").read_text())
PRODUCTION = {profile["id"] for profile in WORKSPACES["profiles"] if profile["production"]}
SPEC = importlib.util.spec_from_file_location("behaviour_gate", ROOT / ".github/scripts/check-foundation-behaviour.py")
BEHAVIOUR = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(BEHAVIOUR)
COVERAGE_SPEC = importlib.util.spec_from_file_location(
    "foundation_coverage", ROOT / "tools/rust-sources/foundation_coverage.py")
COVERAGE = importlib.util.module_from_spec(COVERAGE_SPEC)
COVERAGE_SPEC.loader.exec_module(COVERAGE)


class FoundationCoverageConsumerTests(unittest.TestCase):
    def test_cargo_tree_disables_forced_ci_color_at_process_boundary(self):
        profile = {
            "id": "sqlite-pool-validation", "manifests": ["src/platform/Cargo.toml"],
            "targets": ["x86_64-unknown-linux-gnu"], "features": ["platform-io/sqlite-pool"],
        }
        directory = "src/foundation/code-generation/proc-macro2"
        dependency = f"proc-macro2 v1.0.107 ({ROOT / directory})|default,proc-macro"
        output = (f"platform-io v0.1.0 ({ROOT / 'src/platform'})|sqlite-pool\n"
                  f"{dependency}\n{dependency} (*)\n")
        result = subprocess.CompletedProcess([], 0, output, "")
        with patch.dict(os.environ, {"CARGO_TERM_COLOR": "always"}), \
                patch.object(COVERAGE.subprocess, "run", return_value=result) as run:
            actual = COVERAGE.consumers([profile])
            self.assertEqual(os.environ["CARGO_TERM_COLOR"], "always")
        run.assert_called_once_with(
            ["cargo", "tree", "--locked", "--offline", "--color", "never",
             "-p", "platform-io", "--target", "x86_64-unknown-linux-gnu",
             "-e", "normal,build", "--prefix", "none", "-f", "{p}|{f}",
             "--features", "platform-io/sqlite-pool"],
            cwd=ROOT, text=True, capture_output=True, check=True,
        )
        self.assertEqual(actual, {directory: [{
            "profile": "sqlite-pool-validation", "target": "x86_64-unknown-linux-gnu",
            "features": ["default", "proc-macro"], "consumers": ["platform-io"],
        }]})


class FoundationCoverageMapTests(unittest.TestCase):
    def test_every_active_foundation_package_is_mapped_once(self):
        active = sorted(d for d in CATALOG["packages"] if d.startswith("src/foundation/"))
        self.assertEqual([package["directory"] for package in MAP["packages"]], active)
        self.assertEqual(len({package["package"] for package in MAP["packages"]}), len(active))

    def test_classification_follows_gates_and_profiles(self):
        for package in MAP["packages"]:
            kinds = {gate["kind"] for gate in package["gates"]}
            production = bool(set(package["profiles"]) & PRODUCTION)
            expected = ("executed" if kinds & {"U", "P", "F"} else
                        "needed-not-executed" if production else "reference-only")
            self.assertEqual(package["classification"], expected, package["package"])
            self.assertEqual(package["risk"] is None, expected != "needed-not-executed", package["package"])

    def test_behaviour_suites_count_as_executed_retained_tests(self):
        packages = {package["package"]: package for package in MAP["packages"]}
        for name in BEHAVIOUR.UNIT_SUITES + tuple(suite.subject for suite in BEHAVIOUR.INTEGRATION_SUITES):
            self.assertEqual(packages[name]["classification"], "executed", name)
            self.assertIn("U", {gate["kind"] for gate in packages[name]["gates"]}, name)

    def test_retired_records_are_listed_without_live_packages(self):
        records = {record["record"] for record in MAP["retired"]}
        self.assertEqual(records, set(CATALOG["retired"]) | set(CATALOG["superseded"]))
        self.assertFalse(records & set(CATALOG["packages"]))


if __name__ == "__main__":
    unittest.main()
