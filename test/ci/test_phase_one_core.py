"""Exercise the CI driver with current-JSON fixtures, never Rust or product storage."""
import contextlib
import copy
import io
import json
import os
from pathlib import Path
import runpy
import signal
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import Mock, patch

ROOT = Path(__file__).resolve().parents[2]
GATE = runpy.run_path(str(ROOT / '.github/scripts/check-phase-one-core.py'))


class ArtifactTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix='phase-one-artifact-')
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.target = self.root / 'target/build'
        self.target.mkdir(parents=True)
        self.records = []
        for name, (manifest, source, kind, features) in GATE['TARGETS'].items():
            executable = self.target / name
            executable.write_text('test binary fixture\n')
            executable.chmod(0o700)
            self.records.append({
                'reason': 'compiler-artifact', 'package_id': 'path+file://' + str(self.root / manifest),
                'manifest_path': str(self.root / manifest),
                'target': {'name': name, 'kind': kind, 'src_path': str(self.root / source)},
                'profile': {'test': True}, 'executable': str(executable),
                'features': sorted(features), 'fresh': True,
            })
        self.finished = {'reason': 'build-finished', 'success': True}

    def select(self, records):
        return GATE['test_artifacts']('\n'.join(map(json.dumps, records)), self.root, self.target, GATE['TARGETS'])

    def test_current_artifacts_bind_source_kind_features_and_binary_digest(self):
        selected = self.select([*self.records, self.finished])
        self.assertEqual(set(selected), set(GATE['TARGETS']))
        self.assertEqual(selected['storage_server']['features'], ['core', 'default'])
        self.assertEqual(selected['storage_server']['sha256'], GATE['digest'](self.target / 'storage_server'))

    def test_empty_partial_failed_duplicate_and_missing_completion_fail(self):
        for records in ([], [self.finished], self.records, [*self.records[:-1], self.finished],
                        [*self.records, {'reason': 'build-finished', 'success': False}],
                        [*self.records, self.records[0], self.finished],
                        [*self.records, self.finished, self.finished]):
            with self.subTest(records=records), self.assertRaises(ValueError):
                self.select(records)

    def test_core_and_default_features_are_required_on_current_server_artifacts(self):
        for index, record in enumerate(self.records):
            if record['target']['name'] not in ('storage_server', 'phase1_save_tags', 'phase1_smb_rpc'):
                continue
            for features in ([], ['core'], ['default'], 'core', [None]):
                records = copy.deepcopy(self.records)
                records[index]['features'] = features
                with self.subTest(index=index, features=features), self.assertRaises(ValueError):
                    self.select([*records, self.finished])

    def test_build_completion_requires_boolean_true_not_numeric_or_string_success(self):
        for success in (False, 1, 'true', None):
            with self.subTest(success=success), self.assertRaises(ValueError):
                self.select([*self.records, {'reason': 'build-finished', 'success': success}])

    def test_namesake_manifest_source_and_wrong_kind_are_rejected(self):
        for field, value in [('manifest_path', '/another/Cargo.toml'), ('src_path', '/another/lib.rs'),
                             ('kind', ['bin'])]:
            records = copy.deepcopy(self.records)
            if field == 'manifest_path':
                records[0][field] = value
            else:
                records[0]['target'][field] = value
            with self.subTest(field=field), self.assertRaises(ValueError):
                self.select([*records, self.finished])

    def test_non_test_profile_cannot_supply_test_binary(self):
        for value in (False, 1, 'true'):
            records = copy.deepcopy(self.records)
            records[0]['profile']['test'] = value
            with self.subTest(value=value), self.assertRaises(ValueError):
                self.select([*records, self.finished])

    def test_missing_relative_outside_or_nonexecutable_artifacts_fail(self):
        outside = self.root / 'stale-executable'
        outside.write_text('old\n')
        outside.chmod(0o700)
        for path in (None, 'relative', str(self.target / 'missing'), str(outside)):
            records = copy.deepcopy(self.records)
            records[0]['executable'] = path
            with self.subTest(path=path), self.assertRaises(ValueError):
                self.select([*records, self.finished])
        (self.target / 'storage_server').chmod(0o600)
        with self.assertRaises(ValueError):
            self.select([*self.records, self.finished])

    def test_compiler_diagnostics_and_invalid_json_do_not_pass(self):
        for level in ('warning', 'error'):
            message = {'reason': 'compiler-message', 'message': {'level': level, 'message': 'diagnostic'}}
            with self.subTest(level=level), self.assertRaises(ValueError):
                self.select([*self.records, message, self.finished])
        with self.assertRaises(ValueError):
            GATE['test_artifacts']('not JSON', self.root, self.target, GATE['TARGETS'])


