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

    def choose(self, output, body, answer=True):
        # Substitute only the human decision; never substitute native metadata
        # in the real-APFS successful explicit export fixture below.
        def respond(detail):
            for value in (repr(str(output.path)), "账户：'test'", _ORIGIN + _TARGET, sha256(body).hexdigest(),
                          f"{len(body):,} 字节", "即使系统未返回地址，也可能保留它"):
                self.assertIn(value, detail)
            return answer

        with patch("zipworkspace._macos_save_alert.approve_new_file", side_effect=respond) as alert:
            permit = output.approve_save_as(self.exchange, _CONTAINER, _NODE, length=len(body),
                                            sha256=sha256(body).hexdigest())
            alert.assert_called_once()
            return permit

    def test_explicit_save_as_real_quarantine_with_only_human_decision_substituted(self):
        from zipworkspace.desktop.container_connection import OpenConnection
        from zipworkspace.desktop.container_macos_download import MacNodeSaveAs
        from zipworkspace.desktop.container_management import Branch, Container, Detail, Node

        for name, body in (("native-explicit.txt", b"actual node"), ("native-empty.txt", b"")):
            with self.subTest(name=name):
                node = Node(_NODE, None, len(body), sha256(body).hexdigest())
                branch = Branch("main", node.id, (node,))
                detail = Detail(Container(_CONTAINER, "fixture", 1), (), (branch,))
                owner = OpenConnection(None, self.exchange._authorization, self.exchange, "test", _listed=True)
                path = self.root / name
                plan = MacNodeSaveAs(owner, detail, branch, node, str(path))
                self.addCleanup(plan.close)
                plan.review()
                self.assertFalse(path.exists())
                def accept(text):
                    for field in (repr(str(path)), _ORIGIN + _TARGET, node.sha256, "即使系统未返回地址，也可能保留它"):
                        self.assertIn(field, text)
                    return True
                with patch("zipworkspace._macos_save_alert.approve_new_file", side_effect=accept) as human:
                    permit = plan.approve()
                    human.assert_called_once()
                self.assertFalse(path.exists())
                with self.response(body, node.sha256):
                    result = plan.run(permit)

                # Independent OS observation on the final name and new FD.
                with mac.no_materialization(), path.open("rb") as saved:
                    fd, fields, api = saved.fileno(), export._download_quarantine(), export.native()
                    self.assertEqual(saved.read(), body)
                    self.assertIn(b"com.apple.quarantine", api.attrs(fd))
                    with api.quarantine(path, fd, literal=True) as observed:
                        kind = fields._get(observed, fields._kind)
                        self.assertTrue(kind and api.type(kind) == fields._string_type() and api.equal(kind, fields._download))
                        reported = bool(fields._get(observed, fields._data))
                    if reported:
                        self.require_system_download(path, fd)
                    self.assertIn("系统已返回并核对本次下载地址" if reported else "没有返回来源地址", result)
                self.assertFalse(any(item.name.startswith(export._STAGE_PREFIX) for item in self.root.iterdir()))
        self.assertEqual(self.request, [("GET", _TARGET, "application/octet-stream", None)] * 2)

    def test_explicit_save_as_cancel_foreign_grant_and_changed_parent_make_no_request(self):
        body = b"old"
        child, decoy = self.root / "chosen", self.root / "other"
        child.mkdir(mode=0o700)
        decoy.mkdir(mode=0o700)
        with export.MacNodeOutput(child, "same.txt") as chosen, export.MacNodeOutput(decoy, "same.txt") as other:
            self.assertIsNone(self.choose(chosen, body, answer=False))
            for invalid in (None, True, object()):
                with self.assertRaises(Rejected):
                    chosen.publish_save_as(invalid)
            permit = self.choose(chosen, body)
            with self.assertRaises(Rejected):
                other.publish_save_as(permit)
            # Replace the reviewed parent at exactly the same spelling. Neither
            # the decoy, the replacement nor the held original may get a stage.
            moved = self.root / "held"
            child.rename(moved)
            child.mkdir(mode=0o700)
            with self.assertRaises(Rejected):
                chosen.publish_save_as(permit)
            self.assertEqual([list(p.iterdir()) for p in (child, decoy, moved)], [[], [], []])
            with self.assertRaises(Rejected):
                chosen.publish_save_as(permit)
        self.assertEqual(self.request, [])

    def test_explicit_save_as_refuses_real_collision_and_post_consent_revocation(self):
        body = b"verified"
        with export.MacNodeOutput(self.root, "late.txt") as output:
            permit = self.choose(output, body)
            def occupy():
                with output.path.open("xb") as existing:
                    existing.write(b"untouched")
            with self.response(body, sha256(body).hexdigest()), self.assertRaisesRegex(Rejected, "没有覆盖"):
                output.publish_save_as(permit, begin_publication=occupy)
            self.assertEqual(output.path.read_bytes(), b"untouched")
            self.assertEqual((self.root / output._stage).read_bytes(), body)
        with export.MacNodeOutput(self.root, "revoked.txt") as output:
            permit = self.choose(output, body)
            self.exchange._authorization.revoke()
            with self.assertRaises(Rejected): output.publish_save_as(permit)
            self.assertFalse(output.path.exists())
            self.assertFalse((self.root / output._stage).exists())
        self.assertEqual(len(self.request), 1)

    def test_explicit_save_as_source_change_at_commit_and_unknown_after_actual_move(self):
        body = b"exact selected bytes"
        digest = sha256(body).hexdigest()
        with export.MacNodeOutput(self.root, "no-switch.txt") as output:
            permit = self.choose(output, body)
            old = self.exchange._identity
            try:
                with self.response(body, digest), self.assertRaises(Rejected):
                    output.publish_save_as(permit, begin_publication=lambda: setattr(self.exchange, "_identity", "a"*64))
            finally:
                self.exchange._identity = old
            self.assertFalse(output.path.exists())
            self.assertEqual((self.root / output._stage).read_bytes(), body)
        move = export.rename_new_at
        def lose_reply(*arguments):
            move(*arguments)
            raise OSError("injected failure after actual exclusive rename")
        with export.MacNodeOutput(self.root, "inspect.txt") as output:
            permit = self.choose(output, body)
            with self.response(body, digest), patch.object(export, "rename_new_at", side_effect=lose_reply):
                with self.assertRaises(Uncertain): output.publish_save_as(permit)
            self.assertEqual(output.path.read_bytes(), body)
            with self.assertRaises(Rejected): output.publish_save_as(permit)
        self.assertEqual(len(self.request), 2)

    def test_explicit_save_as_refuses_url_loss_or_revocation_during_last_native_check(self):
        body = b"verified privately"
        digest = sha256(body).hexdigest()
        fields = export._download_quarantine(require_url=False)
        get = fields._get
        # Negative boundary injection: even on an OS which never retains DataURL,
        # exercise a matching URL initially present and missing only at commit.
        with fields.url(_ORIGIN + _TARGET) as expected, export.MacNodeOutput(self.root, "lost-url.txt") as output:
            permit = self.choose(output, body)
            commit = []
            def observed(data, key):
                if key == fields._data:
                    return None if commit else expected
                return get(data, key)
            with self.response(body, digest), patch.object(fields, "_get", side_effect=observed):
                with self.assertRaisesRegex(Rejected, "原下载标记已改变"):
                    output.publish_save_as(permit, begin_publication=lambda: commit.append(True))
            self.assertFalse(output.path.exists())
            self.assertEqual((self.root / output._stage).read_bytes(), body)

        # A valid observation can itself take long enough for the original
        # connection to be revoked; no name may be published on its return.
        with export.MacNodeOutput(self.root, "late-revoke.txt") as output:
            permit, verify, commit = self.choose(output, body), fields.verify, []
            def revoke_late(*args):
                witness = verify(*args)
                if commit: self.exchange._authorization.revoke()
                return witness
            with self.response(body, digest), patch.object(fields, "verify", side_effect=revoke_late):
                with self.assertRaises(Rejected):
                    output.publish_save_as(permit, begin_publication=lambda: commit.append(True))
            self.assertFalse(output.path.exists())
            self.assertEqual((self.root / output._stage).read_bytes(), body)

    def test_explicit_save_as_rejects_missing_mark_and_present_wrong_url(self):
        body = b"must never be written"
        fields = export._download_quarantine(require_url=False)
        with export.MacNodeOutput(self.root, "unmarked.txt") as output:
            permit = self.choose(output, body)
            with self.response(body, sha256(body).hexdigest()), patch.object(fields, "_set", return_value=True):
                with self.assertRaises(Rejected): output.publish_save_as(permit)
            self.assertFalse(output.path.exists())
            self.assertEqual((self.root / output._stage).read_bytes(), b"")
        # Negative injection only: if the OS returns a different URL, user
        # consent must NOT convert it to the allowed "URL not reported" case.
        get = fields._get
        with fields.url("https://wrong.invalid/") as wrong, export.MacNodeOutput(self.root, "wrong-url.txt") as output:
            permit = self.choose(output, body)
            def mismatch(observed, key):
                return wrong if key == fields._data else get(observed, key)
            with self.response(body, sha256(body).hexdigest()), patch.object(fields, "_get", side_effect=mismatch):
                with self.assertRaisesRegex(Rejected, "与本次请求不同"): output.publish_save_as(permit)
            self.assertFalse(output.path.exists())
            self.assertEqual((self.root / output._stage).read_bytes(), b"")

    def test_local_destination_preflight_reports_native_name_collision(self):
        original = self.root / "Keep.txt"
        with mac.no_materialization(), mac.literal_name(self.root, directory=True) as parent:
            flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_CLOEXEC
            fd = os.open(original.name, flags, 0o600, dir_fd=parent)
            with os.fdopen(fd, "wb") as out:
                out.write(b"preserve")
            with self.assertRaisesRegex(Rejected, "已被"):
                export._absent(parent, original.name)
            try:
                alias = os.stat("keep.txt", dir_fd=parent, follow_symlinks=False)
            except FileNotFoundError:
                # Sensitive APFS must admit the different spelling at preflight.
                export._absent(parent, "keep.txt")
            else:
                expected = os.stat(original.name, dir_fd=parent, follow_symlinks=False)
                self.assertEqual((alias.st_dev, alias.st_ino), (expected.st_dev, expected.st_ino))
                with self.assertRaisesRegex(Rejected, "与已有名称视为同一名称"):
                    export._absent(parent, "keep.txt")
        self.assertEqual(list(self.root.iterdir()), [original])
        self.assertEqual(original.read_bytes(), b"preserve")

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

    @contextmanager
    def synthetic_provenance_for_byte_and_rename_tests(self):
        # Test-only seam. Keep real APFS file guards and native exclusive rename.
        # No test using this seam may attest that the system returned an origin.
        phase, witness = [], object()

        @contextmanager
        def isolated_url(url):
            self.assertEqual(url, _ORIGIN + _TARGET)
            yield url

        def simulate_mark(path, fd, url):
            self.assertEqual(url, _ORIGIN + _TARGET)
            self.assertEqual(os.fstat(fd).st_size, 0)
            phase.append(("mark", path.name, os.fstat(fd).st_ino))
            return witness

        def simulate_verify(path, fd, url):
            self.assertEqual(url, _ORIGIN + _TARGET)
            self.assertEqual(os.fstat(fd).st_ino, phase[0][2])
            phase.append(("verify", path.name, os.fstat(fd).st_ino))
            return witness

        fixture = SimpleNamespace(url=isolated_url, mark=simulate_mark, verify=simulate_verify)
        with patch.object(export, "_download_quarantine", return_value=fixture):
            yield phase

    def test_synthetic_provenance_wrong_body_never_reaches_publication(self):
        with self.synthetic_provenance_for_byte_and_rename_tests() as phase, export.MacNodeOutput(
                self.root, "bad-synthetic.txt") as output, patch.object(export, "rename_new_at") as rename:
            def not_ready():
                self.fail("digest mismatch must not allow publication")
            with self.assertRaisesRegex(Rejected, "node GET bytes disagreed with the selected SHA-256"):
                self.publish(output, b"bad", digest=sha256(b"one").hexdigest(), begin_publication=not_ready)
            rename.assert_not_called()
            self.assertFalse(output.path.exists())
            self.assertEqual([step[0] for step in phase], ["mark"])

    def test_synthetic_provenance_late_collision_keeps_both_original_files(self):
        def occupy():
            with (self.root / "occupied-synthetic.txt").open("xb") as winner:
                winner.write(b"existing")

        with self.synthetic_provenance_for_byte_and_rename_tests() as phase, export.MacNodeOutput(
                self.root, "occupied-synthetic.txt") as output:
            stage = self.root / output._stage
            with self.assertRaisesRegex(Rejected, "没有覆盖"):
                self.publish(output, b"new", begin_publication=occupy)
            self.assertEqual(output.path.read_bytes(), b"existing")
            self.assertEqual(stage.read_bytes(), b"new")
            self.assertEqual([step[0] for step in phase], ["mark", "verify"])

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

    def test_native_setter_value_representations(self):
        """Record native observation only; qualification still requires the exact URL."""
        native, fields = export.native(), export._download_quarantine()
        flags = os.O_RDWR | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_CLOEXEC
        with mac.no_materialization(), mac.literal_name(self.root, directory=True) as parent:
            with fields.url(_ORIGIN + _TARGET) as url:
                string = fields._url_string(url)
                self.assertTrue(string and native.type(string) == fields._string_type())
                for label, supplied in (("CFURL", url), ("CFString", string)):
                    with self.subTest(value=label):
                        leaf = "origin-probe-" + label
                        fd = os.open(leaf, flags, 0o600, dir_fd=parent)
                        try:
                            path = self.root / leaf
                            export._attributes(fd)
                            self.assertEqual(export.apfs(fd), export.apfs(parent))
                            native.no_payload_metadata(path, fd, literal=True)
                            try:
                                result = self.native_setter_probe(native, fields, path, fd, supplied, string)
                            except Rejected:
                                result = "native query rejected"
                        finally:
                            os.close(fd)
                        print(f"macOS quarantine probe {label}: {result}", flush=True)

    def native_setter_probe(self, native, fields, path, fd, supplied, expected):
        keys = (C.c_void_p * 2)(fields._kind, fields._data)
        values = (C.c_void_p * 2)(fields._download, supplied)
        attrs = fields._dictionary(None, keys, values, 2, fields._keys, fields._values)
        self.assertTrue(attrs, "cannot create native probe dictionary")
        try:
            with native._file_reference(path, fd, False, literal=True) as (reference, check):
                check()
                accepted = bool(fields._set(reference, native.key, attrs, None))
                check()
        finally:
            native.release(attrs)
        with native._quarantine_property(path, fd, False, literal=True) as observed:
            if not observed:
                return f"setter_accepted={accepted} property=absent"
            if native.type(observed) != native.dict_type() or not 0 < native.count(observed) <= 64:
                return f"setter_accepted={accepted} property=unexpected"
            kind = fields._get(observed, fields._kind)
            kind_matches = bool(kind and native.type(kind) == fields._string_type()
                                and native.equal(kind, fields._download))
            return self.native_probe_url(fields, native, accepted, kind_matches,
                                         fields._get(observed, fields._data), expected)

    @staticmethod
    def native_probe_url(fields, native, accepted, kind_matches, source, expected):
        source_type, match = "absent", False
        if source:
            type_id = native.type(source)
            if type_id == fields._url_type():
                source_type = "CFURL"
                source = fields._url_string(source)
            elif type_id == fields._string_type():
                source_type = "CFString"
            else:
                source_type, source = "unexpected", None
            match = bool(source and native.type(source) == fields._string_type()
                         and native.equal(source, expected))
        return (f"setter_accepted={accepted} kind_matches={kind_matches} "
                f"source={source_type} source_matches={match}")
