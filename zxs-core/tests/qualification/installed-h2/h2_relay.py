"""Bounded synthetic TLS relay for the no-NIC installed-client guest only."""
from dataclasses import dataclass, field
import http.client
import json
import socket
import ssl

from h2.config import H2Configuration
from h2.connection import H2Connection
from h2.events import ConnectionTerminated, DataReceived, RequestReceived, StreamEnded, StreamReset, TrailersReceived

HOST = "127.0.0.1"
BACKEND = 38831
LIMIT = 16 * 1024
TIMEOUT = 18
REQUEST_HEADERS = {
    "accept", "accept-encoding", "authorization", "content-type", "content-length",
    "zxs-base-node", "zxs-initial-name", "zxs-node", "zxs-operation", "zxs-sha256",
    "zxs-view", "zxs-detail", "zxs-history-head",
}


class FixtureFailure(RuntimeError):
    """Assertion text is fixture-defined and must never include wire data."""


def require(condition, reason):
    if not condition:
        raise FixtureFailure(reason)


def json_exact(raw):
    def unique(pairs):
        out = dict(pairs)
        require(len(out) == len(pairs), "duplicate fixture JSON field")
        return out
    return json.loads(raw, object_pairs_hook=unique)


@dataclass(frozen=True)
class Step:
    method: str
    path: str
    status: int
    auth: str = "basic"
    fields: dict = field(default_factory=dict)
    request_body: bytes = b""
    response_body: bytes | None = None
    response_json: dict | None = None
    mint: bool = False
    lose_reply: bool = False


def receive_fields(rows):
    require(len(rows) <= 32, "too many fixture H2 fields")
    fields = {}
    for key, value in rows:
        require(key not in fields and len(value) <= 4096, "ambiguous fixture H2 field")
        fields[key] = value
    return fields


def forward(step, headers, body):
    # No destination is supplied by the client. Only fully checked exact Step
    # values reach the real backend, on the isolated guest loopback.
    require(step.path.startswith("/zxs/v1/"), "unexpected fixture route")
    conn = http.client.HTTPConnection(HOST, BACKEND, timeout=TIMEOUT)
    try:
        conn.request(
            step.method, step.path, body if step.method == "POST" else None,
            dict(headers, Connection="close"),
        )
        reply = conn.getresponse()
        lengths = reply.headers.get_all("Content-Length")
        require(reply.status == step.status and lengths is not None
                and len(lengths) == 1 and lengths[0].isascii()
                and lengths[0].isdecimal() and int(lengths[0]) <= LIMIT,
                "unexpected real backend status or response extent")
        raw = reply.read()
        require(len(raw) == int(lengths[0]), "incomplete real backend response")
        if step.response_body is not None:
            require(raw == step.response_body, "real backend bytes differ")
        if step.response_json is not None:
            require(json_exact(raw) == step.response_json, "real backend receipt differs")
        selected = [(":status", str(reply.status)), ("content-length", lengths[0])]
        for name in ("content-type", "cache-control", "zxs-sha256", "www-authenticate", "retry-after"):
            vals = reply.headers.get_all(name)
            if vals is not None:
                require(len(vals) == 1 and len(vals[0]) <= 128, "ambiguous backend metadata")
                selected.append((name, vals[0]))
        return selected, raw
    finally:
        conn.close()


def tls_context(cert, key, protocol="h2"):
    ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    ctx.minimum_version = ssl.TLSVersion.TLSv1_2
    ctx.load_cert_chain(cert, key)
    ctx.set_alpn_protocols([protocol])
    if protocol == "http/1.1":
        # The client closes immediately after detecting no h2; no post-handshake tickets here.
        ctx.num_tickets = 0
    return ctx


