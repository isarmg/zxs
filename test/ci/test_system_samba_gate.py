"""Isolation and identity contract for the real Debian Samba acceptance gate."""
import importlib.util
import json
import os
from pathlib import Path
import shlex
import subprocess
import sys
from types import SimpleNamespace
import tempfile
import unittest
from unittest.mock import Mock, patch

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("system_samba_gate", ROOT / ".github/scripts/check-system-samba.py")
GATE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(GATE)


class SystemSambaGate(unittest.TestCase):
    def test_host_without_explicit_isolation_is_rejected_before_host_actions(self):
        with tempfile.TemporaryDirectory() as directory, patch.dict(os.environ, {}, clear=True), \
                patch.object(GATE.subprocess, "check_output") as execute, \
                patch.object(GATE.pwd, "getpwnam") as accounts, \
                patch.object(GATE.subprocess, "Popen") as processes:
            with self.assertRaisesRegex(RuntimeError, "isolated Debian fixture"):
                GATE.run(SimpleNamespace(output=Path(directory) / "receipt.json"))
            execute.assert_not_called()
            accounts.assert_not_called()
            processes.assert_not_called()

    def test_phase_markers_publish_complete_expected_bytes(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            GATE.publish_marker(root, "uploaded")
            self.assertEqual((root / "uploaded").read_bytes(), b"ok\n")
            self.assertFalse((root / "uploaded.new").exists())

    def test_io_evidence_binds_each_request_to_its_own_nonroot_header(self):
        def record(uid, request):
            return f"[timestamp, 10, pid=123, effective({uid}, 21002), real(0, 0)] source\n  {request}\n"
        read = "smbd_smb2_read: report.txt - fnum 1"
        write = "smbd_smb2_write: upload.txt - fnum 2"
        good = record(21002, read) + record(21002, write)
        self.assertEqual(set(GATE.nonroot_io_evidence([good], 21002)), {"read-existing", "write-new"})
        for bad in (record(0, read) + record(21002, write),
                    record(21002, read) + record(0, write),
                    record(21002, "unrelated") + "  " + read + "\n" + record(21002, write),
                    good.replace("report.txt", "other.txt"), ""):
            with self.subTest(log=bad), self.assertRaisesRegex(RuntimeError, "identity evidence"):
                GATE.nonroot_io_evidence([bad], 21002)

    def test_io_evidence_handles_rotation_without_cross_file_header_pairing(self):
        header = "[timestamp, 10, pid=47, effective(21002, 21002), real(21002, 0)] source\n"
        read = "  smbd_smb2_read: report.txt - fnum 1\n"
        write = "  smbd_smb2_write: upload.txt - fnum 2\n"
        for logs in ([header + read + header + write, ""],
                     [header + read, header + write],
                     [header + read, header + write + "[later]\n  unrelated\n"]):
            with self.subTest(logs=logs):
                self.assertEqual(set(GATE.nonroot_io_evidence(logs, 21002)),
                                 {"read-existing", "write-new"})
        for logs in ([header + read + header, write],
                     [header + read, ""], ["", header + write],
                     [header.replace("effective(21002,", "effective(0,") + read, header + write],
                     [header + read, header.replace("effective(21002,", "effective(0,") + write]):
            with self.subTest(logs=logs), self.assertRaisesRegex(RuntimeError, "identity evidence"):
                GATE.nonroot_io_evidence(logs, 21002)

    def test_fixture_requires_independent_nonroot_engine_and_samba_user(self):
        script = (ROOT / ".github/scripts/check-system-samba.py").read_text()
        self.assertIn('user=engine_user.pw_uid, group=engine_user.pw_gid, extra_groups=[]', script)
        self.assertIn('"ZXS_SMB_E2E_USER": smb_user.pw_name', script)
        self.assertIn('valid users = {smb_user.pw_name}', script)
        self.assertIn('assert_owner_mode(Path(ready["socket"]), engine_user.pw_uid, 0o600)', script)
        self.assertIn('assert_owner_mode(work / "empty", 0, 0o555)', script)
        self.assertIn('log level = 10\nmax log size = 4096', script)
        self.assertIn('("engine.log", "smbd.log", "samba-identity.log.old", "samba-identity.log")', script)
        self.assertIn('.replace(password, "[redacted]")', script)
        self.assertNotIn('"--debug-stdout"', script)  # stdout omits the debug-uid header
        fixture = (ROOT / "test/server/unit/smb_rpc/system_samba_tests.rs").read_text()
        self.assertIn('allowed_uid: 0,', fixture)
        self.assertIn('engine_uid, 0,', fixture)

    def test_password_api_targets_only_loopback_and_redacts_failure_body(self):
        connection = Mock()
        response = Mock(status=200)
        response.read.return_value = b'{"changed":true}'
        connection.getresponse.return_value = response
        with patch.object(GATE.http.client, "HTTPConnection", return_value=connection) as client:
            result = GATE.password_request(12345, "synthetic-token", "POST", "/api/smb/password",
                                           {"current_password": "old", "new_password": "new"})
        self.assertEqual(result, {"changed": True})
        client.assert_called_once_with("127.0.0.1", 12345, timeout=25)
        connection.close.assert_called_once()
        response.status = 502
        response.read.return_value = b'synthetic-secret-must-not-be-reported'
        with patch.object(GATE.http.client, "HTTPConnection", return_value=connection):
            with self.assertRaisesRegex(RuntimeError, "^password API returned HTTP 502$"):
                GATE.password_request(12345, "synthetic-token", "POST", "/api/smb/password")
        for port in (True, "12345", 0, 65536):
            with self.assertRaisesRegex(RuntimeError, "invalid fixture HTTP port"):
                GATE.password_request(port, "token", "GET", "/api/smb/account")

    def test_real_password_gate_requires_old_rejection_and_new_authentication(self):
        script = (ROOT / ".github/scripts/check-system-samba.py").read_text()
        for marker in ("http-smb-password-changed", "old-smb-password-rejected",
                       "new-smb-password-authenticated", "web-session-preserved"):
            self.assertIn(marker, script)
        self.assertIn('smb ports = 44545 445', script)
        self.assertIn('client("ls", credential=auth, succeeds=False)', script)
        self.assertIn('.replace(new_password, "[redacted]")', script)
        self.assertIn('.replace(web_token, "[redacted]")', script)
        fixture = (ROOT / "test/server/unit/smb_rpc/system_samba_tests.rs").read_text()
        self.assertIn('crate::ManagementRuntime::start(shared.backend.clone(), web_auth, &web_config)', fixture)
        self.assertIn('runtime.smb_account', fixture)
        self.assertIn('"password_http_port": web_port', fixture)

    def test_account_selection_gate_requires_real_new_and_old_authentication(self):
        script = (ROOT / ".github/scripts/check-system-samba.py").read_text()
        for marker in ("http-smb-account-selected", "previous-account-authenticates-but-share-denied",
                       "selected-account-authenticated", "account-switch-back-next-generation"):
            self.assertIn(marker, script)
        self.assertIn('valid users = {smb_user.pw_name} {next_user.pw_name}', script)
        self.assertIn('"//127.0.0.1/IPC$"', script)
        self.assertIn('previous.returncode == 0 or "NT_STATUS_UNSUCCESSFUL" not in previous.stdout', script)
        self.assertIn('.replace(next_password, "[redacted]")', script)

    def test_account_tree_read_sequences_commands_after_buffered_input_is_consumed(self):
        # Model Samba 4.22's poll-before-fgets reader with real buffered pipe I/O.
        # This tests driver synchronization only, not Samba/runtime acceptance.
        reader = '''
import select
import shlex
import sys
from pathlib import Path
poll = select.poll()
poll.register(sys.stdin.fileno(), select.POLLIN | select.POLLHUP)
while True:
    poll.poll()
    line = sys.stdin.readline()
    if not line:
        break
    command = shlex.split(line)
    if command[0] == "get":
        Path(command[2]).write_bytes(sys.argv[1].encode())
    elif command[0] == "!touch":
        Path(command[1]).touch()
'''
        for batched, body in ((True, "old"), (False, "old"), (False, "bad")):
            with self.subTest(batched=batched, body=body), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                before = root / "before read.bin"
                ready = root / "tree ready"
                process = subprocess.Popen([sys.executable, "-c", reader, body],
                    stdin=subprocess.PIPE, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)
                try:
                    if batched:
                        process.stdin.write(f'get report.txt "{before}"\n!touch ' + shlex.quote(str(ready)) + "\n")
                        process.stdin.flush()
                        GATE.wait_file(before, process, timeout=5)
                        self.assertEqual(before.read_bytes(), b"old")
                        # The second line is buffered in the reader, so its next
                        # poll cannot see it until more pipe input (or EOF) arrives.
                        with self.assertRaises(TimeoutError):
                            GATE.wait_file(ready, process, timeout=0.2)
                    else:
                        wait = GATE.wait_file
                        with patch.object(GATE, "wait_file",
                                          side_effect=lambda path, child, timeout: wait(path, child, timeout=5)):
                            if body == "bad":
                                with self.assertRaisesRegex(RuntimeError, "known body before switching"):
                                    GATE.read_account_tree_before_switch(process, before, ready)
                            else:
                                GATE.read_account_tree_before_switch(process, before, ready)
                        self.assertTrue(ready.is_file())
                    self.assertIsNone(process.poll())  # Keep this exact tree alive for the switch.
                finally:
                    _, error = process.communicate(timeout=5)
                self.assertEqual(process.returncode, 0, error)

    def test_existing_tree_gate_requires_specific_read_failure_and_no_returned_bytes(self):
        with tempfile.TemporaryDirectory() as directory:
            target = Path(directory) / "download.bin"
            for status in ("NT_STATUS_IO_DEVICE_ERROR", "NT_STATUS_CONNECTION_DISCONNECTED", "NT_STATUS_ACCESS_DENIED"):
                output = f"{status} opening remote file report.txt"
                self.assertEqual(GATE.revoked_account_tree_read(output, 0, target), [status])
                target.write_bytes(b"")
                self.assertEqual(GATE.revoked_account_tree_read(output, 0, target), [status])
            for output, code in (("connection lost", 0),
                                 ("NT_STATUS_LOGON_FAILURE opening remote file report.txt", 0),
                                 ("NT_STATUS_CONNECTION_DISCONNECTED unrelated operation", 0),
                                 ("NT_STATUS_ACCESS_DENIED opening remote file report.txt", 1),
                                 ("NT_STATUS_UNSUCCESSFUL opening remote file report.txt", 0)):
                with self.subTest(output=output, code=code), self.assertRaises(RuntimeError):
                    GATE.revoked_account_tree_read(output, code, target)
            target.write_bytes(b"o")
            with self.assertRaisesRegex(RuntimeError, "returned file bytes"):
                GATE.revoked_account_tree_read("NT_STATUS_ACCESS_DENIED opening remote file report.txt", 0, target)
        script = (ROOT / ".github/scripts/check-system-samba.py").read_text()
        run = script.split('def run(args):', 1)[1]
        self.assertLess(run.index('read_account_tree_before_switch(previous_tree, before_switch, tree_ready)'),
                        run.index('saved = password_request'))
        self.assertIn('before_switch.read_bytes() != b"old"', script)
        self.assertIn('active_body.read_bytes() != b"old"', script)
        self.assertIn('account-switch-existing-tree-read-revoked', script)

    def test_validation_docs_distinguish_image_accounts_from_host_accounts(self):
        dockerfile = (ROOT / ".github/containers/debian13-runtime.Dockerfile").read_text()
        document = (ROOT / "docs/validation/debian13-system-samba.md").read_text()
        for account in ("zxs-engine-test", "zxs-smb-test", "zxs-smb-next-test"):
            self.assertRegex(dockerfile, rf"useradd[^\n]+{account}")
            self.assertIn(f"`{account}`", document)
        self.assertIn("运行镜像预建", document)
        self.assertIn("不创建或更改宿主 OS 账号", document)
        self.assertNotIn("不创建系统用户", document)

    def test_debian_source_and_abi_are_exact_not_upstream_fallback(self):
        target = json.loads((ROOT / ".github/samba-debian-target.json").read_text())
        self.assertEqual((target["release"], target["architecture"], target["vfs_interface"]), ("13", "amd64", 50))
        build = (ROOT / "src/smb-vfs/build.rs").read_text()
        self.assertIn("samba (" + target["package_version"] + ")", build)
        self.assertIn("SMB_VFS_INTERFACE_VERSION 50", build)
        sdk = (ROOT / ".github/scripts/smb-sdk.sh").read_text()
        self.assertIn("make -f debian/rules configure", sdk)
        self.assertIn("apt-get source --download-only --only-source", sdk)

    def test_runtime_is_network_isolated_and_no_service_autostart(self):
        workflow = (ROOT / ".github/workflows/smb-sdk.yml").read_text()
        self.assertIn("--network none", workflow)
        self.assertIn("$modules/vfs/zxs.so:ro", workflow)
        self.assertIn("vfs objects = zxs", (ROOT / ".github/scripts/check-system-samba.py").read_text())
        self.assertIn("ZXS_ISOLATED_SAMBA_TEST=1", workflow)
        image = (ROOT / ".github/containers/debian13-runtime.Dockerfile").read_text()
        self.assertIn("policy-rc.d", image)
        self.assertIn("exit 101", image)
        self.assertNotIn("build-essential", image)
        self.assertNotIn("samba-dev", image)
        script = (ROOT / ".github/scripts/check-system-samba.py").read_text()
        self.assertIn('"NT_STATUS_ACCESS_DENIED" not in denial', script)
        self.assertIn('"--client-protection=encrypt"', script)
        self.assertIn('wait_file(session_ready, session, timeout=30)', script)
        self.assertLess(script.index('wait_file(session_ready, session'), script.index('publish_marker(engine_work, "uploaded")'))
        self.assertIn('session.communicate("ls\\nquit\\n", timeout=30)', script)
        self.assertIn('result["revoked_new_tree"]', script)


if __name__ == "__main__":
    unittest.main()
