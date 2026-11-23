"""Single-attempt, streaming HTTPS for R3 control and mutations (HTTP/2 only)."""
import hashlib
import ipaddress
import json
import math
import os
import re
import ssl
import tempfile
from contextlib import contextmanager
from pathlib import Path
from urllib.parse import urlsplit

from .container_http import Message, Reply, ReplyKind, project_reply
from .model import Rejected

_MAX_REPLY = 1 << 20
_HEADERS = frozenset(("content-length", "content-type", "zxs-operation", "zxs-base-generation",
                      "zxs-node", "zxs-sha256", "zxs-base-node", "zxs-initial-name", "zxs-fork-from-main"))
_ID = r"[0-9a-f]{32}"
_BRANCH = rf"(?:main|{_ID})"
_ROOT = r"/zxs/v1/containers"
_TARGETS = {
    "GET": re.compile(rf"{_ROOT}/{_ID}/operations/{_ID}"),
    "DELETE": re.compile(rf"{_ROOT}/{_ID}"),
    "PUT": re.compile(rf"{_ROOT}/{_ID}/(?:name|tags|branches/{_ID})"),
    "POST": re.compile(rf"{_ROOT}/{_ID}/branches/{_BRANCH}/nodes"),
}
_METADATA = re.compile(rf"{_ROOT}(?:/{_ID}(?:/branches/{_BRANCH}/nodes)?)?")
_LOOKUP = re.compile(rf"{_ROOT}/{_ID}/operations/{_ID}")


def _origin(value):
    if type(value) is not str or not value or any(ord(ch) <= 32 or ord(ch) == 127 for ch in value):
        raise Rejected("HTTPS origin must be explicit and unambiguous")
    try:
        parsed = urlsplit(value)
        host, port = parsed.hostname, parsed.port
        if (parsed.scheme != "https" or host is None or parsed.username is not None or parsed.password is not None
                or parsed.path not in ("", "/") or parsed.query or parsed.fragment
                or any(ch in value for ch in ("%", "?", "#", "\\")) or parsed.netloc.endswith(":")
                or port == 0):
            raise ValueError("not an HTTPS origin")
        try:
            literal = ipaddress.ip_address(host)
        except ValueError:
            host = host.encode("idna").decode("ascii")
            if not re.fullmatch(r"[A-Za-z0-9.-]{1,253}", host):
                raise ValueError("invalid DNS name")
        else:
            host = literal.compressed
        return host, 443 if port is None else port
    except (UnicodeError, ValueError) as exc:
        raise Rejected("connection requires one selected HTTPS origin without userinfo, route or proxy") from exc


def _decimal(value):
    if type(value) is not str or re.fullmatch(r"0|[1-9][0-9]{0,19}", value) is None:
        raise Rejected("request or response length is not one canonical decimal")
    length = int(value)
    if length >= 1 << 64:
        raise Rejected("length exceeds protocol uint64")
    return length


def _header_text(value):
    return isinstance(value, str) and bool(value) and all(32 <= ord(ch) < 127 for ch in value)


def _body_parts(body, declared):
    sent = 0
    if body is not None:
        for part in ((body,) if type(body) is bytes else body):
            if type(part) is not bytes or sent + len(part) > declared:
                raise Rejected("not the exact declared sealed body length")
            sent += len(part)
            if part:
                yield part
    if sent != declared:
        raise Rejected("sealed body ended before its declared length")


def _control(message):
    if (type(message) is not Message or message.method not in _TARGETS
            or _TARGETS[message.method].fullmatch(message.target) is None):
        raise Rejected("not a supported R3 control or mutation")
    known, headers = {}, {}
    for name, value in message.headers:
        key = name.lower()
        if key not in _HEADERS or key in known or not _header_text(value):
            raise Rejected("ambiguous or forbidden R3 request header")
        known[key], headers[name] = value, value
    size = _decimal(known.get("content-length", "0"))
    if message.method in ("GET", "DELETE"):
        if size or message.body is not None:
            raise Rejected("GET and DELETE must not carry file data")
        upload = None
    else:
        if "content-length" not in known or message.body is None:
            raise Rejected("mutation needs exact non-chunked body framing")
        upload = _body_parts(message.body, size)
    headers["Accept"] = "application/json"
    headers["Accept-Encoding"] = "identity"
    return headers, upload