class InventoryTests(unittest.TestCase):
    def test_reviewed_inventory_has_all_67_exact_required_cases(self):
        self.assertEqual(len(set(GATE['SERVER_TESTS'])), 29)
        self.assertEqual(len(set(GATE['FILE_TESTS'])), 12)
        self.assertEqual(len(set(GATE['DIRECTORY_TESTS'])), 10)
        self.assertEqual(len(set(GATE['DIRECTORY_SAFETY_TESTS'])), 5)
        self.assertEqual(len(set(GATE['STAGED_SOURCE_TESTS'])), 2)
        self.assertEqual(len(set(GATE['TAG_TESTS'])), 3)
        self.assertEqual(len(set(GATE['SMB_TESTS'])), 3)
        self.assertEqual(len(set(GATE['COLD_TAG_TESTS'])), 2)
        self.assertEqual(len(set(GATE['CONTRACT_TESTS'])), 1)
        self.assertEqual(sum('::tcp::' in name for name in GATE['SERVER_TESTS']), 2)
        self.assertEqual(sum('phase1_new_file_tests::' in name for name in GATE['SERVER_TESTS']), 3)
        self.assertEqual(sum('phase_one_write_admission::' in name for name in GATE['SERVER_TESTS']), 9)

    def test_exact_names_no_empty_duplicate_or_similar_prefix(self):
        parse = GATE['listed_tests']
        self.assertEqual(parse('a: test\nb: test\n2 tests, 0 benchmarks\n', ('a',)), ['a', 'b'])
        for output, expected in [('', ('a',)), ('0 tests, 0 benchmarks', ('a',)),
                                 ('a_more: test', ('a',)), ('a: test\na: test', ('a',)),
                                 ('a: test', ()), ('a: test', ('a', 'a')),
                                 ('a: test\nnoise', ('a',))]:
            with self.subTest(output=output), self.assertRaises(ValueError):
                parse(output, expected)
        with self.assertRaises(ValueError):
            parse('a: test\nb: test', ('a',), True)

    def test_completion_requires_actual_nonzero_passes_and_no_ignored_tests(self):
        parse = GATE['passed_tests']
        summary = 'test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 99 filtered out; finished in 0.1s\n'
        parse(summary, 1)
        for output, count, whole in [('', 1, False), (summary + summary, 1, False),
                                     (summary.replace('1 passed', '0 passed'), 1, False),
                                     (summary.replace('0 ignored', '1 ignored'), 1, False),
                                     (summary.replace('0 failed', '1 failed'), 1, False),
                                     (summary.replace('0 measured', '1 measured'), 1, False),
                                     (summary, 0, False), (summary, 2, False), (summary, 1, True)]:
            with self.subTest(output=output), self.assertRaises(ValueError):
                parse(output, count, whole)
        parse(summary.replace('99 filtered', '0 filtered'), 1, True)

    def test_commands_use_root_lock_default_core_and_only_selected_targets(self):
        server, directory, contract = GATE['build_commands']('/fresh/target')
        for _, command, _ in (server, directory, contract):
            for flag in ('--locked', '--offline', '--no-run', '--message-format=json'):
                self.assertIn(flag, command)
            for forbidden in ('--no-default-features', '--workspace', '--manifest-path'):
                self.assertNotIn(forbidden, command)
        self.assertIn('--lib', server[1])
        self.assertEqual(server[1][server[1].index('--features') + 1], 'core')
        self.assertNotIn('--lib', directory[1])  # Never queue all Engine unit tests.
        self.assertEqual(server[2], ('storage_server', 'phase1_save_tags', 'phase1_smb_rpc'))
        self.assertEqual(directory[2], ('directory_git_index', 'directory_git_safety',
                                       'directory_git_staged_source', 'phase1_tag_regressions',
                                       'phase1_tag_cold_replay'))
        for _, command, expected in (server, directory):
            selected = tuple(command[index + 1] for index, value in enumerate(command) if value == '--test')
            self.assertEqual(selected, tuple(name for name in expected if name != 'storage_server'))

    def test_directory_safety_inventories_reject_missing_or_extra_cases(self):
        for name in ('DIRECTORY_SAFETY_TESTS', 'STAGED_SOURCE_TESTS'):
            expected = GATE[name]
            complete = '\n'.join(test + ': test' for test in expected)
            self.assertEqual(GATE['listed_tests'](complete, expected, True), list(expected))
            for output in ('\n'.join(test + ': test' for test in expected[:-1]),
                           complete + '\nunreviewed_case: test'):
                with self.subTest(inventory=name, output=output), self.assertRaises(ValueError):
                    GATE['listed_tests'](output, expected, True)


