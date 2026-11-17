"""Keep cleanup and shutdown changes mapped to their actual behavior inventories."""
import importlib.util
import json
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location(
    "shutdown_cleanup_targeted_ci", ROOT / ".github/scripts/check-targeted-ci.py")
CI = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CI)

CLEANUP_PATHS = [
    "src/server/src/spool_budget.rs",
    "test/server/unit/spool_budget/tests.rs",
    "src/engine/src/disk/admission/upload.rs",
    "test/engine/unit/disk/upload_resources_tests.rs",
    "src/engine/src/engine/workspace_storage.rs",
    "test/engine/unit/engine/workspace_storage_tests.rs",
    "test/engine/integration/workspace_upload_retries.rs",
]
SHUTDOWN_PATHS = [
    "src/server/src/management.rs",
    "test/server/unit/management_shutdown_tests.rs",
    "src/server/src/management/ordinary_conversion.rs",
    "src/server/src/engine_backend/ordinary_conversion.rs",
    "test/server/unit/management/ordinary_conversion.rs",
]


class ShutdownCleanupSelection(unittest.TestCase):
    def setUp(self):
        self.policy = json.loads((ROOT / CI.PLAN_PATH).read_text())

    def test_actual_paths_retain_their_behavior_groups_and_previous_selections(self):
        for group, paths in [
            ("upload-restart-cleanup", CLEANUP_PATHS),
            ("normal-shutdown-owners", SHUTDOWN_PATHS),
        ]:
            for path in paths:
                with self.subTest(path=path):
                    plan = CI.select_plan(self.policy, [path], shardable=True)
                    self.assertIn(group, plan["groups"])
                    self.assertIn("test_shutdown_cleanup_targeted_ci", plan["python_tests"])
        for path in CLEANUP_PATHS:
            if path.startswith("test/engine/unit/"):
                plan = CI.select_plan(self.policy, [path], shardable=True)
                self.assertIn("fresh-managed-core", plan["groups"])
        for path in SHUTDOWN_PATHS:
            if path.endswith("management/ordinary_conversion.rs"):
                plan = CI.select_plan(self.policy, [path], shardable=True)
                self.assertIn("ordinary-conversion-name-budget", plan["groups"])

    def test_cleanup_inventories_cover_real_modules_not_only_unrelated_fresh_pool_cases(self):
        group = self.policy["groups"]["upload-restart-cleanup"]
        runtime = [target for target in group["rust_targets"]
                   if target.get("mode", "test") == "test"]
        engine = next(target for target in runtime
                      if target["package"] == "storage-engine" and target["kind"] == "lib")
        self.assertEqual(engine["prefixes"], [
            "disk::admission::upload::tests::", "engine::workspace_storage::tests::"])
        self.assertEqual(engine["features"], ["fault-injection"])
        upload = next(target for target in runtime
                      if target.get("name") == "workspace_upload_retries")
        self.assertEqual(upload["kind"], "test")
        self.assertTrue(upload["whole"])
        self.assertEqual(upload["max_tests"], 7)
        spool = next(target for target in runtime if target["package"] == "storage-server")
        self.assertEqual(spool["prefixes"], ["spool_budget::tests::"])
        self.assertEqual(spool["max_tests"], 13)
        checks = [target for target in group["rust_targets"] if target.get("mode") == "check"]
        self.assertEqual({target["package"] for target in checks},
                         {"storage-engine", "storage-server"})
        self.assertTrue(all("fault-injection" not in target.get("features", [])
                            for target in checks))

    def test_shutdown_inventory_keeps_original_conversion_budget_and_product_modes(self):
        group = self.policy["groups"]["normal-shutdown-owners"]
        runtime = group["rust_targets"][0]
        self.assertEqual(runtime["prefixes"], ["management::shutdown_tests::"])
        self.assertEqual(len(runtime["exact"]), 4)
        self.assertTrue(all(name.startswith("management::ordinary_conversion::tests::")
                            for name in runtime["exact"]))
        self.assertEqual(runtime["max_tests"], 16)
        previous = self.policy["groups"]["ordinary-conversion-name-budget"]["rust_targets"][1]
        self.assertEqual(previous["prefixes"], ["management::ordinary_conversion::tests::"])
        self.assertEqual(previous["max_tests"], 26)
        self.assertEqual(
            {target.get("default_features", True) for target in group["rust_targets"]
             if target.get("mode") == "check"}, {True, False})
        unrelated = CI.select_plan(self.policy, ["src/contract/src/tracked.rs"], shardable=True)
        self.assertNotIn("upload-restart-cleanup", unrelated["groups"])
        self.assertNotIn("normal-shutdown-owners", unrelated["groups"])


if __name__ == "__main__":
    unittest.main()
