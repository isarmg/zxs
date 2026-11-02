"""The dedicated IO gate must execute its exact minimal-feature contracts."""
import json
import os
from pathlib import Path
import runpy
from types import SimpleNamespace
import tempfile
import tomllib
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
with mock.patch('subprocess.run', side_effect=AssertionError('real subprocess is forbidden')):
    IO = runpy.run_path(str(ROOT / '.github/scripts/futures_io_contracts.py'))
    PARENT = runpy.run_path(str(ROOT / '.github/scripts/check-futures-consolidation.py'))
    BEHAVIOUR = runpy.run_path(str(ROOT / '.github/scripts/check-foundation-behaviour.py'))
INVENTORY = 'test/ci/fixtures/foundation-futures-io-tests.json'


def completed(names, *, ignored=False, filtered=False, zero=False):
    if zero:
        return 'test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\n'
    outcomes = [(name, 'ok') for name in names]
    if ignored:
        outcomes[0] = (outcomes[0][0], 'ignored')
    if filtered:
        outcomes = outcomes[1:]
    passed = len(names) - int(ignored) - int(filtered)
    return (''.join(f'test {name} ... {status}\n' for name, status in outcomes)
            + f'test result: ok. {passed} passed; 0 failed; {int(ignored)} ignored; '
            + f'0 measured; {int(filtered)} filtered out\n')


class FakeLog:
    def __init__(self, harness, path, header):
        self.harness, self.path, self.closed = harness, path, False
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(header)
        harness.logs.append(self)

    def run(self, command, cwd):
        output = self.harness.respond(command, cwd)
        with self.path.open('a') as handle:
            handle.write('\n$ ' + ' '.join(command) + '\n' + output)
        return output

    def close(self):
        self.closed = True


class Harness:
    def __init__(self, case, root):
        self.case, self.root = case, root
        self.expected = json.loads((ROOT / INVENTORY).read_text())
        self.commands, self.logs = [], []
        self.listing, self.execution, self.features = {}, {}, {}
        self.change_after_execution = None
        root.mkdir(parents=True, exist_ok=True)
        (root / 'Cargo.lock').write_bytes(b'root lock must remain byte-identical\n')
        for relative in IO['IDENTITY']:
            path = root / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(b'identity fixture: ' + relative.encode() + b'\n')
        (root / INVENTORY).write_text(json.dumps(self.expected))

    def respond(self, command, cwd):
        self.case.assertEqual(cwd, self.root)
        self.case.assertEqual(command[0], 'cargo')
        manifest = Path(command[command.index('--manifest-path') + 1])
        data = tomllib.loads(manifest.read_text())
        key = data['package']['name'].removeprefix('futures-io-')
        self.case.assertIn(key, self.expected['suites'])
        self.commands.append((key, list(command), manifest, data))
        record = self.expected['suites'][key]
        if 'generate-lockfile' in command:
            manifest.with_name('Cargo.lock').write_bytes(b'temporary offline lock\n')
            return ''
        if 'metadata' in command:
            selected = self.features.get(key, record['resolved_features'])
            return json.dumps({
                'packages': [{'id': 'subject', 'manifest_path': str(
                    self.root / 'src/foundation/concurrency/futures/Cargo.toml')}],
                'resolve': {'nodes': [{'id': 'subject', 'features': selected}]},
            })
        self.case.assertIn('test', command)
        if '--list' in command:
            return ''.join(f'{name}: test\n' for name in self.listing.get(key, record['tests']))
        output = self.execution.get(key, completed(record['tests']))
        if key == 'write-zero' and self.change_after_execution:
            path = self.root / self.change_after_execution
            path.write_bytes(path.read_bytes() + b'changed during execution\n')
        return output

    def run(self):
        def load_common(path):
            self.case.assertEqual(Path(path), self.root / '.github/scripts/check-foundation-behaviour.py')
            return {
                'Log': lambda path, header: FakeLog(self, path, header),
                'source_state': lambda: ('fixture-commit', False, 'rustc fixture'),
                'listed': BEHAVIOUR['listed'],
                'check_run': BEHAVIOUR['check_run'],
            }

        check = IO['check']
        with mock.patch.dict(check.__globals__, {'runpy': SimpleNamespace(run_path=load_common), 'check_rejections': mock.Mock(return_value=[])}), \
                mock.patch('subprocess.run', side_effect=AssertionError('real subprocess is forbidden')), \
                mock.patch('builtins.print'):
            check(self.root, self.root / 'consumers', self.root / 'shared-target')

    def summaries(self):
        return list((self.root / 'target/foundation-behaviour/logs/futures-io').glob('run-*/summary.json'))


