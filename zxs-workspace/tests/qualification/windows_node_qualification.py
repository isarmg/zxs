"""Opt-in exact R3 node export on native Windows NTFS, with no server connection."""
from concurrent.futures import ThreadPoolExecutor
from contextlib import ExitStack, contextmanager
from hashlib import sha256
import os
from pathlib import Path
import sys
from threading import Event
from types import SimpleNamespace
import unittest

if __name__ == "__main__":
    # Run from a checkout with isolated Python, not from a writable installed search path.
    sys.path.insert(0, str(Path(__file__).resolve().parents[2]))

from zipworkspace import _windows_node as export
from zipworkspace._windows_access import NativeSecurity
from zipworkspace._windows_origin import DATA, ZONE
from zipworkspace._windows_output import PinnedParent
from zipworkspace.container_https import HttpsExchange
from zipworkspace.desktop.container_connection import OpenConnection, _Authorization
from zipworkspace.desktop.container_management import Branch, Container, Detail, Node
from zipworkspace.desktop.container_windows_download import WindowsContainerNodeCopy
from zipworkspace.model import Rejected, new_id


CONTAINER, NODE = "1" * 32, "2" * 32
ORIGIN = "https://workspace-node-qualification.invalid:443"
TARGET = f"/zxs/v1/containers/{CONTAINER}/nodes/{NODE}/data"
PAYLOAD = b"\0original selected bytes\r\n\xff" * 253


