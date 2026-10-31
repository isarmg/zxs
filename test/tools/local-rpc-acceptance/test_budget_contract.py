"""Independent arithmetic and grammar checks for the conservative support bound."""
import importlib.util
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[3]
SPEC = importlib.util.spec_from_file_location('budget_contract', ROOT/'tools/local-rpc-acceptance/budget_contract.py')
BUDGET = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(BUDGET)


class BudgetContractTests(unittest.TestCase):
    def test_220_is_initial_history_not_sample_count(self):
        p = BUDGET.proof()
        self.assertEqual((p['files'], p['seed_sequence'], p['original_saves']), (500, 220, 20))
        self.assertEqual(p['original_end_sequence'], 240)
        self.assertEqual((p['tail_rounds'], p['tail_publications'], p['required_end_sequence']), (40, 120, 360))

    def test_lower_bound_exceeds_default_without_other_evidence(self):
        p = BUDGET.proof()
        self.assertEqual(p['two_map_min_bytes_per_workspace'], 211002)
        self.assertEqual(p['original_full_workspace_min'], 238)
        self.assertEqual(p['required_two_map_min_bytes'], 75538716)
        self.assertEqual(p['excess_min_bytes'], 8429852)
        self.assertEqual(p['principal_limit_bytes'], 67108864)
        self.assertLess(p['original_two_map_min_bytes'], p['principal_limit_bytes'])
        self.assertEqual(p['support'], 'FAIL_DEFAULT_BUDGET')

    def test_id_prefixes_and_outer_field_names_are_excluded_conservatively(self):
        p = BUDGET.proof()
        self.assertIn('ID prefixes', p['exclusions'])
        self.assertIn('initial upload workspace', p['exclusions'])
        self.assertEqual(p['supported_full_workspaces_upper'], 318)


if __name__ == '__main__':
    unittest.main()
