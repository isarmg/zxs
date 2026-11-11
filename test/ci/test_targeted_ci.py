"""Exercise change selection and fail-closed runtime evidence with synthetic inputs."""
import copy
import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location('targeted_ci', ROOT / '.github/scripts/check-targeted-ci.py')
CI = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CI)


class TargetedSelection(unittest.TestCase):
    def setUp(self):
        self.policy = json.loads((ROOT / CI.PLAN_PATH).read_text())

    def test_tooling_change_selects_only_related_python_modules(self):
        plan = CI.select_plan(self.policy, ['.github/scripts/check-targeted-ci.py'])
        self.assertEqual(plan['rust_targets'], [])
        self.assertIn('test_targeted_ci', plan['python_tests'])
        self.assertTrue(plan['unexecuted'])

    def test_layout_and_ci_changes_union_groups_once(self):
        plan = CI.select_plan(self.policy, ['src/engine/src/persist/layout.rs',
                            'src/engine/src/journal.rs', '.github/workflows/ci.yml'])
        self.assertEqual(len(plan['rust_targets']), 2)
        self.assertEqual(len(plan['groups']), 3)
        self.assertIn('test_development_ci', plan['python_tests'])

    def test_unknown_and_empty_changes_need_target_selection(self):
        for changed in ([], ['__unmapped_fixture__/unknown.rs']):
            with self.subTest(changed=changed), self.assertRaises(ValueError):
                CI.select_plan(self.policy, changed)

    def test_malformed_or_unbounded_target_is_rejected(self):
        for field, value in [('prefixes', []), ('max_tests', 0), ('max_tests', 193),
                             ('prefixes', ['']), ('package', '--workspace'), ('timeout_seconds', 901)]:
            policy = copy.deepcopy(self.policy)
            policy['groups']['held-directory-layout']['rust_targets'][0][field] = value
            with self.subTest(field=field), self.assertRaises(ValueError):
                CI.select_plan(policy, ['src/engine/src/persist/layout.rs'])

    def test_current_inventory_resolves_new_test_names_and_excludes_ignored(self):
        target = {'prefixes': ['layout::'], 'max_tests': 2}
        names = ['layout::renamed_behavior', 'layout::ignored', 'other::test']
        self.assertEqual(CI.selected_tests(names, ['layout::ignored'], target),
                         ['layout::renamed_behavior'])

    def test_empty_module_ignored_only_and_over_budget_fail(self):
        for names, ignored, maximum in [([], [], 2), (['layout::x'], ['layout::x'], 2),
                                        (['layout::x', 'layout::y'], [], 1)]:
            with self.subTest(names=names), self.assertRaises(ValueError):
                CI.selected_tests(names, ignored, {'prefixes': ['layout::'], 'max_tests': maximum})

    def test_exact_nonzero_test_result_is_required(self):
        line = 'test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 20 filtered out; finished in 0.1s\n'
        CI.passed_test(line, 21)
        for invalid in ['', line.replace('1 passed', '0 passed'),
                        line.replace('0 ignored', '1 ignored'), line.replace('20 filtered', '19 filtered')]:
            with self.subTest(invalid=invalid), self.assertRaises(ValueError):
                CI.passed_test(invalid, 21)

    def test_namespace_child_summary_uses_the_final_parent_inventory(self):
        child = 'test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.1s\n'
        parent = child.replace('0 filtered', '20 filtered')
        CI.passed_test(child + parent, 21)

    def test_documentation_change_runs_lightweight_policy_regressions(self):
        plan = CI.select_plan(self.policy, ['TASKS.md'])
        self.assertEqual(plan['rust_targets'], [])
        self.assertTrue(plan['python_tests'])

    def test_python_zero_tests_missing_import_and_skips_cannot_pass(self):
        CI.passed_python('Ran 2 tests in 0.001s\n\nOK\n', 2)
        for output in ['Ran 0 tests in 0.001s\n\nOK\n',
                       'Ran 1 test in 0.001s\n\nFAILED (errors=1)\n',
                       'Ran 1 test in 0.001s\n\nOK (skipped=1)\n']:
            with self.assertRaises(ValueError):
                CI.passed_python(output, 1)

    def test_named_core_integration_build_is_explicit(self):
        target = {'package': 'storage-server', 'manifest': 'src/server/Cargo.toml',
                  'kind': 'test', 'name': 'phase1_smb_rpc', 'features': ['core'],
                  'prefixes': ['http_soft_deletion::'], 'max_tests': 30}
        command = CI.cargo_command(target, '/build')
        self.assertIn('--test', command)
        self.assertIn('phase1_smb_rpc', command)
        self.assertIn('--no-run', command)
        self.assertEqual(command[command.index('--features') + 1], 'core')
        self.assertNotIn('--lib', command)

    def test_wasm_check_and_native_bin_check_do_not_claim_tests(self):
        target = {'package': 'web-app', 'mode': 'check', 'compile_target': 'wasm32-unknown-unknown'}
        command = CI.cargo_command(target, '/build')
        self.assertEqual(command[:2], ['cargo', 'check'])
        self.assertIn('--release', command)
        self.assertIn('wasm32-unknown-unknown', command)
        self.assertNotIn('--no-run', command)
        binary = CI.cargo_command({'package': 'perf-check', 'mode': 'check',
                                  'kind': 'bin', 'name': 'recovery-startup'}, '/build')
        self.assertIn('recovery-startup', binary)
        self.assertNotIn('--all-targets', binary)

    def test_exact_selection_must_exist_and_be_runnable(self):
        target = {'exact': ['module::test'], 'max_tests': 2}
        self.assertEqual(CI.selected_tests(['module::test'], [], target), ['module::test'])
        for names, ignored in [([], []), (['module::test'], ['module::test'])]:
            with self.assertRaises(ValueError):
                CI.selected_tests(names, ignored, target)

    def test_artifact_kind_name_features_manifest_and_completion_are_bound(self):
        target = {'package': 'web-app', 'manifest': 'src/web/Cargo.toml'}
        item = {'reason': 'compiler-artifact', 'target': {'name': 'web_app',
                'kind': ['cdylib', 'rlib']}, 'manifest_path': str(ROOT / target['manifest']),
                'profile': {'test': True}, 'features': [], 'executable': '/build/web_app'}
        def output(artifact, success=True):
            return json.dumps(artifact) + '\n' + json.dumps({'reason': 'build-finished', 'success': success})
        self.assertEqual(CI.current_artifact(output(item), target, ROOT), item)
        for mutation in [{'target': {'name': 'other', 'kind': ['lib']}},
                         {'manifest_path': '/different/Cargo.toml'},
                         {'profile': {'test': False}}]:
            with self.assertRaises(ValueError):
                CI.current_artifact(output(dict(item, **mutation)), target, ROOT)
        with self.assertRaises(ValueError):
            CI.current_artifact(output(item, False), target, ROOT)
        with self.assertRaises(ValueError):
            CI.current_artifact(output(item), dict(target, features=['core']), ROOT)

    def overlapping_policy(self, first, second):
        return {'groups': {
            'first': {'reason': 'first behavior', 'python_tests': [], 'rust_targets': [first]},
            'second': {'reason': 'second behavior', 'python_tests': [], 'rust_targets': [second]}},
            'rules': [{'path': 'change', 'groups': ['first', 'second']}],
            'unexecuted': 'other behaviors'}

    def test_overlapping_build_keeps_separate_budgets_and_strictest_timeout(self):
        first = {'package': 'storage-engine', 'manifest': 'src/engine/Cargo.toml',
                 'prefixes': ['left::'], 'max_tests': 2, 'timeout_seconds': 900}
        second = dict(first, prefixes=['right::'], exact=['left::shared'],
                      max_tests=2, timeout_seconds=120)
        plan = CI.select_plan(self.overlapping_policy(first, second), ['change'])
        self.assertEqual(len(plan['rust_targets']), 1)
        target = plan['rust_targets'][0]
        names = ['left::shared', 'left::only', 'right::only', 'other::ignored']
        evidence = CI.selection_evidence(names, ['other::ignored'], target)
        self.assertEqual([item['group'] for item in evidence], ['first', 'second'])
        self.assertEqual([item['configuration'] for item in evidence], [first, second])
        self.assertEqual(CI.test_timeouts(evidence),
                         {'left::shared': 120, 'left::only': 900, 'right::only': 120})
        self.assertEqual(CI.selected_tests(names, ['other::ignored'], target),
                         ['left::only', 'left::shared', 'right::only'])
        self.assertEqual(first['max_tests'], 2)

    def test_overlap_cannot_mask_missing_ignored_or_over_budget_selection(self):
        first = {'package': 'storage-engine', 'manifest': 'src/engine/Cargo.toml',
                 'whole': True, 'max_tests': 10}
        for second, names, ignored in [
            (dict(first, whole=False, prefixes=['missing::']), ['other::x'], []),
            (dict(first, whole=False, exact=['other::x']), ['other::x', 'other::y'],
             ['other::x']),
            (dict(first, max_tests=1), ['other::x', 'other::y'], [])]:
            with self.subTest(second=second):
                target = CI.select_plan(self.overlapping_policy(first, second),
                                        ['change'])['rust_targets'][0]
                with self.assertRaises(ValueError):
                    CI.selection_evidence(names, ignored, target)

    def test_build_identity_normalizes_feature_order_but_preserves_graphs(self):
        first = {'package': 'storage-server', 'manifest': 'src/server/Cargo.toml',
                 'mode': 'check', 'features': ['core', 'extra']}
        reordered = dict(first, features=['extra', 'core'])
        plan = CI.select_plan(self.overlapping_policy(first, reordered), ['change'])
        self.assertEqual(len(plan['rust_targets']), 1)
        self.assertEqual(plan['rust_targets'][0]['selection_groups'], ['first', 'second'])
        for mutation in [{'features': ['core']}, {'default_features': False},
                         {'compile_target': 'wasm32-unknown-unknown'},
                         {'kind': 'bin', 'name': 'zxs'},
                         {'mode': 'test', 'whole': True, 'max_tests': 2}]:
            with self.subTest(mutation=mutation):
                plan = CI.select_plan(self.overlapping_policy(first, dict(first, **mutation)),
                                      ['change'])
                self.assertEqual(len(plan['rust_targets']), 2)
        with self.assertRaises(ValueError):
            CI.select_plan(self.overlapping_policy(first, dict(first, manifest='helper/Cargo.toml')),
                           ['change'])

    def test_unique_build_cap_applies_after_overlap_coalescing(self):
        target = {'package': 'storage-engine', 'manifest': 'src/engine/Cargo.toml',
                  'whole': True, 'max_tests': 2}
        policy = self.overlapping_policy(target, dict(target))
        policy['groups']['first']['rust_targets'] = [dict(target) for _ in range(17)]
        self.assertEqual(len(CI.select_plan(policy, ['change'])['rust_targets']), 1)
        policy['groups']['first']['rust_targets'] = [
            dict(target, kind='test', name=f'target_{index}') for index in range(16)]
        with self.assertRaises(ValueError):
            CI.select_plan(policy, ['change'])
        policy['groups']['first']['rust_targets'] = [dict(target, max_tests=193)]
        with self.assertRaises(ValueError):
            CI.select_plan(policy, ['change'])

    def test_path_consumer_repairs_and_layout_policy_share_sixteen_builds(self):
        plan = CI.select_plan(self.policy, ['src/engine/src/business/directory.rs',
                            'src/engine/src/business/workspace.rs',
                            'test/ci/test_layout_targeted_ci.py'])
        self.assertEqual(plan['groups'], ['ci-selection', 'held-directory-layout',
                                         'historical-directory-runtime', 'portable-path-consumers',
                                         'projection-cache-layout'])
        self.assertEqual(len(plan['rust_targets']), 16)
        engine = next(target for target in plan['rust_targets']
                      if target['package'] == 'storage-engine' and target.get('kind', 'lib') == 'lib')
        self.assertEqual(engine['selection_groups'],
                         ['held-directory-layout', 'historical-directory-runtime',
                          'portable-path-consumers', 'projection-cache-layout'])
        self.assertEqual([selection['max_tests'] for selection in engine['selections']],
                         [187, 6, 160, 52])

    def test_typed_name_source_and_provenance_paths_are_admitted(self):
        paths = [
            'src/contract/src/repository_name.rs', 'test/contract/unit/repository_name.rs',
            'src/engine/src/business/mod.rs', 'src/engine/src/foreign_import/intent.rs',
            'src/engine/src/foreign_import/mapping.rs', 'src/engine/src/historical_save.rs',
            'src/engine/src/lib.rs', 'test/engine/unit/foreign_import/tests.rs',
            'src/naming/src/lib.rs', 'src/naming/src/name.rs', 'test/naming/unit/names.rs',
            'src/server/src/backend_file_relocation.rs', 'src/server/src/backend_listing.rs',
            'src/server/src/backend_object_move.rs', 'src/server/src/engine_backend/file_manager.rs',
            'src/server/src/http_file_relocation.rs', 'src/server/src/http_object_move.rs',
            'test/server/integration/http_engine/history_alias_operations.rs',
            'test/server/unit/backend_listing.rs']
        for path in paths:
            with self.subTest(path=path):
                self.assertIn('typed-name-consumers', CI.select_plan(self.policy, [path])['groups'])
        for path in ['compliance/external-tests/integrated.json',
                     'docs/THIRD_PARTY_RUST_SOURCES.md']:
            with self.subTest(path=path):
                self.assertEqual(CI.select_plan(self.policy, [path])['rust_targets'], [])
        self.assertIn('portable-path-consumers', CI.select_plan(
            self.policy, ['compliance/rust-sources.json'])['groups'])

    def test_typed_name_overlap_retains_old_groups_with_fifteen_builds(self):
        plan = CI.select_plan(self.policy, [
            'src/naming/src/lib.rs', 'src/engine/src/lib.rs',
            'src/contract/src/repository_name.rs',
            'src/server/src/backend_listing.rs',
            'test/server/integration/http_engine/history_alias_operations.rs',
            '.github/ci/targeted-tests.json'])
        self.assertEqual(plan['groups'], ['ci-selection', 'held-directory-layout',
                                         'portable-path-consumers', 'typed-name-consumers'])
        self.assertEqual(len(plan['rust_targets']), 15)
        engine = next(target for target in plan['rust_targets']
                      if target['package'] == 'storage-engine' and target.get('kind', 'lib') == 'lib')
        self.assertEqual(engine['selection_groups'],
                         ['held-directory-layout', 'portable-path-consumers', 'typed-name-consumers'])
        http = next(target for target in plan['rust_targets'] if target.get('name') == 'http_engine')
        self.assertEqual(http['features'], ['core'])
        self.assertEqual([selection['prefixes'] for selection in http['selections']],
                         [['backup_namespace::'], ['history_alias_operations::']])

    def test_invalid_inventory_is_rejected(self):
        self.assertEqual(CI.inventory('module::test: test\n\n1 test, 0 benchmarks\n'), ['module::test'])
        for invalid in ['unexpected output', 'x: test\nx: test\n']:
            with self.assertRaises(ValueError):
                CI.inventory(invalid)

