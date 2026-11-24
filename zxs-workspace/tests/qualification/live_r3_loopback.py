"""One bounded Workspace ↔ ZXS socket exercise for the disposable Linux guest.

Not an application listener or deployment configuration. The fixture accepts
three known requests, only on loopback, and uses an ephemeral TLS identity
owned by the harness. Call only against a fresh disposable product fixture.
"""
import hashlib
import http.client
import os
from pathlib import Path
import socket
import ssl
from concurrent.futures import ThreadPoolExecutor
from queue import Empty, Queue
from threading import Event

from h2.config import H2Configuration
from h2.connection import H2Connection
from h2.events import ConnectionTerminated, DataReceived, RequestReceived, StreamEnded, StreamReset, TrailersReceived

from zipworkspace.container_http import Message, ReplyKind
from zipworkspace.container_https import HttpsExchange


_HOST = "127.0.0.1"
_ROOT = "/zxs/v1/containers"
_CONTAINER = "71" * 16
_NODE = "72" * 16
_OPERATION = "73" * 16
_NAME = "loopback-h2-fixture.bin"
_BODY = b"workspace complete-file transport fixture\x00\xff\n" * 17
_HASH = hashlib.sha256(_BODY).hexdigest()
_LEAF = f"{_ROOT}/{_CONTAINER}"
_SAVE = f"{_LEAF}/branches/main/nodes"
_RECEIPT = f"{_LEAF}/operations/{_OPERATION}"
_DATA = f"{_LEAF}/nodes/{_NODE}/data"
_REQUEST_FIELDS = {
    "accept", "accept-encoding", "authorization", "content-type", "content-length",
    "zxs-base-node", "zxs-initial-name", "zxs-node", "zxs-operation", "zxs-sha256", "zxs-view",
}
_LIMIT = 16384
_TIMEOUT = 15


def require(condition, reason):
    if not condition:
        raise RuntimeError(reason)


def fields(rows):
    result = {}
    require(len(rows) <= 32, "too many synthetic H2 fields")
    for key, value in rows:
        require(key not in result and len(value) <= 4096, "duplicate or large H2 field")
        result[key] = value
    return result


def upstream(method, target, headers, body, backend_port, expected_status):
    connection = http.client.HTTPConnection(_HOST, backend_port, timeout=_TIMEOUT)
    try:
        connection.request(method, target, body if method == "POST" else None, headers)
        response = connection.getresponse()
        lengths = response.headers.get_all("Content-Length")
        require(response.status == expected_status and lengths is not None and len(lengths) == 1
                and lengths[0].isascii() and lengths[0].isdecimal()
                and int(lengths[0]) <= _LIMIT, "unexpected ZXS response status or extent")
        raw = response.read()
        require(len(raw) == int(lengths[0]), "short ZXS response")
        out = [(":status", str(response.status)), ("content-length", lengths[0])]
        for name in ("content-type", "zxs-sha256"):
            selected = response.headers.get_all(name)
            if selected is not None:
                require(len(selected) == 1 and len(selected[0]) <= 128,
                        "ambiguous ZXS response metadata")
                out.append((name, selected[0]))
        return out, raw
    finally:
        connection.close()


def exchange_one(sock, context, expected, backend_port, authority, authorization):
    with context.wrap_socket(sock, server_side=True) as tls:
        tls.settimeout(_TIMEOUT)
        require(tls.selected_alpn_protocol() == "h2", "real TLS did not select h2")
        h2 = H2Connection(H2Configuration(client_side=False, header_encoding="ascii"))
        h2.initiate_connection()
        tls.sendall(h2.data_to_send())
        head = None
        stream = None
        body = bytearray()
        ended = False
        for _ in range(64):
            block = tls.recv(_LIMIT)
            require(block, "short H2 request")
            for event in h2.receive_data(block):
                if isinstance(event, RequestReceived):
                    require(head is None and event.stream_id == 1, "unexpected H2 stream")
                    head, stream = fields(event.headers), event.stream_id
                elif isinstance(event, DataReceived):
                    require(head is not None and event.stream_id == stream
                            and len(body) + len(event.data) <= _LIMIT, "unexpected H2 DATA")
                    body.extend(event.data)
                    h2.acknowledge_received_data(event.flow_controlled_length, stream)
                elif isinstance(event, StreamEnded):
                    require(head is not None and event.stream_id == stream, "unexpected H2 end")
                    ended = True
                elif isinstance(event, (ConnectionTerminated, StreamReset, TrailersReceived)):
                    raise RuntimeError("H2 connection ended before request")
            pending = h2.data_to_send()
            if pending:
                tls.sendall(pending)
            if ended:
                break
        require(ended and head is not None, "no complete bounded H2 request")
        method, target, status, expected_body = expected
        require(head.pop(":method", None) == method and head.pop(":path", None) == target
                and head.pop(":scheme", None) == "https" and head.pop(":authority", None) == authority,
                "wrong original H2 endpoint")
        require(set(head) <= _REQUEST_FIELDS and head.get("authorization") == authorization,
                "unexpected H2 fields or fixture authorization")
        if method == "POST":
            require(expected_body is not None and head.get("content-length") == str(len(expected_body))
                    and body == expected_body,
                    "H2 full body differs from the synthetic original")
        else:
            require(not body and head.get("content-length", "0") == "0", "unexpected H2 GET body")
        response, raw = upstream(method, target, head, bytes(body), backend_port, status)
        h2.send_headers(stream, response, end_stream=not raw)
        if raw:
            require(len(raw) <= min(h2.max_outbound_frame_size, h2.local_flow_control_window(stream)),
                    "synthetic response exceeds single-frame fixture limit")
            h2.send_data(stream, raw, end_stream=True)
        tls.sendall(h2.data_to_send())


