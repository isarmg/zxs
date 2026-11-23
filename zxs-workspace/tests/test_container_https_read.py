from contextlib import contextmanager
import hashlib
from types import SimpleNamespace
import unittest

from zipworkspace.container_https import HttpsExchange
from zipworkspace.desktop.container_connection import _History, _Metadata, _catalog_page, _detail_page
from zipworkspace.model import Rejected


ROOT = "/zxs/v1/containers"
CONTAINER, NODE = "1" * 32, "2" * 32


def reply(parts, media, *, length=None, digest=None, status=200, extra=()):
    size = sum(map(len, parts)) if length is None else length
    headers = [(b"content-type", media), (b"content-length", str(size).encode())]
    if digest is not None:
        headers.append((b"zxs-sha256", digest.encode()))
    return SimpleNamespace(status=status, headers=headers + list(extra),
                           extensions={"http_version": b"HTTP/2"}, iter_stream=lambda: iter(parts))


class ExchangeFixture(HttpsExchange):
    def __init__(self, response, fail_close=False):
        self.response = response
        self.fail_close = fail_close
        self.calls = []
        self.headers = []
        self.closed = False

    def _exchange_errors(self):
        return (OSError, UnicodeError)

    @contextmanager
    def _exchange(self, method, target, headers, body):
        self.calls.append((method, target, headers["Accept"], body))
        self.headers.append(headers)
        yield self.response
        self.closed = True
        if self.fail_close:
            raise OSError("injected close failure")


class NativeReadTest(unittest.TestCase):
    def test_metadata_fixed_routes_and_strict_json(self):
        media = b"application/json"
        path = f"{ROOT}/{CONTAINER}"
        exchange = ExchangeFixture(reply((b"{", b"}"), media))
        self.assertEqual(exchange.read_container_detail_page(CONTAINER), {})
        self.assertTrue(exchange.closed)
        self.assertEqual(exchange.calls, [("GET", path, "application/json", None)])
        self.assertEqual(exchange.headers[0]["Zxs-Detail"], "paged")
        for body in (b'{"x":0,"x":1}', b"[NaN]", b"[\xff]", b"[]tail"):
            exchange = ExchangeFixture(reply((body,), media))
            with self.assertRaises(Rejected):
                exchange.read_container_detail_page(CONTAINER)
        exchange = ExchangeFixture(reply((b"[]",), media))
        for invalid in ("/outside", ROOT, ROOT + "?q=1", f"{path}/nodes/{NODE}/data"):
            with self.assertRaises(Rejected):
                exchange.read_container_detail_page(invalid)
        self.assertFalse(exchange.calls)

    def test_non_h2_response_never_reaches_metadata_or_node_reader(self):
        for extensions in ({"http_version": b"HTTP/1.1"}, {}):
            with self.subTest(extensions=extensions):
                exchange = ExchangeFixture(SimpleNamespace(extensions=extensions))
                with self.assertRaisesRegex(Rejected, "HTTP/2"):
                    exchange.read_container_detail_page(CONTAINER)
                with self.assertRaisesRegex(Rejected, "HTTP/2"):
                    exchange.read_node_into(CONTAINER, NODE, length=0, sha256="0" * 64,
                                            write=lambda _: self.fail("non-HTTP/2 response reached node sink"))

    def test_large_file_uses_exact_node_route_and_no_metadata_cap(self):
        block = b"a" * (600 * 1024)
        digest = hashlib.sha256(block + block).hexdigest()
        exchange = ExchangeFixture(reply((block, block), b"application/octet-stream", digest=digest))
        with exchange.download_node(CONTAINER, NODE, length=2 * len(block), sha256=digest) as stream:
            self.assertTrue(exchange.closed)
            self.assertFalse(stream.writable())
            self.assertEqual(stream.read(len(block)), block)
            self.assertEqual(stream.read(len(block)), block)
            self.assertEqual(stream.read(1), b"")
        self.assertTrue(stream.closed)
        self.assertEqual(exchange.calls, [("GET", f"{ROOT}/{CONTAINER}/nodes/{NODE}/data",
                                           "application/octet-stream", None)])

    def test_bad_framing_body_stale_metadata_and_close_failure_never_yield(self):
        media = b"application/octet-stream"
        digest = hashlib.sha256(b"ok").hexdigest()
        cases = (
            (reply((b"o",), media, length=2, digest=digest), False),
            (reply((b"okay",), media, length=2, digest=digest), False),
            (reply((b"no",), media, length=2, digest=digest), False),
            (reply((b"ok",), media, digest="0" * 64), False),
            (reply((b"ok",), media, digest=digest, extra=((b"content-length", b"2"),)), False),
            (reply((b"ok",), media, digest=digest, extra=((b"content-range", b"bytes 0-1/2"),)), False),
            (reply((b"ok",), media, digest=digest, status=302), False),
            (reply((b"ok",), media, digest=digest), True),
        )
        for response, fail_close in cases:
            exchange = ExchangeFixture(response, fail_close)
            with self.assertRaises(Rejected):
                with exchange.download_node(CONTAINER, NODE, length=2, sha256=digest):
                    self.fail("an incomplete node was offered to the caller")
            self.assertEqual(len(exchange.calls), 1)