def _read_headers(response, media):
    if response.extensions.get("http_version") != b"HTTP/2":
        raise Rejected("Workspace requires an HTTP/2 response")
    if response.status in (401, 403):
        raise Rejected("Workspace read authorization refused")
    if response.status != 200:
        raise Rejected("Workspace read did not return a complete response")
    fields = {}
    for name, value in response.headers:
        fields.setdefault(name.lower(), []).append(value)
    if (any(name in fields for name in (b"transfer-encoding", b"content-encoding", b"content-range", b"trailer"))
            or fields.get(b"content-type") != [media]
            or len(fields.get(b"content-length", ())) != 1):
        raise Rejected("Workspace read response framing is ambiguous")
    return _decimal(fields[b"content-length"][0].decode("ascii")), fields


def _read_parts(response, length):
    count = 0
    for part in response.iter_stream():
        if type(part) is not bytes or len(part) > length - count:
            raise Rejected("Workspace response exceeded its declared length")
        count += len(part)
        yield part
    if count != length:
        raise Rejected("Workspace response ended before its declared length")


def _unique_object(items):
    result = {}
    for key, value in items:
        if key in result:
            raise ValueError("duplicate JSON object key")
        result[key] = value
    return result


def _invalid_constant(_value):
    raise ValueError("invalid JSON numeric constant")


def _reply(response, lookup):
    if response.extensions.get("http_version") != b"HTTP/2" or response.status not in (200, 404, 409):
        return Reply(ReplyKind.UNKNOWN)
    header = {}
    for name, value in response.headers:
        header.setdefault(name.lower(), []).append(value)
    if any(name in header for name in (b"transfer-encoding", b"content-encoding", b"content-range", b"trailer")):
        return Reply(ReplyKind.UNKNOWN)
    lengths = header.get(b"content-length", ())
    if len(lengths) != 1:
        return Reply(ReplyKind.UNKNOWN)
    length = _decimal(lengths[0].decode("ascii"))
    if length > _MAX_REPLY:
        return Reply(ReplyKind.UNKNOWN)
    if response.status in (200, 404) and header.get(b"content-type") != [b"application/json"]:
        return Reply(ReplyKind.UNKNOWN)
    data = bytearray()
    for block in response.iter_stream():
        if type(block) is not bytes or len(data) + len(block) > length:
            return Reply(ReplyKind.UNKNOWN)
        data.extend(block)
    if len(data) != length:
        return Reply(ReplyKind.UNKNOWN)
    return project_reply(response.status, bytes(data), lookup=lookup)


def _connect_h2(core, origin, tls, timeout):
    if origin.scheme != b"https" or not tls.check_hostname or tls.verify_mode != ssl.CERT_REQUIRED:
        raise Rejected("selected origin and verified TLS are required")
    # HTTPConnection advertises h1 as well even with http1 disabled in
    # httpcore 1.0.9. Establish TLS explicitly before using HTTP2Connection.
    host = origin.host.decode("ascii")
    tls.set_alpn_protocols(["h2"])
    plain = core.SyncBackend().connect_tcp(host, origin.port, timeout=timeout)
    try:
        secure = plain.start_tls(tls, server_hostname=host, timeout=timeout)
    except BaseException:
        plain.close()
        raise
    try:
        peer = secure.get_extra_info("ssl_object")
        if peer is None or peer.selected_alpn_protocol() != "h2":
            raise Rejected("Workspace requires TLS ALPN h2")
        return core.HTTP2Connection(origin, stream=secure)
    except BaseException:
        secure.close()
        raise


