"""Select the logical-input consumer proof without replacing older coverage."""
import importlib.util
import json
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location(
    "workspace_input_targeted_ci", ROOT / ".github/scripts/check-targeted-ci.py")
CI = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CI)

PATHS = [
    "src/engine/src/business/workspace.rs",
    "src/engine/src/business/workspace/file_location.rs",
    "src/engine/src/business/namespace.rs",
    "src/engine/src/engine/disk_registry/current.rs",
    "src/engine/src/business/mod.rs",
    "src/engine/src/business/workspace/input.rs",
    "test/engine/unit/business/workspace_input_tests.rs",
    "test/ci/test_workspace_input_targeted_ci.py",
]
EXACT = [
    "business::workspace::selection_tests::empty_generations_still_validate_workspace_references_in_order",
    "business::workspace::selection_tests::directory_round_trip_fences_ancestors_but_not_siblings_or_other_repositories",
    "business::workspace::selection_tests::workspace_namespace_plan_keeps_frozen_source_across_preparatory_commits",
    "engine::disk_registry::current::tests::membership_registry_current_workspace_baseline_recovers_only_from_published_receipt",
    "engine::disk_registry::current::tests::membership_registry_current_cold_workspace_admission_survives_later_head_and_restart_index",
    "engine::disk_registry::current::tests::workspace_overlap::membership_registry_workspace_overlap_actual_d3_preserves_target_and_cold_restart",
]
OLD_GROUPS = {
    "src/engine/src/business/workspace.rs": {
        "workspace-version-construction", "container-tags-engine", "container-unchanged-save",
        "portable-path-consumers", "container-path-facts-engine", "container-lifecycle-engine",
        "upload-occurrence-source",
    },
    "src/engine/src/business/workspace/file_location.rs": {
        "container-path-facts-engine", "container-lifecycle-engine", "upload-occurrence-source",
    },
    "src/engine/src/business/namespace.rs": {
        "current-name-namespace-consumers", "container-current-name",
    },
    "src/engine/src/business/mod.rs": {
        "typed-name-consumers", "container-path-facts-engine", "container-lifecycle-engine",
    },
    "src/engine/src/engine/disk_registry/current.rs": {
        "container-tags-engine", "historical-directory-runtime",
        "container-path-facts-engine", "container-lifecycle-engine",
    },
}


class WorkspaceInputSelection(unittest.TestCase):
    def setUp(self):
        self.policy = json.loads((ROOT / CI.PLAN_PATH).read_text())

    def test_each_actual_path_selects_the_new_and_preserves_old_consumers(self):
        for path in PATHS:
            with self.subTest(path=path):
                plan = CI.select_plan(self.policy, [path], shardable=True)
                self.assertIn("workspace-logical-input", plan["groups"])
                self.assertTrue(OLD_GROUPS.get(path, set()).issubset(plan["groups"]))
                self.assertIn("test_workspace_input_targeted_ci", plan["python_tests"])
        unrelated = CI.select_plan(self.policy, ["src/engine/src/protection.rs"], shardable=True)
        self.assertNotIn("workspace-logical-input", unrelated["groups"])

    def test_exact_two_new_six_existing_and_product_checks_are_not_stale(self):
        group = self.policy["groups"]["workspace-logical-input"]
        runtime = [target for target in group["rust_targets"]
                   if target.get("mode", "test") == "test"]
        self.assertEqual(len(runtime), 1)
        self.assertEqual(runtime[0]["prefixes"], ["business::workspace::input::tests::"])
        self.assertEqual(runtime[0]["exact"], EXACT)
        self.assertEqual(runtime[0]["max_tests"], 8)
        checks = [target for target in group["rust_targets"] if target.get("mode") == "check"]
        self.assertEqual(
            {(target["package"], target.get("default_features", True)) for target in checks},
            {("storage-engine", True), ("storage-engine", False)},
        )
        previous = self.policy["groups"]["workspace-version-construction"]["rust_targets"][0]
        self.assertEqual(previous["prefixes"], ["business::workspace::version_build::tests::"])
        self.assertEqual(previous["max_tests"], 2)


if __name__ == "__main__":
    unittest.main()
