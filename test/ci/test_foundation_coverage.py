"""Invariants of the generated foundation coverage map (no Cargo needed)."""
import importlib.util
import json
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]
MAP = json.loads((ROOT / "test/ci/fixtures/foundation-test-coverage.json").read_text())
CATALOG = json.loads((ROOT / "compliance/rust-sources.json").read_text())
WORKSPACES = json.loads((ROOT / "tools/rust-sources/workspaces.json").read_text())
PRODUCTION = {profile["id"] for profile in WORKSPACES["profiles"] if profile["production"]}
SPEC = importlib.util.spec_from_file_location("behaviour_gate", ROOT / ".github/scripts/check-foundation-behaviour.py")
BEHAVIOUR = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(BEHAVIOUR)


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