class HttpsExchange:
    """Ordinary Python workers, with a fresh verified HTTPS connection per request.

    Platform packaging must supply the pinned public httpcore H2 dependency
    closure. TLS uses reviewed normal CA trust (default bundle or explicitly
    selected private CA). Existing certificate renewal stays the same origin;
    no leaf-pin, TOFU, redirects, proxy, connection pool or automatic replay.
    """

    def __init__(self, origin, *, trusted_ca_pem=None, authorization=None, timeout=30.0):
        if not ssl.HAS_ALPN:
            raise Rejected("this desktop TLS backend has no ALPN support")
        self._host, self._port = _origin(origin)
        if (trusted_ca_pem is not None and
                (type(trusted_ca_pem) is not str or not trusted_ca_pem or len(trusted_ca_pem) > _MAX_REPLY)):
            raise Rejected("invalid selected optional TLS trust anchors")
        if type(timeout) not in (float, int) or not math.isfinite(timeout) or not 0 < timeout <= 300:
            raise Rejected("invalid HTTPS I/O timeout")
        if authorization is not None and not callable(authorization):
            raise Rejected("use the already selected native authentication supplier")
        try:
            import httpcore
            import h2  # fail during configuration, not midway into a body
        except ImportError as exc:
            raise Rejected("the pinned Workspace HTTPS runtime is unavailable") from exc
        if httpcore.__version__ != "1.0.9" or h2.__version__ != "4.3.0":
            raise Rejected("unreviewed Workspace HTTPS runtime")
        self._core = httpcore
        self._trust, self._timeout, self._authorization = trusted_ca_pem, timeout, authorization
        authority = "[" + self._host + "]" if ":" in self._host else self._host
        self._authority = f"{authority}:{self._port}"
        self._location = "https://" + self._authority
        disposition = b"public-ca" if trusted_ca_pem is None else b"private-ca:" + trusted_ca_pem.encode("utf-8")
        self._identity = hashlib.sha256(b"R3-TLS-origin-v1\0" + self._location.encode("ascii") +
                                        b"\0" + hashlib.sha256(disposition).digest()).hexdigest()

    @property
    def destination_identity(self):
        return self._identity

    def _tls(self):
        # A per-request context keeps the single ALPN offer local to this connection.
        if self._trust is None:
            tls = self._core.default_ssl_context()
        else:
            tls = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
            tls.load_verify_locations(cadata=self._trust)
        tls.minimum_version = ssl.TLSVersion.TLSv1_2
        return tls

    def _exchange_errors(self):
        core = self._core
        return (OSError, UnicodeError, core.NetworkError, core.TimeoutException, core.ProtocolError,
                core.ConnectionNotAvailable, core.ProxyError, core.UnsupportedProtocol)

    @contextmanager
    def _exchange(self, method, target, headers, body):
        core = self._core
        limits = dict.fromkeys(("read", "write"), self._timeout)
        origin = core.Origin(b"https", self._host.encode("ascii"), self._port)
        with _connect_h2(core, origin, self._tls(), self._timeout) as connection:
            # Resolve native authorization only after verified ALPN. The same
            # authority, including brackets for IPv6, becomes :authority.
            headers["Host"] = self._authority
            if self._authorization is not None:
                value = self._authorization()
                if not _header_text(value) or len(value) > 8192:
                    raise Rejected("invalid native authorization header")
                headers["Authorization"] = value
            # One connection and one attempt, with no pool to replay on GOAWAY.
            with connection.stream(method, self._location + target,
                                   headers=headers, content=body, extensions={"timeout": limits}) as response:
                yield response

    def request(self, message):
        headers, body = _control(message)
        try:
            with self._exchange(message.method, message.target, headers, body) as response:
                return _reply(response, bool(_LOOKUP.fullmatch(message.target)) and message.method == "GET")
        except self._exchange_errors() + (Rejected,):
            # Only exact original durable receipt lookup resolves uncertainty.
            return Reply(ReplyKind.UNKNOWN)

    def read_metadata(self, target):
        """Read only the fixed R3 collection, container, and branch-node metadata routes."""
        if type(target) is not str or _METADATA.fullmatch(target) is None:
            raise Rejected("not a supported Workspace metadata route")
        headers = {"Accept": "application/json", "Accept-Encoding": "identity"}
        try:
            with self._exchange("GET", target, headers, None) as response:
                length, _ = _read_headers(response, b"application/json")
                if length > _MAX_REPLY:
                    raise Rejected("Workspace metadata exceeds its protocol bound")
                raw = b"".join(_read_parts(response, length))
            result = json.loads(raw.decode("utf-8"), object_pairs_hook=_unique_object,
                                parse_constant=_invalid_constant)
        except Rejected:
            raise
        except self._exchange_errors() + (ValueError, RecursionError) as exc:
            raise Rejected("Workspace metadata read did not complete") from exc
        if type(result) not in (dict, list):
            raise Rejected("unexpected Workspace metadata shape")
        return result

    read_json = read_metadata

    @staticmethod
    def _node_target(container, node, length, sha256):
        if (not _read_id(container) or not _read_id(node)
                or type(length) is not int or not 0 <= length < 1 << 64
                or type(sha256) is not str or re.fullmatch(r"[0-9a-f]{64}", sha256) is None):
            raise Rejected("node GET needs exact selected IDs, length and SHA-256")
        return f"{_ROOT}/{container}/nodes/{node}/data"

    def _receive_node(self, target, length, sha256, write):
        headers = {"Accept": "application/octet-stream", "Accept-Encoding": "identity"}
        with self._exchange("GET", target, headers, None) as response:
            declared, fields = _read_headers(response, b"application/octet-stream")
            if declared != length or fields.get(b"zxs-sha256") != [sha256.encode("ascii")]:
                raise Rejected("node GET disagreed with the selected metadata")
            digest = hashlib.sha256()
            for block in _read_parts(response, declared):
                if write(block) != len(block):
                    raise OSError("short local node write")
                digest.update(block)
            if digest.hexdigest() != sha256:
                raise Rejected("node GET bytes disagreed with the selected SHA-256")

    def read_node_into(self, container, node, *, length, sha256, write):
        """Stream one verified node into an already private sink; the caller publishes."""
        target = self._node_target(container, node, length, sha256)
        try:
            self._receive_node(target, length, sha256, write)
        except self._exchange_errors() as exc:
            raise Rejected("Workspace node read did not complete") from exc

    @contextmanager
    def download_node(self, container, node, *, length, sha256):
        """Lend only complete, verified immutable-node bytes from a private local copy.

        The caller supplies the selected node's expected length and SHA-256 from
        typed metadata. The completed copy is temporary and never automatically
        installed in the worktree or given the server's display name.
        """
        target = self._node_target(container, node, length, sha256)
        with tempfile.TemporaryDirectory(prefix="workspace-node-") as directory:
            body = Path(directory) / "body"
            try:
                with body.open("xb") as output:
                    self._receive_node(target, length, sha256, output.write)
                    output.flush()
                    os.fsync(output.fileno())
                original = body.open("rb")
            except self._exchange_errors() as exc:
                raise Rejected("Workspace node read did not complete") from exc
            with original:
                yield original

    @contextmanager
    def download_native_node(self, container, node, *, length, sha256):
        """Windows/macOS: lend the exact completed original read handle, not a path."""
        from .container_native_body import stage_pair

        target = self._node_target(container, node, length, sha256)
        with stage_pair() as (output, original):
            try:
                self._receive_node(target, length, sha256, output.write)
                output.flush()
                os.fsync(output.fileno())
                output.close()
                if os.fstat(original.fileno()).st_size != length:
                    raise Rejected("the held node byte count changed")
                original.seek(0)
            except self._exchange_errors() as exc:
                raise Rejected("Workspace node read did not complete") from exc
            yield original


def _read_id(value):
    return type(value) is str and re.fullmatch(_ID, value) is not None and value != "0" * 32
