"""The MIME build gate must preserve real source and reject incomplete evidence."""
import json
from pathlib import Path
import runpy
import subprocess
import tempfile
import tomllib
from types import SimpleNamespace
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
GATE = runpy.run_path(str(ROOT / '.github/scripts/check-mime-build.py'))
COMMON = runpy.run_path(str(ROOT / '.github/scripts/check-foundation-behaviour.py'))


class MimeBuildGateTests(unittest.TestCase):
    def seed(self, root):
        for relative in GATE['IDENTITY']:
            path = root / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text('public fixture: ' + relative + '\n')
        (root / 'Cargo.lock').write_text('root lock sentinel\n')

    def exercise(self, root, *, listing=None, execution=None, features=None, mutate=False):
        self.seed(root)
        calls = []
        names = sorted(GATE['TESTS'])
        expected = {'default': ['default', 'rev-mappings'],
                    'explicit': ['rev-mappings'], 'forward-only': []}

        class Log:
            def __init__(self, path, header):
                path.parent.mkdir(parents=True, exist_ok=True)
                self.handle = path.open('w')
                self.handle.write(header)

            def run(inner, command, cwd):
                self.assertEqual(cwd, root)
                manifest = Path(command[command.index('--manifest-path') + 1])
                data = tomllib.loads(manifest.read_text())
                calls.append((command, data))
                inner.handle.write(' '.join(command) + '\n')
                if 'generate-lockfile' in command:
                    return ''
                if 'metadata' in command:
                    mode = data['package']['name'].removeprefix('mime-build-')
                    return json.dumps({'packages': [{'id': 'subject', 'manifest_path': str(
                        root / GATE['COMPONENT'] / 'Cargo.toml')}],
                        'resolve': {'nodes': [{'id': 'subject', 'features':
                            features if features is not None else expected[mode]}]}})
                if 'test' in command:
                    if '--list' in command:
                        return listing if listing is not None else ''.join(f'{name}: test\n' for name in names)
                    return execution if execution is not None else (
                        ''.join(f'test {name} ... ok\n' for name in names)
                        + 'test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\n')
                self.assertIn('check', command)
                if mutate and data['package']['name'].endswith('forward-only'):
                    (root / GATE['COMPONENT'] / 'build.rs').write_text('changed during validation\n')
                return ''

            def close(inner):
                inner.handle.close()

        check = GATE['check']
        common = {'Log': Log, 'listed': COMMON['listed'], 'check_run': COMMON['check_run'],
                  'source_state': lambda: ('fixture-commit', False, 'rustc fixture')}
        with mock.patch.dict(check.__globals__, runpy=SimpleNamespace(run_path=lambda _: common)), \
                mock.patch.object(subprocess, 'run', side_effect=AssertionError('real subprocess forbidden')), \
                mock.patch('builtins.print'):
            check(root)
        return calls

    def summaries(self, root):
        return list((root / 'target/foundation-behaviour/logs/mime-build').glob('run-*/summary.json'))

    def test_private_subject_preserves_exact_source_prefix_and_relative_data(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            self.seed(root)
            subject = root / 'private'
            GATE['private_subject'](root, subject)
            source = (root / GATE['COMPONENT'] / 'build.rs').read_bytes()
            fixture = (root / GATE['FIXTURE']).read_bytes()
            self.assertEqual((subject / 'subject.rs').read_bytes(), source + b'\n' + fixture)
            self.assertEqual((subject / 'src/mime_types.rs').read_bytes(),
                             (root / GATE['COMPONENT'] / 'src/mime_types.rs').read_bytes())
            manifest = tomllib.loads((subject / 'Cargo.toml').read_text())
            self.assertFalse(manifest['package']['build'])
            self.assertEqual(manifest['lib']['path'], 'subject.rs')
            self.assertEqual(manifest['features'], {'default': [], 'rev-mappings': []})

    def test_exact_runtime_and_feature_builds_keep_locks_and_identity(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            calls = self.exercise(root)
            self.assertEqual(len(calls), 12)
            self.assertEqual((root / 'Cargo.lock').read_text(), 'root lock sentinel\n')
            for command, _ in calls:
                self.assertIn('--offline', command)
                if 'generate-lockfile' not in command:
                    self.assertIn('--locked', command)
            modes = {data['package']['name']: data['dependencies']['mime_guess']
                     for command, data in calls if 'check' in command}
            self.assertEqual(len(modes), 3)
            self.assertTrue(modes['mime-build-default']['default-features'])
            self.assertEqual(modes['mime-build-explicit']['features'], ['rev-mappings'])
            self.assertFalse(modes['mime-build-forward-only']['default-features'])
            summary = json.loads(self.summaries(root)[0].read_text())
            self.assertEqual(summary['tests'], sorted(GATE['TESTS']))
            self.assertEqual(summary['feature_builds'], {
                'default': ['default', 'rev-mappings'],
                'explicit': ['rev-mappings'], 'forward-only': []})

    def test_empty_or_wrong_named_runtime_cannot_write_success(self):
        for listing in ('0 tests, 0 benchmarks\n', 'wrong: test\n' * 5):
            with self.subTest(listing=listing), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                with self.assertRaises(AssertionError):
                    self.exercise(root, listing=listing)
                self.assertFalse(self.summaries(root))

    def test_ignored_or_filtered_or_zero_output_cannot_write_success(self):
        for summary in ('0 passed; 0 failed; 5 ignored; 0 measured; 0 filtered out',
                        '0 passed; 0 failed; 0 ignored; 0 measured; 5 filtered out',
                        '0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out'):
            with self.subTest(summary=summary), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                with self.assertRaises(AssertionError):
                    self.exercise(root, execution='test result: ok. ' + summary + '\n')
                self.assertFalse(self.summaries(root))

    def test_wrong_resolved_features_and_source_mutation_are_rejected(self):
        for options in ({'features': []}, {'mutate': True}):
            with self.subTest(options=options), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                with self.assertRaises(AssertionError):
                    self.exercise(root, **options)
                self.assertFalse(self.summaries(root))

    def test_ci_runs_once_and_keeps_explicit_command_inventory(self):
        shell = (ROOT / '.github/scripts/check-foundation.sh').read_text()
        self.assertEqual(shell.count('python3 .github/scripts/check-mime-build.py'), 1)
        commands = json.loads((ROOT / 'test/ci/foundation-commands.json').read_text())['foundation-async']
        self.assertEqual(sum(item['args'] == ['.github/scripts/check-mime-build.py']
                             for item in commands), 1)


if __name__ == '__main__':
    unittest.main()
