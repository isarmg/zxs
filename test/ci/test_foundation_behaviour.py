"""Static wiring of the foundation behaviour gate: suites, inventory, features and owners."""
import importlib.util
import json
from pathlib import Path
import re
import tomllib
import unittest

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("behaviour_gate", ROOT / ".github/scripts/check-foundation-behaviour.py")
GATE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(GATE)

# Owners whose files need proof-maintenance receipts or a new adapter (docs/EXTERNAL_TEST_SOURCES.md).
PROOF_BOUND = ("tokio", "synchronization", "system-entropy", "libc", "memchr", "mio", "rustc-demangle",
               "ring", "http-transport")


class FoundationBehaviourGateTests(unittest.TestCase):
    def test_gate_runs_once_in_the_async_shard(self):
        script = (ROOT / ".github/scripts/check-foundation.sh").read_text()
        async_case = script.split("  foundation-async)\n", 1)[1].split(";;", 1)[0]
        self.assertEqual(script.count("check-foundation-behaviour.py"), 1)
        self.assertIn("python3 .github/scripts/check-foundation-behaviour.py", async_case)

    def test_suite_sources_exist_and_unexecuted_material_is_explicit(self):
        executed = set()
        for suite in GATE.INTEGRATION_SUITES:
            for path in suite.tests:
                self.assertTrue((ROOT / path).is_file(), path)
                executed.add(path)
            for path in suite.examples:
                self.assertTrue((GATE.foundation_path(ROOT, suite.subject) / path).is_file(), path)
        for path, reason in GATE.NOT_EXECUTED.items():
            self.assertTrue((ROOT / path).is_file(), path)
            self.assertNotIn(path, executed)
            self.assertTrue(reason.strip())
        self.assertEqual(len(GATE.suite_keys()), len(set(GATE.suite_keys())))

    def test_inventory_is_generated_for_exactly_the_suite_table(self):
        inventory = json.loads(GATE.INVENTORY.read_text())
        self.assertEqual(inventory["schema"], 1)
        self.assertEqual(sorted(inventory["suites"]), sorted(GATE.suite_keys()))
        for suite in GATE.INTEGRATION_SUITES:
            recorded = inventory["suites"]["integration:" + suite.subject]
            self.assertEqual(sorted(recorded["targets"]), sorted(map(GATE.target_name, suite.tests)))
        for key, value in inventory["suites"].items():
            for target, names in value["targets"].items():
                self.assertTrue(names, f"{key}/{target} would accept an empty binary")
                self.assertEqual(names, sorted(set(names)), f"{key}/{target}")
        self.assertEqual(GATE.INVENTORY.read_text(), GATE.inventory_text(inventory["suites"]))

    def test_integration_members_use_production_features_not_all_features(self):
        production = GATE.production_features()
        flume = next(suite for suite in GATE.INTEGRATION_SUITES if suite.subject == "flume")
        manifest = tomllib.loads(GATE.member_manifest(flume, production))
        self.assertEqual(production["flume"], ["async"])
        self.assertEqual(manifest["dependencies"]["flume"]["features"], ["async"])
        self.assertFalse(manifest["dependencies"]["flume"]["default-features"])
        self.assertEqual(manifest["features"]["default"], ["async"])
        declared = tomllib.loads((GATE.foundation_path(ROOT, "flume") / "Cargo.toml").read_text())["features"]
        self.assertEqual(set(manifest["features"]) - {"default"}, set(declared) - {"default"})
        source = (ROOT / ".github/scripts/check-foundation-behaviour.py").read_text()
        self.assertNotIn("--all-features", source)

    def test_suites_do_not_execute_material_of_receipt_bound_owners(self):
        for suite in GATE.INTEGRATION_SUITES:
            self.assertNotIn(suite.subject, PROOF_BOUND)
            for path in suite.tests:
                for owner in PROOF_BOUND:
                    self.assertFalse(path.startswith(f"test/foundation/{owner}/"), path)
        self.assertFalse(set(GATE.UNIT_SUITES) & set(PROOF_BOUND))

    def test_retained_tests_use_local_providers_only(self):
        absent = re.compile(r"\b(?:rand|easy_parallel|futures_test|pin_utils|async_std|crossbeam_channel)::")
        for suite in GATE.INTEGRATION_SUITES:
            for path in suite.tests:
                self.assertIsNone(absent.search((ROOT / path).read_text()), path)


if __name__ == "__main__":
    unittest.main()
