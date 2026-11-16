"""Keep verification of the named-disk tool consumers separate from runtime claims."""
import importlib.util
import json
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]
POLICY = ROOT / '.github/ci/targeted-tests.json'
SPEC = importlib.util.spec_from_file_location(
    'targeted_ci', ROOT / '.github/scripts/check-targeted-ci.py',
)
CI = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CI)


class NamedDiskVerificationTools(unittest.TestCase):
    def test_named_consumers_admit_only_their_explicit_compile_targets(self):
        group = 'named-disk-verification-tools'
        runner = 'test/tools/browser-smoke/runner'
        files = [
            '.github/architecture-policy.json',
            f'{runner}/foreign_review/fixture.rs',
            f'{runner}/management.rs',
            f'{runner}/placement_configuration/fixture.rs',
            'tools/browser-smoke/Cargo.toml',
            'tools/perf-check/Cargo.toml',
            'test/ci/test_named_disk_verification_tools.py',
        ]
        policy = json.loads(POLICY.read_text())
        for path in files:
            with self.subTest(path=path):
                plan = CI.select_plan(policy, [path])
                self.assertEqual(plan['groups'], [group])
                self.assertEqual(plan['python_tests'], [
                    'test_named_disk_verification_tools',
                ])
                self.assertCountEqual(
                    [(target['package'], target['name'], target['kind'], target['mode'])
                     for target in plan['rust_targets']],
                    [
                        ('browser-smoke', 'browser-smoke', 'bin', 'check'),
                        ('perf-check', 'perf-check', 'bin', 'check'),
                        ('perf-check', 'recovery-startup', 'bin', 'check'),
                    ],
                )