class NativeNodePublication(unittest.TestCase):
    def __init__(self, method, root, spelling, security, actor, profile, zone):
        super().__init__(method)
        self.root, self.spelling = root, spelling
        self.security, self.actor, self.profile, self.zone = security, actor, profile, zone

    def setUp(self):
        root, security, actor = self.root, self.security, self.actor
        root.check()
        self.assertEqual(security.read(root.handle, actor, directory=True), self.profile)
        leaf = "zw-node-" + new_id()
        native = Path(root.name(leaf))
        # Fresh ordinary inheritance from the already admitted directory. A special
        # mkdir(0700) DACL would not exercise the existing original output profile.
        native.mkdir()
        case = PinnedParent(self.spelling / leaf, output=True)
        self.addCleanup(case.close)
        self.assertEqual(case.chain[-1][2], str(native))
        security.read(case.handle, actor, directory=True)
        self.staging, self.destination = self.spelling / leaf / "stage", self.spelling / leaf / "out"
        Path(case.name("stage")).mkdir()
        Path(case.name("out")).mkdir()
        self.requests = []

    def response(self, parts, expected, *, version=b"HTTP/2", before=None, after=None):
        # Replace only the already-framed transport. Keep the exact production
        # HttpsExchange type, request target, framing, count and SHA read path.
        exchange = object.__new__(HttpsExchange)
        exchange._identity, exchange._location = "d" * 64, ORIGIN
        exchange._authorization = _Authorization("fixture", "synthetic only")
        self.addCleanup(exchange._authorization.revoke)
        exchange._exchange_errors = lambda: (OSError, UnicodeError)

        @contextmanager
        def stream(method, target, headers, body):
            self.assertEqual((method, target, headers, body),
                             ("GET", TARGET, {"Accept": "application/octet-stream",
                                              "Accept-Encoding": "identity"}, None))
            self.requests.append(target)
            if before is not None:
                before()
            yield SimpleNamespace(
                status=200, extensions={"http_version": version},
                headers=[(b"content-type", b"application/octet-stream"),
                         (b"content-length", str(len(expected)).encode("ascii")),
                         (b"zxs-sha256", sha256(expected).hexdigest().encode("ascii"))],
                iter_stream=lambda: iter(parts))
            if after is not None:
                after()

        exchange._exchange = stream
        return exchange

    def choose(self, name):
        return export.WindowsNodeOutput(self.destination, name, staging_parent=self.staging)

    def publish(self, plan, exchange, expected, *, confirmed=True):
        return plan.publish(exchange, CONTAINER, NODE, length=len(expected), sha256=sha256(expected).hexdigest(),
                            confirmed=confirmed, checkpoint=exchange._authorization.require_live,
                            begin_publication=lambda: None)

    def stage_id(self, plan):
        stage = plan._staging
        with stage.api.witness(stage.handle, plan._stage) as h:
            row = stage.api.ordinary(h, stage.volume, stage.name(plan._stage))
            self.assertEqual(row.size, 0)  # Observed before the first response byte.
            return stage.volume, row.file_id

    def check_result(self, receipt, name, body, stage_id=None):
        # Compare the native file and ADS independently of the exporter's verification helper.
        mark = f"[ZoneTransfer]\r\nZoneId={self.zone}\r\nHostUrl={ORIGIN}{TARGET}\r\n".encode("ascii")
        with PinnedParent(self.destination, output=True) as dest:
            native = dest.name(name)
            self.assertEqual(receipt, {"container": CONTAINER, "node": NODE, "bytes": len(body),
                                       "sha256": sha256(body).hexdigest(), "new_file": native,
                                       "origin_recorded": True})
            self.assertEqual(Path(native).read_bytes(), body)
            self.assertEqual(Path(native + ":Zone.Identifier").read_bytes(), mark)
            with dest.api.inspected_file(dest.handle, name) as held:
                item = dest.api.ordinary(held, dest.volume, native)
                if stage_id is not None:
                    self.assertEqual((dest.volume, item.file_id), stage_id)
                self.assertEqual(dest.api.streams(held), {DATA: len(body), ZONE: len(mark)})
                inherited = self.security.read(held, self.actor, directory=False)
                parent = self.security.read(dest.handle, self.actor, directory=True)
                self.assertEqual(inherited.group, parent.group)
                self.assertEqual({sid for sid, _ in inherited.grants}, {sid for sid, _ in parent.grants})
        self.assertEqual(list(self.staging.iterdir()), [])

    def test_exact_binary_and_zero_length_preserve_native_file_identity_acl_and_zone(self):
        for name, body in (("exact.bin", PAYLOAD), ("zero.bin", b"")):
            with self.subTest(name=name), self.choose(name) as plan:
                identity = []
                parts = (body[:3], b"", body[3:]) if body else ()
                exchange = self.response(parts, body, before=lambda: identity.append(self.stage_id(plan)))
                receipt = self.publish(plan, exchange, body)
                self.assertEqual(len(identity), 1)
                self.check_result(receipt, name, body, identity[0])
        self.assertEqual(self.requests, [TARGET, TARGET])

    def test_late_case_insensitive_collision_leaves_actual_occupant(self):
        with self.choose("conflict.bin") as plan:
            target = Path(plan._destination.name("CONFLICT.BIN"))

            def compete():
                with target.open("xb") as original:
                    original.write(b"the other writer's bytes")

            exchange = self.response((PAYLOAD,), PAYLOAD, after=compete)
            with self.assertRaisesRegex(Rejected, "目的名称已被占用，没有覆盖"):
                self.publish(plan, exchange, PAYLOAD)
        self.assertEqual(target.read_bytes(), b"the other writer's bytes")
        self.assertEqual([path.name for path in self.destination.iterdir()], ["CONFLICT.BIN"])
        self.assertEqual(self.requests, [TARGET])

    def test_non_h2_short_or_wrong_digest_never_publish(self):
        for name, parts, version, error in (
                ("not-h2.bin", (b"good",), b"HTTP/1.1", "HTTP/2"),
                ("short.bin", (b"", b"goo"), b"HTTP/2", "ended before"),
                ("wrong.bin", (b"evil",), b"HTTP/2", "disagreed with the selected SHA-256")):
            with self.subTest(name=name), self.choose(name) as plan:
                exchange = self.response(parts, b"good", version=version)
                with self.assertRaisesRegex(Rejected, error):
                    self.publish(plan, exchange, b"good")
        self.assertEqual(list(self.destination.iterdir()), [])
        self.assertEqual(self.requests, [TARGET] * 3)

    def test_without_confirmation_or_after_unclaimed_close_no_files_or_requests(self):
        with self.choose("declined.bin") as plan:
            exchange = self.response((PAYLOAD,), PAYLOAD)
            with self.assertRaisesRegex(Rejected, "请先确认"):
                self.publish(plan, exchange, PAYLOAD, confirmed=False)
            plan.close()
            with self.assertRaisesRegex(Rejected, "已关闭或已开始"):
                self.publish(plan, exchange, PAYLOAD)
        self.assertEqual(self.requests, [])
        self.assertEqual(list(self.staging.iterdir()), [])
        self.assertEqual(list(self.destination.iterdir()), [])

    def test_revocation_during_last_client_callback_never_publishes(self):
        callbacks = []
        with self.choose("revoked.bin") as plan:
            exchange = self.response((PAYLOAD,), PAYLOAD)

            def disconnect():
                callbacks.append(True)
                exchange._authorization.revoke()

            with self.assertRaisesRegex(Rejected, "this window disconnected"):
                plan.publish(exchange, CONTAINER, NODE, length=len(PAYLOAD), sha256=sha256(PAYLOAD).hexdigest(),
                             confirmed=True, checkpoint=lambda: None, begin_publication=disconnect)
        self.assertEqual(callbacks, [True])
        self.assertEqual(self.requests, [TARGET])
        self.assertEqual(list(self.destination.iterdir()), [])

    def test_desktop_plan_review_decline_and_disconnect_never_requests(self):
        exchange = self.response((PAYLOAD,), PAYLOAD)
        authorization = exchange._authorization
        connection = OpenConnection(None, authorization, exchange, "fixture", _listed=True)
        node = Node(NODE, None, len(PAYLOAD), sha256(PAYLOAD).hexdigest())
        branch = Branch("main", NODE, (node,))
        detail = Detail(Container(CONTAINER, "selected", 1), (), (branch,))
        plan = WindowsContainerNodeCopy(connection, detail, branch, node, self.destination,
                                        "reviewed.bin", staging_parent=self.staging)
        try:
            review = plan.review()
            self.assertIn(NODE, review)
            self.assertIn(node.sha256, review)
            self.assertIn("reviewed.bin", review)
            self.assertEqual(self.requests, [])
            self.assertEqual(list(self.staging.iterdir()), [])
            self.assertEqual(list(self.destination.iterdir()), [])
            with self.assertRaisesRegex(Rejected, "请先确认"):
                plan.run(confirmed=False)
            authorization.revoke()
            with self.assertRaisesRegex(Rejected, "this window disconnected"):
                plan.run(confirmed=True)
            with self.assertRaises(Rejected):
                plan.run(confirmed=True)
        finally:
            plan.close()
        self.assertEqual(self.requests, [])
        self.assertEqual(list(self.staging.iterdir()), [])
        self.assertEqual(list(self.destination.iterdir()), [])

    def test_close_and_second_publish_cannot_take_worker_handles(self):
        started, resume = Event(), Event()

        def pause():
            started.set()
            self.assertTrue(resume.wait(45), "synthetic worker was not released")

        with self.choose("owned.bin") as plan, ThreadPoolExecutor(max_workers=1) as pool:
            exchange = self.response((PAYLOAD,), PAYLOAD, before=pause)
            work = pool.submit(self.publish, plan, exchange, PAYLOAD)
            try:
                self.assertTrue(started.wait(30), "worker never reached the synthetic GET")
                plan.close()
                with self.assertRaisesRegex(Rejected, "已关闭或已开始"):
                    self.publish(plan, exchange, PAYLOAD)
            finally:
                resume.set()
            receipt = work.result(timeout=45)
        self.check_result(receipt, "owned.bin", PAYLOAD)
        self.assertEqual(self.requests, [TARGET])


