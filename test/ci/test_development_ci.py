"""Exercise actual full-matrix output and preserve main-push and manual, non-publishing CI.

These are policy and retained-recipe regressions, not Rust/browser/SDK execution.
"""
import importlib.util
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / '.github/scripts/development-ci.py'
SPEC = importlib.util.spec_from_file_location('development_ci', SCRIPT)
POLICY = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(POLICY)
EXPECTED = ['rust-sources', 'quality', 'security', 'workspace',
            'foundation-platform', 'foundation-crypto', 'foundation-data',
            'foundation-language', 'foundation-async', 'web', 'contracts',
            'build-tools', 'smb-protocol']


def jobs(source):
    parts = re.split(r'^  ([a-z][a-z0-9-]*):$', source.split('\njobs:\n', 1)[1],
                     flags=re.MULTILINE)
    return dict(zip(parts[1::2], parts[2::2]))


class DevelopmentCiPolicy(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix='development-ci-')
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        for folder in ('.github/workflows', '.github/ci/full-workflows'):
            (self.root / folder).mkdir(parents=True)
            for name in POLICY.SNAPSHOT_SHA256:
                (self.root / folder / name).write_bytes((ROOT / folder / name).read_bytes())

    def reject_edit(self, name, old, new):
        path = self.root / '.github/workflows' / name
        original = path.read_text()
        self.assertIn(old, original)
        path.write_text(original.replace(old, new, 1))
        try:
            with self.assertRaisesRegex(ValueError, 'development policy'):
                POLICY.check_workflows(self.root)
        finally:
            path.write_text(original)

    def test_current_workflows_match_reviewed_projection(self):
        POLICY.check_workflows(ROOT)
        POLICY.check_workflows(self.root)

    def test_main_push_ci_preserves_other_paused_entrypoints(self):
        for name, triggers in (('ci.yml', ['push', 'workflow_dispatch']),
                               ('release.yml', ['workflow_dispatch']),
                               ('smb-sdk.yml', ['workflow_call'])):
            source = (ROOT / '.github/workflows' / name).read_text()
            events = source.split('\non:\n', 1)[1].split('\npermissions:', 1)[0]
            self.assertEqual(re.findall(r'^  ([a-z_]+):', events, flags=re.MULTILINE),
                             triggers)
            crons = re.findall(r"^    - cron: '([^']+)'$", events, flags=re.MULTILINE)
            self.assertEqual(crons, [])
        receipt = POLICY.receipt()
        self.assertTrue(receipt['automatic_triggers'])
        self.assertEqual(receipt['push_branches'], ['main'])
        self.assertIn('  push:\n    branches: [main]\n', (ROOT / '.github/workflows/ci.yml').read_text())
        self.assertNotIn('scheduled_workflow', receipt)
        self.assertNotIn('schedule', receipt)

    def test_unreviewed_automatic_triggers_are_rejected(self):
        for name in ('ci.yml', 'release.yml', 'smb-sdk.yml'):
            for event in ('push', 'pull_request', 'schedule', 'workflow_run',
                          'repository_dispatch'):
                with self.subTest(workflow=name, event=event):
                    self.reject_edit(name, 'on:\n', f'on:\n  {event}:\n')

    def test_push_scope_cannot_expand_or_shrink(self):
        for changed in ('[main, development]', '[development]', '["**"]'):
            self.reject_edit('ci.yml', '    branches: [main]\n', f'    branches: {changed}\n')
        self.reject_edit('ci.yml', '    branches: [main]\n', '')
        self.reject_edit('ci.yml', '    branches: [main]\n', '    branches: [main]\n    tags: ["v*"]\n')

    def test_schedule_cannot_return_or_cancel_running_checks(self):
        for cron in ('17 * * * *', '* * * * *', '17 */2 * * *', '17 0 * * *'):
            self.reject_edit('ci.yml', 'on:\n', f"on:\n  schedule:\n    - cron: '{cron}'\n")
        self.reject_edit('ci.yml', '  cancel-in-progress: false\n',
                         '  cancel-in-progress: true\n')

    def test_actual_workflow_command_emits_exact_thirteen_with_no_reuse(self):
        source = (ROOT / '.github/workflows/ci.yml').read_text()
        static = jobs(source)['static']
        self.assertIn('suites: ${{ steps.suites.outputs.suites }}', static)
        self.assertIn('id: suites\n        run: python3 .github/scripts/development-ci.py select', static)
        command = [sys.executable, str(SCRIPT), 'select']
        # Inputs/environment/event data cannot shrink or reuse the complete matrix.
        for event in ('schedule', 'push', 'pull_request', 'workflow_dispatch', 'workflow_call', ''):
            with self.subTest(event=event):
                output = self.root / f'{event or "missing"}.out'
                summary = self.root / f'{event or "missing"}.summary'
                event_path = self.root / 'event.json'
                event_path.write_text(json.dumps({'schedule': '17 * * * *',
                                                  'inputs': {'full_ci': True, 'suite': 'workspace'}}))
                env = dict(os.environ, GITHUB_OUTPUT=str(output), GITHUB_STEP_SUMMARY=str(summary),
                           GITHUB_EVENT_NAME=event, GITHUB_EVENT_PATH=str(event_path),
                           INPUT_FULL_CI='true', ZXS_FULL_CI='1', CI_SUITES=json.dumps(['web']))
                result = subprocess.run(command, cwd=ROOT, env=env, text=True,
                                        capture_output=True, check=True, timeout=10)
                actual = json.loads(result.stdout)
                self.assertEqual(actual['suites'], EXPECTED)
                self.assertEqual(actual['paused_suites'], [])
                self.assertEqual(actual['mode'], 'full-ci')
                self.assertEqual(actual['reused'], [])
                self.assertNotIn('evidence', actual)
                self.assertEqual(output.read_text(), 'suites=' + json.dumps(EXPECTED, separators=(',', ':')) + '\n')
                self.assertIn('selected, not yet passed; Release paused', summary.read_text())
                POLICY.check_receipt(actual)

    def test_unrecognized_full_mode_fails_without_any_matrix_output(self):
        output = self.root / 'forbidden.out'
        env = dict(os.environ, GITHUB_OUTPUT=str(output))
        for args in (['full'], ['select', '--full'], ['select', '--suite', 'workspace']):
            with self.subTest(args=args):
                result = subprocess.run([sys.executable, str(SCRIPT), *args], cwd=ROOT,
                                        env=env, capture_output=True, timeout=10)
                self.assertNotEqual(result.returncode, 0)
                self.assertFalse(output.exists())

    def test_no_empty_partial_duplicate_or_extra_matrix_can_be_emitted(self):
        for suites in ([], EXPECTED[:-1], EXPECTED + ['unknown'], EXPECTED + [EXPECTED[0]], ['web']):
            changed = POLICY.receipt()
            changed['suites'] = suites
            with self.subTest(suites=suites), self.assertRaises(ValueError):
                POLICY.emit(changed, output=self.root / 'bad.out')
            self.assertFalse((self.root / 'bad.out').exists())
        changed = POLICY.receipt()
        changed['reused'] = ['workspace']
        with self.assertRaises(ValueError):
            POLICY.check_receipt(changed)

    def test_all_ci_jobs_run_with_original_full_budgets(self):
        ci = jobs((ROOT / '.github/workflows/ci.yml').read_text())
        self.assertEqual(set(ci), {'static', 'tests', 'web-production', 'browser-smoke',
                                   'smb-sdk', 'phase-one-core'})
        for name, body in ci.items():
            self.assertNotRegex(body, r'(?m)^    (if|continue-on-error):')
        self.assertIn('    timeout-minutes: 10\n', ci['static'])
        self.assertIn("    timeout-minutes: ${{ matrix.suite == 'workspace' && 120 || "
                      "matrix.suite == 'foundation-crypto' && 60 || 30 }}\n", ci['tests'])
        self.assertIn('    timeout-minutes: 45\n', ci['web-production'])
        self.assertIn('    timeout-minutes: 60\n', ci['browser-smoke'])
        self.assertIn('    timeout-minutes: 60\n', ci['phase-one-core'])
        self.assertNotRegex(ci['static'], r'(?m)^    needs:')
        self.assertEqual(re.findall(r'^    needs: (.+)$', ci['tests'], flags=re.MULTILINE), ['static'])
        self.assertIn('suite: ${{ fromJSON(needs.static.outputs.suites) }}', ci['tests'])
        self.assertIn('fail-fast: false', ci['tests'])
        self.assertNotIn('max-parallel:', ci['tests'])
        self.assertIn('    needs: [static, web-production, smb-sdk]\n', ci['browser-smoke'])
        self.assertIn('    needs: static\n', ci['smb-sdk'])
        self.assertIn('    needs: static\n', ci['phase-one-core'])
        self.assertEqual(POLICY.PAUSED_JOBS['ci.yml'], ())
        self.assertEqual(POLICY.PAUSED_JOBS['smb-sdk.yml'], ())
        sdk = jobs((ROOT / '.github/workflows/smb-sdk.yml').read_text())
        self.assertEqual(set(sdk), {'smb-sdk'})
        self.assertNotRegex(sdk['smb-sdk'], r'(?m)^    (if|continue-on-error):')
        release = jobs((ROOT / '.github/workflows/release.yml').read_text())
        self.assertEqual(set(release) - set(POLICY.PAUSED_JOBS['release.yml']), {'static'})
        self.assertIn('    timeout-minutes: 5\n', release['static'])

    def test_full_ci_jobs_cannot_be_silently_paused(self):
        for name in ('ci.yml', 'smb-sdk.yml'):
            for job in jobs((ROOT / '.github/workflows' / name).read_text()):
                for condition in ('false', 'inputs.full_ci', 'env.ZXS_FULL_CI'):
                    with self.subTest(workflow=name, job=job, condition=condition):
                        self.reject_edit(name, f'  {job}:\n',
                                         f'  {job}:\n    if: ${{{{ {condition} }}}}\n')

    def test_each_pause_is_constant_and_cannot_be_removed_or_enabled(self):
        for name, paused in POLICY.PAUSED_JOBS.items():
            for job in paused:
                condition = ('    if: ${{ false && (' + POLICY.PUBLISH_CONDITION + ') }}\n'
                             if job == 'publish' else POLICY.PAUSE)
                old = f'  {job}:\n' + condition
                for replacement in ('', '    if: ${{ true }}\n',
                                    '    if: ${{ inputs.full_ci }}\n',
                                    '    if: ${{ env.ZXS_FULL_CI }}\n',
                                    '    if: ${{ always() }}\n'):
                    with self.subTest(workflow=name, job=job, replacement=replacement):
                        self.reject_edit(name, old, f'  {job}:\n' + replacement)

    def test_budget_failure_and_nonempty_test_command_cannot_be_weakened(self):
        for old, new in (
            ('    timeout-minutes: 10\n', '    timeout-minutes: 5\n'),
            ("matrix.suite == 'workspace' && 120", "matrix.suite == 'workspace' && 10"),
            ('  tests:\n', '  tests:\n    continue-on-error: true\n'),
            ('  static:\n', '  static:\n    if: false\n'),
            ('  static:\n', "  static:\n    if: github.event_name == 'workflow_dispatch'\n"),
            ('  tests:\n', "  tests:\n    if: github.event_name == 'workflow_dispatch'\n"),
            ('      fail-fast: false\n', '      fail-fast: false\n      max-parallel: 1\n'),
            ('        run: bash .github/scripts/check.sh "${{ matrix.suite }}"', '        run: true'),
            ('        run: bash .github/scripts/check.sh "${{ matrix.suite }}"',
             '        run: bash .github/scripts/check.sh "${{ matrix.suite }}" || true'),
            ('suite: ${{ fromJSON(needs.static.outputs.suites) }}', 'suite: [workspace]'),
            ('development-ci.py select', 'select-ci-suites.py'),
        ):
            with self.subTest(old=old, new=new):
                self.reject_edit('ci.yml', old, new)

    def test_release_dispatch_and_extra_entrypoints_cannot_expand_scope(self):
        for name in POLICY.SNAPSHOT_SHA256:
            self.reject_edit(name, 'jobs:\n', 'jobs:\n  extra:\n    runs-on: ubuntu-24.04\n'
                             '    steps:\n      - run: bash .github/scripts/check.sh workspace\n')
        self.reject_edit('release.yml', '  workflow_dispatch:', '  workflow_dispatch:\n    inputs:\n      full_ci:')
        self.reject_edit('smb-sdk.yml', '  workflow_call:', '  workflow_dispatch:\n  workflow_call:')
        (self.root / '.github/workflows/extra.yaml').write_text('jobs: {}\n')
        with self.assertRaisesRegex(ValueError, 'entrypoints'):
            POLICY.check_workflows(self.root)

    def test_original_acceptance_dependencies_and_artifacts_are_not_removed(self):
        for name, old in (
            ('ci.yml', '    needs: [static, web-production, smb-sdk]\n'),
            ('release.yml', '    needs: [version, verify, linux-web, smb-sdk]\n'),
            ('release.yml', '    needs: [version, assemble]\n'),
            ('ci.yml', '          name: debian13-vfs-${{ github.sha }}\n'),
            ('ci.yml', '          if-no-files-found: error\n'),
            ('smb-sdk.yml', '        run: bash .github/scripts/debian13-run.sh bash .github/scripts/smb-sdk.sh\n'),
        ):
            with self.subTest(workflow=name, old=old):
                self.reject_edit(name, old, '')

    def test_full_recipe_cannot_be_silently_rewritten_to_match_active_policy(self):
        path = self.root / '.github/ci/full-workflows/ci.yml'
        path.write_text(path.read_text().replace('    needs: [static, web-production, smb-sdk]',
                                                  '    needs: static'))
        with self.assertRaisesRegex(ValueError, 'retained full recipe changed'):
            POLICY.check_workflows(self.root)

    def test_static_entrypoint_checks_policy_before_other_regressions(self):
        source = (ROOT / '.github/scripts/check.sh').read_text()
        static = source.split('  static)\n', 1)[1].split('    ;;', 1)[0]
        command = 'python3 .github/scripts/development-ci.py check'
        self.assertEqual(static.count(command), 1)
        self.assertLess(static.index(command), static.index('python3 -m unittest discover -s test/ci'))
        self.assertNotIn('|| true', static)
        self.assertNotIn('set +e', static)


if __name__ == '__main__':
    unittest.main()