class CompactCatalogTest(unittest.TestCase):
    def test_catalog_uses_fixed_get_and_cursor_headers(self):
        exchange = ExchangeFixture(reply((b'{"items":[],"next_after":null}',), b"application/json"))
        self.assertEqual(exchange.read_catalog_page(), {"items": [], "next_after": None})
        self.assertEqual(exchange.read_catalog_page(CONTAINER), {"items": [], "next_after": None})
        self.assertEqual(exchange.calls, [("GET", ROOT, "application/json", None)] * 2)
        self.assertEqual(exchange.headers[0]["Zxs-View"], "compact")
        self.assertNotIn("Zxs-After", exchange.headers[0])
        self.assertEqual(exchange.headers[1]["Zxs-After"], CONTAINER)
        with self.assertRaises(Rejected):
            exchange.read_catalog_page("A" * 32)
        self.assertEqual(len(exchange.calls), 2)

    def test_catalog_no_partial_index_and_strict_continuation(self):
        items = [{"container_id": f"{n:032x}", "name": f"文{n}", "sequence": n}
                 for n in range(1, 257)]
        cursor = items[-1]["container_id"]
        first = {"items": items, "next_after": cursor}
        terminal = {"items": [], "next_after": None}

        class Pages:
            def __init__(self, pages):
                self.pages, self.after = iter(pages), []

            def read_catalog_page(self, after):
                self.after.append(after)
                return next(self.pages)

        exchange = Pages((first, terminal))
        shown = _Metadata(exchange).list_containers()
        self.assertEqual([row.id for row in shown], [row["container_id"] for row in items])
        self.assertEqual(exchange.after, [None, cursor])
        for raw, after in (
            (dict(first, next_after=None), None),
            (dict(first, next_after=CONTAINER), None),
            (first, cursor),
            (dict(first, items=items + items[:1]), None),
            (dict(first, items=items[::-1]), None),
            ({"items": [items[0], items[0]], "next_after": None}, None),
            ({"items": [dict(items[0], name="bad/name")], "next_after": None}, None),
            ({"items": [dict(items[0], sequence=True)], "next_after": None}, None),
            ({"items": [dict(items[0], tags=[])], "next_after": None}, None),
            ({"items": [], "next_after": cursor}, None),
        ):
            with self.subTest(after=after), self.assertRaises(Rejected):
                _catalog_page(raw, after)
        with self.assertRaises(Rejected):
            _Metadata(Pages((first, dict(terminal, next_after=cursor)))).list_containers()

class BranchHistoryTest(unittest.TestCase):
    def test_history_uses_only_the_original_branch_get_and_anchor_headers(self):
        exchange = ExchangeFixture(reply((b'{"head":"fixture","nodes":[],"next":null}',),
                                         b"application/json"))
        first = "3" * 32
        next_node = "4" * 32
        exchange.read_history_page(CONTAINER, "main", first)
        exchange.read_history_page(CONTAINER, "main", first, next_node)
        self.assertEqual(exchange.calls,
                         [("GET", f"{ROOT}/{CONTAINER}/branches/main/nodes", "application/json", None)] * 2)
        self.assertEqual(exchange.headers[0]["Zxs-History-Head"], first)
        self.assertNotIn("Zxs-History-Next", exchange.headers[0])
        self.assertEqual(exchange.headers[1]["Zxs-History-Head"], first)
        self.assertEqual(exchange.headers[1]["Zxs-History-Next"], next_node)
        for branch, cursor in (("main", first), ("main", "F" * 32), ("../nodes", None)):
            with self.assertRaises(Rejected):
                exchange.read_history_page(CONTAINER, branch, first, cursor)
        self.assertEqual(len(exchange.calls), 2)

    def test_history_rejects_unbound_and_incomplete_pages(self):
        def key(n):
            return f"{n:032x}"

        def row(n):
            return {"node_id": key(n), "operation_id": key(n + 500), "parent_node": key(n - 1) if n > 1 else None,
                    "original_branch": "main", "sequence": n, "length": 0, "sha256": "0" * 64}

        head, parent = key(257), key(1)
        full = {"head": head, "nodes": [row(n) for n in range(257, 1, -1)], "next": parent}
        end = {"head": head, "nodes": [row(1)], "next": None}
        chain = _History("main", head, 257)
        chain.accept(full)
        with self.assertRaises(Rejected):
            chain.finish()
        chain.accept(end)
        self.assertEqual([entry.id for entry in chain.finish().nodes], [key(n) for n in range(257, 0, -1)])

        for bad in (dict(full, next=None), dict(full, next=key(2)), dict(full, head=key(256)),
                    dict(full, nodes=full["nodes"][:1]), dict(full, nodes=full["nodes"] + [row(1)]),
                    dict(full, nodes=[])):
            with self.assertRaises(Rejected):
                _History("main", head, 257).accept(bad)

        for bad in (dict(end, nodes=[row(2)]), dict(end, nodes=[dict(row(1), sequence=2)]),
                    dict(end, nodes=[dict(row(1), parent_node=head)], next=head)):
            chain = _History("main", head, 257)
            chain.accept(full)
            with self.assertRaises(Rejected):
                chain.accept(bad)

