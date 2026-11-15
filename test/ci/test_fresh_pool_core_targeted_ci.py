"""Select the real fresh engine flow without dropping existing target coverage."""
import importlib.util
import json
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location(
    "fresh_pool_core_targeted_ci", ROOT / ".github/scripts/check-targeted-ci.py")
CI = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CI)

PARENT = "test/engine/integration/workspace_commit_replay.rs"
CHILD = "test/engine/integration/workspace_commit_replay/fresh_pool.rs"
ORACLE = "test/ci/test_fresh_pool_core_targeted_ci.py"
SHARED_LAYOUT = "src/engine/src/container_storage/shared_layout.rs"
INITIALIZATION = "persist::layout::tests::initialization_and_recovery_use_the_existing_disk_paths_and_bytes"
EXACT = "fresh_pool::empty_held_pool_stages_reopens_commits_and_selects_real_file_history"


class FreshPoolCoreSelection(unittest.TestCase):
    def setUp(self):
        self.policy = json.loads((ROOT / CI.PLAN_PATH).read_text())

    def test_every_actual_path_selects_the_real_flow_and_preserves_parent_consumers(self):
        for path in [PARENT, CHILD, ORACLE, SHARED_LAYOUT]:
            with self.subTest(path=path):
                plan = CI.select_plan(self.policy, [path], shardable=True)
                self.assertIn("fresh-pool-core", plan["groups"])
                self.assertIn("test_fresh_pool_core_targeted_ci", plan["python_tests"])
                if path == PARENT:
                    self.assertIn("portable-path-consumers", plan["groups"])
                elif path in [SHARED_LAYOUT, ORACLE]:
                    self.assertEqual(plan["groups"], ["directory-layout-probe", "fresh-pool-core"])
                else:
                    self.assertEqual(plan["groups"], ["fresh-managed-core", "fresh-pool-core"])
        previous = self.policy["groups"]["portable-path-consumers"]["rust_targets"]
        retained = [target for target in previous
                    if target.get("name") == "workspace_commit_replay"]
        self.assertEqual(len(retained), 1)
        self.assertTrue(retained[0]["whole"])
        self.assertEqual(retained[0]["max_tests"], 160)

    def test_exact_runtime_cases_and_both_product_modes_are_selected(self):
        group = self.policy["groups"]["fresh-pool-core"]
        runtime = [target for target in group["rust_targets"]
                   if target.get("mode", "test") == "test"]
        self.assertEqual(len(runtime), 2)
        initialization = next(target for target in runtime if target["exact"] == [INITIALIZATION])
        self.assertEqual(initialization["max_tests"], 1)
        self.assertEqual(runtime[0]["name"], "workspace_commit_replay")
        self.assertEqual(runtime[0]["exact"], [EXACT])
        self.assertEqual(runtime[0]["max_tests"], 1)
        self.assertNotIn("prefixes", runtime[0])
        self.assertNotIn("whole", runtime[0])
        self.assertNotIn("features", runtime[0])
        checks = [target for target in group["rust_targets"] if target.get("mode") == "check"]
        self.assertEqual(
            {(target["package"], target.get("default_features", True)) for target in checks},
            {("storage-engine", True), ("storage-engine", False)},
        )
        unrelated = CI.select_plan(
            self.policy, ["src/engine/src/business/workspace/version_build.rs"], shardable=True)
        self.assertNotIn("fresh-pool-core", unrelated["groups"])
        self.assertIn("workspace-version-construction", unrelated["groups"])


if __name__ == "__main__":
    unittest.main()
