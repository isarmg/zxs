"""Keep parallel CI scheduling separate from accepting its generated assets."""
from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parents[2]


def dependency_errors(source):
    """Check our block-style jobs; GitHub remains the full workflow parser."""
    parts = re.split(r'^  ([A-Za-z0-9_-]+):\s*$',
                     source.split('\njobs:\n', 1)[1], flags=re.MULTILINE)
    jobs = dict(zip(parts[1::2], parts[2::2]))
    errors = []
    expected = {
        'static': '',
        'tests': 'static',
        'web-production': '',
        'browser-smoke': '[static, web-production, smb-sdk]',
        'smb-sdk': 'static',
    }
    for name, dependencies in expected.items():
        if name not in jobs:
            errors.append(f'missing job: {name}')
            continue
        body = jobs[name]
        actual = re.findall(r'^    needs: ([^\n]+)$', body, flags=re.MULTILINE)
        if actual != ([dependencies] if dependencies else []):
            errors.append(f'wrong dependencies: {name}')
        if re.search(r'^    if:', body, flags=re.MULTILINE):
            errors.append(f'conditional job: {name}')
        if re.search(r'^\s+continue-on-error:', body, flags=re.MULTILINE):
            errors.append(f'ignored failure: {name}')
    for name, command in (
        ('static', 'bash .github/scripts/check.sh static'),
        ('web-production', 'bash .github/scripts/check.sh web-production'),
    ):
        body = jobs.get(name, '')
        if command not in body or re.search(r'^        if:', body, flags=re.MULTILINE):
            errors.append(f'optional required check: {name}')
    return errors


class ParallelCiWorkflow(unittest.TestCase):
    def setUp(self):
        self.source = (ROOT / '.github/workflows/ci.yml').read_text()

    def test_wasm_starts_early_but_acceptance_keeps_all_dependencies(self):
        self.assertEqual(dependency_errors(self.source), [])
        self.assertIn('          path: artifacts/browser-smoke/placement/\n', self.source)

    def test_browser_cannot_drop_any_predecessor(self):
        dependencies = ['static', 'web-production', 'smb-sdk']
        for missing in dependencies:
            with self.subTest(missing=missing):
                reduced = ', '.join(name for name in dependencies if name != missing)
                changed = self.source.replace(
                    'needs: [static, web-production, smb-sdk]', f'needs: [{reduced}]')
                self.assertIn('wrong dependencies: browser-smoke', dependency_errors(changed))

    def test_failure_or_skip_cannot_be_overridden_by_a_job_condition(self):
        for name in ['static', 'tests', 'web-production', 'browser-smoke', 'smb-sdk']:
            for condition in ['always()', '!cancelled()', 'true', 'false']:
                with self.subTest(job=name, condition=condition):
                    changed = self.source.replace(f'  {name}:\n',
                                                  f'  {name}:\n    if: {condition}\n')
                    self.assertIn(f'conditional job: {name}', dependency_errors(changed))
            changed = self.source.replace(f'  {name}:\n',
                                          f'  {name}:\n    continue-on-error: true\n')
            self.assertIn(f'ignored failure: {name}', dependency_errors(changed))

    def test_source_and_build_checks_cannot_become_optional(self):
        for suite, name in [('static', 'static'), ('web-production', 'web-production')]:
            command = f'        run: bash .github/scripts/check.sh {suite}'
            changed = self.source.replace(command, f'        if: false\n{command}')
            self.assertIn(f'optional required check: {name}', dependency_errors(changed))


if __name__ == '__main__':
    unittest.main()
