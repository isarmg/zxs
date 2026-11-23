import ssl
from types import SimpleNamespace
import unittest

from zipworkspace.container_http import Message, ReplyKind
from zipworkspace.container_https import _backend, _body_parts, _control, _origin, _reply
from zipworkspace.model import Rejected

C, O = "a" * 32, "b" * 32


class FakeResponse:
    status = 404
    extensions = {"http_version": b"HTTP/2"}

    def __init__(self, body, lengths):
        self.body = body
        self.headers = [(b"Content-Type", b"application/json")]
        self.headers.extend((b"Content-Length", length.encode("ascii")) for length in lengths)

    def iter_stream(self):
        yield self.body


class HttpsPolicyTest(unittest.TestCase):
    def test_origin_disallows_plaintext_credentials_route_and_redirect_host(self):
        self.assertEqual(("example.test", 443), _origin("https://example.test"))
        self.assertEqual(("::1", 8443), _origin("https://[::1]:8443/"))
        for unsafe in ("http://example.test", "https://other@example.test", "https://example.test/a",
                       "https://example.test?x", "https://example.test:0", "https://example.test:"):
            with self.assertRaises(Rejected):
                _origin(unsafe)
        with self.assertRaises(Rejected):
            _control(Message("GET", "https://else.test"))

    def test_sealed_stream_counts_exact_bytes_without_reusing_iterator(self):
        source = (part for part in (b"ab", b"c"))
        allowed = Message("POST", f"/zxs/v1/containers/{C}/branches/main/nodes",
                          (("Content-Length", "3"), ("Zxs-Operation", O)), source)
        headers, stream = _control(allowed)
        self.assertNotIn("Connection", headers)
        self.assertEqual("identity", headers["Accept-Encoding"])
        self.assertEqual([b"ab", b"c"], list(stream))
        with self.assertRaises(Rejected):
            list(_body_parts((b"too long",), 1))
        with self.assertRaises(Rejected):
            list(_body_parts((b"x",), 2))
        with self.assertRaises(Rejected):
            _control(Message("POST", allowed.target, (("Content-Length", "3"), ("content-length", "3")), source))

    def test_only_well_framed_r3_receipt_lookup_404_is_absence(self):
        body = b'{"state":"not_found"}'
        size = str(len(body))
        self.assertIs(ReplyKind.ABSENT, _reply(FakeResponse(body, (size,)), True).kind)
        self.assertIs(ReplyKind.UNKNOWN, _reply(FakeResponse(body, (size,)), False).kind)
        self.assertIs(ReplyKind.UNKNOWN, _reply(FakeResponse(body, (size, size)), True).kind)
        self.assertIs(ReplyKind.UNKNOWN, _reply(FakeResponse(body[:-1], (size,)), True).kind)
        wrapped = FakeResponse(body, (size,))
        wrapped.headers.append((b"Transfer-Encoding", b"chunked"))
        self.assertIs(ReplyKind.UNKNOWN, _reply(wrapped, True).kind)


    def test_thin_alpn_boundary_offers_h2_then_h1_and_keeps_exact_verification(self):
        observations = []

        class Stream:
            def start_tls(self, context, host, timeout):
                observations.append((context, host, timeout))
                return "negotiated fixture"

        class Sync:
            def connect_tcp(self, host, port, timeout, local_address, options):
                observations.append((host, port))
                return Stream()

        core = SimpleNamespace(NetworkStream=object, NetworkBackend=object, SyncBackend=Sync)
        backend = _backend(core, "example.test", 8443)
        with self.assertRaises(Rejected):
            backend.connect_tcp("other.test", 8443)
        stream = backend.connect_tcp("example.test", 8443)
        context = SimpleNamespace(check_hostname=True, verify_mode=ssl.CERT_REQUIRED,
                                  set_alpn_protocols=lambda protocols: observations.append(protocols))
        self.assertEqual("negotiated fixture", stream.start_tls(context, "example.test", 5))
        self.assertEqual([("example.test", 8443), ["h2", "http/1.1"], (context, "example.test", 5)], observations)
