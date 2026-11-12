"""Keep pure-encoding reuse covered without relaxing Backup's original budget."""
import json
import subprocess
import tempfile
from unittest import mock
from pathlib import Path
import unittest

from test_targeted_ci import CI

ROOT = Path(__file__).resolve().parents[2]
PATHS = [
    'src/engine/src/engine/backup_preparation/native_control/generation_objects.rs',
    'src/engine/src/journal/retained_encoding.rs',
    'test/engine/unit/journal/retained_encoding_tests.rs',
    'src/engine/src/engine/backup_preparation/native_control.rs',
    'src/engine/src/engine/backup_preparation/native_control/databases.rs',
    'src/engine/src/engine/backup_preparation/native_control/databases/scoped/encoding_memo.rs',
    'src/engine/src/engine/backup_preparation/native_control/databases/scoped.rs',
    'src/engine/src/engine/backup_preparation/native_control/databases/scoped/projection.rs',
    'src/engine/src/engine/backup_preparation/native_control/databases/scoped/cutover.rs',
    'src/engine/src/engine/backup_preparation/native_control/databases/scoped/cutover/installation.rs',
    'test/engine/unit/backup_installation_encoding_tests.rs',
    'test/engine/unit/metadata_map/read_owned_tests.rs',
    'test/engine/unit/backup_published_scope_context_tests.rs',
    'test/engine/unit/backup_original_scope_encoding_tests.rs',
    'src/engine/src/engine/backup_preparation/native_control/databases/scoped/cutover/registration/first_candidate.rs',
    'test/engine/unit/backup_first_candidate_cost.rs',
    'src/engine/src/engine/backup_preparation/native_control/databases/scoped/cutover/registration/serving.rs',
    'test/engine/unit/backup_serving_cost_tests.rs',
    '.github/scripts/backup-process-sampling.py',
    'src/engine/src/engine/backup_preparation/native_control/databases/scoped/cutover/registration/published_scopes/projection/timing.rs',
    "src/engine/src/engine/backup_preparation/native_control/databases/scoped/cutover/registration.rs",
    "src/engine/src/engine/backup_preparation/native_control/databases/scoped/cutover/registration/runtime.rs",
    "src/engine/src/engine/backup_preparation/native_control/databases/scoped/cutover/registration/published_scopes.rs",
    "src/engine/src/engine/backup_preparation/native_control/databases/scoped/cutover/registration/published_scopes/projection.rs",
    "src/engine/src/engine/backup_preparation/native_control/databases/scoped/cutover/registration/published_scopes/encoding_memo.rs",
    "test/engine/unit/backup_scope_encoding_memo_tests.rs",
    "src/engine/src/engine/index/preparation.rs",
    "src/engine/src/engine/disk_catalog/history/preparation.rs",
    "test/engine/unit/engine/index/preparation_readback_tests.rs",
    "test/engine/unit/engine/disk_scope_encoding_tests.rs"
]