def serve_one(listener, context, authority, expected, basic, *, refused_tls=None):
    sock, peer = listener.accept()
    with sock:
        require(peer[0] == HOST, "non-loopback peer")
        sock.settimeout(TIMEOUT)
        if refused_tls is not None:
            require(refused_tls in ("certificate", "no-h2") and not expected, "invalid TLS refusal case")
            try:
                with context.wrap_socket(sock, server_side=True) as tls:
                    if refused_tls == "no-h2":
                        require(tls.selected_alpn_protocol() is None,
                                "non-HTTP/2 case did not complete TLS without a matching ALPN")
                    require(not tls.recv(1), "client sent preface or request on refused TLS")
            except ssl.SSLError:
                # Certificate-negative cases have independent chain/SAN checks.
                # A generic TLS error cannot count as the no-H2 outcome.
                require(refused_tls == "certificate", "non-HTTP/2 case failed TLS instead of ALPN")
            return (0, 0)
        with context.wrap_socket(sock, server_side=True) as tls:
            require(tls.selected_alpn_protocol() == "h2", "actual TLS did not negotiate h2")
            h2 = H2Connection(H2Configuration(client_side=False, header_encoding="ascii"))
            h2.initiate_connection()
            tls.sendall(h2.data_to_send())
            index = 0
            lost = 0
            token = None
            held = None
            for _ in range(128):
                block = tls.recv(LIMIT)
                if not block:
                    break
                for event in h2.receive_data(block):
                    if isinstance(event, RequestReceived):
                        require(held is None and index < len(expected),
                                "extra or simultaneous H2 request outside exact fixture plan")
                        held = (event.stream_id, receive_fields(event.headers), bytearray())
                    elif isinstance(event, DataReceived):
                        require(held is not None and held[0] == event.stream_id
                                and len(held[2]) + len(event.data) <= LIMIT,
                                "unbound or oversized H2 body")
                        held[2].extend(event.data)
                        h2.acknowledge_received_data(event.flow_controlled_length, event.stream_id)
                    elif isinstance(event, (StreamReset, TrailersReceived)):
                        raise RuntimeError("unexpected H2 reset or trailers")
                    elif isinstance(event, ConnectionTerminated):
                        require(held is None and index == len(expected), "early H2 connection end")
                    elif isinstance(event, StreamEnded):
                        require(held is not None and held[0] == event.stream_id, "unexpected H2 end")
                        stream_id, headers, body = held
                        step = expected[index]
                        require(headers.pop(":method", None) == step.method
                                and headers.pop(":path", None) == step.path
                                and headers.pop(":scheme", None) == "https"
                                and headers.pop(":authority", None) == authority,
                                "unexpected exact fixture authority, route or method")
                        auth = basic if step.auth == "basic" else token
                        require(auth is not None and set(headers) <= REQUEST_HEADERS
                                and headers.get("authorization") == auth,
                                "unexpected fixture request scope or credential scheme")
                        require(all(headers.get(k) == v for k, v in step.fields.items())
                                and body == step.request_body
                                and (step.method == "POST" or headers.get("content-length", "0") == "0")
                                and (step.method != "POST"
                                     or headers.get("content-length") == str(len(body))),
                                "the exact bounded fixture body or headers changed")
                        answer, raw = forward(step, headers, bytes(body))
                        if step.mint:
                            issued = json_exact(raw)
                            require(set(issued) == {"access_token", "token_type", "expires_in"}
                                    and issued["token_type"] == "Bearer"
                                    and issued["expires_in"] == 900
                                    and isinstance(issued["access_token"], str)
                                    and len(issued["access_token"]) == 43,
                                    "real backend did not produce the initial fixture session")
                            token = "Bearer " + issued["access_token"]
                        index += 1
                        held = None
                        if step.lose_reply:
                            # A real successful reply was obtained above. Send
                            # no headers or body to the Rust client for this stream.
                            require(step.status == 200 and step.response_json is not None
                                    and index == len(expected), "invalid intentional loss point")
                            return (index, 1)
                        h2.send_headers(stream_id, answer, end_stream=not raw)
                        if raw:
                            require(len(raw) <= min(h2.max_outbound_frame_size,
                                                   h2.local_flow_control_window(stream_id)),
                                    "fixture response exceeded the bounded frame")
                            h2.send_data(stream_id, raw, end_stream=True)
                pending = h2.data_to_send()
                if pending:
                    tls.sendall(pending)
            else:
                raise FixtureFailure("peer exceeded the bounded H2 receive count")
            require(held is None and index == len(expected), "incomplete fixture request sequence")
            return (index, lost)


def listener():
    sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    sock.bind((HOST, 0))
    sock.listen(1)
    sock.settimeout(TIMEOUT)
    return sock