class TargetedExecution(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        (self.root / 'Cargo.lock').write_text('fixture lock\n')
        self.run = CI.Run(self.root)
        self.target = {'package': 'fixture', 'manifest': 'src/fixture/Cargo.toml',
                       'prefixes': ['module::'], 'max_tests': 2,
                       'selection_groups': ['fixture']}
        self.calls = []
        self.failures = {}
        self.names = ['module::a', 'module::b', 'other::unselected']
        self.ignored = []
        self.plan = {'python_tests': [], 'rust_targets': [self.target]}
        self.real_process = subprocess.run
        self.addCleanup(mock.patch.stopall)
        mock.patch.object(self.run, 'plan', return_value=self.plan).start()
        mock.patch.dict(CI.os.environ, {'GITHUB_STEP_SUMMARY': ''}).start()
        mock.patch.object(CI.subprocess, 'run', side_effect=self.process).start()

    def process(self, args, **kwargs):
        self.calls.append(args)
        key = args[args.index('--exact') + 1] if '--exact' in args else args[0]
        failure = self.failures.get(key)
        if isinstance(failure, Exception):
            raise failure
        if failure:
            kwargs['stderr'].write('fixture process failed\n')
            return subprocess.CompletedProcess(args, failure)
        if args[0] == 'cargo':
            package = args[args.index('-p') + 1]
            build = Path(args[args.index('--target-dir') + 1])
            build.mkdir(exist_ok=True)
            binary = build / package
            binary.write_text('current compiled fixture')
            artifact = {'reason': 'compiler-artifact', 'target': {'name': package,
                        'kind': ['lib']}, 'manifest_path': str(self.root / 'src' / package
                                                             / 'Cargo.toml'),
                        'profile': {'test': args[1] == 'test'}, 'features': [],
                        'executable': str(binary), 'filenames': [str(binary)]}
            output = json.dumps(artifact) + '\n' + json.dumps(
                {'reason': 'build-finished', 'success': True})
        elif '--list' in args:
            names = self.ignored if '--ignored' in args else self.names
            output = ''.join(name + ': test\n' for name in names)
        else:
            output = ('test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; '
                      f'{len(self.names) - 1} filtered out; finished in 0.1s\n')
        kwargs['stdout'].write(output)
        return subprocess.CompletedProcess(args, 0)

    def persisted(self):
        return json.loads((self.run.directory / 'summary.json').read_text())

    def test_failure_continues_selected_tests_and_target_but_still_raises(self):
        self.failures['module::a'] = 101
        check = {'package': 'consumer', 'manifest': 'src/consumer/Cargo.toml', 'mode': 'check'}
        self.plan['rust_targets'].append(check)
        with self.assertRaises(RuntimeError):
            self.run.execute('run')
        summary = self.persisted()
        self.assertEqual(summary['status'], 'failed')
        self.assertEqual([item['name'] for item in summary['passed']], ['module::b'])
        self.assertEqual(summary['compiled'], [check])
        self.assertEqual(summary['skipped'], [])
        self.assertEqual([(item['stage'], item['name']) for item in summary['failed']],
                         [('test', 'module::a')])
        executed = [args[args.index('--exact') + 1] for args in self.calls if '--exact' in args]
        self.assertEqual(executed, ['module::a', 'module::b'])
        self.assertEqual(summary['artifacts'][0]['selected'], executed)
        self.assertEqual(summary['artifacts'][0]['unexecuted'], ['other::unselected'])

    def test_failed_compile_never_runs_stale_binary_and_continues_next_target(self):
        stale = self.run.directory / 'build/fixture'
        stale.parent.mkdir()
        stale.write_text('stale binary')
        second = dict(self.target, package='consumer', manifest='src/consumer/Cargo.toml')
        self.plan['rust_targets'].append(second)
        original = self.process
        def process(args, **kwargs):
            if args[:2] == ['cargo', 'test'] and args[args.index('-p') + 1] == 'fixture':
                self.calls.append(args)
                return subprocess.CompletedProcess(args, 101)
            return original(args, **kwargs)
        with mock.patch.object(CI.subprocess, 'run', side_effect=process):
            with self.assertRaises(RuntimeError):
                self.run.execute('run')
        summary = self.persisted()
        self.assertFalse(any(args[0] == str(stale) for args in self.calls))
        self.assertEqual([item['configuration'] for item in summary['passed']], [second, second])
        self.assertEqual(summary['failed'][0]['stage'], 'compile')
        self.assertEqual(summary['skipped'][0]['configuration'], self.target)

    def test_inventory_and_selection_failures_are_failed_not_successful_skips(self):
        cases = [(['module::a', 'module::a'], [], 2, 'inventory'),
                 (['other::unselected'], [], 2, 'selection'),
                 (['module::a'], ['module::a'], 2, 'selection'),
                 (['module::a', 'module::b'], [], 1, 'selection')]
        for names, ignored, maximum, stage in cases:
            with self.subTest(stage=stage, names=names):
                self.run = CI.Run(self.root)
                self.names, self.ignored = names, ignored
                self.target['max_tests'] = maximum
                with mock.patch.object(self.run, 'plan', return_value=self.plan):
                    with self.assertRaises(RuntimeError):
                        self.run.execute('run')
                summary = self.persisted()
                self.assertEqual(summary['status'], 'failed')
                self.assertEqual(summary['failed'][0]['stage'], stage)
                self.assertEqual(summary['passed'], [])
                self.assertEqual(summary['compiled'], [])
                self.assertEqual(len(summary['skipped']), 1)
        self.assertFalse(any('--exact' in args for args in self.calls))

    def test_launch_error_is_classified_and_does_not_stop_next_test(self):
        self.failures['module::a'] = OSError('fixture launch error')
        with self.assertRaises(RuntimeError):
            self.run.execute('run')
        summary = self.persisted()
        self.assertEqual(summary['failed'][0]['kind'], 'OSError')
        self.assertEqual(summary['commands'][3]['status'], 'error')
        self.assertEqual([item['name'] for item in summary['passed']], ['module::b'])
        self.assertEqual(summary['blocked'], [])

    def test_test_timeout_preserves_completed_results_and_blocks_remaining_scope(self):
        self.names.insert(2, 'module::c')
        self.target['max_tests'] = 3
        self.failures['module::b'] = subprocess.TimeoutExpired(['fixture'], 1)
        later = dict(self.target, package='consumer', manifest='src/consumer/Cargo.toml')
        self.plan['rust_targets'].append(later)
        with self.assertRaises(subprocess.TimeoutExpired):
            self.run.execute('run')
        summary = self.persisted()
        self.assertEqual(summary['status'], 'failed')
        self.assertEqual(summary['failed'][0]['kind'], 'timeout')
        self.assertEqual(summary['commands'][-1]['status'], 'timed-out')
        self.assertEqual([item['name'] for item in summary['passed']], ['module::a'])
        self.assertEqual(summary['blocked'][0]['names'], ['module::c'])
        self.assertEqual(summary['blocked'][1]['rust_targets'], [later])
        executed = [args[args.index('--exact') + 1] for args in self.calls if '--exact' in args]
        self.assertEqual(executed, ['module::a', 'module::b'])
        self.assertEqual(len([args for args in self.calls if args[0] == 'cargo']), 1)

    def test_real_subprocess_timeout_stops_before_next_selected_test(self):
        original = self.process
        def process(args, **kwargs):
            if '--exact' in args:
                self.calls.append(args)
                kwargs['timeout'] = 0.1
                return self.real_process([CI.sys.executable, '-c',
                                          'import time; time.sleep(30)'], **kwargs)
            return original(args, **kwargs)
        later = dict(self.target, package='consumer', manifest='src/consumer/Cargo.toml')
        self.plan['rust_targets'].append(later)
        with mock.patch.object(CI.subprocess, 'run', side_effect=process):
            with self.assertRaises(subprocess.TimeoutExpired):
                self.run.execute('run')
        summary = self.persisted()
        self.assertEqual(summary['status'], 'failed')
        self.assertEqual(summary['commands'][-1]['status'], 'timed-out')
        self.assertEqual(summary['failed'][0]['kind'], 'timeout')
        self.assertEqual(summary['failed'][0]['name'], 'module::a')
        self.assertEqual(summary['passed'], [])
        self.assertEqual(summary['blocked'][0]['names'], ['module::b'])
        self.assertEqual(summary['blocked'][1]['rust_targets'], [later])
        self.assertEqual(len(self.calls), 4)

    def test_compile_and_inventory_timeout_stop_all_later_targets(self):
        later = dict(self.target, package='consumer', manifest='src/consumer/Cargo.toml')
        self.plan['rust_targets'].append(later)
        for stage, key in [('compile', 'cargo'),
                           ('inventory', str(self.run.directory / 'build/fixture'))]:
            with self.subTest(stage=stage):
                self.run = CI.Run(self.root)
                self.calls.clear()
                self.failures = {key: subprocess.TimeoutExpired([key], 1)}
                with mock.patch.object(self.run, 'plan', return_value=self.plan):
                    with self.assertRaises(subprocess.TimeoutExpired):
                        self.run.execute('run')
                summary = self.persisted()
                self.assertEqual(summary['failed'][0]['stage'], stage)
                self.assertEqual(summary['commands'][-1]['status'], 'timed-out')
                self.assertEqual(summary['passed'], [])
                self.assertEqual(summary['blocked'][0]['configuration'], self.target)
                self.assertEqual(summary['blocked'][1]['rust_targets'], [later])
                self.assertEqual(len(self.calls), 1 if stage == 'compile' else 2)

    def test_python_timeout_stops_remaining_modules_and_all_rust_targets(self):
        self.plan['python_tests'] = ['test_first', 'test_timeout', 'test_later']
        visited = []
        def python(name):
            visited.append(name)
            if name == 'test_timeout':
                self.run.command(['python-fixture'], 1)
            self.run.summary['passed'].append({'python_module': name, 'tests': 1})
        self.failures['python-fixture'] = subprocess.TimeoutExpired(['python-fixture'], 1)
        with mock.patch.object(self.run, 'python', side_effect=python):
            with self.assertRaises(subprocess.TimeoutExpired):
                self.run.execute('run')
        summary = self.persisted()
        self.assertEqual(visited, ['test_first', 'test_timeout'])
        self.assertEqual(summary['failed'][0]['stage'], 'python')
        self.assertEqual(summary['commands'][-1]['status'], 'timed-out')
        self.assertEqual(summary['passed'], [{'python_module': 'test_first', 'tests': 1}])
        self.assertEqual(summary['blocked'][0]['python_modules'], ['test_later'])
        self.assertEqual(summary['blocked'][0]['rust_targets'], [self.target])
        self.assertEqual(self.calls, [['python-fixture']])

    def test_toolchain_timeout_blocks_native_and_cross_targets(self):
        cross = dict(self.target, mode='check', compile_target='wasm32-unknown-unknown')
        self.plan['rust_targets'].insert(0, cross)
        self.failures['rustup'] = subprocess.TimeoutExpired(['rustup'], 1)
        with self.assertRaises(subprocess.TimeoutExpired):
            self.run.execute('run')
        summary = self.persisted()
        self.assertEqual(summary['failed'][0]['stage'], 'toolchain')
        self.assertEqual(summary['commands'][-1]['status'], 'timed-out')
        self.assertEqual(summary['blocked'][0]['rust_targets'], [cross, self.target])
        self.assertEqual(summary['passed'], [])
        self.assertEqual(len(self.calls), 1)

    def test_python_failure_continues_other_module_and_selected_rust(self):
        self.plan['python_tests'] = ['test_first', 'test_second']
        visited = []
        def python(name):
            visited.append(name)
            if name == 'test_first':
                raise ValueError('fixture invalid Python evidence')
            self.run.summary['passed'].append({'python_module': name, 'tests': 1})
        with mock.patch.object(self.run, 'python', side_effect=python):
            with self.assertRaises(RuntimeError):
                self.run.execute('run')
        summary = self.persisted()
        self.assertEqual(visited, self.plan['python_tests'])
        self.assertEqual(summary['failed'][0]['stage'], 'python')
        self.assertEqual(len(summary['passed']), 3)

    def test_toolchain_failure_skips_only_cross_target(self):
        cross = dict(self.target, mode='check', compile_target='wasm32-unknown-unknown')
        self.plan['rust_targets'].insert(0, cross)
        self.failures['rustup'] = 1
        with self.assertRaises(RuntimeError):
            self.run.execute('run')
        summary = self.persisted()
        self.assertEqual(summary['failed'][0]['stage'], 'toolchain')
        self.assertEqual(summary['skipped'][0]['configuration'], cross)
        self.assertEqual(len(summary['passed']), 2)
        self.assertFalse(any('--target' in args for args in self.calls))

    def test_success_and_compile_only_evidence_stay_distinct(self):
        self.plan['rust_targets'].append(dict(self.target, mode='check'))
        self.run.execute('run')
        summary = self.persisted()
        self.assertEqual(summary['status'], 'selected-tests-passed')
        self.assertEqual(len(summary['passed']), 2)
        self.assertEqual(len(summary['compiled']), 1)
        self.assertEqual(summary['failed'], [])
        self.assertEqual(summary['skipped'], [])

    def test_failed_run_exits_nonzero_after_retaining_later_pass(self):
        script = """
import json
import sys
sys.path.insert(0, 'test/ci')
from test_targeted_ci import TargetedExecution
case = TargetedExecution()
case.setUp()
case.failures['module::a'] = 101
try:
    case.run.execute('run')
finally:
    print('RESULT_JSON=' + json.dumps(case.persisted()))
    case.doCleanups()
"""
        # Popen is not the mocked subprocess.run used by the synthetic Cargo boundary.
        with subprocess.Popen([CI.sys.executable, '-c', script], cwd=ROOT,
                              stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                              text=True) as child:
            stdout, stderr = child.communicate(timeout=30)
        self.assertNotEqual(child.returncode, 0, stderr)
        results = [line.removeprefix('RESULT_JSON=') for line in stdout.splitlines()
                   if line.startswith('RESULT_JSON=')]
        self.assertEqual(len(results), 1, stderr)
        summary = json.loads(results[0])
        self.assertEqual(summary['status'], 'failed')
        self.assertEqual([item['name'] for item in summary['passed']], ['module::b'])
        self.assertEqual(summary['failed'][0]['name'], 'module::a')

    def test_zero_exit_with_invalid_test_evidence_still_fails_and_continues(self):
        original = self.process
        def process(args, **kwargs):
            if '--exact' in args and args[args.index('--exact') + 1] == 'module::a':
                self.calls.append(args)
                kwargs['stdout'].write('test result: ok. 0 passed; 0 failed; 0 ignored; '
                                       '0 measured; 3 filtered out; finished in 0.1s\n')
                return subprocess.CompletedProcess(args, 0)
            return original(args, **kwargs)
        with mock.patch.object(CI.subprocess, 'run', side_effect=process):
            with self.assertRaises(RuntimeError):
                self.run.execute('run')
        summary = self.persisted()
        self.assertEqual(summary['failed'][0]['stage'], 'test')
        self.assertEqual([item['name'] for item in summary['passed']], ['module::b'])

    def test_modified_binary_stops_remaining_tests_of_only_that_target(self):
        original = self.process
        def process(args, **kwargs):
            result = original(args, **kwargs)
            if '--exact' in args:
                Path(args[0]).write_text('replaced executable')
            return result
        check = {'package': 'consumer', 'manifest': 'src/consumer/Cargo.toml', 'mode': 'check'}
        self.plan['rust_targets'].append(check)
        with mock.patch.object(CI.subprocess, 'run', side_effect=process):
            with self.assertRaises(RuntimeError):
                self.run.execute('run')
        summary = self.persisted()
        self.assertEqual(summary['failed'][0]['stage'], 'integrity')
        self.assertEqual(summary['skipped'][0]['names'], ['module::b'])
        self.assertEqual([item['name'] for item in summary['passed']], ['module::a'])
        self.assertEqual(summary['compiled'], [check])

    def test_invalid_plan_stops_before_any_execution(self):
        with mock.patch.object(self.run, 'plan', side_effect=ValueError('invalid selection')):
            with self.assertRaises(ValueError):
                self.run.execute('run')
        self.assertEqual(self.persisted()['status'], 'failed')
        self.assertEqual(self.calls, [])


if __name__ == '__main__':
    unittest.main()
