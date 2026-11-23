"""Opt-in real APFS publication with synthetic responses and no network.

Use only a caller-designated, private disposable directory. The only Apple
origin changes are on fresh synthetic outputs, never existing files or settings.
"""
from contextlib import contextmanager
import ctypes as C
from hashlib import sha256
import os
from pathlib import Path
import sys
from types import SimpleNamespace
import unittest
from unittest.mock import patch

from zipworkspace import macos_node_export as export, macos_read as mac
from zipworkspace.container_https import HttpsExchange
from zipworkspace.desktop.container_connection import _Authorization
from zipworkspace.model import Rejected, new_id
from zipworkspace.publish import Uncertain

_SCRATCH = os.environ.get("ZIPWORKSPACE_APFS_DISPOSABLE_PRIVATE_DIRECTORY")
_CONTAINER, _NODE = "1" * 32, "2" * 32
_ORIGIN = "https://unit.example:443"
_TARGET = f"/zxs/v1/containers/{_CONTAINER}/nodes/{_NODE}/data"


@unittest.skipUnless(sys.platform == "darwin" and _SCRATCH, "requires a disposable private local APFS directory")
class MacNodePublication(unittest.TestCase):
    def setUp(self):
        root = Path(_SCRATCH)
        with mac.no_materialization(), mac.literal_name(root, directory=True) as parent:
            export._attributes(parent, directory=True)
            export.apfs(parent, fresh_snapshot=True)
            leaf = "remote-node-fixture-" + new_id()
            os.mkdir(leaf, 0o700, dir_fd=parent)
            self.root = root / leaf
        self.exchange = object.__new__(HttpsExchange)
        self.exchange._identity, self.exchange._location = "d" * 64, _ORIGIN
        self.exchange._authorization = _Authorization("test", "synthetic only")
        self.addCleanup(self.exchange._authorization.revoke)
        self.request = []

    @contextmanager
    def response(self, body, advertised):
        @contextmanager
        def stream(owner, method, target, headers, request_body):
            self.assertIs(owner, self.exchange)
            self.request.append((method, target, headers["Accept"], request_body))
            received = [(b"content-type", b"application/octet-stream"),
                        (b"content-length", str(len(body)).encode("ascii")),
                        (b"zxs-sha256", advertised.encode("ascii"))]
            yield SimpleNamespace(status=200, headers=received, extensions={"http_version": b"HTTP/2"},
                                  iter_stream=lambda: iter((body[:1], body[1:]) if body else ()))
        with patch.object(HttpsExchange, "_exchange", stream), patch.object(
                HttpsExchange, "_exchange_errors", return_value=(OSError, UnicodeError)):
            yield

    def publish(self, output, body, digest=None, **hooks):
        digest = sha256(body).hexdigest() if digest is None else digest
        with self.response(body, digest):
            return output.publish(self.exchange, _CONTAINER, _NODE, length=len(body), sha256=digest,
                                  confirmed=True, **hooks)

    def require_system_download(self, path, fd):
        # Separate readback on the now-published name. Do not use the publisher's
        # verify() result as the oracle, or treat a setter returning true as proof.
        native, fields = export.native(), export._download_quarantine()
        cf = C.CDLL(export._CF)
        string_bytes = export._bind(cf, "CFStringGetCString", C.c_ubyte, C.c_void_p,
                                    C.POINTER(C.c_char), C.c_long, C.c_uint32)
        expected = (_ORIGIN + _TARGET).encode("ascii") + b"\0"
        actual = C.create_string_buffer(len(expected))
        self.assertIn(b"com.apple.quarantine", native.attrs(fd))
        with native.quarantine(path, fd, literal=True) as value:
            kind, source = fields._get(value, fields._kind), fields._get(value, fields._data)
            self.assertTrue(kind and native.equal(kind, fields._download), "wrong native quarantine type")
            self.assertTrue(source, "native quarantine has no download URL")
            if native.type(source) == fields._url_type(): source = fields._url_string(source)
            self.assertTrue(source and native.type(source) == fields._string_type())
            self.assertTrue(string_bytes(source, actual, len(actual), 0x08000100), "no complete native URL bytes")
            self.assertEqual(actual.raw, expected)

    def test_review_has_no_effect_then_exact_body_and_system_origin_survive_real_rename(self):
        for name, body in (("file.txt", b"abc"), ("empty.txt", b"")):
            with self.subTest(name=name), export.MacNodeOutput(self.root, name) as output:
                self.assertFalse(output.path.exists())
                stage = self.root / output._stage
                self.assertFalse(stage.exists())
                staged_identity = []

                def before_rename():
                    observed = stage.stat(follow_symlinks=False)
                    staged_identity.append((observed.st_dev, observed.st_ino))

                self.assertEqual(self.publish(output, body, begin_publication=before_rename), output.path)
                self.assertFalse(stage.exists())
                with mac.no_materialization(), output.path.open("rb") as file:
                    self.assertEqual(file.read(), body)
                    meta = os.fstat(file.fileno())
                    self.assertEqual(meta.st_mode & 0o777, 0o600)
                    self.assertEqual(staged_identity, [(meta.st_dev, meta.st_ino)])
                    self.require_system_download(output.path, file.fileno())
                with self.assertRaises(Rejected):
                    self.publish(output, body)
        self.assertEqual(self.request, [("GET", _TARGET, "application/octet-stream", None)] * 2)

    def test_wrong_body_or_ignored_origin_setter_never_publishes(self):
        with export.MacNodeOutput(self.root, "bad.txt") as output, self.assertRaisesRegex(
                Rejected, "node GET bytes disagreed with the selected SHA-256"):
            self.publish(output, b"bad", digest=sha256(b"one").hexdigest())
        self.assertFalse((self.root / "bad.txt").exists())
        with export.MacNodeOutput(self.root, "ignored.txt") as output, patch.object(
                export._download_quarantine(), "_set", return_value=True) as ignored:
            with self.assertRaises(Rejected): self.publish(output, b"no marker")
            ignored.assert_called_once()
        self.assertFalse((self.root / "ignored.txt").exists())
        self.assertEqual(self.request, [("GET", _TARGET, "application/octet-stream", None)] * 2)

    def test_late_occupied_name_remains_unchanged(self):
        def occupy():
            with (self.root / "occupied.txt").open("xb") as winner:
                winner.write(b"existing")
        with export.MacNodeOutput(self.root, "occupied.txt") as output, self.assertRaisesRegex(Rejected, "没有覆盖"):
            self.publish(output, b"new", begin_publication=occupy)
        self.assertEqual((self.root / "occupied.txt").read_bytes(), b"existing")

    def test_last_callback_cannot_publish_a_replaced_stage(self):
        with export.MacNodeOutput(self.root, "never.txt") as output:
            stage, original = self.root / output._stage, self.root / "held-original.partial"

            def substitute():
                stage.rename(original)
                with stage.open("xb") as replacement:
                    replacement.write(b"new")  # Same length, different actual inode.

            with patch.object(export, "rename_new_at") as rename, self.assertRaises(Rejected):
                self.publish(output, b"old", begin_publication=substitute)
            rename.assert_not_called()
            self.assertFalse(output.path.exists())
            self.assertEqual(original.read_bytes(), b"old")  # Callback really ran.
            self.assertEqual(stage.read_bytes(), b"new")

    def test_failure_after_real_rename_is_not_reported_as_no_publication(self):
        move = export.rename_new_at
        def lost_result(*args):
            move(*args)
            raise OSError("injected uncertainty after native publication")
        with export.MacNodeOutput(self.root, "uncertain.txt") as output, patch.object(
                export, "rename_new_at", side_effect=lost_result), self.assertRaises(Uncertain):
            self.publish(output, b"possible")
        self.assertEqual((self.root / "uncertain.txt").read_bytes(), b"possible")

    def test_no_confirmation_or_revoked_source_never_stages_or_requests(self):
        with export.MacNodeOutput(self.root, "stopped.txt") as output:
            with self.assertRaises(Rejected):
                output.publish(self.exchange, _CONTAINER, _NODE, length=0, sha256=sha256(b"").hexdigest())
            self.exchange._authorization.revoke()
            with self.assertRaises(Rejected): self.publish(output, b"")
        self.assertEqual(list(self.root.iterdir()), [])
        self.assertEqual(self.request, [])
