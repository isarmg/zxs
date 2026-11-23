from contextlib import contextmanager
import hashlib
import os
from pathlib import Path
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

from zipworkspace.container_plan import LocalFile, plan_submit
from zipworkspace.container_spool import capture_submit, reopen_submit
from zipworkspace.model import Rejected

SUBMISSION, CONTAINER = "1" * 32, "2" * 32
DESTINATION = "d" * 64


class NativeCutFixture:
    """Only a scoped plain local FD; no mounted driver or service."""

    def __init__(self, root):
        self.root = root
        self.member = SimpleNamespace(path="current.txt")
        self.file_live = self.cut_live = self.fail_postcheck = False
        self.events = []

    @contextmanager
    def freeze(self):
        fd = os.open(self.root, os.O_RDONLY | os.O_DIRECTORY)
        self.cut_live = True
        self._work_fd = fd
        try:
            yield self
            self.events.append("cut-postcheck")
            if self.fail_postcheck:
                raise Rejected("fixture cut postcheck refused the exact captured body")
        finally:
            self.cut_live = False
            os.close(fd)

    @property
    def changed_files(self):
        return (self.member,)

    def check_work(self):
        if not self.cut_live:
            raise Rejected("cut expired")

    @contextmanager
    def open_file(self, bound):
        self.check_work()
        if bound is not self.member:
            raise Rejected("not this exact cut member")
        fd = os.open(bound.path, os.O_RDONLY, dir_fd=self._work_fd)
        self.file_live = True
        try:
            def check_file():
                self.check_work()
                if not self.file_live:
                    raise Rejected("original file loan expired")

            yield SimpleNamespace(bound=bound, fd=fd, size=os.fstat(fd).st_size, check_file=check_file)
            check_file()
            self.events.append("file-postcheck")
        finally:
            self.file_live = False
            os.close(fd)


@unittest.skipUnless(sys.platform.startswith("linux"), "Linux native local spool")
class ContainerSpoolTest(unittest.TestCase):
    def setUp(self):
        temp = Path(self.enterContext(tempfile.TemporaryDirectory()))
        self.work, self.state = temp / "work", temp / "sealed"
        self.work.mkdir(mode=0o700)
        self.state.mkdir(mode=0o700)
        self.original = self.work / "current.txt"
        self.original.write_bytes(b"original individual file")
        self.cut = NativeCutFixture(self.work)
        # Topology evidence is mocked; these cases never create any mount.
        self.enterContext(patch("zipworkspace.container_spool.local_mount", return_value=10))
        self.enterContext(patch("zipworkspace.container_spool.mount_id", return_value=10))

    def capture(self, select=lambda seq: {CONTAINER: seq[0]}):
        def plan(captured):
            self.assertFalse(self.cut.cut_live or self.cut.file_live)
            self.assertEqual(["file-postcheck", "cut-postcheck"], self.cut.events)
            path, body = captured[CONTAINER]
            return plan_submit((), (LocalFile(CONTAINER, path, body),), (), attested_complete=True)

        return capture_submit(self.state, self.cut, submission=SUBMISSION, destination=DESTINATION,
                              select_files=select, build_plan=plan)

    def test_both_original_scopes_end_before_private_bytes_are_recoverable(self):
        self.assertEqual(SUBMISSION, self.capture())
        self.original.write_bytes(b"new draft while the prior request is pending")
        with reopen_submit(self.state, SUBMISSION, destination=DESTINATION) as record:
            action, = record.plan.actions
            self.assertEqual("save_node", action.kind)
            with record.body(action) as retained:
                transferred = b"".join(retained)
            self.assertEqual(b"original individual file", transferred)
            self.assertEqual(hashlib.sha256(transferred).hexdigest(), action.body.sha256)
            self.assertEqual(len(transferred), action.body.length)
        self.assertEqual(b"new draft while the prior request is pending", self.original.read_bytes())

    def test_unselected_identity_cannot_open_and_failed_cut_postcheck_leaves_no_pending(self):
        with self.assertRaises(Rejected):
            self.capture(select=lambda _seq: {CONTAINER: SimpleNamespace(path="current.txt")})
        self.cut.fail_postcheck = True
        with self.assertRaises(Rejected):
            self.capture()
        self.assertFalse((self.state / (SUBMISSION + ".pending")).exists())
        self.assertEqual(b"original individual file", self.original.read_bytes())
