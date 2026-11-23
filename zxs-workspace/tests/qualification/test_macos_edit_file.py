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

    @contextmanager
    def retention_state(self):
        from zipworkspace import macos_retained_edit as retained
        # Separate disposable root: the product does not accept this path, and
        # anonymous GET assertions still apply inside self.root.
        support = self.root.parent / ("mock-application-support-" + new_id())
        support.mkdir(mode=0o700)
        with patch.object(retained, "_support", return_value=support):
            with self.assertRaises(Rejected):
                retained.initialize_state()
            ident = retained.initialize_state(confirmed=True)
            with self.assertRaises(FileExistsError):
                retained.initialize_state(confirmed=True)
            yield retained, ident, support / retained._ROOT

    @staticmethod
    def change(file, data):
        writer = file.open_handle(writable=True)
        file.truncate(writer, 0)
        file.write(writer, 0, data)
        file.release(writer)
        file.stop_new_handles()
        return file.seal_for_submit(confirmed=True)

    def new_connection(self, *, credential_login="test", presented_login="test"):
        exchange = object.__new__(edit.HttpsExchange)
        exchange._identity, exchange._location = self.exchange._identity, self.exchange._location
        auth = exchange._authorization = _Authorization(credential_login, "synthetic reconnect only")
        self.addCleanup(auth.revoke)
        return edit.OpenConnection(edit.ContainerClient(_Metadata(exchange), exchange), auth, exchange,
                                   presented_login, _listed=True)

    def test_retention_records_key_before_rename_and_reopens_original_account_and_body(self):
        with self.retention_state() as (store, ident, root):
            source = _CONTAINER, "main", _NODE
            owner = root / store._owner_name(self.exchange.destination_identity, "test", *source)
            seen_keys = []
            rename = store.rename_new_at
            def observe_rename(parent, scratch, final, **kwargs):
                key = store.RetentionKey.decode(owner.read_bytes())
                self.assertEqual(final, key.submission + ".pending")
                self.assertEqual(owner.stat().st_mode & 0o777, 0o400)
                self.assertFalse((root / final).exists())
                seen_keys.append(key)
                return rename(parent, scratch, final, **kwargs)
            with self.opened() as file:
                seal = self.change(file, b"retained whole file")
                with self.assertRaises(Rejected):
                    store.retain_edit(file, replace(seal), ident, confirmed=True)
                with patch.object(store, "rename_new_at", side_effect=observe_rename):
                    key = store.retain_edit(file, seal, ident, confirmed=True)
                with self.assertRaises(Rejected):
                    store.retain_edit(file, seal, ident, confirmed=True)
            self.assertEqual(seen_keys, [key])
            self.connection.revoke()
            packet = root / (key.submission + ".pending")
            leaf = "body-" + _CONTAINER
            self.assertEqual({p.name for p in packet.iterdir()}, {leaf, "submit.json"})
            self.assertEqual((packet / leaf).stat().st_mode & 0o777, 0o400)
            fresh = self.new_connection()
            def read(connection, consume, selection=source):
                return store.read_retained(connection, ident, *selection, consume, confirmed=True)
            with self.assertRaises(Rejected):
                read(self.connection, lambda *_: self.fail())
            with self.assertRaises(Rejected):
                read(self.new_connection(credential_login="other"), lambda *_: self.fail())
            with self.assertRaises(FileNotFoundError):
                read(fresh, lambda *_: self.fail(), (_CONTAINER, "main", _HEAD))
            seen = []
            def inspect(plan, blocks):
                action, = plan.actions
                self.assertEqual((action.container_id, action.fork_from_main, action.head, action.base_revision),
                                 (_CONTAINER, _NODE, None, 7))
                self.assertNotEqual(action.branch, "main")
                self.assertIsNone(action.name)
                self.assertEqual(plan.placements, ((_CONTAINER, leaf),))
                seen.append(plan.encode())
                return b"".join(blocks)
            for _ in range(2):
                self.assertEqual(read(fresh, inspect), b"retained whole file")
            self.assertEqual(seen[0], seen[1])
            later = read(fresh, lambda _, blocks: blocks)
            with self.assertRaises(Rejected):
                next(later)
            # A byte-identical replacement is not the original APFS body.
            (packet / leaf).rename(root / ("original-body-" + new_id()))
            (packet / leaf).write_bytes(b"retained whole file")
            (packet / leaf).chmod(0o400)
            with self.assertRaises(store.Uncertain):
                read(fresh, lambda *_: self.fail("no bytes may be lent"))

    def test_pre_rename_interruption_keeps_the_reservation_without_a_final_or_retry(self):
        with self.retention_state() as (store, ident, root):
            with self.opened() as file:
                file.stop_new_handles()
                unchanged = file.seal_for_submit(confirmed=True)
                with self.assertRaises(Rejected):
                    store.retain_edit(file, unchanged, ident, confirmed=True)
                self.assertEqual(list(root.iterdir()), [])
            with self.opened() as file:
                seal = self.change(file, b"changed")
                with self.assertRaises(Rejected):
                    store.retain_edit(file, seal, ident)
                with patch.object(store, "rename_new_at", side_effect=OSError(errno.EIO, "before rename")):
                    with self.assertRaises(store.Uncertain):
                        store.retain_edit(file, seal, ident, confirmed=True)
                self.assertEqual(b"".join(file.blocks(seal)), b"changed")
            with self.assertRaises(store.Uncertain):
                store.read_retained(self.connection, ident, _CONTAINER, "main", _NODE,
                                    lambda *_: self.fail("do not adopt orphaned scratch"), confirmed=True)
            old = {p.name for p in root.iterdir()}
            self.assertFalse(any(name.endswith(".pending") for name in old))
            # A second independent session cannot write another packet for the same pending source.
            with self.opened() as another:
                other = self.change(another, b"another")
                with self.assertRaises(Rejected):
                    store.retain_edit(another, other, ident, confirmed=True)
            self.assertEqual({p.name for p in root.iterdir()}, old)

    def test_after_rename_uncertainty_reopens_known_selection_without_a_returned_key(self):
        with self.retention_state() as (store, ident, root), self.opened() as file:
            seal = self.change(file, b"")
            real = store._barrier
            def late(fd):
                if list(root.glob("*.pending")):
                    raise OSError(errno.EIO, "injected final device flush failure")
                real(fd)
            with patch.object(store, "_barrier", side_effect=late), self.assertRaises(store.Uncertain):
                store.retain_edit(file, seal, ident, confirmed=True)
            self.assertEqual(len(list(root.glob("*.pending"))), 1)
            with self.assertRaises(Rejected):
                store.retain_edit(file, seal, ident, confirmed=True)
            self.assertEqual(store.read_retained(self.connection, ident, _CONTAINER, "main", _NODE,
                                                 lambda _, blocks: b"".join(blocks), confirmed=True), b"")
            self.assertEqual(len(self.requests), 1)  # No send, extra GET, or remote lookup from recovery.


    def test_early_body_barrier_failure_reserves_only_the_original_source(self):
        with self.retention_state() as (store, ident, root), self.opened() as file:
            seal = self.change(file, b"early failure")
            with patch.object(store, "_barrier", side_effect=OSError(errno.EIO, "before key completes")):
                with self.assertRaises(store.Uncertain):
                    store.retain_edit(file, seal, ident, confirmed=True)
            owner = root / store._owner_name(self.exchange.destination_identity, "test", _CONTAINER, "main", _NODE)
            self.assertTrue(owner.is_file())
            self.assertEqual(owner.stat().st_size, 0)
            self.assertFalse(any(p.name.endswith(".pending") for p in root.iterdir()))
            with self.assertRaises(store.Uncertain):
                store.read_retained(self.connection, ident, _CONTAINER, "main", _NODE,
                                    lambda *_: self.fail("incomplete owner never lends a body"), confirmed=True)
            with self.assertRaises(Rejected):
                store.retain_edit(file, seal, ident, confirmed=True)

    def test_exact_retained_post_is_preceded_by_absence_and_a_durable_original_attempt(self):
        from zipworkspace import macos_retained_submit as sender
        data = b"verified original whole file"
        with self.retention_state() as (store, ident, root):
            with self.opened() as file:
                key = store.retain_edit(file, self.change(file, data), ident, confirmed=True)
            fresh = self.new_connection()
            calls, proof = [], {}
            chosen = sender.review_original(fresh, ident, _CONTAINER, "main", _NODE, confirmed=True)
            self.assertEqual(chosen.key, key)
            op = chosen.action
            attempt, receipt = root / ("attempt-" + key.submission), root / ("receipt-" + key.submission)
            @contextmanager
            def wire(exchange, method, target, headers, stream):
                self.assertIs(exchange, fresh._exchange)
                calls.append((method, target))
                if method == "GET":
                    self.assertEqual(target, f"/zxs/v1/containers/{_CONTAINER}/operations/{op.operation_id}")
                    self.assertIsNone(stream)
                else:
                    self.assertEqual((method, target), ("POST", f"/zxs/v1/containers/{_CONTAINER}/branches/{op.branch}/nodes"))
                    self.assertEqual(json.loads(attempt.read_bytes())["value"], op.operation_id)
                    self.assertEqual(attempt.stat().st_mode & 0o777, 0o400)
                    self.assertEqual(headers["Zxs-Operation"], op.operation_id)
                    self.assertEqual(headers["Zxs-Fork-From-Main"], _NODE)
                    self.assertEqual(b"".join(stream), data)
                    if proof.get("respond"):
                        proof["record"] = dict(container_id=_CONTAINER, operation_id=op.operation_id, kind="save_node",
                            sequence=8, node_id=op.node_id, length=len(data), sha256=op.body.sha256,
                            target={"kind": "historical_main", "branch": op.branch, "from_node": _NODE})
                record = proof.get("record")
                status = proof.get("status", 200 if record else (404 if method == "GET" else 500))
                raw = json.dumps(record or {"state": "not_found"}).encode()
                yield SimpleNamespace(status=status, headers=[(b"content-type",b"application/json"),
                                      (b"content-length",str(len(raw)).encode())],
                                      extensions={"http_version":b"HTTP/2"}, iter_stream=lambda: iter((raw,)))
            with patch.object(edit.HttpsExchange, "_exchange", wire):
                for connection in (fresh, self.new_connection(credential_login="other")):
                    with self.assertRaises(Rejected):
                        sender.submit_original(connection, chosen)
                self.assertFalse(calls)
                absent = sender.status_original(fresh, chosen, confirmed=True)
                self.assertEqual((absent.phase, absent.attempted), (sender.Phase.ABSENT, False))
                self.assertFalse(attempt.exists() or receipt.exists())
                unknown = sender.submit_original(fresh, chosen, confirmed=True)
                self.assertEqual((unknown.phase, unknown.attempted), (sender.Phase.UNKNOWN, True))
                next_read = sender.submit_original(fresh, chosen, confirmed=True)
                self.assertEqual((next_read.phase, next_read.attempted), (sender.Phase.ABSENT, True))
                self.assertEqual([m for m, _ in calls], ["GET","GET","POST","GET"])
                proof["respond"] = True
                final = sender.retry_original(fresh, next_read, confirmed=True)
                self.assertEqual((final.phase, final.fence.head), (sender.Phase.CONFIRMED, op.node_id))
                self.assertEqual(json.loads(receipt.read_bytes())["value"]["sequence"], 8)
                self.assertEqual(receipt.stat().st_mode & 0o777, 0o400)
                self.assertEqual(sender.submit_original(fresh, chosen, confirmed=True), final)
                self.assertEqual([m for m, _ in calls], ["GET","GET","POST","GET","GET","POST","GET"])
                proof["record"] = {**proof["record"], "sha256": "0" * 64}
                self.assertNotEqual(sender.status_original(fresh, chosen, confirmed=True).phase, sender.Phase.CONFIRMED)
                self.assertEqual(sum(m == "POST" for m, _ in calls), 2)
                proof["status"] = 409
                self.assertEqual(sender.status_original(fresh, chosen, confirmed=True).phase, sender.Phase.UNKNOWN)
                self.assertTrue(receipt.is_file())
                self.assertEqual(sum(m == "POST" for m, _ in calls), 2)

    def test_torn_attempt_or_foreign_login_does_not_contact_remote(self):
        from zipworkspace import macos_retained_submit as sender
        with self.retention_state() as (store, ident, root), self.opened() as file:
            key = store.retain_edit(file, self.change(file, b"new"), ident, confirmed=True)
            fresh = self.new_connection()
            chosen = sender.review_original(fresh, ident, _CONTAINER, "main", _NODE, confirmed=True)
            marker = root / ("attempt-" + key.submission)
            marker.touch(mode=0o400)
            with patch.object(edit.HttpsExchange, "request", side_effect=AssertionError("no request")):
                with self.assertRaises(Rejected):
                    sender.submit_original(fresh, chosen, confirmed=True)
                with self.assertRaises(Rejected):
                    sender.submit_original(self.new_connection(credential_login="other"), chosen, confirmed=True)

    def test_attempt_device_flush_failure_after_valid_absence_cannot_start_post(self):
        from zipworkspace import macos_retained_submit as sender
        from zipworkspace.container_http import Reply, ReplyKind
        with self.retention_state() as (store, ident, root), self.opened() as file:
            key = store.retain_edit(file, self.change(file, b"save me"), ident, confirmed=True)
            original = sender.review_original(self.connection, ident, _CONTAINER, "main", _NODE, confirmed=True)
            attempt = root / ("attempt-" + key.submission)
            requests = []
            def wire(_exchange, message):
                requests.append(message.method)
                self.assertEqual(message.method, "GET", "cannot pass any POST before the attempted marker sync")
                return Reply(ReplyKind.ABSENT)
            native = sender._barrier
            def fail_attempt(fd):
                if attempt.exists() and os.path.samestat(attempt.stat(), os.fstat(fd)):
                    raise OSError(errno.EIO, "injected attempted-marker flush uncertainty")
                return native(fd)
            with patch.object(edit.HttpsExchange, "request", wire):
                with patch.object(sender, "_barrier", side_effect=fail_attempt), self.assertRaises(store.Uncertain):
                    sender.submit_original(self.connection, original, confirmed=True)
                self.assertEqual(requests, ["GET"])
                self.assertTrue(attempt.exists())
                # A complete but previously uncertain marker, still on the original vnode,
                # permits only status through another FIRST call. Never automatically send.
                got = sender.submit_original(self.connection, original, confirmed=True)
                self.assertEqual((got.phase, got.attempted), (sender.Phase.ABSENT, True))
                self.assertEqual(requests, ["GET", "GET"])
