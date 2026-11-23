"""Opt-in anonymous APFS edit-body qualification. Synthetic server; no provider or mount."""
from contextlib import contextmanager
from dataclasses import replace
import errno
from hashlib import sha256
import json
import os
from pathlib import Path
import sys
from types import SimpleNamespace
import unittest
from unittest.mock import patch

from zipworkspace import macos_edit_file as edit, macos_read as mac
from zipworkspace.desktop.container_connection import _Authorization, _Metadata
from zipworkspace.model import Rejected, new_id

_SCRATCH = os.environ.get("ZIPWORKSPACE_APFS_DISPOSABLE_PRIVATE_DIRECTORY")
_CONTAINER, _NODE, _HEAD = "1" * 32, "2" * 32, "3" * 32


@unittest.skipUnless(sys.platform == "darwin" and _SCRATCH, "requires a private disposable APFS directory")
class MacEditBody(unittest.TestCase):
    def setUp(self):
        with mac.no_materialization(), mac.literal_name(Path(_SCRATCH), directory=True) as parent:
            edit._attributes(parent, directory=True)
            edit.apfs(parent, fresh_snapshot=True)
            leaf = "edit-file-fixture-" + new_id()
            os.mkdir(leaf, 0o700, dir_fd=parent)
        self.root = Path(_SCRATCH) / leaf
        self.exchange = object.__new__(edit.HttpsExchange)
        self.exchange._identity, self.exchange._location = "d" * 64, "https://download.invalid:443"
        auth = _Authorization("test", "synthetic only")
        self.exchange._authorization = auth
        self.addCleanup(auth.revoke)
        self.requests, self.details = [], []
        self.body = b"complete original"
        self.node = edit.Node(_NODE, None, len(self.body), sha256(self.body).hexdigest())
        head = edit.Node(_HEAD, _NODE, 0, sha256(b"").hexdigest())
        self.branch = edit.Branch("main", head.id, (head, self.node))
        self.detail = edit.Detail(edit.Container(_CONTAINER, "Report.txt", 7), (), (self.branch,))
        self.fresh = self.detail

        source = _Metadata(self.exchange)
        self.connection = edit.OpenConnection(edit.ContainerClient(source, self.exchange), auth, self.exchange,
                                              "test", _listed=True)

    @contextmanager
    def opened(self, *, data=None, on_body=lambda: None, confirmed=True):
        @contextmanager
        def response(exchange, method, target, headers, outbound):
            self.assertIs(exchange, self.exchange)
            self.assertEqual(list(self.root.iterdir()), [], "remote bytes must never visit a named stage")
            self.assertIsNone(outbound)
            self.assertEqual(method, "GET")
            container = f"/zxs/v1/containers/{_CONTAINER}"
            if target == container:
                self.details.append(target)
                self.assertEqual(headers["Zxs-Detail-Sequence"], str(self.detail.container.revision))
                record = dict(container_id=_CONTAINER, sequence=self.fresh.container.revision,
                              name=self.fresh.container.name, tags=[], heads=[dict(branch="main", head=_HEAD)],
                              next_after=None)
                payload = json.dumps(record).encode()
                fields = [(b"content-type", b"application/json")]
            elif target == container + "/branches/main/nodes":
                self.details.append(target)
                self.assertEqual(headers["Zxs-History-Head"], _HEAD)
                nodes = [dict(node_id=n.id, parent_node=n.parent, operation_id=f"{i:032x}",
                              original_branch="main", sequence=i, length=n.length, sha256=n.sha256)
                         for i, n in zip((7, 1), self.fresh.branches[0].nodes, strict=True)]
                payload = json.dumps(dict(head=_HEAD, nodes=nodes, next=None)).encode()
                fields = [(b"content-type", b"application/json")]
            else:
                self.assertEqual(target, container + f"/nodes/{_NODE}/data")
                self.requests.append((method, target))
                on_body()
                payload = self.body if data is None else data
                fields = [(b"content-type", b"application/octet-stream"), (b"zxs-sha256", self.node.sha256.encode())]
            fields.append((b"content-length", str(len(payload)).encode()))
            yield SimpleNamespace(status=200, headers=fields, extensions={"http_version": b"HTTP/2"},
                                  iter_stream=lambda: iter((payload,)))
        with patch.object(edit.HttpsExchange, "_exchange", response), patch.object(
                edit.HttpsExchange, "_exchange_errors", return_value=(OSError, UnicodeError)):
            with edit.selected_edit_file(self.connection, self.detail, self.branch, self.node,
                                         self.root, confirmed=confirmed) as value:
                yield value

    def test_original_anonymous_inode_one_writer_drain_and_explicit_whole_seal(self):
        import fcntl
        with self.opened() as file:
            rf, wf = file._reader.fileno(), file._writer.fileno()
            self.assertTrue(os.path.samestat(os.fstat(wf), os.fstat(rf)))
            self.assertEqual(os.fstat(rf).st_nlink, 0)
            self.assertEqual(fcntl.fcntl(rf, fcntl.F_GETFL) & os.O_ACCMODE, os.O_RDONLY)
            reader, writer = file.open_handle(), file.open_handle(writable=True)
            self.assertEqual(file.read(reader, 0, 100), self.body)
            with self.assertRaises(OSError):
                file.open_handle(writable=True)
            with self.assertRaises(OSError):
                file.write(reader, 0, b"forbidden")
            self.assertEqual(file.write(writer, 0, b"AB"), 2)
            file.truncate(writer, 2)
            file.flush(writer)
            with self.assertRaises(Rejected):
                file.seal_for_submit(confirmed=True)
            file.stop_new_handles()
            with self.assertRaises(Rejected):
                file.open_handle()
            self.assertEqual(file.write(writer, 4, b"Z"), 1)  # Already open writer can drain.
            # Invalid request arguments never touched the file and must not
            # prevent sealing prior successful edits.
            with self.assertRaises(OSError):
                file.write(writer, -1, b"no effect")
            with self.assertRaises(OSError):
                file.truncate(writer, edit.MAX_FILE + 1)
            self.assertEqual(file.read(reader, 1, 100), b"B\0\0Z")
            file.release(writer)
            with self.assertRaises(OSError):
                file.write(writer, 0, b"stale")
            with self.assertRaises(Rejected):
                file.seal_for_submit(confirmed=True)  # Original reader still open.
            file.release(reader)
            with self.assertRaises(Rejected):
                file.seal_for_submit()
            seal = file.seal_for_submit(confirmed=True)
            self.assertTrue(file._writer.closed)
            self.assertEqual((seal.selected.container, seal.selected.head, seal.selected.node),
                             (self.detail.container, _HEAD, self.node))
            expected = b"AB\0\0Z"
            self.assertEqual(seal.body, edit.FullBody(len(expected), sha256(expected).hexdigest()))
            with self.assertRaises(Rejected):
                list(file.blocks(replace(seal)))
            self.assertEqual(b"".join(file.blocks(seal)), expected)
            self.assertEqual(list(self.root.iterdir()), [])
        with self.assertRaises(Rejected):
            list(file.blocks(seal))
        self.assertEqual(len(self.requests), 1)
        root = f"/zxs/v1/containers/{_CONTAINER}"
        self.assertEqual(self.details, [root, root + "/branches/main/nodes", root] * 2)

    def test_zero_length_and_close_without_seal(self):
        with self.opened() as file:
            writer = file.open_handle(writable=True)
            file.truncate(writer, 0)
            file.release(writer)
            self.assertIsNone(file._sealed)
            file.stop_new_handles()
            seal = file.seal_for_submit(confirmed=True)
            self.assertEqual(seal.body, edit.FullBody(0, sha256(b"").hexdigest()))
            self.assertEqual(list(file.blocks(seal)), [])
        with self.opened() as abandoned:
            writer = abandoned.open_handle(writable=True)
            abandoned.write(writer, 0, b"discard")
        self.assertIsNone(abandoned._sealed)
        self.assertEqual(list(self.root.iterdir()), [])
        self.assertEqual(len(self.requests), 2)

    def test_partial_local_write_or_sync_error_never_seals(self):
        real = os.pwrite
        def short_then_full(fd, data, offset):
            if offset:
                raise OSError(errno.ENOSPC, "injected full APFS")
            return real(fd, data[:1], offset)
        for operation in ("write", "sync"):
            with self.subTest(operation=operation), self.opened() as file:
                writer = file.open_handle(writable=True)
                file.stop_new_handles()
                with self.assertRaises(OSError):
                    if operation == "write":
                        with patch.object(os, "pwrite", side_effect=short_then_full):
                            file.write(writer, 0, b"lost")
                    else:
                        with patch.object(os, "fsync", side_effect=OSError(errno.EIO, "injected barrier failure")):
                            file.flush(writer)
                file.release(writer)
                with self.assertRaises(Rejected):
                    file.seal_for_submit(confirmed=True)

    def test_stale_or_corrupt_remote_selection_and_revocation_never_offer_edit(self):
        with self.assertRaises(Rejected), self.opened(confirmed=False):
            self.fail("unconfirmed local edit")
        self.assertFalse(self.details or self.requests)
        with self.assertRaises(Rejected), self.opened(data=b"X" * len(self.body)):
            self.fail("wrong original digest")
        def advance():
            self.fresh = replace(self.detail, container=replace(self.detail.container, revision=8))
        with self.assertRaises(Rejected), self.opened(on_body=advance):
            self.fail("branch metadata changed across the GET")
        self.fresh = self.detail
        with self.opened() as file:
            file.stop_new_handles()
            self.connection.revoke()
            with self.assertRaises(Rejected):
                file.seal_for_submit(confirmed=True)
        self.assertEqual(list(self.root.iterdir()), [])
