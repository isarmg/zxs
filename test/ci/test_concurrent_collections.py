import importlib.util
from pathlib import Path
import unittest
from unittest.mock import call, patch

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("concurrency_gate", ROOT / ".github/scripts/check-concurrent-collections.py")
GATE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(GATE)


class ConcurrentCollectionsGateTests(unittest.TestCase):
    def test_sqlite_default_and_optional_pool_consumers_are_both_compiled(self):
        with patch.object(GATE, 'run') as execute:
            GATE.check_consumers()
        self.assertEqual(execute.call_args_list, [
            call(['cargo', 'check', '--locked', '--offline', '-p', 'rayon', '-p', 'sqlite-access']),
            call(['cargo', 'check', '--locked', '--offline', '-p', 'platform-io',
                  '--features', 'platform-io/sqlite-pool']),
        ])

    def test_native_and_wasm_feature_matrix(self):
        self.assertEqual(len(GATE.TARGETS) * len(GATE.FEATURES), 6)
        self.assertEqual(GATE.FEATURES[0], ())
        self.assertIn(("alloc",), GATE.FEATURES)
        self.assertEqual(set(GATE.TARGETS), {"x86_64-unknown-linux-gnu", "wasm32-unknown-unknown"})

    def test_all_retained_integrations_exist(self):
        self.assertEqual(len(GATE.INTEGRATIONS), 6)
        for module, name in GATE.INTEGRATIONS:
            self.assertTrue((ROOT / "test/foundation/concurrent-collections" / module / "tests" / (name + ".rs")).is_file())

    def test_formerly_dormant_integrations_run_once_in_the_behaviour_gate(self):
        spec = importlib.util.spec_from_file_location(
            "behaviour_gate", ROOT / ".github/scripts/check-foundation-behaviour.py")
        behaviour = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(behaviour)
        suite, = [s for s in behaviour.INTEGRATION_SUITES if s.subject == "concurrent-collections"]
        executed = {path.removeprefix("test/foundation/concurrent-collections/") for path in suite.tests}
        self.assertEqual(executed, set(GATE.BEHAVIOUR_GATE))
        self.assertFalse(set(GATE.BEHAVIOUR_GATE) & {f"{module}/tests/{name}.rs" for module, name in GATE.INTEGRATIONS})
        for path in GATE.BEHAVIOUR_GATE:
            source = (ROOT / "test/foundation/concurrent-collections" / path).read_text()
            self.assertNotRegex(source, r"\brand::")

    def test_semantics_fixture_covers_four_real_cases(self):
        source = (ROOT / "test/tools/concurrent-collections/semantics.rs").read_text()
        self.assertEqual(source.count("#[test]"), 4)
        for name in ("rayon_parallel_consumer", "bounded_mpmc_exactly_once", "unbounded_fifo_and_drop_exactly_once", "batch_steal_preserves_all_tasks"):
            self.assertIn("fn " + name, source)

    def test_empty_nightly_compatibility_feature_is_removed(self):
        import tomllib
        manifest = tomllib.loads((ROOT / "src/foundation/concurrency/concurrent-collections/Cargo.toml").read_text())
        self.assertNotIn("nightly", manifest["features"])
        self.assertEqual(GATE.FEATURES, ((), ("alloc",), ("std",)))
        self.assertTrue(callable(GATE.check_removed_feature))
