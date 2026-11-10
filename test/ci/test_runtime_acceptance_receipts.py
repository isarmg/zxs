"""Receipt lifecycle only; process/HTTP doubles do not prove runtime acceptance."""
from contextlib import ExitStack
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


def load_gate(name):
    spec = importlib.util.spec_from_file_location(name, ROOT / '.github/scripts' / f'{name}.py')
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


SAMBA = load_gate('check-system-samba')
SINGLE = load_gate('check-debian13-single-binary')


def reject_before_runtime(gate, output):
    if gate is SAMBA:
        with patch.dict(os.environ, {}, clear=True):
            SAMBA.run(SimpleNamespace(output=output))
    else:
        with patch('sys.argv', ['check-debian13-single-binary.py', '--binary', 'unused',
                                '--helper', 'unused', '--web', 'unused', '--output', str(output)]), \
                patch.object(SINGLE.platform, 'freedesktop_os_release', return_value={'ID': 'other'}):
            SINGLE.main()


class RuntimeAcceptanceReceipts(unittest.TestCase):
    def test_precondition_failure_invalidates_only_named_receipt(self):
        for gate in (SAMBA, SINGLE):
            with self.subTest(gate=gate.__name__), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                output = root / 'receipt.json'
                output.write_text('{"previous": "success"}\n')
                unrelated = root / 'other-evidence.json'
                unrelated.write_text('preserve unrelated evidence')
                with self.assertRaisesRegex(RuntimeError, 'requires'):
                    reject_before_runtime(gate, output)
                self.assertFalse(output.exists())
                self.assertEqual(unrelated.read_text(), 'preserve unrelated evidence')

    def test_receipt_symlink_is_removed_without_following_its_target(self):
        for gate in (SAMBA, SINGLE):
            with self.subTest(gate=gate.__name__), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                target = root / 'unrelated.json'
                target.write_text('preserve symlink target')
                output = root / 'receipt.json'
                output.symlink_to(target)
                with self.assertRaisesRegex(RuntimeError, 'requires'):
                    reject_before_runtime(gate, output)
                self.assertFalse(output.is_symlink())
                self.assertEqual(target.read_text(), 'preserve symlink target')

    def test_receipt_directory_is_rejected_without_removing_contents(self):
        for gate in (SAMBA, SINGLE):
            with self.subTest(gate=gate.__name__), tempfile.TemporaryDirectory() as directory:
                output = Path(directory) / 'receipt.json'
                output.mkdir()
                (output / 'preserve').write_text('not a receipt')
                with self.assertRaises(IsADirectoryError):
                    reject_before_runtime(gate, output)
                self.assertEqual((output / 'preserve').read_text(), 'not a receipt')

    def test_samba_midrun_failure_keeps_logs_without_success_receipt(self):
        with tempfile.TemporaryDirectory() as directory, ExitStack() as stack:
            root = Path(directory)
            output = root / 'receipt.json'
            output.write_text('{"previous": "success"}\n')
            modules = root / 'modules'
            (modules / 'vfs').mkdir(parents=True)
            module = modules / 'vfs/zxs.so'
            module.write_bytes(b'unit-test module placeholder')
            target = json.loads((ROOT / '.github/samba-debian-target.json').read_text())
            users = {
                name: SimpleNamespace(pw_uid=uid, pw_gid=uid, pw_name=name)
                for name, uid in (('zxs-engine-test', 21001),
                                  ('zxs-smb-test', 21002),
                                  ('zxs-smb-next-test', 21003))
            }

            def command_output(command, **_):
                if command[0] == 'dpkg-query':
                    return target['package_version']
                if command == ['smbd', '-b']:
                    return f'MODULESDIR: {modules}\n'
                self.assertEqual(command, ['smbd', '--version'])
                return 'unit-test Samba version'

            def fail_after_start(path, _process):
                self.assertFalse(output.exists())
                (path.parent.parent / 'engine.log').write_text('current fixture failed')
                raise RuntimeError('injected fixture startup failure')

            stack.enter_context(patch.dict(os.environ, {'ZXS_ISOLATED_SAMBA_TEST': '1'}))
            stack.enter_context(patch.object(SAMBA.os, 'geteuid', return_value=0))
            accounts = stack.enter_context(patch.object(SAMBA.pwd, 'getpwnam', side_effect=users.__getitem__))
            stack.enter_context(patch.object(SAMBA.os, 'chown'))
            kill = stack.enter_context(patch.object(SAMBA.os, 'killpg'))
            stack.enter_context(patch.object(SAMBA.subprocess, 'check_output', side_effect=command_output))
            stack.enter_context(patch.object(SAMBA.subprocess, 'run', return_value=SimpleNamespace(returncode=1)))
            child = Mock(pid=12345)
            child.poll.return_value = None
            stack.enter_context(patch.object(SAMBA.subprocess, 'Popen', return_value=child))
            stack.enter_context(patch.object(SAMBA, 'wait_file', side_effect=fail_after_start))
            with self.assertRaisesRegex(RuntimeError, 'injected fixture startup failure'):
                SAMBA.run(SimpleNamespace(output=output, module=module, fixture=root / 'fixture'))
            self.assertCountEqual([call.args[0] for call in accounts.call_args_list], users)
            self.assertFalse(output.exists())
            self.assertEqual((root / 'engine.log').read_text(), 'current fixture failed')
            kill.assert_called_once_with(child.pid, SAMBA.signal.SIGTERM)
            child.wait.assert_called_once_with(timeout=10)

    def check_single_binary_run(self, directory, *, asset_matches):
        root = Path(directory)
        output = root / 'receipt.json'
        output.write_text('{"previous": "success"}\n')
        binary = root / 'candidate'
        binary.write_bytes(b'unit-test binary placeholder')
        web = root / 'web'
        web.mkdir()
        asset = b'unit-test embedded resource'
        checksum = hashlib.sha256(asset).hexdigest()
        (web / 'index.html').write_bytes(asset)
        (web / 'build-manifest.json').write_text(json.dumps({'files': {'index.html': checksum}}))
        child = Mock()
        child.poll.return_value = None
        child.wait.return_value = 0

        def start_child(command, **kwargs):
            self.assertNotIn('--allow-development-setup', command)
            self.assertNotIn('--setup-disk', command)
            self.assertIn('/zxs/data/state', command)
            self.assertEqual((kwargs['user'], kwargs['group'], kwargs['extra_groups']), (21001, 21001, []))
            self.assertFalse(output.exists())
            kwargs['stdout'].write(b'current single-ELF log')
            kwargs['stdout'].flush()
            return child

        ready = Mock(status=200)
        ready.read.return_value = json.dumps({'management': 'alive', 'authentication': 'ready',
            'storage': {'state': 'ready', 'recovering': False}, 'auxiliary': {'web_assets': 'ready'}}).encode()
        probe = Mock(status=200)
        probe.read.return_value = b'{"ready":true}'
        response = Mock(status=200)
        response.read.return_value = asset if asset_matches else b'wrong asset'
        client = Mock()
        client.getresponse.side_effect = [ready, probe, ready, probe, response]
        reservation = Mock()
        reservation.__enter__ = Mock(return_value=reservation)
        reservation.__exit__ = Mock(return_value=False)
        reservation.getsockname.return_value = ('127.0.0.1', 12345)
        with patch('sys.argv', ['check-debian13-single-binary.py', '--binary', str(binary),
                                '--helper', 'unused', '--web', str(web), '--output', str(output)]), \
                patch.object(SINGLE.platform, 'freedesktop_os_release', return_value={'ID': 'debian', 'VERSION_ID': '13'}), \
                patch.object(SINGLE.platform, 'machine', return_value='x86_64'), \
                patch.object(SINGLE.shutil, 'which', return_value=None), \
                patch.object(SINGLE, 'prepare_installation', return_value=(SimpleNamespace(pw_uid=21001, pw_gid=21001), {'test': 'prepared'})), \
                patch.object(SINGLE, 'reject_helper_while_daemon', return_value={'exit_code': 1, 'diagnostic': SINGLE.LEASE_BUSY}), \
                patch.object(SINGLE.os, 'chown'), \
                patch.object(SINGLE.subprocess, 'Popen', side_effect=start_child), \
                patch.object(SINGLE.http.client, 'HTTPConnection', return_value=client), \
                patch.object(SINGLE.socket, 'socket', return_value=reservation):
            if asset_matches:
                SINGLE.main()
                receipt = json.loads(output.read_text())
                self.assertEqual(receipt['binary_sha256'], hashlib.sha256(binary.read_bytes()).hexdigest())
                self.assertEqual(receipt['web_assets'], {'index.html': checksum})
                self.assertNotIn('previous', receipt)
            else:
                with self.assertRaisesRegex(ValueError, 'embedded asset differs'):
                    SINGLE.main()
                self.assertFalse(output.exists())
        child.terminate.assert_called_once_with()
        child.wait.assert_called_once_with(timeout=10)
        self.assertEqual((root / 'debian13-single-binary.log').read_bytes(), b'current single-ELF log')

    def test_single_binary_asset_failure_keeps_logs_without_success_receipt(self):
        with tempfile.TemporaryDirectory() as directory:
            self.check_single_binary_run(directory, asset_matches=False)

    def test_single_binary_success_replaces_previous_receipt(self):
        with tempfile.TemporaryDirectory() as directory:
            self.check_single_binary_run(directory, asset_matches=True)


if __name__ == '__main__':
    unittest.main()
