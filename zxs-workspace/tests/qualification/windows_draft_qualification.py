"""Manual ordinary-user Windows NTFS qualification of the unmounted local R3 draft; no sockets."""
from concurrent.futures import ThreadPoolExecutor
from hashlib import sha256
from pathlib import Path
import sys
from threading import Event
import unittest
from unittest.mock import patch

if __name__ == "__main__":
    here = Path(__file__).resolve()
    sys.path[:0] = [str(here.parents[2]), str(here.parent)]

from windows_node_qualification import CONTAINER, NODE, ORIGIN, TARGET, NativeNodePublication
from zipworkspace._windows_access import NativeSecurity
from zipworkspace._windows_output import PinnedParent
from zipworkspace.desktop.container_client import ContainerClient
from zipworkspace.desktop.container_connection import OpenConnection, _Metadata
from zipworkspace.desktop.container_management import open_checked
from zipworkspace.desktop import container_windows_draft as draft_model
from zipworkspace.model import CHUNK, Rejected


class LocalWorkingFile(NativeNodePublication):
    def choose_draft(self, body, **transport):
        h2 = self.response((body,), body, **transport)
        observed = {"name": "CON", "sequence": 2, "requests": []}  # Exact R3 name, never installed on NTFS.
        def catalog(after):
            self.assertIsNone(after)
            return {"items": [{"container_id": CONTAINER, "name": observed["name"],
                               "sequence": observed["sequence"]}], "next_after": None}

        def detail(container, sequence, after):
            self.assertEqual((container, after), (CONTAINER, None))
            self.assertIn(sequence, (None, observed["sequence"]))
            observed["requests"].append("detail")
            return {"container_id": CONTAINER, "name": observed["name"], "sequence": observed["sequence"],
                    "tags": [], "heads": [{"branch": "main", "head": NODE}], "next_after": None}

        def history(container, branch, head, after):
            self.assertEqual((container, branch, head, after), (CONTAINER, "main", NODE, None))
            observed["requests"].append("history")
            return {"head": NODE, "nodes": [{"node_id": NODE, "operation_id": "3" * 32, "parent_node": None,
                                             "original_branch": "main", "sequence": 1, "length": len(body),
                                             "sha256": sha256(body).hexdigest()}], "next": None}

        h2.read_catalog_page, h2.read_container_detail_page, h2.read_history_page = catalog, detail, history
        h2.request = lambda *_: self.fail("a local draft issued a remote operation")
        connection = OpenConnection(ContainerClient(_Metadata(h2), h2), h2._authorization, h2, "fixture")
        connection, _ = connection.establish(None)
        selected = open_checked(connection.operations, CONTAINER)
        branch = selected.branches[0]
        choice = draft_model.WindowsDraftChoice(connection, selected, branch, branch.nodes[0], self.staging)
        self.addCleanup(choice.close)
        return choice, observed

    def test_draft_offsets_late_writes_full_review_and_source(self):
        mark = f"[ZoneTransfer]\r\nZoneId={self.zone}\r\nHostUrl={ORIGIN}{TARGET}\r\n".encode("ascii")
        seen = []
        def before():
            # Observe the actual named stream on the original, while its default data is still empty.
            parent = choice._home
            with parent.api.witness(parent.handle, choice._leaf) as witness:
                row = parent.api.ordinary(witness, parent.volume, choice.local_file)
                self.assertEqual(row.size, 0)
                with parent.api.zone_witness(witness) as stream:
                    self.assertEqual(parent.api.read(stream, len(mark) + 1), mark)
                seen.append((parent.volume, row.file_id))

        choice, metadata = self.choose_draft(b"abcdef", before=before)
        path = choice.local_file
        self.assertEqual(self.requests, [])
        self.assertFalse(Path(path).exists())
        self.assertNotEqual(choice._leaf, choice.source.container.name)
        previously = len(metadata["requests"])
        with choice.begin(confirmed=True) as draft:
            self.assertGreater(len(metadata["requests"]), previously)
            frozen_count = len(metadata["requests"])
            r = draft.open()
            w = draft.open(writable=True)
            with self.assertRaises(Rejected):
                draft.write(r, 0, b"x")
            for off, count in ((-1, 1), ((1 << 63) - 1, 1), (0, CHUNK + 1), (True, 1)):
                with self.assertRaises(Rejected):
                    draft.read(r, off, count)
            with self.assertRaises(Rejected):
                draft.resize(w, 1 << 63)
            self.assertEqual(draft.write(w, 2, b"QQ"), 2)
            self.assertEqual(draft.write(w, 8, b"!"), 1)
            self.assertEqual(draft.read(r, 0, 30), b"abQQef\0\0!")
            self.assertEqual(draft.begin_drain(), 2)
            with self.assertRaises(Rejected):
                draft.open()
            draft.resize(w, 4)
            draft.resize(w, 8)
            draft.write(w, 7, b"Z")  # Still-open context, even after new opens were stopped.
            draft.release(w)
            with self.assertRaises(Rejected):
                draft.write(w, 0, b"")
            with self.assertRaises(Rejected):
                draft.review(confirmed=True)
            self.assertEqual(draft.read(r, 0, 100), b"abQQ\0\0\0Z")
            draft.release(r)
            with self.assertRaises(Rejected):
                draft.review()
            review = draft.review(confirmed=True)
            self.assertEqual(len(metadata["requests"]), frozen_count)
            self.assertEqual(seen, [(review.volume, review.file_id)])
            self.assertEqual(review.source, choice.source)
            self.assertEqual((review.source.origin, review.source.account, review.source.container.name,
                              review.source.container.id, review.source.container.revision,
                              review.source.branch, review.source.head.id),
                             (ORIGIN, "fixture", "CON", CONTAINER, 2, "main", NODE))
            self.assertTrue(review.changed)
            self.assertEqual((review.length, review.sha256), (8, sha256(b"abQQ\0\0\0Z").hexdigest()))
            with self.assertRaises(Rejected):
                draft.open(writable=True)
        self.assertEqual(Path(path).read_bytes(), b"abQQ\0\0\0Z")
        self.assertEqual(Path(path + ":Zone.Identifier").read_bytes(), mark)
        self.assertEqual(self.requests, [TARGET])

    def test_draft_empty_clean_and_unconfirmed(self):
        choice, observed = self.choose_draft(b"")
        read_count = len(observed["requests"])
        with self.assertRaises(Rejected):
            choice.begin()
        self.assertEqual(len(observed["requests"]), read_count)
        path = choice.local_file
        self.assertFalse(Path(path).exists())
        with choice.begin(confirmed=True) as draft:
            with self.assertRaises(Rejected):
                choice.begin(confirmed=True)
            h = draft.open(writable=True)
            self.assertEqual(draft.write(h, 99, b""), 0)
            self.assertEqual(draft.read(h, 11, 7), b"")
            draft.release(h)
            self.assertEqual(draft.begin_drain(), 0)
            review = draft.review(confirmed=True)
            self.assertFalse(review.changed)
            self.assertEqual((review.length, review.sha256), (0, sha256(b"").hexdigest()))
        self.assertEqual(Path(path).read_bytes(), b"")
        self.assertEqual(self.requests, [TARGET])

    def test_draft_no_get_or_file_for_replaced_remote_selection(self):
        choice, observed = self.choose_draft(b"bytes")
        path = choice.local_file
        observed["name"], observed["sequence"] = "con", 3
        with self.assertRaisesRegex(Rejected, "selected container"):
            choice.begin(confirmed=True)
        self.assertEqual(self.requests, [])
        self.assertFalse(Path(path).exists())

    def test_draft_partial_native_write_poisons_review_and_preserves_bytes(self):
        choice, _ = self.choose_draft(b"original")
        path = choice.local_file
        with choice.begin(confirmed=True) as draft:
            w = draft.open(writable=True)
            original = draft._home.api.write
            def partial(h, data):
                original(h, data[:1])
                raise OSError("synthetic failure after an actual native write")
            with patch.object(draft._home.api, "write", partial), self.assertRaises(OSError):
                draft.write(w, 0, b"XYZ")
            draft.release(w)
            with self.assertRaises(Rejected):
                draft.begin_drain()
            with self.assertRaises(Rejected):
                draft.review(confirmed=True)
        self.assertEqual(Path(path).read_bytes(), b"Xriginal")
        self.assertEqual(self.requests, [TARGET])

    def test_draft_disconnect_retains_unsent_edits(self):
        choice, _ = self.choose_draft(b"before")
        path = choice.local_file
        with choice.begin(confirmed=True) as draft:
            w = draft.open(writable=True)
            draft.write(w, 0, b"after!")
            choice._connection.revoke()
            with self.assertRaises(Rejected):
                draft.read(w, 0, 1)
            draft.release(w)
            with self.assertRaises(Rejected):
                draft.review(confirmed=True)
        self.assertEqual(Path(path).read_bytes(), b"after!")
        self.assertEqual(self.requests, [TARGET])

    def test_draft_cancel_during_get_does_not_handoff_or_steal_the_worker_handle(self):
        started, finish = Event(), Event()
        def pause():
            started.set()
            self.assertTrue(finish.wait(45), "qualification worker was not released")
        choice, _ = self.choose_draft(b"seed", before=pause)
        path = choice.local_file
        with ThreadPoolExecutor(max_workers=1) as pool:
            task = pool.submit(choice.begin, confirmed=True)
            try:
                self.assertTrue(started.wait(30))
                choice.close()
                with self.assertRaises(Rejected):
                    choice.begin(confirmed=True)
            finally:
                finish.set()
            with self.assertRaisesRegex(Rejected, "abandoned"):
                task.result(timeout=45)
        self.assertTrue(Path(path).is_file())  # Never re-open/adopt/erase incomplete material in production.
        self.assertEqual(self.requests, [TARGET])