class ContainerDetailTest(unittest.TestCase):
    def test_detail_uses_only_original_get_and_revision_headers(self):
        exchange = ExchangeFixture(reply((b'{"fixture":true}',), b"application/json"))
        exchange.read_container_detail_page(CONTAINER)
        exchange.read_container_detail_page(CONTAINER, 7, "main")
        exchange.read_container_detail_page(CONTAINER, 7)
        self.assertEqual(exchange.calls, [("GET", f"{ROOT}/{CONTAINER}", "application/json", None)] * 3)
        self.assertEqual(exchange.headers[0]["Zxs-Detail"], "paged")
        self.assertNotIn("Zxs-Detail-Sequence", exchange.headers[0])
        self.assertEqual(exchange.headers[1]["Zxs-Detail-Sequence"], "7")
        self.assertEqual(exchange.headers[1]["Zxs-Detail-After"], "main")
        self.assertNotIn("Zxs-Detail-After", exchange.headers[2])
        for revision, cursor in ((None, "main"), (0, None), (True, None), (7, "../nodes")):
            with self.assertRaises(Rejected):
                exchange.read_container_detail_page(CONTAINER, revision, cursor)
        self.assertEqual(len(exchange.calls), 3)

    def test_detail_pages_bind_original_revision_and_require_final_recheck(self):
        def key(n):
            return f"{n:032x}"

        branch_heads = [{"branch": key(n), "head": key(n + 300)} for n in range(1, 257)]
        first = {"container_id": CONTAINER, "sequence": 900, "name": "original",
                 "tags": ["kept"], "heads": branch_heads, "next_after": key(256)}
        final = dict(first, heads=[{"branch": "main", "head": key(900)}], next_after=None)

        class Pages:
            def __init__(self, revisions):
                self.calls, self.revisions = [], iter(revisions)

            def read_container_detail_page(self, container, sequence, after):
                self.calls.append((container, sequence, after))
                return next(self.revisions)

            def read_history_page(self, _container, branch, head, cursor):
                if cursor is not None:
                    raise AssertionError("fixture contains only one node per branch")
                return {"head": head, "nodes": [{"node_id": head, "operation_id": head,
                                                "parent_node": None, "original_branch": branch,
                                                "sequence": 1, "length": 0, "sha256": "0" * 64}],
                        "next": None}

        exchange = Pages((first, final, first))
        detail = _Metadata(exchange).open_container(CONTAINER)
        self.assertEqual(len(detail.branches), 257)
        self.assertEqual(exchange.calls, [(CONTAINER, None, None), (CONTAINER, 900, key(256)),
                                          (CONTAINER, 900, None)])
        for later in (dict(final, sequence=901), dict(final, tags=["altered"]),
                      dict(final, heads=[]), dict(final, heads=[branch_heads[-1]])):
            with self.assertRaises(Rejected):
                _Metadata(Pages((first, later))).open_container(CONTAINER)
        with self.assertRaises(Rejected):
            _Metadata(Pages((first, final, dict(first, name="renamed")))).open_container(CONTAINER)

        for bad in (dict(first, next_after=None), dict(first, next_after=key(255)),
                    dict(first, heads=branch_heads[::-1]),
                    dict(first, heads=branch_heads + branch_heads[:1]),
                    dict(first, container_id=key(999))):
            with self.assertRaises(Rejected):
                _detail_page(bad, CONTAINER)