class FuturesIoContractsTests(unittest.TestCase):
    def test_four_locked_offline_consumers_preserve_features_fixtures_and_root_lock(self):
        with tempfile.TemporaryDirectory() as temporary:
            harness = Harness(self, Path(temporary))
            lock_before = (harness.root / 'Cargo.lock').read_bytes()
            harness.run()
            self.assertEqual((harness.root / 'Cargo.lock').read_bytes(), lock_before)
            self.assertEqual(set(key for key, *_ in harness.commands), set(harness.expected['suites']))
            manifests = {manifest for _, _, manifest, _ in harness.commands}
            self.assertEqual(len(manifests), 4)
            for key, command, manifest, data in harness.commands:
                self.assertTrue(manifest.is_relative_to(harness.root / 'consumers'))
                self.assertIn('--offline', command)
                self.assertEqual(data['workspace'], {'resolver': '2'})
                dependency = data['dependencies']['futures']
                self.assertFalse(dependency['default-features'])
                self.assertEqual(dependency['features'], harness.expected['suites'][key]['features'])
                self.assertEqual(Path(dependency['path']), harness.root / 'src/foundation/concurrency/futures')
                self.assertEqual(data['test'], [{'name': 'contracts', 'path': str(
                    harness.root / harness.expected['suites'][key]['fixture'])}])
                if 'generate-lockfile' not in command:
                    self.assertIn('--locked', command)
                if 'test' in command:
                    self.assertEqual(command[command.index('--target') + 1], IO['TARGET'])
                    self.assertEqual(command[command.index('--target-dir') + 1], str(harness.root / 'shared-target'))
                    self.assertEqual(command[command.index('--test') + 1], 'contracts')
                    self.assertNotIn('--all-features', command)
            for key in harness.expected['suites']:
                commands = [command for owner, command, *_ in harness.commands if owner == key]
                self.assertEqual(sum('generate-lockfile' in command for command in commands), 1)
                listings = [command for command in commands if '--list' in command]
                executions = [command for command in commands if 'test' in command and '--list' not in command]
                self.assertEqual(len(listings), 1)
                self.assertEqual(len(executions), 1)
                self.assertEqual(listings[0][:-2], executions[0])
            self.assertTrue(all(log.closed for log in harness.logs))
            summaries = harness.summaries()
            self.assertEqual(len(summaries), 1)
            summary = json.loads(summaries[0].read_text())
            self.assertEqual(summary['suites'], harness.expected['suites'])
            self.assertTrue({
                '.github/scripts/futures_io_contracts.py',
                '.github/scripts/check-futures-consolidation.py',
                INVENTORY,
            } <= set(summary['source_sha256']))

    def test_exact_listing_rejects_empty_and_same_count_renamed_tests(self):
        first = next(iter(IO['SUITES']))
        names = json.loads((ROOT / INVENTORY).read_text())['suites'][first]['tests']
        for listing in ([], [*names[:-1], 'unexpected_same_count_test']):
            with self.subTest(listing=listing), tempfile.TemporaryDirectory() as temporary:
                harness = Harness(self, Path(temporary))
                harness.listing[first] = listing
                with self.assertRaises(AssertionError):
                    harness.run()
                self.assertFalse(harness.summaries())
                self.assertFalse(any('test' in command and '--list' not in command
                                     for _, command, *_ in harness.commands))

    def test_actual_metadata_rejects_broadened_and_missing_capabilities(self):
        expected = json.loads((ROOT / INVENTORY).read_text())['suites']
        cases = (
            ('seek-relative', sorted([*expected['seek-relative']['resolved_features'], 'sink'])),
            ('write-zero', [feature for feature in expected['write-zero']['resolved_features'] if feature != 'sink']),
        )
        for key, selected in cases:
            with self.subTest(suite=key), tempfile.TemporaryDirectory() as temporary:
                harness = Harness(self, Path(temporary))
                harness.features[key] = selected
                with self.assertRaises(AssertionError):
                    harness.run()
                self.assertFalse(harness.summaries())

    def test_ignored_filtered_or_zero_execution_cannot_produce_success_summary(self):
        first = next(iter(IO['SUITES']))
        names = json.loads((ROOT / INVENTORY).read_text())['suites'][first]['tests']
        for status in ('ignored', 'filtered', 'zero'):
            with self.subTest(status=status), tempfile.TemporaryDirectory() as temporary:
                harness = Harness(self, Path(temporary))
                harness.execution[first] = completed(names, **{status: True})
                with self.assertRaises(AssertionError):
                    harness.run()
                self.assertFalse(harness.summaries())
                self.assertTrue(all(log.closed for log in harness.logs))

    def test_source_gate_or_inventory_changes_invalidate_the_completed_run(self):
        for relative in (
            'src/foundation/concurrency/futures/src/io/into_sink.rs',
            '.github/scripts/futures_io_contracts.py',
            INVENTORY,
        ):
            with self.subTest(path=relative), tempfile.TemporaryDirectory() as temporary:
                harness = Harness(self, Path(temporary))
                harness.change_after_execution = relative
                with self.assertRaisesRegex(AssertionError, 'changed during validation'):
                    harness.run()
                self.assertFalse(harness.summaries())
                self.assertEqual(sum('test' in command and '--list' not in command
                                     for _, command, *_ in harness.commands), 4)

    def test_compile_rejections_require_expected_code_and_primary_fixture_span(self):
        check = IO['check_rejections']
        for fault in (None, 'accepted', 'wrong-code', 'wrong-path', 'secondary-only'):
            with self.subTest(fault=fault), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                manifest = root / 'Cargo.toml'
                manifest.write_text('[package]\nname="fixture"\n')
                commands = []
                def run(command, **kwargs):
                    commands.append(command)
                    name = command[command.index('--bin') + 1]
                    _, filename, code = next(row for row in IO['REJECTIONS'] if row[0] == name)
                    fixture = root / 'test/ci/fixtures' / filename
                    message = {'reason': 'compiler-message', 'message': {
                        'level': 'error', 'code': {'code': 'E0000' if fault == 'wrong-code' else code},
                        'spans': [{'file_name': str(root / 'other.rs' if fault == 'wrong-path' else fixture),
                                   'is_primary': fault != 'secondary-only'}],
                    }}
                    return SimpleNamespace(returncode=0 if fault == 'accepted' else 101,
                                           stdout=json.dumps(message) + '\n', stderr='')
                with mock.patch.dict(check.__globals__, {'subprocess': SimpleNamespace(run=run)}):
                    if fault:
                        with self.assertRaises(AssertionError):
                            check(root, manifest, root / 'target', root)
                    else:
                        records = check(root, manifest, root / 'target', root)
                        self.assertEqual([row['code'] for row in records], ['E0277', 'E0597'])
                        self.assertEqual(len(commands), 2)
                        for command in commands:
                            self.assertIn('--locked', command)
                            self.assertIn('--offline', command)
                            self.assertIn('--message-format=json', command)

    def test_existing_parent_calls_io_helper_once_with_the_shared_target(self):
        check = PARENT['check']
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            target = root / 'shared-target'
            observed = []

            def child(*args):
                observed.append(args)
                self.assertTrue(args[1].is_relative_to(root / 'target/scope-consumers'))

            def load_child(path):
                self.assertEqual(Path(path), root / '.github/scripts/futures_io_contracts.py')
                return {'check': child}

            overrides = {
                'check_native_consumer_features': mock.Mock(),
                'check_features': mock.Mock(),
                'check_retained_tests': mock.Mock(),
                'foundation_path': lambda selected_root, name: selected_root / 'absent' / name,
                'run': mock.Mock(),
                'runpy': SimpleNamespace(run_path=load_child),
            }
            with mock.patch.dict(check.__globals__, overrides), \
                    mock.patch.dict(os.environ, {'CARGO_TARGET_DIR': str(target)}), \
                    mock.patch('subprocess.run', side_effect=AssertionError('real subprocess is forbidden')):
                check(root)
            self.assertEqual(len(observed), 1)
            self.assertEqual(observed[0][0], root)
            self.assertEqual(observed[0][2], target)
            overrides['check_features'].assert_called_once()
            overrides['check_retained_tests'].assert_called_once()
            self.assertNotIn('futures_io_contracts.py', (ROOT / '.github/scripts/check-foundation.sh').read_text())


if __name__ == '__main__':
    unittest.main()