class BackupEncodingMemoSelection(unittest.TestCase):
    def setUp(self):
        self.policy = json.loads((ROOT / CI.PLAN_PATH).read_text())

    def test_each_encoder_cache_and_fixture_selects_the_closed_regression_group(self):
        for path in PATHS + [
            'docs/validation/backup-scoped-encoding-memo.md',
            'test/ci/test_backup_encoding_memo_ci.py',
        ]:
            with self.subTest(path=path):
                plan = CI.select_plan(self.policy, [path])
                self.assertIn('backup-scoped-encoding-memo', plan['groups'])
                self.assertIn('test_backup_encoding_memo_ci', plan['python_tests'])

    def test_graph_decoder_consumers_keep_backup_and_existing_shared_coverage(self):
        for path in [
            'src/engine/src/journal.rs',
            'src/engine/src/metadata_map.rs',
            'src/engine/src/history_package/graph.rs',
            'test/engine/unit/history_package/graph/tests.rs',
        ]:
            with self.subTest(path=path):
                plan = CI.select_plan(self.policy, [path])
                self.assertIn('backup-scoped-encoding-memo', plan['groups'])
        group = self.policy['groups']['backup-scoped-encoding-memo']
        self.assertIn('metadata_map::read_owned_tests::',
                      group['rust_targets'][0]['prefixes'])
        journal = CI.select_plan(self.policy, ['src/engine/src/journal.rs'])
        self.assertIn('version-publication-location', journal['groups'])
        self.assertIn('held-directory-layout', journal['groups'])
        metadata = CI.select_plan(self.policy, ['src/engine/src/metadata_map.rs'])
        self.assertIn('container-tags-storage', metadata['groups'])
        graph = CI.select_plan(self.policy, ['src/engine/src/history_package/graph.rs'])
        self.assertIn('container-tags-storage', graph['groups'])

    def test_complete_slice_keeps_product_check_before_original_900_second_transaction(self):
        plan = CI.select_plan(self.policy, PATHS + [
            'docs/validation/backup-scoped-encoding-memo.md',
            'test/ci/test_backup_encoding_memo_ci.py',
            '.github/ci/targeted-tests.json',
            '.github/scripts/check-targeted-ci.py',
            'TASKS.md',
        ])
        self.assertEqual(plan['groups'], ['backup-scoped-encoding-memo', 'ci-selection'])
        self.assertEqual(len(plan['rust_targets']), 3)
        self.assertEqual(len(plan['python_tests']), 8)
        group = self.policy['groups']['backup-scoped-encoding-memo']
        unit, product, transaction = group['rust_targets']
        self.assertIn('engine::index::preparation::tests::', unit['prefixes'])
        self.assertIn('journal::retained_encoding::tests::', unit['prefixes'])
        self.assertIn('engine::disk_catalog::history::preparation::encoding_tests::',
                      unit['prefixes'])
        self.assertIn('container_storage::backup_original_objects::tests::',
                      unit['prefixes'])
        self.assertIn('container_storage::backup_entities::budget_tests::',
                      unit['prefixes'])
        self.assertIn(
            'engine::backup_preparation::native_control::databases::scoped::cutover::'
            'registration::serving::cost_tests::', unit['prefixes'])
        self.assertIn(
            'engine::backup_preparation::native_control::databases::scoped::'
            'encoding_memo::tests::', unit['prefixes'])
        self.assertIn(
            'engine::backup_preparation::native_control::databases::scoped::cutover::'
            'registration::original_scope_encoding_tests::', unit['prefixes'])
        self.assertIn(
            'engine::backup_preparation::native_control::databases::scoped::cutover::'
            'installation::encoding_tests::', unit['prefixes'])
        self.assertEqual(product['mode'], 'check')
        self.assertFalse(product['default_features'])
        self.assertEqual(transaction['name'], 'recursive_backup_paths')
        self.assertEqual(transaction['exact'], [
            'backup_preparation::transaction_serving::'
            'same_backup_switch_serves_and_replays_normal_successor_publications'])
        self.assertEqual(transaction['max_tests'], 1)
        self.assertEqual(transaction['timeout_seconds'], 900)


    def test_archive_decode_keeps_existing_admission_and_backup_budgets(self):
        archive = ('src/engine/src/engine/backup_preparation/'
                   'native_control/archive.rs')
        fixture = 'test/engine/unit/backup_native_decode_tests.rs'
        prefix = 'engine::backup_preparation::native_control::archive::decode_tests::'
        for path in (archive, fixture):
            with self.subTest(path=path):
                plan = CI.select_plan(self.policy, [path])
                self.assertIn('backup-scoped-encoding-memo', plan['groups'])
                self.assertIn('test_backup_encoding_memo_ci', plan['python_tests'])
        plan = CI.select_plan(self.policy, [archive, fixture])
        self.assertIn('container-tags-format', plan['groups'])
        self.assertIn('held-directory-layout', plan['groups'])
        group = self.policy['groups']['backup-scoped-encoding-memo']
        unit, product, transaction = group['rust_targets']
        self.assertIn(prefix, unit['prefixes'])
        self.assertEqual(unit['max_tests'], 96)
        self.assertEqual(unit['timeout_seconds'], 900)
        self.assertEqual(product['mode'], 'check')
        self.assertFalse(product['default_features'])
        self.assertTrue(transaction['isolate'])
        self.assertEqual(transaction['max_tests'], 1)
        self.assertEqual(transaction['timeout_seconds'], 900)
        self.assertEqual(transaction['exact'], [
            'backup_preparation::transaction_serving::'
            'same_backup_switch_serves_and_replays_normal_successor_publications'])


