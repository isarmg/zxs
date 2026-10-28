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

    def test_comparison_and_date_suites_use_only_retained_functional_targets(self):
        self.assertIn("cmov", GATE.UNIT_SUITES)
        self.assertIn("httpdate", GATE.UNIT_SUITES)
        # On x86_64 CI, lib tests are instruction-count gated or AArch64-only, not a functional suite.
        self.assertNotIn("constant_time_eq", GATE.UNIT_SUITES)
        suites = {suite.subject: suite for suite in GATE.INTEGRATION_SUITES}
        expected = {
            "cmov": ("test/foundation/cmov/tests/core_impls.rs",
                     "test/foundation/cmov/tests/regression.rs"),
            "constant_time_eq": ("test/foundation/constant_time_eq/tests/exhaustive.rs",),
        }
        for name, paths in expected.items():
            with self.subTest(subject=name):
                self.assertEqual(suites[name].tests, paths)
                self.assertEqual(suites[name].providers, ())
                self.assertEqual(suites[name].examples, ())

    def test_comparison_and_date_features_match_real_consumers(self):
        production = GATE.production_features()
        inventory = json.loads(GATE.INVENTORY.read_text())["suites"]
        for name in ("cmov", "httpdate"):
            with self.subTest(subject=name):
                self.assertEqual(production[name], [])
                self.assertEqual(inventory["unit:" + name]["features"], [])
        suites = {suite.subject: suite for suite in GATE.INTEGRATION_SUITES}
        for name, features in (("cmov", []), ("constant_time_eq", ["std"])):
            with self.subTest(subject=name):
                self.assertEqual(production[name], features)
                manifest = tomllib.loads(GATE.member_manifest(suites[name], production))
                self.assertEqual(set(manifest["dependencies"]), {name})
                self.assertEqual(manifest["dependencies"][name]["features"], features)
                self.assertFalse(manifest["dependencies"][name]["default-features"])
                self.assertEqual(manifest["features"]["default"], features)
                self.assertEqual(inventory["integration:" + name]["features"], features)

    def test_missing_comparison_providers_remain_explicitly_unexecuted(self):
        expected = {"test/foundation/cmov/tests/proptests.rs": "proptest"}
        expected.update({f"test/foundation/constant_time_eq/tests/{target}.rs": "count_instructions"
                         for target in ("count_instructions", "count_instructions_classic",
                                        "count_instructions_generic")})
        for path, provider in expected.items():
            with self.subTest(path=path):
                self.assertIn(provider, GATE.NOT_EXECUTED[path])

    def test_retained_tests_use_local_providers_only(self):
        absent = re.compile(r"\b(?:rand|easy_parallel|futures_test|pin_utils|async_std|crossbeam_channel)::")
        for suite in GATE.INTEGRATION_SUITES:
            for path in suite.tests:
                self.assertIsNone(absent.search((ROOT / path).read_text()), path)


if __name__ == "__main__":
    unittest.main()
