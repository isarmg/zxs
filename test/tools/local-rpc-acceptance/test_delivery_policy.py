"""User-scoped advisory metrics never relax data, resource or other gates."""
import unittest
import screen


class DeliveryPolicyTests(unittest.TestCase):
    def test_only_the_three_removed_targets_are_advisory_and_raw_failures_remain(self):
        checks = {name: {'passed': False, 'measured': 123} for name in screen.NON_BLOCKING_METRICS}
        checks['read_p95'] = {'passed': True}
        rows = [{'checks': checks}]
        self.assertEqual(screen.delivery_gate(rows), 'PASS')
        for name in screen.NON_BLOCKING_METRICS:
            self.assertFalse(checks[name]['passed'])
            self.assertFalse(checks[name]['blocking'])
            self.assertEqual(checks[name]['measured'], 123)
        self.assertTrue(checks['read_p95']['blocking'])
        self.assertEqual(screen.DELIVERY_POLICY['effective_utc'], '2026-10-31T08:28:00Z')

    def test_every_other_check_remains_blocking_including_unknown_future_checks(self):
        for name in ('read_p95', 'read_p99', 'list_p95', 'list_p99', 'save_batch',
                     'flush_p95', 'close_p95', 'write_through_p95', 'capacity', 'future_gate'):
            with self.subTest(name=name):
                self.assertEqual(screen.delivery_gate([{'checks': {name: {'passed': False}}}]), 'FAIL')

    def test_missing_or_empty_evidence_cannot_pass(self):
        for rows in ([], [{'checks': {}}]):
            with self.subTest(rows=rows), self.assertRaises(AssertionError):
                screen.delivery_gate(rows)