# External diagnostics tests: these do not contribute to Rust test outcomes.
import importlib.util as _sampling_import
import io as _sampling_io
import os as _sampling_os
import sys as _sampling_sys
import tempfile as _sampling_tempfile
from types import SimpleNamespace as _SamplingValue
from unittest import mock as _sampling_mock

_sampling_spec = _sampling_import.spec_from_file_location(
    'backup_process_sampling', ROOT / '.github/scripts/backup-process-sampling.py')
_sampling = _sampling_import.module_from_spec(_sampling_spec)
_sampling_spec.loader.exec_module(_sampling)


class BackupProcessSampling(unittest.TestCase):
    def run_fake(self, child, clock=None, observer=None):
        clock = clock or _SamplingValue(now=0.0)
        observer = observer or _sampling_mock.Mock()
        with _sampling_mock.patch.object(
                _sampling.subprocess, 'Popen', return_value=child) as launch, \
             _sampling_mock.patch.object(
                 _sampling.time, 'monotonic', side_effect=lambda: clock.now), \
             _sampling_mock.patch.object(_sampling, 'Observer', return_value=observer):
            result = _sampling.run(['/verified/test', '--exact', 'the_test'],
                cwd='/repo', env={'A': 'B'}, stdout='OUT', stderr='ERR',
                timeout=900, diagnostic_path='observation.log')
        launch.assert_called_once_with(['/verified/test', '--exact', 'the_test'],
            cwd='/repo', env={'A': 'B'}, stdout='OUT', stderr='ERR')
        return result

    def test_normal_and_nonzero_results_are_unmodified(self):
        for code in (0, 7, -9):
            child = _sampling_mock.Mock(pid=123)
            child.wait.return_value = code
            result = self.run_fake(child)
            self.assertEqual(result.returncode, code)
            self.assertEqual(result.args, ['/verified/test', '--exact', 'the_test'])
            self.assertIsNone(result.stdout)
            self.assertIsNone(result.stderr)
            child.kill.assert_not_called()

    def test_timeout_keeps_one_deadline_and_reaps_direct_child(self):
        clock = _SamplingValue(now=0.0)
        child = _sampling_mock.Mock(pid=123)
        child.poll.return_value = None
        waits = []
        def wait(timeout=None):
            if timeout is None:
                return -9
            waits.append(timeout)
            clock.now += timeout
            raise _sampling.subprocess.TimeoutExpired('child', timeout)
        child.wait.side_effect = wait
        with self.assertRaises(_sampling.subprocess.TimeoutExpired) as error:
            self.run_fake(child, clock)
        self.assertEqual(error.exception.timeout, 900)
        self.assertEqual(error.exception.cmd, ['/verified/test', '--exact', 'the_test'])
        self.assertEqual(clock.now, 900)
        self.assertEqual(sum(waits), 900)
        self.assertTrue(all(0 < value <= 5 for value in waits))
        child.kill.assert_called_once_with()
        self.assertEqual(child.wait.call_args, _sampling_mock.call())

    def test_diagnostic_time_consumes_not_extends_test_budget(self):
        clock = _SamplingValue(now=0.0)
        child = _sampling_mock.Mock(pid=123)
        child.poll.return_value = None
        observer = _sampling_mock.Mock()
        observer.sample.side_effect = lambda: setattr(clock, 'now', clock.now + 899)
        def wait(timeout=None):
            if timeout is None:
                return -9
            clock.now += timeout
            raise _sampling.subprocess.TimeoutExpired('child', timeout)
        child.wait.side_effect = wait
        with self.assertRaises(_sampling.subprocess.TimeoutExpired):
            self.run_fake(child, clock, observer)
        self.assertEqual(clock.now, 900)
        self.assertEqual(child.wait.call_args_list[0], _sampling_mock.call(timeout=1))

    def test_sampling_error_cannot_mask_nonzero_result(self):
        child = _sampling_mock.Mock(pid=123)
        child.wait.return_value = 23
        observer = _sampling_mock.Mock()
        observer.sample.side_effect = RuntimeError('diagnostics unavailable')
        self.assertEqual(self.run_fake(child, observer=observer).returncode, 23)
        child.kill.assert_not_called()

    def test_unavailable_observer_cannot_mask_success(self):
        child = _sampling_mock.Mock(pid=123)
        child.wait.return_value = 0
        with _sampling_mock.patch.object(_sampling.subprocess, 'Popen', return_value=child), \
             _sampling_mock.patch.object(_sampling, 'Observer', side_effect=OSError):
            result = _sampling.run(['/verified/test'], cwd='/repo', env={},
                stdout=None, stderr=None, timeout=900, diagnostic_path='x')
        self.assertEqual(result.returncode, 0)

    def test_launch_failure_is_not_replaced(self):
        with _sampling_mock.patch.object(_sampling.subprocess, 'Popen',
                side_effect=FileNotFoundError('missing binary')):
            with self.assertRaises(FileNotFoundError):
                _sampling.run(['/verified/test'], cwd='/repo', env={},
                    stdout=None, stderr=None, timeout=900, diagnostic_path='x')

    def test_stat_parser_excludes_arbitrary_comm_and_uses_exact_fields(self):
        fields = ['S'] + ['0'] * 21
        for index, value in ((11, '12'), (12, '13'), (17, '3'), (19, '999'), (21, '88')):
            fields[index] = value
        text = '123 (private name ) with ( parens)) ' + ' '.join(fields) + '\n'
        result = _sampling.parse_stat(text)
        self.assertEqual(result, dict(pid=123, state='S', user_ticks=12,
            system_ticks=13, threads=3, start_ticks=999, rss_pages=88))
        self.assertNotIn('private', str(result))
        with self.assertRaises(ValueError):
            _sampling.parse_stat('123 (short) S 0\n')

    def test_bounded_read_short_denied_gone_and_truncation(self):
        for data, expected in ((b'x\n', 'ok'), (b'x', 'short'), (b'', 'short'),
                               (b'x' * (_sampling.READ_LIMIT + 1), 'truncated')):
            with _sampling_mock.patch('builtins.open', return_value=_sampling_io.BytesIO(data)):
                _, status = _sampling.read_text('unused')
            self.assertEqual(status, expected)
        for error, expected in ((PermissionError, 'denied'), (FileNotFoundError, 'gone'),
                                (OSError, 'unavailable')):
            with _sampling_mock.patch('builtins.open', side_effect=error):
                self.assertEqual(_sampling.read_text('unused'), (None, expected))

    def test_optional_numeric_fields_reject_missing_or_malformed_data(self):
        with _sampling_mock.patch.object(_sampling, 'read_text', return_value=('1 2 3\n', 'ok')):
            self.assertEqual(_sampling.optional_record('unused', 'schedstat'),
                dict(status='ok', cpu_ns=1, wait_ns=2, slices=3))
        with _sampling_mock.patch.object(_sampling, 'read_text', return_value=('rchar: 2\n', 'ok')):
            self.assertEqual(_sampling.optional_record('unused', 'io'), dict(status='malformed'))

    def test_pid_binding_and_changed_identity_rejected_without_thread_scan(self):
        for pid, inode, previous in ((124, 1, None), (123, 2, None),
                                     (123, 1, (123, 998, 1, 1))):
            observer = _sampling.Observer(123, '/verified/test', 'x')
            observer.identity = previous
            observer.emit = _sampling_mock.Mock()
            with _sampling_mock.patch.object(_sampling, 'stat_record',
                    return_value=(dict(pid=pid, start_ticks=999), 'ok')), \
                 _sampling_mock.patch.object(_sampling.os, 'stat', side_effect=[
                    _SamplingValue(st_dev=1, st_ino=inode), _SamplingValue(st_dev=1, st_ino=1)]), \
                 _sampling_mock.patch.object(_sampling.os, 'scandir') as scan:
                observer.sample()
            self.assertEqual(observer.emit.call_args.args[0]['status'], 'identity-mismatch')
            scan.assert_not_called()

    def test_threads_and_sample_count_are_bounded(self):
        observer = _sampling.Observer(123, '/verified/test', 'x')
        observer.emit = _sampling_mock.Mock()
        entries = _sampling_mock.MagicMock()
        entries.__enter__.return_value = iter(
            [_SamplingValue(name=str(number)) for number in range(1000, 1100)])
        def stat(path):
            pid = 123 if str(path) == '/proc/123/stat' else int(path.parent.name)
            return dict(pid=pid, start_ticks=999), 'ok'
        with _sampling_mock.patch.object(_sampling, 'stat_record', side_effect=stat), \
             _sampling_mock.patch.object(_sampling.os, 'stat',
                return_value=_SamplingValue(st_dev=1, st_ino=1)), \
             _sampling_mock.patch.object(_sampling.os, 'scandir', return_value=entries), \
             _sampling_mock.patch.object(_sampling, 'optional_record',
                return_value=dict(status='denied')), \
             _sampling_mock.patch.object(_sampling, 'wchan_record',
                return_value=dict(status='denied')):
            observer.sample()
        row = observer.emit.call_args.args[0]
        self.assertEqual(row['status'], 'ok')
        self.assertEqual(len(row['threads']), 64)
        self.assertTrue(row['threads_truncated'])
        observer.samples = _sampling.MAX_SAMPLES
        observer.emit.reset_mock()
        observer.sample()
        observer.emit.assert_not_called()

    def test_missing_proc_and_log_write_errors_are_diagnostic_only(self):
        observer = _sampling.Observer(123, '/verified/test', 'x')
        observer.emit = _sampling_mock.Mock()
        with _sampling_mock.patch.object(_sampling, 'stat_record', return_value=(None, 'gone')):
            observer.sample()
        self.assertEqual(observer.emit.call_args.args[0]['status'], 'gone')
        with _sampling_mock.patch('builtins.open', side_effect=PermissionError):
            _sampling.Observer(123, '/verified/test', 'x').emit(dict(status='denied'))

    def test_unexpected_wait_error_and_keyboard_interrupt_reap_then_propagate(self):
        for error in (RuntimeError('wait failed'), KeyboardInterrupt()):
            child = _sampling_mock.Mock(pid=123)
            child.wait.side_effect = [error, -9]
            with self.assertRaises(type(error)):
                self.run_fake(child)
            child.kill.assert_called_once_with()
            self.assertEqual(child.wait.call_args, _sampling_mock.call())

    def test_sampling_failure_does_not_mask_timeout(self):
        clock = _SamplingValue(now=899.0)
        child = _sampling_mock.Mock(pid=123)
        child.poll.return_value = None
        observer = _sampling_mock.Mock()
        def sample():
            clock.now += 900
            raise PermissionError('sample failed')
        observer.sample.side_effect = sample
        child.wait.return_value = -9
        with self.assertRaises(_sampling.subprocess.TimeoutExpired) as error:
            self.run_fake(child, clock, observer)
        self.assertEqual(error.exception.timeout, 900)
        child.kill.assert_called_once_with()

    def test_wchan_without_newline_is_bounded_and_symbol_only(self):
        for data, expected in (
                (b'futex_wait_queue', dict(status='ok', symbol='futex_wait_queue')),
                (b'io_schedule\n', dict(status='ok', symbol='io_schedule')),
                (b'0', dict(status='ok', symbol='0')),
                (b'', dict(status='short')),
                (b'x' * 129, dict(status='truncated')),
                (b'private/path', dict(status='malformed')),
                (b'word another', dict(status='malformed'))):
            with _sampling_mock.patch('builtins.open',
                                      return_value=_sampling_io.BytesIO(data)):
                self.assertEqual(_sampling.wchan_record('unused'), expected)
        for error, status in ((PermissionError, 'denied'), (FileNotFoundError, 'gone')):
            with _sampling_mock.patch('builtins.open', side_effect=error):
                self.assertEqual(_sampling.wchan_record('unused'), dict(status=status))

    def test_elapsed_and_clock_rate_are_explicit(self):
        with _sampling_mock.patch.object(_sampling.time, 'monotonic',
                                        side_effect=[100.0, 102.5]), \
             _sampling_mock.patch.object(_sampling.os, 'sysconf', return_value=100) as rate:
            observer = _sampling.Observer(123, '/verified/test', 'x')
            observer.emit = _sampling_mock.Mock()
            with _sampling_mock.patch.object(_sampling, 'stat_record',
                                            return_value=(None, 'gone')):
                observer.sample()
        row = observer.emit.call_args.args[0]
        self.assertEqual(row['elapsed_seconds'], 2.5)
        self.assertEqual(row['clock_ticks'], 100)
        self.assertEqual(row['clock_status'], 'ok')
        rate.assert_called_once_with('SC_CLK_TCK')
        with _sampling_mock.patch.object(_sampling.os, 'sysconf', side_effect=ValueError):
            observer = _sampling.Observer(123, '/verified/test', 'x')
        self.assertIsNone(observer.clock_ticks)
        self.assertEqual(observer.clock_status, 'unavailable')

    def test_post_sample_exec_change_or_exception_discards_partial_fields(self):
        for after, status in (
                (_SamplingValue(st_dev=1, st_ino=2), 'changed-during-sample'),
                (FileNotFoundError(), 'gone'),
                (PermissionError(), 'denied'),
                (OSError(), 'unavailable')):
            observer = _sampling.Observer(123, '/verified/test', 'x')
            observer.emit = _sampling_mock.Mock()
            entries = _sampling_mock.MagicMock()
            entries.__enter__.return_value = iter([])
            with _sampling_mock.patch.object(_sampling, 'stat_record',
                    return_value=(dict(pid=123, start_ticks=999), 'ok')), \
                 _sampling_mock.patch.object(_sampling.os, 'stat', side_effect=[
                    _SamplingValue(st_dev=1, st_ino=1),
                    _SamplingValue(st_dev=1, st_ino=1), after]), \
                 _sampling_mock.patch.object(_sampling.os, 'scandir', return_value=entries), \
                 _sampling_mock.patch.object(_sampling, 'optional_record',
                    return_value=dict(status='ok', read_bytes=123)):
                observer.sample()
            row = observer.emit.call_args.args[0]
            self.assertEqual(row['status'], status)
            self.assertIsNone(row['process'])
            self.assertIsNone(row['io'])
            self.assertEqual(row['threads'], [])

    def test_post_sample_lost_stat_discards_partial_fields(self):
        observer = _sampling.Observer(123, '/verified/test', 'x')
        observer.emit = _sampling_mock.Mock()
        entries = _sampling_mock.MagicMock()
        entries.__enter__.return_value = iter([])
        with _sampling_mock.patch.object(_sampling, 'stat_record', side_effect=[
                (dict(pid=123, start_ticks=999), 'ok'), (None, 'denied')]), \
             _sampling_mock.patch.object(_sampling.os, 'stat',
                return_value=_SamplingValue(st_dev=1, st_ino=1)), \
             _sampling_mock.patch.object(_sampling.os, 'scandir', return_value=entries), \
             _sampling_mock.patch.object(_sampling, 'optional_record',
                return_value=dict(status='ok', read_bytes=123)):
            observer.sample()
        row = observer.emit.call_args.args[0]
        self.assertEqual(row['status'], 'denied')
        self.assertIsNone(row['process'])
        self.assertIsNone(row['io'])
        self.assertEqual(row['threads'], [])

    @unittest.skipUnless(_sampling_sys.platform.startswith('linux'), 'Linux proc regression')
    def test_real_linux_child_preserves_output_and_emits_bound_proc_observation(self):
        with _sampling_tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            log = root / 'sampling.log'
            environment = dict(_sampling_os.environ, SAMPLING_SECRET='do-not-log-this-value')
            with (root / 'stdout').open('w+') as out, (root / 'stderr').open('w+') as err:
                result = _sampling.run([
                    _sampling_sys.executable, '-c',
                    'import time; print("original-child-output", flush=True); time.sleep(1)'],
                    cwd=root, env=environment, stdout=out, stderr=err,
                    timeout=10, diagnostic_path=log)
                out.seek(0)
                self.assertEqual(out.read(), 'original-child-output\n')
                self.assertEqual(result.returncode, 0)
            text = log.read_text()
            rows = [json.loads(line) for line in text.splitlines()]
            valid = [row for row in rows if row['status'] == 'ok']
            self.assertTrue(valid)
            for row in valid:
                self.assertGreater(row['process']['pid'], 0)
                self.assertGreater(row['process']['start_ticks'], 0)
                self.assertGreater(row['clock_ticks'], 0)
                self.assertGreaterEqual(row['elapsed_seconds'], 0)
                self.assertNotIn('comm', row['process'])
            self.assertNotIn('do-not-log-this-value', text)
            self.assertNotIn('original-child-output', text)
            self.assertNotIn('SAMPLING_SECRET', text)

    def test_perf_presence_and_policy_are_read_only_not_permission_claims(self):
        with _sampling_mock.patch.object(_sampling.shutil, 'which',
                return_value='/not-retained/perf') as which, \
             _sampling_mock.patch.object(_sampling, 'read_text',
                return_value=('4\n', 'ok')) as read:
            value = _sampling.perf_availability()
        self.assertEqual(value, dict(tool_present=True, paranoid=4, status='ok'))
        which.assert_called_once_with('perf')
        read.assert_called_once_with('/proc/sys/kernel/perf_event_paranoid')
        self.assertNotIn('/not-retained', str(value))
        with _sampling_mock.patch.object(_sampling.shutil, 'which', return_value=None), \
             _sampling_mock.patch.object(_sampling, 'read_text',
                return_value=(None, 'denied')):
            self.assertEqual(_sampling.perf_availability(),
                dict(tool_present=False, paranoid=None, status='denied'))

    def test_sample_crossing_deadline_cannot_admit_late_zero_exit(self):
        clock = _SamplingValue(now=0.0)
        child = _sampling_mock.Mock(pid=123)
        child.poll.return_value = 0
        child.wait.return_value = 0
        observer = _sampling_mock.Mock()
        observer.sample.side_effect = lambda: setattr(clock, 'now', 901.0)
        with self.assertRaises(_sampling.subprocess.TimeoutExpired) as error:
            self.run_fake(child, clock, observer)
        self.assertEqual(error.exception.timeout, 900)
        child.poll.assert_not_called()
        child.kill.assert_called_once_with()
        child.wait.assert_called_once_with()

    def test_wait_returning_zero_after_absolute_deadline_still_times_out(self):
        clock = _SamplingValue(now=0.0)
        child = _sampling_mock.Mock(pid=123)
        def wait(timeout=None):
            if timeout is not None:
                clock.now = 901.0
            return 0
        child.wait.side_effect = wait
        with self.assertRaises(_sampling.subprocess.TimeoutExpired) as error:
            self.run_fake(child, clock)
        self.assertEqual(error.exception.timeout, 900)
        child.kill.assert_called_once_with()
        self.assertEqual(child.wait.call_args, _sampling_mock.call())

    def test_thread_exit_or_reuse_discards_thread_fields_only(self):
        for after, expected in (
                ((None, 'gone'), 'gone'),
                ((None, 'denied'), 'denied'),
                ((dict(pid=1000, start_ticks=222), 'ok'), 'changed-during-sample'),
                ((dict(pid=1001, start_ticks=111), 'ok'), 'changed-during-sample')):
            observer = _sampling.Observer(123, '/verified/test', 'x')
            observer.emit = _sampling_mock.Mock()
            entries = _sampling_mock.MagicMock()
            entries.__enter__.return_value = iter([_SamplingValue(name='1000')])
            process = dict(pid=123, start_ticks=999)
            thread = dict(pid=1000, start_ticks=111)
            with _sampling_mock.patch.object(_sampling, 'stat_record', side_effect=[
                    (process, 'ok'), (thread, 'ok'), after, (process, 'ok')]), \
                 _sampling_mock.patch.object(_sampling.os, 'stat',
                    return_value=_SamplingValue(st_dev=1, st_ino=1)), \
                 _sampling_mock.patch.object(_sampling.os, 'scandir', return_value=entries), \
                 _sampling_mock.patch.object(_sampling, 'optional_record',
                    return_value=dict(status='ok', cpu_ns=123)), \
                 _sampling_mock.patch.object(_sampling, 'wchan_record',
                    return_value=dict(status='ok', symbol='futex_wait_queue')):
                observer.sample()
            row = observer.emit.call_args.args[0]
            self.assertEqual(row['status'], 'ok')
            self.assertEqual(row['process'], process)
            self.assertEqual(row['threads'], [dict(
                tid=1000, status=expected, stat=None, schedstat=None, wchan=None)])


