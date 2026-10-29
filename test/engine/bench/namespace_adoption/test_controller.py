#!/usr/bin/env python3
"""Controller boundary regressions; product execution belongs to the GitHub jobs."""
import os
import json
from pathlib import Path
import tempfile
import unittest
from unittest import mock

import test_harness as fixtures
import observer_compare

ENVIRONMENT = fixtures.ENVIRONMENT
summarize = fixtures.summarize


class ControllerBoundaryTests(unittest.TestCase):
    def test_observer_validation_cannot_satisfy_final_accounting(self):
        value = fixtures.SummaryBoundaryTests().pair_summary(500, 3)
        value['validation_mode'] = 'observer-validation'
        value['artifact_identity']['mode'] = 'observer-validation'
        with mock.patch.dict(os.environ, ENVIRONMENT, clear=True):
            errors = summarize.audit_pair(value, 500, 3)
            self.assertTrue(any('validation_mode' in error for error in errors))
            self.assertEqual(summarize.audit_pair(value, 500, 3, expected_mode='observer-validation'), [])

    def test_completed_bpf_claim_requires_actual_admission_and_activation(self):
        value = fixtures.SummaryBoundaryTests().pair_summary(500, 3)
        value['trace_observer'] = 'seccomp-bpf'
        for run in value['runs']:
            for phase in run['phases']:
                phase['observer'].update(requested_mode='seccomp-bpf', effective_mode='seccomp-bpf')
        with mock.patch.dict(os.environ, ENVIRONMENT, clear=True):
            errors = summarize.audit_pair(value, 500, 3)
        self.assertTrue(any('real activation or same-job contract admission' in error for error in errors))

    def test_missing_plain_probe_keeps_full_product_pair_but_cannot_complete_validation(self):
        value = fixtures.SummaryBoundaryTests().pair_summary(500, 3)
        value['validation_mode'] = 'observer-validation'
        value['artifact_identity']['mode'] = 'observer-validation'
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            results = root / 'namespace-observer-plain-1/results'
            results.mkdir(parents=True)
            (results / 'summary.json').write_text(json.dumps(value))
            for run in value['runs']:
                directory = results / f'500-3-{run["implementation"]}'
                directory.mkdir()
                for phase in fixtures.runner.PHASES:
                    (directory / f'{phase}.strace').write_text('100.00 0.000001 1 1 fsync\n')
            with mock.patch.dict(os.environ, ENVIRONMENT, clear=True):
                entry = observer_compare.collect_pair(root, 'plain')
        self.assertEqual(entry['measurement_status'], 'completed_measurements')
        self.assertEqual(entry['status'], 'incomplete')
        self.assertEqual(entry['reason'], 'same_runner_observer_contract_unavailable')


if __name__ == '__main__':
    unittest.main()
