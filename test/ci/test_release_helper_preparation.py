"""Exercise release-driver requests and teardown; native helper remains a CI gate."""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
from types import SimpleNamespace
import tempfile
import unittest
from unittest.mock import Mock, patch

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location('release_single', ROOT / '.github/scripts/check-debian13-single-binary.py')
GATE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(GATE)


class NativeReplies:
    def __init__(self, failure=None):
        self.calls = []
        self.failure = failure
        self.plan = {'native_plan': 'copied without rewriting'}

    def request(self, path, body):
        self.calls.append((path, body))
        name, action = path.removeprefix('/api/initialization/').rsplit('-', 1)
        if action == 'preview':
            return {'request_digest': name + '-digest', 'plan': self.plan}
        if name == self.failure:
            raise RuntimeError('injected native rejection')
        if name == 'storage':
            return {'format': 'zxs-helper-first-storage-batch-complete',
                    'request_digest': 'storage-digest', 'storage_activation_completed': True}
        states = {'layout': 'new_empty_private_layout_prepared',
                  'administrator': 'first_management_administrator_durably_created',
                  'storage-directories': 'original_empty_storage_directories_durably_prepared'}
        receipt = {'request_digest': name + '-digest'}
        return {'state': states[name], **({'receipt': receipt} if name == 'administrator' else receipt)}