# Separate executable fixtures exercise the real orchestration and JSON/log
# handoff. They intentionally simulate Cargo/libtest; no Rust test is claimed.
FAKE_BINARY = r'''
import json, os, sys
from pathlib import Path
config = json.loads(Path(os.environ['PHASE_ONE_FIXTURE']).read_text())
names = config['names'][TARGET]
with Path(config['calls']).open('a') as log:
    log.write(json.dumps({'tool': TARGET, 'args': sys.argv[1:], 'cwd': os.getcwd()}) + '\n')
if '--list' in sys.argv:
    if config.get('empty_list') == TARGET:
        names = []
    for name in names:
        print(name + ': test')
    print(f'{len(names)} tests, 0 benchmarks')
    if config.get('tamper_list') == TARGET:
        Path(sys.argv[0]).write_text('replaced after Cargo selection\n')
else:
    chosen = [sys.argv[sys.argv.index('--exact') + 1]] if '--exact' in sys.argv else names
    passed = len([name for name in chosen if name in names])
    ignored = passed if config.get('ignored') == TARGET else 0
    passed -= ignored
    print(f'test result: ok. {passed} passed; 0 failed; {ignored} ignored; 0 measured; {len(names)-len(chosen)} filtered out; finished in 0.01s')
'''
FAKE_CARGO = r'''
import json, os, sys
from pathlib import Path
config = json.loads(Path(os.environ['PHASE_ONE_FIXTURE']).read_text())
with Path(config['calls']).open('a') as log:
    log.write(json.dumps({'tool': 'cargo', 'args': sys.argv[1:], 'cwd': os.getcwd()}) + '\n')
if config.get('fail_build'):
    sys.exit(17)
target_dir = Path(sys.argv[sys.argv.index('--target-dir')+1])
target_dir.mkdir(parents=True, exist_ok=True)
if 'storage-server' in sys.argv:
    selected = (['storage_server'] if '--lib' in sys.argv else [])
    selected += [sys.argv[index+1] for index, value in enumerate(sys.argv) if value == '--test']
elif 'storage-engine' in sys.argv:
    selected = [sys.argv[index+1] for index, value in enumerate(sys.argv) if value == '--test']
else:
    selected = ['contract-check']
for name in selected:
    manifest, source, kind, features = config['targets'][name]
    binary = target_dir / name
    binary.write_text('#!' + sys.executable + '\nTARGET=' + repr(name) + '\n' + config['binary'])
    binary.chmod(0o700)
    if config.get('omit_artifacts'):
        continue
    if config.get('missing_core'):
        features = []
    print(json.dumps({'reason':'compiler-artifact','package_id':'path+file://'+str(Path.cwd()/manifest),
                     'manifest_path':str(Path.cwd()/manifest),
                     'target':{'name':name,'kind':kind,'src_path':str(Path.cwd()/source)},
                     'profile':{'test':True},'features':features,'executable':str(binary),'fresh':False}))
if config.get('mutate_lock'):
    Path('Cargo.lock').write_text('changed\n')
print(json.dumps({'reason':'build-finished','success':True}))
'''


class OrchestrationTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix='phase-one-orchestration-')
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        (self.root / 'Cargo.toml').write_text('[workspace]\nmembers=[]\n')
        (self.root / 'Cargo.lock').write_text('fixture lock\n')
        tools = self.root / 'tools'
        tools.mkdir()
        for name, source in [('cargo', FAKE_CARGO), ('git', "print('f'*40)\n")]:
            path = tools / name
            path.write_text('#!' + sys.executable + '\n' + source)
            path.chmod(0o700)
        self.config = {
            'calls': str(self.root / 'calls.jsonl'), 'binary': FAKE_BINARY,
            'targets': {name: [manifest, source, kind, sorted(features)]
                        for name, (manifest, source, kind, features) in GATE['TARGETS'].items()},
            'names': {'storage_server': list(GATE['SERVER_TESTS']) + ['unrelated::test'],
                      'phase1_save_tags': GATE['FILE_TESTS'], 'directory_git_index': GATE['DIRECTORY_TESTS'],
                      'directory_git_safety': GATE['DIRECTORY_SAFETY_TESTS'],
                      'directory_git_staged_source': GATE['STAGED_SOURCE_TESTS'],
                      'phase1_smb_rpc': GATE['SMB_TESTS'], 'phase1_tag_cold_replay': GATE['COLD_TAG_TESTS'],
                      'phase1_tag_regressions': GATE['TAG_TESTS'], 'contract-check': GATE['CONTRACT_TESTS']},
        }
        self.config_path = self.root / 'fixture.json'
        self.environment = {'PATH': str(tools) + os.pathsep + os.environ['PATH'],
                            'PHASE_ONE_FIXTURE': str(self.config_path)}

    def run_gate(self, **changes):
        self.config.update(changes)
        self.config_path.write_text(json.dumps(self.config))
        with patch.dict(os.environ, self.environment), contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
            gate = GATE['Gate'](self.root)
            gate.execute()
            return gate

    def calls(self):
        path = self.root / 'calls.jsonl'
        return [] if not path.exists() else [json.loads(row) for row in path.read_text().splitlines()]

    def summary(self):
        return json.loads((self.root / 'target/phase-one-core/summary.json').read_text())

    def test_pipeline_builds_lists_and_runs_67_unique_tests_without_engine_libtest(self):
        gate = self.run_gate()
        self.assertEqual(gate.summary['status'], 'passed')
        self.assertEqual(gate.summary['passed_count'], 67)
        self.assertEqual(len({(x['target'], x['name']) for x in gate.summary['passed']}), 67)
        calls = self.calls()
        self.assertEqual([x['tool'] for x in calls[:3]], ['cargo', 'cargo', 'cargo'])
        self.assertTrue(all(x['cwd'] == str(self.root) for x in calls))
        first_run = next(i for i, x in enumerate(calls) if '--nocapture' in x['args'])
        self.assertEqual(sum('--list' in x['args'] for x in calls[:first_run]), 9)
        self.assertEqual(sum('--exact' in x['args'] for x in calls), 30)
        self.assertEqual(gate.environment['TMPDIR'], str(self.root / 'target/phase-one-core/fixtures'))

    def test_missing_root_lock_fails_before_cargo(self):
        (self.root / 'Cargo.lock').unlink()
        with self.assertRaises(ValueError):
            self.run_gate()
        self.assertFalse(self.calls())
        self.assertEqual(self.summary()['status'], 'failed')

    def test_old_binary_and_old_success_receipt_cannot_satisfy_failed_current_build(self):
        self.run_gate()
        with self.assertRaises(subprocess.CalledProcessError):
            self.run_gate(fail_build=True)
        self.assertEqual(self.summary()['status'], 'failed')
        self.assertEqual(self.summary()['passed'], [])

    def test_existing_binary_without_current_artifact_is_rejected(self):
        with self.assertRaises(ValueError):
            self.run_gate(omit_artifacts=True)
        self.assertTrue((self.root / 'target/phase-one-core/build/storage_server').is_file())
        self.assertFalse(any('--list' in x['args'] for x in self.calls()))

    def test_current_noncore_artifact_is_rejected_before_execution(self):
        with self.assertRaises(ValueError):
            self.run_gate(missing_core=True)
        self.assertFalse(any('--list' in x['args'] for x in self.calls()))

    def test_missing_inventory_prevents_any_test_execution(self):
        for target in ('phase1_save_tags', 'phase1_smb_rpc', 'phase1_tag_cold_replay',
                       'directory_git_safety', 'directory_git_staged_source'):
            with self.subTest(target=target), self.assertRaises(ValueError):
                self.run_gate(empty_list=target)
        self.assertFalse(any('--nocapture' in x['args'] for x in self.calls()))

    def test_executable_change_after_listing_is_rejected_before_running(self):
        with self.assertRaises(ValueError):
            self.run_gate(tamper_list='storage_server')
        self.assertFalse(any('--nocapture' in x['args'] for x in self.calls()))
        self.assertEqual(self.summary()['status'], 'failed')

    def test_successful_ignored_test_does_not_count_as_pass(self):
        with self.assertRaises(ValueError):
            self.run_gate(ignored='storage_server')
        self.assertEqual(self.summary()['status'], 'failed')
        self.assertEqual(self.summary()['passed'], [])

    def test_ignored_directory_safety_target_never_completes_the_gate(self):
        for target in ('directory_git_safety', 'directory_git_staged_source'):
            with self.subTest(target=target), self.assertRaises(ValueError):
                self.run_gate(ignored=target)
            summary = self.summary()
            self.assertEqual(summary['status'], 'failed')
            self.assertNotIn('passed_count', summary)
            self.assertFalse(any(case['target'] == target for case in summary['passed']))

    def test_lock_change_is_rejected(self):
        with self.assertRaises(ValueError):
            self.run_gate(mutate_lock=True)
        self.assertFalse(any('--list' in x['args'] for x in self.calls()))

    def test_timeout_terminates_owned_process_group_and_keeps_logs(self):
        gate = GATE['Gate'](self.root)
        process = Mock(pid=1234)
        process.wait.side_effect = [subprocess.TimeoutExpired('fixture', 1), -9]
        with patch.object(GATE['subprocess'], 'Popen', return_value=process), \
                patch.object(GATE['os'], 'killpg') as kill, contextlib.redirect_stdout(io.StringIO()):
            with self.assertRaises(subprocess.TimeoutExpired):
                gate.run('timeout', ['fixture'], 1)
        kill.assert_called_once_with(1234, signal.SIGKILL)
        self.assertTrue(gate.summary['commands'][0]['timed_out'])
        self.assertTrue((gate.logs / 'timeout.stdout.log').is_file())


class WorkflowTests(unittest.TestCase):
    def test_dedicated_job_waits_only_for_static_and_preserves_evidence(self):
        workflow = (ROOT / '.github/workflows/ci.yml').read_text()
        job = workflow.split('  phase-one-core:\n', 1)[1].split('\n  web-production:', 1)[0]
        self.assertIn('    needs: static\n', job)
        self.assertNotIn('needs: tests', job)
        self.assertIn('python3 .github/scripts/check-phase-one-core.py', job)
        self.assertIn('if: always()', job)
        self.assertIn('target/phase-one-core/logs/', job)
        self.assertIn('target/phase-one-core/summary.json', job)
        self.assertNotIn('target/phase-one-core/build', job)
        self.assertNotIn('target/phase-one-core/fixtures', job)
        self.assertIn('persist-credentials: false', job)


if __name__ == '__main__':
    unittest.main()
