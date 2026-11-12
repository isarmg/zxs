"""Keep pure-encoding reuse covered without relaxing Backup's original budget."""
import json
from pathlib import Path
import unittest

from test_targeted_ci import CI

ROOT = Path(__file__).resolve().parents[2]
PATHS = [
    'src/engine/src/engine/backup_preparation/native_control/databases/scoped/cutover/registration/published_scopes/projection/timing.rs',
    "src/engine/src/engine/backup_preparation/native_control/databases/scoped/cutover/registration.rs",
    "src/engine/src/engine/backup_preparation/native_control/databases/scoped/cutover/registration/runtime.rs",
    "src/engine/src/engine/backup_preparation/native_control/databases/scoped/cutover/registration/published_scopes.rs",
    "src/engine/src/engine/backup_preparation/native_control/databases/scoped/cutover/registration/published_scopes/projection.rs",
    "src/engine/src/engine/backup_preparation/native_control/databases/scoped/cutover/registration/published_scopes/encoding_memo.rs",
    "test/engine/unit/backup_scope_encoding_memo_tests.rs",
    "src/engine/src/engine/index/preparation.rs",
    "src/engine/src/engine/disk_catalog/history/preparation.rs",
    "test/engine/unit/engine/index/preparation_readback_tests.rs",
    "test/engine/unit/engine/disk_scope_encoding_tests.rs"
]

class BackupEncodingMemoSelection(unittest.TestCase):
    def setUp(self):
        self.policy = json.loads((ROOT / CI.PLAN_PATH).read_text())

    def test_each_encoder_cache_and_fixture_selects_the_closed_regression_group(self):
        for path in PATHS + [
            'docs/validation/backup-scoped-encoding-memo.md',
            'test/ci/test_backup_encoding_memo_ci.py',
        ]:
            with self.subTest(path=path):
                plan = CI.select_plan(self.policy, [path])
                self.assertIn('backup-scoped-encoding-memo', plan['groups'])
                self.assertIn('test_backup_encoding_memo_ci', plan['python_tests'])

    def test_complete_slice_keeps_product_check_before_original_900_second_transaction(self):
        plan = CI.select_plan(self.policy, PATHS + [
            'docs/validation/backup-scoped-encoding-memo.md',
            'test/ci/test_backup_encoding_memo_ci.py',
            '.github/ci/targeted-tests.json',
            'TASKS.md',
        ])
        self.assertEqual(plan['groups'], ['backup-scoped-encoding-memo', 'ci-selection'])
        self.assertEqual(len(plan['rust_targets']), 3)
        self.assertEqual(len(plan['python_tests']), 8)
        group = self.policy['groups']['backup-scoped-encoding-memo']
        unit, product, transaction = group['rust_targets']
        self.assertIn('engine::index::preparation::tests::', unit['prefixes'])
        self.assertIn('engine::disk_catalog::history::preparation::encoding_tests::',
                      unit['prefixes'])
        self.assertIn('container_storage::backup_original_objects::tests::',
                      unit['prefixes'])
        self.assertIn('container_storage::backup_entities::budget_tests::',
                      unit['prefixes'])
        self.assertEqual(product['mode'], 'check')
        self.assertFalse(product['default_features'])
        self.assertEqual(transaction['name'], 'recursive_backup_paths')
        self.assertEqual(transaction['exact'], [
            'backup_preparation::transaction_serving::'
            'same_backup_switch_serves_and_replays_normal_successor_publications'])
        self.assertEqual(transaction['max_tests'], 1)
        self.assertEqual(transaction['timeout_seconds'], 900)

if __name__ == '__main__':
    unittest.main()
