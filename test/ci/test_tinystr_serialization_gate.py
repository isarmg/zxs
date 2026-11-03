"""Keep the optional tinystr serialization gate independent of production features."""
if not __debug__:
    raise RuntimeError('Validation requires assertions; do not use Python optimization (-O/-OO or PYTHONOPTIMIZE).')

import importlib.util
import io
import json
from pathlib import Path
import subprocess
import tempfile
import tomllib
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location(
    'tinystr_serialization_gate', ROOT / '.github/scripts/check-serialization-family.py')
GATE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(GATE)


class TinystrSerializationGate(unittest.TestCase):
    def test_consumers_select_only_the_registered_optional_contract(self):
        registration = tomllib.loads((ROOT / 'src/foundation/text-unicode/tinystr/Cargo.toml').read_text())
        target = next(row for row in registration['test'] if row['name'] == 'serialization_contract')
        self.assertEqual(target['required-features'], ['serialization'])
        self.assertEqual(
            (ROOT / 'src/foundation/text-unicode/tinystr' / target['path']).resolve(),
            ROOT / 'test/foundation/tinystr/tests/serialization_contract.rs')
        observed = []

        def inspect(root, work, environment, target, **selection):
            manifest = tomllib.loads((work / 'Cargo.toml').read_text())
            self.assertEqual(set(manifest['dependencies']), {'tinystr'})
            dependency = manifest['dependencies']['tinystr']
            self.assertEqual(dependency['features'], ['serialization'])
            self.assertEqual(Path(dependency['path']), ROOT / 'src/foundation/text-unicode/tinystr')
            self.assertEqual(selection, {'tests': True, 'test_target': ('tinystr', 'serialization_contract')})
            self.assertEqual(target, GATE.NATIVE)
            observed.append(dependency['default-features'])

        with tempfile.TemporaryDirectory() as temporary, \
                mock.patch.object(GATE.subprocess, 'run') as cargo, \
                mock.patch.object(GATE, 'build', side_effect=inspect):
            GATE.check_tinystr_contracts(ROOT, Path(temporary), {'CARGO_BUILD_JOBS': '1'})
            self.assertEqual(observed, [True, False])
            self.assertEqual(cargo.call_count, 2)
            for call in cargo.call_args_list:
                self.assertEqual(call.args[0][:3], ['cargo', 'generate-lockfile', '--offline'])
                self.assertTrue(call.kwargs['check'])

    def results(self, *, filtered=0, status=0):
        names = sorted(GATE.TINYSTR_CONTRACT_TESTS)
        listing = ''.join(f'{name}: test\n' for name in names)
        executed = ''.join(f'test {name} ... ok\n' for name in names)
        executed += (f'test result: ok. {len(names)} passed; 0 failed; 0 ignored; '
                     f'0 measured; {filtered} filtered out\n')
        return [subprocess.CompletedProcess([], 0, listing, ''),
                subprocess.CompletedProcess([], status, executed, '')]

    def test_all_named_tests_must_run_without_filtering(self):
        with mock.patch.object(GATE.subprocess, 'run', side_effect=self.results()) as run, \
                mock.patch('sys.stdout', new=io.StringIO()):
            GATE.run_tinystr_contract(ROOT, {}, 'binary')
            self.assertEqual([call.args[0] for call in run.call_args_list],
                             [['binary', '--list'], ['binary']])

    def test_empty_listing_stops_before_execution(self):
        result = subprocess.CompletedProcess([], 0, '0 tests, 0 benchmarks\n', '')
        with mock.patch.object(GATE.subprocess, 'run', return_value=result) as run, \
                mock.patch('sys.stdout', new=io.StringIO()), self.assertRaises(AssertionError):
            GATE.run_tinystr_contract(ROOT, {}, 'binary')
        self.assertEqual(run.call_count, 1)

    def test_filtered_summary_is_rejected(self):
        with mock.patch.object(GATE.subprocess, 'run', side_effect=self.results(filtered=1)), \
                mock.patch('sys.stdout', new=io.StringIO()), self.assertRaises(AssertionError):
            GATE.run_tinystr_contract(ROOT, {}, 'binary')

    def test_failed_binary_is_not_reported_as_passed(self):
        with mock.patch.object(GATE.subprocess, 'run', side_effect=self.results(status=101)), \
                mock.patch('sys.stdout', new=io.StringIO()), \
                self.assertRaises(subprocess.CalledProcessError):
            GATE.run_tinystr_contract(ROOT, {}, 'binary')

    def test_build_selects_the_explicit_binary_not_the_consumer_library(self):
        class Diagnostics:
            def __init__(self, _):
                pass

            def consume(self, line, number):
                pass

            def result(self, status):
                return status

        events = [dict(reason='compiler-artifact', target={'name': name},
                       profile={'test': True}, executable=name)
                  for name in ('serialization_family_regression', 'serialization_contract')]
        process = mock.MagicMock()
        process.__enter__.return_value = process
        process.stdout = [json.dumps(event) for event in events]
        process.wait.return_value = 0
        with mock.patch.object(GATE.runpy, 'run_path', return_value={'CargoDiagnostics': Diagnostics}), \
                mock.patch.object(GATE.subprocess, 'Popen', return_value=process) as cargo, \
                mock.patch.object(GATE, 'run_tinystr_contract') as execute:
            GATE.build(ROOT, ROOT, {}, GATE.NATIVE, tests=True,
                       test_target=('tinystr', 'serialization_contract'))
            command = cargo.call_args.args[0]
            self.assertIn('--no-run', command)
            self.assertEqual(command[-4:], ['-p', 'tinystr', '--test', 'serialization_contract'])
            execute.assert_called_once_with(ROOT, {}, 'serialization_contract')


if __name__ == '__main__':
    unittest.main()