def qualify(spelling, *, hosted):
    if sys.platform != "win32":
        raise SystemExit("Windows native node qualification requires actual Windows.")
    with ExitStack() as pins:
        try:
            security = NativeSecurity()
            actor = security.caller()
            root = pins.enter_context(PinnedParent(spelling, output=True))
            profile = security.read(root.handle, actor, directory=True)
            zone = export._effective_zone(ORIGIN + TARGET)
            if zone not in (3, 4):
                raise Rejected("existing Windows policy does not mark the inert qualification URL")
        except (OSError, Rejected) as error:
            print(f"Windows native exact-node output: NOT RUN (capability preflight: {error}).", flush=True)
            return 0 if hosted else 2

        names = unittest.defaultTestLoader.getTestCaseNames(NativeNodePublication)
        if len(names) != 7:
            raise RuntimeError("expected all seven mandatory Windows native output cases")
        suite = unittest.TestSuite(NativeNodePublication(name, root, spelling, security, actor, profile, zone)
                                   for name in names)
        result = unittest.TextTestRunner(verbosity=2).run(suite)
        if not result.wasSuccessful() or result.testsRun != len(names) or result.skipped:
            return 1
        print("Windows native exact-node output: seven cases passed on NTFS with real URL policy "
              "and synthetic H2 responses; no actual TLS connection or UI.", flush=True)
        return 0


if __name__ == "__main__":
    args = sys.argv[1:]
    hosted = len(args) == 2 and args[0] == "--hosted"
    if hosted and os.environ.get("GITHUB_ACTIONS") != "true":
        raise SystemExit("--hosted is only for the dedicated GitHub-hosted qualification.")
    if not hosted and (len(args) != 1 or args[0].startswith("-")):
        raise SystemExit("Supply one existing, disposable, private ordinary-user NTFS directory.")
    raise SystemExit(qualify(Path(args[-1]), hosted=hosted))
