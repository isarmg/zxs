"""Pinned Samba request evidence parsing, not a substitute for real acceptance."""
import hashlib
import importlib.util
from pathlib import Path
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("system_samba_gate", ROOT / ".github/scripts/check-system-samba.py")
GATE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(GATE)
# These arbitrary identifiers and the non-existent path are test-only constants.
# Build the minimum parser contract from scratch, never from a captured log.
PID, UID, MID = 101, 202, 303
EXPORT = "/synthetic/export"
CLIENT = "NT_STATUS_OBJECT_PATH_NOT_FOUND opening remote file \\report.txt"


def frame(function, body, pid=PID, uid=UID):
    return (f"[synthetic, 1, pid={pid}, effective({uid}, {uid}), synthetic] "
            f"synthetic.c:1({function})\n{body}\n")


def synthetic_trace():
    return (
        frame("smbd_smb2_request_dispatch",
              f"  smbd_smb2_request_dispatch: opcode[SMB2_OP_CREATE] mid = {MID}")
        + frame("smbd_smb2_create_send", "  smbd_smb2_create_send: name [report.txt]")
        + "zxs SMB: RPC connection is closed\n"
        + frame("openat_pathref_fsp_dot",
                f"  openat_pathref_fsp_dot: smb_vfs_openat({EXPORT}/.) failed: Input/output error")
        + frame("filename_convert_dirfsp_nosymlink",
                "  filename_convert_dirfsp_nosymlink: opening directory  failed: NT_STATUS_IO_DEVICE_ERROR")
        + frame("smbd_smb2_request_done_ex",
                f"  smbd_smb2_request_done_ex: mid [{MID}] idx[1] status[NT_STATUS_OBJECT_PATH_NOT_FOUND] ")
    )


TRACE = synthetic_trace()


def known_read(pid=PID, uid=UID):
    return frame("smb2_read_complete",
                 "  smbd_smb2_read: fnum 1, file report.txt, length=3 offset=0 read=3", pid, uid)