class HelperPreparation(unittest.TestCase):
    def test_public_operations_keep_original_plan_and_do_not_save_password(self):
        helper = NativeReplies()
        with tempfile.TemporaryDirectory() as directory:
            record = Path(directory) / 'helper.json'
            evidence = {}
            with patch.object(GATE.secrets, 'token_urlsafe', return_value='one-test-secret'):
                GATE.initialize_storage(helper, evidence, record)
            self.assertEqual([path.rsplit('/', 1)[1] for path, _ in helper.calls], [
                'layout-preview', 'layout-prepare', 'administrator-preview', 'administrator-create',
                'storage-directories-preview', 'storage-directories-prepare', 'storage-preview', 'storage-create'])
            sent = dict(helper.calls)
            self.assertEqual(sent['/api/initialization/layout-prepare'], {
                'input': {'service_user': 'zxs-engine-test'}, 'request_digest': 'layout-digest',
                'confirmation': 'prepare-private-layout-only'})
            admin = sent['/api/initialization/administrator-create']
            self.assertEqual((admin['password'], admin['repeat_password']), ('one-test-secret', 'one-test-secret'))
            self.assertEqual(admin['confirmation'], 'initialize-first-management-administrator')
            directories = sent['/api/initialization/storage-directories-prepare']
            self.assertEqual(directories['input']['roots'], ['/zxs/data/disks/acceptance-primary'])
            self.assertEqual(directories['confirmation'], 'prepare-first-storage-directories-only')
            storage = sent['/api/initialization/storage-create']
            self.assertIs(storage['plan'], helper.plan)
            self.assertEqual(storage['input'], {'service_user': 'zxs-engine-test', 'request_digest': 'storage-digest'})
            self.assertEqual(storage['confirmation'], 'initialize-first-storage-batch')
            for body in (admin, directories, storage):
                self.assertIs(body['external_writers_stopped'], True)
            self.assertNotIn('one-test-secret', record.read_text())
            self.assertEqual(json.loads(record.read_text()), evidence)

    def test_native_failure_stops_later_operations_and_retains_completed_stage_evidence(self):
        helper = NativeReplies(failure='storage-directories')
        with tempfile.TemporaryDirectory() as directory:
            record = Path(directory) / 'helper.json'
            with self.assertRaisesRegex(RuntimeError, 'native rejection'):
                GATE.initialize_storage(helper, {}, record)
            self.assertEqual(list(json.loads(record.read_text())), ['layout', 'administrator'])
            self.assertEqual(helper.calls[-1][0], '/api/initialization/storage-directories-prepare')
            self.assertEqual(len(helper.calls), 6)

    def test_wrong_receipt_or_incomplete_activation_never_becomes_preparation_proof(self):
        for field, value in [('request_digest', 'other-operation'), ('storage_activation_completed', False)]:
            helper = NativeReplies()
            original = helper.request
            def reply(path, body):
                result = original(path, body)
                if path.endswith('/storage-create'):
                    result[field] = value
                return result
            helper.request = reply
            with self.subTest(field=field), tempfile.TemporaryDirectory() as directory:
                record = Path(directory) / 'helper.json'
                with self.assertRaisesRegex(RuntimeError, 'original first-storage activation'):
                    GATE.initialize_storage(helper, {}, record)
                self.assertNotIn('storage', json.loads(record.read_text()))

    def test_helper_http_preserves_csrf_origin_and_closes_connection(self):
        client = Mock()
        client.getresponse.return_value = SimpleNamespace(status=200, read=lambda _: b'{"ok":true}')
        helper = GATE.Helper(12345)
        helper.csrf = 'test-csrf'
        with patch.object(GATE.http.client, 'HTTPConnection', return_value=client):
            self.assertEqual(helper.request('/api/example', {'input': 'original'}), {'ok': True})
        client.request.assert_called_once_with('POST', '/api/example', '{"input": "original"}', {
            'Content-Type': 'application/json', 'Origin': 'http://127.0.0.1:12345',
            'x-zxs-helper-csrf': 'test-csrf'})
        client.close.assert_called_once()

    def test_failed_initialization_reaps_helper_and_removes_its_executable(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            helper = root / 'delivered-helper'
            helper.write_bytes(b'test-only helper bytes')
            host = root / 'host.json'
            host.write_text(json.dumps({'helper_sha256': hashlib.sha256(helper.read_bytes()).hexdigest()}))
            child = Mock()
            child.poll.return_value = None
            child.wait.return_value = 0
            commands = []
            def start(command, **kwargs):
                commands.append(command)
                self.assertEqual(Path(command[0]).read_bytes(), helper.read_bytes())
                self.assertNotIn('password', ' '.join(command))
                return child
            with patch.object(GATE.os, 'geteuid', return_value=0), \
                    patch.dict(os.environ, {'ZXS_ISOLATED_RELEASE_TEST': '1'}), \
                    patch.object(GATE.pwd, 'getpwnam', return_value=SimpleNamespace(pw_uid=21001, pw_gid=21001)), \
                    patch.object(GATE, 'HOST_PREPARATION', host), \
                    patch.object(GATE, 'free_port', return_value=12345), \
                    patch.object(GATE.subprocess, 'Popen', side_effect=start), \
                    patch.object(GATE.Helper, 'request', return_value={'csrf': 'test-csrf'}), \
                    patch.object(GATE, 'initialize_storage', side_effect=RuntimeError('native failed')):
                with self.assertRaisesRegex(RuntimeError, 'native failed'):
                    GATE.prepare_installation(helper, root / 'receipt.json', root / 'daemon')
            child.terminate.assert_called_once()
            child.wait.assert_called_once_with(timeout=10)
            self.assertEqual(commands[0][1:], ['serve', '--listen', '127.0.0.1:12345'])
            self.assertFalse(Path(commands[0][0]).exists())
            self.assertTrue((root / 'debian13-single-binary-helper.log').exists())
            self.assertFalse((root / 'receipt.json').exists())

    def test_preparation_requires_explicit_isolated_root_driver(self):
        for uid, isolated in [(1000, '1'), (0, '')]:
            with self.subTest(uid=uid), patch.object(GATE.os, 'geteuid', return_value=uid), \
                    patch.dict(os.environ, {'ZXS_ISOLATED_RELEASE_TEST': isolated}):
                with self.assertRaisesRegex(RuntimeError, 'isolated root'):
                    GATE.prepare_installation(Path('not-read'), Path('not-written'), Path('not-run'))

    def test_prepared_image_uses_exact_native_host_digest_then_removes_helper(self):
        text = (ROOT / '.github/containers/debian13-service-runtime.Dockerfile').read_text()
        script = text.split("RUN python3 - <<'PYTHON'\n", 1)[1].split('\nPYTHON', 1)[0]
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            helper = root / 'zxs-helper'
            helper.write_bytes(b'same-release helper')
            checksum = hashlib.sha256(helper.read_bytes()).hexdigest()
            receipt = root / 'host.json'
            def path(value):
                return {'/tmp/zxs-helper': helper, '/opt/zxs-host-preparation.json': receipt, '/zxs': root / 'zxs'}[value]
            with patch('pathlib.Path', side_effect=path), patch('subprocess.check_output', side_effect=[
                    b'{"request_digest":"exact-digest","already_present":true}', b'{"state":"existing_host_root_durably_verified","path":"/zxs","service_exclusion_held_after_response":true}'
            ]) as run:
                exec(compile(script, '<host-preparation>', 'exec'), {})
            self.assertEqual([call.args[0] for call in run.call_args_list], [
                [str(helper), 'host', 'preview'],
                [str(helper), 'host', 'prepare', '--request-digest', 'exact-digest', '--confirm-host-root-only']])
            self.assertEqual(json.loads(receipt.read_text())['helper_sha256'], checksum)
            self.assertFalse(helper.exists())

    def test_alive_web_with_faulted_storage_or_authentication_is_not_ready(self):
        for section, key, value in [('storage', 'state', 'unavailable'),
                                    ('storage', 'recovering', True),
                                    ('top', 'authentication', 'recovery_required')]:
            management = {'management': 'alive', 'authentication': 'ready',
                          'storage': {'state': 'ready', 'recovering': False},
                          'auxiliary': {'web_assets': 'ready'}}
            (management if section == 'top' else management[section])[key] = value
            with self.subTest(section=section, key=key), patch.object(GATE, 'read_json', return_value=management) as read:
                self.assertIsNone(GATE.readiness_snapshot(12345))
                read.assert_called_once_with(12345, '/api/management/status')

    def test_forced_shutdown_cannot_be_a_successful_exit(self):
        child = Mock()
        child.poll.return_value = None
        child.wait.side_effect = [GATE.subprocess.TimeoutExpired('test-child', 10), -9]
        self.assertFalse(GATE.stop(child))
        child.terminate.assert_called_once()
        child.kill.assert_called_once()
        self.assertEqual(child.wait.call_count, 2)

    def test_lease_probe_requires_exact_busy_failure_and_is_bounded(self):
        for code, diagnostic, accepted in [(1, GATE.LEASE_BUSY, True),
                                            (0, GATE.LEASE_BUSY, False),
                                            (1, "configuration invalid", False),
                                            (2, GATE.LEASE_BUSY, False)]:
            with self.subTest(code=code, diagnostic=diagnostic), patch.object(
                    GATE.subprocess, 'run', return_value=SimpleNamespace(
                        returncode=code, stderr=diagnostic)) as run:
                if accepted:
                    receipt = GATE.require_lease_rejection(['test-only-probe'], user=21001)
                    self.assertEqual(receipt, {'exit_code': 1, 'diagnostic': diagnostic})
                else:
                    with self.assertRaisesRegex(RuntimeError, 'expected lifetime lease rejection'):
                        GATE.require_lease_rejection(['test-only-probe'], user=21001)
                run.assert_called_once_with(['test-only-probe'], capture_output=True,
                                            text=True, timeout=10, user=21001)

    def test_probes_use_delivered_private_copies_and_real_service_identity(self):
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory) / 'delivered'
            source.write_bytes(b'exact delivered bytes')
            account = SimpleNamespace(pw_uid=21001, pw_gid=21001)
            commands = []
            def probe(command, **options):
                copied = Path(command[0])
                self.assertEqual(copied.read_bytes(), source.read_bytes())
                self.assertEqual(copied.stat().st_mode & 0o777, 0o700)
                commands.append((command, options))
                return {'exit_code': 1, 'diagnostic': GATE.LEASE_BUSY}
            with patch.object(GATE.os, 'chown') as chown, \
                    patch.object(GATE, 'free_port', return_value=12345), \
                    patch.object(GATE, 'require_lease_rejection', side_effect=probe):
                GATE.reject_daemon_while_helper(source, account)
                GATE.reject_helper_while_daemon(source)
            self.assertEqual(chown.call_count, 2)
            command, options = commands[0]
            self.assertEqual(command[1:], ['--root', '/zxs/data/state', '--management-root',
                '/zxs/data/management', '--storage-discovery-root', '/zxs/data/disks',
                '--listen', '127.0.0.1:12345'])
            self.assertEqual((options['user'], options['group'], options['extra_groups']),
                             (21001, 21001, []))
            self.assertEqual(commands[1][0][1:], ['serve', '--listen', '127.0.0.1:12345'])
            self.assertNotIn('user', commands[1][1])
            self.assertTrue(all(not Path(command[0]).exists() for command, _ in commands))

    def test_runtime_remains_offline_and_browser_uses_prepared_image(self):
        script = (ROOT / '.github/scripts/check-debian13-release.sh').read_text()
        self.assertIn('docker build --network none', script)
        self.assertIn('docker run --rm --init --network none --env ZXS_ISOLATED_RELEASE_TEST=1', script)
        self.assertIn('--helper "release-assets/zxs-helper-${RELEASE_VERSION}-debian13-amd64"', script)
        self.assertNotIn('--privileged', script)
        browser = (ROOT / '.github/scripts/release-acceptance.sh').read_text()
        self.assertEqual(browser.count('--server-container-image zxs-debian13-service-runtime'), 2)
        self.assertNotIn('mkdir /zxs', script + browser)


if __name__ == '__main__':
    unittest.main()