def serve(listener, context, backend_port, authority, authorization, expected_requests):
    for expected in expected_requests:
        conn, address = listener.accept()
        with conn:
            require(address[0] == _HOST, "non-loopback fixture client")
            conn.settimeout(_TIMEOUT)
            exchange_one(conn, context, expected, backend_port, authority, authorization)


def fixture_context(backend_port, fixture_authorization, certfile, keyfile):
    marker = Path("/run/zxs-acceptance-fixture")
    require(os.getuid() == os.geteuid() == 23100
            and marker.lstat().st_uid == 0 and not marker.is_symlink()
            and marker.stat().st_mode & 0o777 == 0o444
            and marker.read_bytes() == b"same-sha-disposable-guest\n"
            and set(os.listdir("/sys/class/net")) == {"lo"},
            "use only the disposable guest with no external interface")
    require(type(backend_port) is int and backend_port == 38831
            and fixture_authorization.startswith("Basic "), "wrong fixture backend or credential form")
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.minimum_version = ssl.TLSVersion.TLSv1_2
    context.load_cert_chain(certfile, keyfile)
    context.set_alpn_protocols(["h2"])
    return context


def run_live_r3(*, backend_port, fixture_authorization, certfile, keyfile, ca_pem):
    context = fixture_context(backend_port, fixture_authorization, certfile, keyfile)
    expected = (("POST", _SAVE, 200, _BODY), ("GET", _RECEIPT, 200, None), ("GET", _DATA, 200, None))
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as listener:
        listener.bind((_HOST, 0))
        listener.listen(1)
        listener.settimeout(_TIMEOUT)
        authority = f"{_HOST}:{listener.getsockname()[1]}"
        with ThreadPoolExecutor(max_workers=1) as worker:
            action = worker.submit(serve, listener, context, backend_port, authority, fixture_authorization, expected)
            try:
                client = HttpsExchange(f"https://{authority}", trusted_ca_pem=ca_pem,
                                       authorization=lambda: fixture_authorization, timeout=_TIMEOUT)
                headers = (("content-type", "application/octet-stream"), ("content-length", str(len(_BODY))),
                           ("zxs-operation", _OPERATION), ("zxs-node", _NODE), ("zxs-sha256", _HASH),
                           ("zxs-base-node", "0"), ("zxs-initial-name", _NAME.encode().hex()))
                sent = client.request(Message("POST", _SAVE, headers, (_BODY[:13], _BODY[13:])))
                require(sent.kind is ReplyKind.SELECTED and sent.receipt is not None,
                        "real H2 SAVE did not select its R3 receipt")
                receipt = sent.receipt
                effect = receipt.effect
                require((receipt.container_id, receipt.operation_id, receipt.sequence)
                        == (_CONTAINER, _OPERATION, 1)
                        and (effect.kind, effect.target, effect.branch, effect.name, effect.new_node, effect.source_node)
                        == ("save_node", "first_main", "main", _NAME, _NODE, None)
                        and effect.body is not None and (effect.body.length, effect.body.sha256) == (len(_BODY), _HASH),
                        "R3 first-save receipt differs from original request")
                require(client.request(Message("GET", _RECEIPT)) == sent, "exact original R3 lookup differs")
                captured = bytearray()

                def take(chunk):
                    captured.extend(chunk)
                    return len(chunk)

                client.read_node_into(_CONTAINER, _NODE, length=len(_BODY), sha256=_HASH, write=take)
                require(captured == _BODY, "exact committed body was not returned through H2")
            finally:
                # On failure stop new accepts; an already admitted socket has its
                # own timeout. A worker error is not converted to client success.
                listener.close()
                action.result(timeout=2 * _TIMEOUT)
    print("ZXS-WORKSPACE-H2 fixture negotiated TLS, saved, looked up, and read exact bytes", flush=True)

def run_live_fuse_r3(*, backend_port, fixture_authorization, certfile, keyfile, ca_pem,
                     fixture_login, fixture_password):
    from live_linux_edit_mount import run_mounted_first_save

    context = fixture_context(backend_port, fixture_authorization, certfile, keyfile)
    planned = Queue(maxsize=1)
    cancel = Event()
    expected_count = 5
    announced = 0

    def shots():
        for _ in range(expected_count):
            while not cancel.is_set():
                try:
                    expected = planned.get(timeout=0.2)
                except Empty:
                    continue
                yield expected
                break
            else:
                return

    def expect(method, path, status, body):
        nonlocal announced
        announced += 1
        require(announced <= expected_count and path.startswith(_ROOT),
                "unexpected mounted-volume transport count or route")
        planned.put((method, path, status, body), timeout=1)

    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as listener:
        listener.bind((_HOST, 0))
        listener.listen(1)
        listener.settimeout(_TIMEOUT)
        authority = f"{_HOST}:{listener.getsockname()[1]}"
        with ThreadPoolExecutor(max_workers=1) as worker:
            action = worker.submit(serve, listener, context, backend_port, authority, fixture_authorization, shots())
            try:
                def connect(credentials):
                    return HttpsExchange(f"https://{authority}", trusted_ca_pem=ca_pem,
                                         authorization=credentials, timeout=_TIMEOUT)

                run_mounted_first_save(exchange=connect, fixture_login=fixture_login,
                                       fixture_password=fixture_password, expect=expect)
                require(announced == expected_count, "not all mounted-volume requests were announced")
                action.result(timeout=_TIMEOUT)
            finally:
                cancel.set()
                listener.close()