def qualify(spelling):
    if sys.platform != "win32":
        raise SystemExit("Only an actual ordinary-user Windows NTFS environment qualifies.")
    with PinnedParent(spelling, output=True) as root:
        security = NativeSecurity()
        actor = security.caller()
        profile = security.read(root.handle, actor, directory=True)
        zone = draft_model._effective_zone(ORIGIN + TARGET)
        if zone not in (3, 4):
            raise SystemExit("This fixture requires an unchanged native marking policy for the inert URL.")
        cases = [name for name in unittest.defaultTestLoader.getTestCaseNames(LocalWorkingFile)
                 if name.startswith("test_draft_")]
        if len(cases) != 6:
            raise RuntimeError("all six dedicated draft cases are mandatory")
        suite = unittest.TestSuite(LocalWorkingFile(name, root, spelling, security, actor, profile, zone)
                                   for name in cases)
        result = unittest.TextTestRunner(verbosity=2).run(suite)
        return int(not (result.wasSuccessful() and result.testsRun == len(cases) and not result.skipped))


if __name__ == "__main__":
    if len(sys.argv) != 2 or sys.argv[1].startswith("-"):
        raise SystemExit("Supply one existing disposable ordinary-user private NTFS directory.")
    raise SystemExit(qualify(Path(sys.argv[1])))
