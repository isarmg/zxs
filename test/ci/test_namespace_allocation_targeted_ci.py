"""Preserve scoped namespace allocation coverage and overlapping consumers."""
import importlib.util
import json
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location(
    "namespace_allocation_targeted_ci", ROOT / ".github/scripts/check-targeted-ci.py")
CI = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CI)

PATHS = [
    'src/engine/src/business/namespace.rs',
    'src/engine/src/business/namespace/draft.rs',
    'test/engine/unit/business/namespace/tests.rs',
    'test/engine/unit/business/namespace/draft_tests.rs',
    'test/ci/test_namespace_allocation_targeted_ci.py',
]
NEW = [
    'business::namespace::tests::ordered_namespace_allocations_preserve_chained_paths_and_deletion_facts',
    'business::namespace::tests::namespace_preconditions_fail_before_allocating_any_new_identity',
    'business::namespace::draft::tests::explicit_parent_ids_preserve_failure_prefix_and_retry_only_missing_parents',
]
OLD = [
    'business::namespace::tests::directory_rename_follows_normalized_ancestry_without_slicing_utf8_by_key_length',
    'business::namespace::tests::namespace_deletion_facts_preserve_repeated_intermediate_paths_and_tree_members',
    'business::namespace::tests::canonical_equivalent_namespace_sources_preserve_identity_and_strict_destinations',
    'business::namespace::draft::tests::shared_parent_creation_matches_legacy_order_and_preserves_existing_identity',
    'business::namespace::draft::tests::parent_file_errors_do_not_replace_or_drop_the_existing_entry',
    'business::namespace::draft::tests::flattened_draft_keeps_namespace_operations_and_deletion_order_unchanged',
]
OLD_GROUPS = {
    "src/engine/src/business/namespace.rs": {
        "workspace-logical-input", "current-name-namespace-consumers", "container-current-name",
    },
    "test/engine/unit/business/namespace/tests.rs": {"portable-path-consumers"},
    "test/engine/unit/business/namespace/draft_tests.rs": {"portable-path-consumers"},
}


class NamespaceAllocationSelection(unittest.TestCase):
    def setUp(self):
        self.policy = json.loads((ROOT / CI.PLAN_PATH).read_text())

    def test_each_actual_path_preserves_all_older_consumers(self):
        for path in PATHS:
            with self.subTest(path=path):
                plan = CI.select_plan(self.policy, [path], shardable=True)
                self.assertIn("namespace-allocation", plan["groups"])
                self.assertTrue(OLD_GROUPS.get(path, set()).issubset(plan["groups"]))
                self.assertIn("test_namespace_allocation_targeted_ci", plan["python_tests"])
        unrelated = CI.select_plan(
            self.policy, ["src/engine/src/business/workspace/version_build.rs"], shardable=True)
        self.assertNotIn("namespace-allocation", unrelated["groups"])
        self.assertIn("workspace-version-construction", unrelated["groups"])

    def test_three_new_six_existing_and_both_product_modes_are_selected(self):
        group = self.policy["groups"]["namespace-allocation"]
        runtime = [target for target in group["rust_targets"]
                   if target.get("mode", "test") == "test"]
        self.assertEqual(len(runtime), 1)
        self.assertEqual(runtime[0]["exact"], NEW + OLD)
        self.assertNotIn("prefixes", runtime[0])
        self.assertEqual(runtime[0]["max_tests"], 9)
        checks = [target for target in group["rust_targets"] if target.get("mode") == "check"]
        self.assertEqual(
            {(target["package"], target.get("default_features", True)) for target in checks},
            {("storage-engine", True), ("storage-engine", False)},
        )
        self.assertEqual(
            self.policy["groups"]["workspace-logical-input"]["rust_targets"][0]["max_tests"], 8)
        self.assertEqual(
            self.policy["groups"]["workspace-version-construction"]["rust_targets"][0]["max_tests"], 2)


if __name__ == "__main__":
    unittest.main()