class BackupProcessSamplingWiring(unittest.TestCase):
    def test_sampling_is_only_for_the_exact_current_backup_test_configuration(self):
        target = {
            'package': 'storage-engine',
            'manifest': 'src/engine/Cargo.toml',
            'kind': 'test',
            'name': 'recursive_backup_paths',
        }
        name = ('backup_preparation::transaction_serving::'
                'same_backup_switch_serves_and_replays_normal_successor_publications')
        self.assertTrue(CI.observe_backup_process(target, name))
        self.assertFalse(CI.observe_backup_process(target, name + '_other'))
        for field, value in (
            ('package', 'other'),
            ('manifest', 'other/Cargo.toml'),
            ('kind', 'lib'),
            ('name', 'other'),
            ('mode', 'check'),
        ):
            with self.subTest(field=field):
                changed = dict(target, **{field: value})
                self.assertFalse(CI.observe_backup_process(changed, name))

    def test_sampled_command_keeps_original_result_and_does_not_execute_twice(self):
        with tempfile.TemporaryDirectory() as directory:
            run = CI.Run(directory)
            arguments = ['/compiled/exact-test', '--exact', 'original', '--test-threads=1']

            def execute(args, **kwargs):
                self.assertEqual(args, arguments)
                self.assertEqual(kwargs['timeout'], 900)
                self.assertEqual(kwargs['cwd'], Path(directory))
                self.assertIs(kwargs['env'], run.environment)
                self.assertTrue(str(kwargs['diagnostic_path']).endswith('.sampling.log'))
                kwargs['stdout'].write('real test output')
                return subprocess.CompletedProcess(args, 0)

            observer = mock.Mock()
            observer.run.side_effect = execute
            with mock.patch.object(CI, 'backup_process_sampler', return_value=observer), \
                    mock.patch.object(CI.subprocess, 'run') as original:
                self.assertEqual(run.command(arguments, 900, observe_process=True),
                                 'real test output')
            observer.run.assert_called_once()
            original.assert_not_called()
            record = run.summary['commands'][0]
            self.assertEqual(record['command'], arguments)
            self.assertEqual(record['status'], 'passed')
            self.assertIn('not test success evidence', record['process_sampling']['role'])
            self.assertEqual(run.summary['passed'], [])

    def test_observed_failure_or_timeout_is_not_retried_or_converted_to_success(self):
        arguments = ['/compiled/exact-test', '--exact', 'original', '--test-threads=1']
        outcomes = [
            (subprocess.CompletedProcess(arguments, 7), subprocess.CalledProcessError, 'failed'),
            (subprocess.TimeoutExpired(arguments, 900), subprocess.TimeoutExpired, 'timed-out'),
        ]
        for outcome, error, status in outcomes:
            with self.subTest(status=status), tempfile.TemporaryDirectory() as directory:
                run = CI.Run(directory)
                observer = mock.Mock()
                if isinstance(outcome, Exception):
                    observer.run.side_effect = outcome
                else:
                    observer.run.return_value = outcome
                with mock.patch.object(CI, 'backup_process_sampler', return_value=observer), \
                        mock.patch.object(CI.subprocess, 'run') as original:
                    with self.assertRaises(error):
                        run.command(arguments, 900, observe_process=True)
                observer.run.assert_called_once()
                original.assert_not_called()
                self.assertEqual(run.summary['commands'][0]['status'], status)
                self.assertEqual(run.summary['passed'], [])

if __name__ == '__main__':
    unittest.main()