class SambaRejectionEvidence(unittest.TestCase):
    def test_minimal_synthetic_trace_proves_exact_host_mapping(self):
        proof = GATE.mapped_account_tree_rejection(TRACE, PID, UID, EXPORT)
        self.assertEqual(proof, {
            "smbd_pid": PID, "uid": UID, "create_mid": MID, "file": "report.txt",
            "vfs_error": "RPC connection is closed", "directory_status": "NT_STATUS_IO_DEVICE_ERROR",
            "wire_status": "NT_STATUS_OBJECT_PATH_NOT_FOUND"})
        with tempfile.TemporaryDirectory() as directory:
            target = Path(directory) / "after.bin"
            self.assertEqual(GATE.revoked_account_tree_read(CLIENT, 0, target, proof),
                             ["NT_STATUS_OBJECT_PATH_NOT_FOUND"])
            with self.assertRaises(RuntimeError):
                GATE.revoked_account_tree_read(CLIENT, 0, target)
            for output, code in ((CLIENT, 1), (CLIENT.replace("report.txt", "unrelated"), 0),
                                 (CLIENT.replace("report.txt", "report.txt.bak"), 0),
                                 (CLIENT.replace("OBJECT_PATH_NOT_FOUND", "LOGON_FAILURE"), 0)):
                with self.subTest(output=output, code=code), self.assertRaises(RuntimeError):
                    GATE.revoked_account_tree_read(output, code, target, proof)
            target.write_bytes(b"o")
            with self.assertRaisesRegex(RuntimeError, "returned file bytes"):
                GATE.revoked_account_tree_read(CLIENT, 0, target, proof)

    def test_missing_or_wrong_stage_never_proves_a_rejection(self):
        replacements = (
            ("name [report.txt]", "name [report.txt.bak]"),
            ("RPC connection is closed", "unrelated RPC diagnostic"),
            ("Input/output error", "No such file or directory"),
            ("NT_STATUS_IO_DEVICE_ERROR", "NT_STATUS_OBJECT_NAME_NOT_FOUND"),
            (f"mid [{MID}]", f"mid [{MID + 1}]"),
            ("status[NT_STATUS_OBJECT_PATH_NOT_FOUND]", "status[NT_STATUS_OK]"),
            (EXPORT + "/.", EXPORT + "/other"),
        )
        for old, new in replacements:
            with self.subTest(stage=old), self.assertRaises(RuntimeError):
                GATE.mapped_account_tree_rejection(TRACE.replace(old, new), PID, UID, EXPORT)
        lines = TRACE.splitlines(True)
        for index, line in enumerate(lines):
            if line.startswith("["):
                for old, new in ((f"pid={PID},", f"pid={PID + 1},"), (f"effective({UID},", f"effective({UID + 1},")):
                    changed = list(lines)
                    changed[index] = line.replace(old, new)
                    with self.subTest(line=index, identity=old), self.assertRaises(RuntimeError):
                        GATE.mapped_account_tree_rejection("".join(changed), PID, UID, EXPORT)
        for text in ("zxs SMB: RPC connection is closed\n", TRACE[:-100],
                     TRACE.replace("(openat_pathref_fsp_dot)", "(malformed")):
            with self.subTest(trace=text[-100:]), self.assertRaises(RuntimeError):
                GATE.mapped_account_tree_rejection(text, PID, UID, EXPORT)
        for function in ("smbd_smb2_request_dispatch", "smbd_smb2_create_send",
                         "openat_pathref_fsp_dot", "filename_convert_dirfsp_nosymlink",
                         "smbd_smb2_request_done_ex"):
            lines = TRACE.splitlines(True)
            index = next(i for i, line in enumerate(lines) if line.rstrip().endswith(f"({function})"))
            del lines[index:index + 2]
            with self.subTest(missing=function), self.assertRaises(RuntimeError):
                GATE.mapped_account_tree_rejection("".join(lines), PID, UID, EXPORT)

    def test_interleaved_or_reordered_request_is_rejected(self):
        dispatch = "\n".join(TRACE.splitlines()[:2]) + "\n"
        anchor = "zxs SMB: RPC connection is closed\n"
        for extra in (dispatch.replace(f"mid = {MID}", f"mid = {MID + 1}"),
                      dispatch.replace("SMB2_OP_CREATE", "SMB2_OP_READ")):
            with self.subTest(extra=extra), self.assertRaises(RuntimeError):
                GATE.mapped_account_tree_rejection(TRACE.replace(anchor, extra + anchor), PID, UID, EXPORT)
        lines = TRACE.splitlines(True)
        a = next(i for i, line in enumerate(lines) if "(openat_pathref_fsp_dot)" in line)
        b = next(i for i, line in enumerate(lines) if "(filename_convert_dirfsp_nosymlink)" in line)
        lines[a:a + 2], lines[b:b + 2] = lines[b:b + 2], lines[a:a + 2]
        with self.assertRaises(RuntimeError):
            GATE.mapped_account_tree_rejection("".join(lines), PID, UID, EXPORT)

    def test_boundary_binds_new_pid_file_successful_read_and_exact_prefix(self):
        with tempfile.TemporaryDirectory() as directory:
            work = Path(directory)
            prior = work / f"samba-identity.{PID - 1}.log"
            prior.write_text(known_read(PID - 1))
            log = work / f"samba-identity.{PID}.log"
            log.write_text(known_read())
            path, pid, boundary = GATE.account_tree_log_boundary(work, {prior.name}, UID)
            self.assertEqual((path, pid), (log, PID))
            self.assertEqual(boundary[3], hashlib.sha256(known_read().encode()).digest())
            log.write_text(known_read() + TRACE)
            self.assertEqual(GATE.account_tree_log_after(path, boundary), TRACE)
            self.assertEqual(GATE.identity_log_files(work), [prior, log])
            log.write_text(known_read().replace("read=3", "read=2") + TRACE)
            with self.assertRaisesRegex(RuntimeError, "rotated or changed"):
                GATE.account_tree_log_after(path, boundary)
            log.write_text(known_read()[:10])
            with self.assertRaisesRegex(RuntimeError, "rotated or changed"):
                GATE.account_tree_log_after(path, boundary)
            log.rename(work / (log.name + ".old"))
            log.write_text(known_read() + TRACE)
            with self.assertRaisesRegex(RuntimeError, "rotated or changed"):
                GATE.account_tree_log_after(path, boundary)

    def test_stale_mixed_pid_wrong_uid_and_ambiguous_reads_are_rejected(self):
        for content, previous in ((known_read(), {f"samba-identity.{PID}.log"}),
                                  (known_read(PID + 1), set()), (known_read(uid=0), set()),
                                  (known_read() + known_read(), set()),
                                  (known_read() + known_read(PID + 1), set()),
                                  (known_read().rstrip(), set())):
            with self.subTest(content=content, previous=previous), tempfile.TemporaryDirectory() as directory:
                work = Path(directory)
                (work / f"samba-identity.{PID}.log").write_text(content)
                with self.assertRaises(RuntimeError):
                    GATE.account_tree_log_boundary(work, previous, UID)

    def test_preboundary_trace_cannot_prove_a_new_rejection(self):
        with tempfile.TemporaryDirectory() as directory:
            work = Path(directory)
            log = work / f"samba-identity.{PID}.log"
            log.write_text(known_read() + TRACE)
            path, pid, boundary = GATE.account_tree_log_boundary(work, set(), UID)
            with self.assertRaisesRegex(RuntimeError, "CREATE request"):
                GATE.mapped_account_tree_rejection(GATE.account_tree_log_after(path, boundary), pid, UID, EXPORT)
            (work / f"samba-identity.{PID + 1}.log").write_text(known_read(PID + 1))
            with self.assertRaisesRegex(RuntimeError, "one new per-PID log"):
                GATE.account_tree_log_boundary(work, set(), UID)

    def test_mapping_is_applied_only_after_body_and_liveness_checks(self):
        run = (ROOT / ".github/scripts/check-system-samba.py").read_text().split("def run(args):", 1)[1]
        ordered = ["account_tree_log_boundary(work", "saved = password_request",
                   "previous_tree.communicate", "account_tree_log_after(tree_log",
                   'active_body.read_bytes() != b"old"',
                   'raise RuntimeError("account switch denial coincided with a fixture/service exit")',
                   "mapped_account_tree_rejection(", "statuses = revoked_account_tree_read("]
        positions = [run.index(marker) for marker in ordered]
        self.assertEqual(positions, sorted(positions))


if __name__ == "__main__":
    unittest.main()
