from contextlib import contextmanager
import ssl
from types import SimpleNamespace
import unittest

from zipworkspace.container_http import Message, ReplyKind
from zipworkspace.container_https import HttpsExchange, _body_parts, _control, _origin, _reply
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

    def test_non_h2_receipt_never_projects_status_or_body(self):
        for extensions in ({"http_version": b"HTTP/1.1"}, {}):
            with self.subTest(extensions=extensions):
                self.assertIs(ReplyKind.UNKNOWN, _reply(SimpleNamespace(extensions=extensions), True).kind)

    @staticmethod
    def _connection(alpn, *, verify_hostname=True, verify_mode=ssl.CERT_REQUIRED):
        events = []
        context = SimpleNamespace(check_hostname=verify_hostname, verify_mode=verify_mode,
                                  set_alpn_protocols=lambda protocols: events.append(("offer", protocols)))

        class Secure:
            def get_extra_info(self, name):
                events.append(("get", name))
                return None if alpn is False else SimpleNamespace(selected_alpn_protocol=self.select)

            def select(self):
                events.append(("selected", alpn))
                return alpn

            def close(self):
                events.append("secure close")

        class Plain:
            def start_tls(self, supplied, *, server_hostname, timeout):
                events.append(("tls", supplied is context, server_hostname, timeout))
                return Secure()

        class Backend:
            def connect_tcp(self, host, port, *, timeout):
                events.append(("connect", host, port, timeout))
                return Plain()

        class H2:
            def __init__(self, origin, *, stream):
                events.append(("h2", origin.scheme, origin.host, origin.port))
                self.network = stream

            def __enter__(self):
                return self

            def __exit__(self, *_):
                self.network.close()

            @contextmanager
            def stream(self, method, url, *, headers, content, extensions):
                events.append(("request", method, url, headers.copy(), extensions))
                events.append(("data", list(content)))
                yield "response"

        exchange = object.__new__(HttpsExchange)
        exchange._core = SimpleNamespace(
            Origin=lambda scheme, host, port: SimpleNamespace(scheme=scheme, host=host, port=port),
            SyncBackend=Backend, HTTP2Connection=H2)
        exchange._host, exchange._port = "example.test", 8443
        exchange._authority, exchange._location = "example.test:8443", "https://example.test:8443"
        exchange._tls, exchange._timeout = lambda: context, 5
        exchange._authorization = lambda: events.append("native authorization") or "Bearer fixture"
        return exchange, events

    def test_negotiation_precedes_request_native_authorization_and_body(self):
        target = f"/zxs/v1/containers/{C}/branches/main/nodes"
        exchange, events = self._connection("h2")

        def payload():
            events.append("body consumed")
            yield b"abc"

        with exchange._exchange("POST", target, {"Content-Length": "3"}, payload()) as response:
            self.assertEqual(response, "response")
        self.assertEqual(events, [
            ("offer", ["h2"]), ("connect", "example.test", 8443, 5), ("tls", True, "example.test", 5),
            ("get", "ssl_object"), ("selected", "h2"), ("h2", b"https", b"example.test", 8443),
            "native authorization",
            ("request", "POST", exchange._location + target,
             {"Content-Length": "3", "Host": exchange._authority, "Authorization": "Bearer fixture"},
             {"timeout": {"read": 5, "write": 5}}),
            "body consumed", ("data", [b"abc"]), "secure close",
        ])

    def test_no_http_attempt_without_h2_or_verified_tls(self):
        target = f"/zxs/v1/containers/{C}/branches/main/nodes"
        cases = ((False, True, ssl.CERT_REQUIRED), (None, True, ssl.CERT_REQUIRED),
                 ("http/1.1", True, ssl.CERT_REQUIRED), ("h2", False, ssl.CERT_REQUIRED),
                 ("h2", True, ssl.CERT_OPTIONAL))
        for alpn, verify_hostname, verify_mode in cases:
            with self.subTest(alpn=alpn, verify_hostname=verify_hostname, verify_mode=verify_mode):
                exchange, events = self._connection(alpn, verify_hostname=verify_hostname, verify_mode=verify_mode)

                def payload():
                    self.fail("sealed body was consumed before negotiated HTTP/2")
                    yield b"abc"

                with self.assertRaises(Rejected), exchange._exchange("POST", target, {}, payload()):
                    self.fail("a non-HTTP/2 connection exposed a response")
                if not verify_hostname or verify_mode != ssl.CERT_REQUIRED:
                    self.assertEqual(events, [])
                else:
                    expected = [("offer", ["h2"]), ("connect", "example.test", 8443, 5),
                                ("tls", True, "example.test", 5), ("get", "ssl_object")]
                    if alpn is not False:
                        expected.append(("selected", alpn))
                    self.assertEqual(events, expected + ["secure close"])
