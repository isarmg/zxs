#!/usr/bin/env python3
"""Opt-in client checks only. See rust-smb-client.md before use."""

import argparse
from contextlib import contextmanager, nullcontext, suppress
import fcntl
import hashlib
import importlib.metadata
import importlib.util
import ipaddress
import json
import logging
import os
import re
import secrets
import stat
import subprocess
import sys
import uuid
import warnings


SERVER = "zxs-lab.test"
TARGET = "192.0.2.1"
CLIENT = "192.0.2.2"
LINK = "zxsaccept0"
PORT = 445
TIMEOUT = 7
ENV = {"LC_ALL": "C", "HOME": "/nonexistent", "PATH": "/usr/bin:/bin"}
ROUTED_KEYS = {"gateway", "via", "multipath", "encap", "nhid"}
EVIDENCE_KEYS = {
    "profile", "observed", "source_commit", "zxs_sha256", "client_netns", "outside_netns",
    "host", "address", "data_label", "recycle_label",
}
MOVE_MODES = ("seed-move-a", "seed-move-b", "move-stock", "move-original")
MOVE_KEYS = {"target_data_label", "move_case"}
UBUNTU_CLIENT = "ubuntu_smbclient"
# Canonical Resolute amd64 smbclient 2:4.23.6+dfsg-1ubuntu2.2 archive.
UBUNTU_CLIENT_PACKAGE = "b1adf344e27e90c1c851f10ea8a800e001fefaed89f7898b5d514f01b6197d6b"
UBUNTU_CLIENT_VERSION = "Version 4.23.6-Ubuntu-4.23.6+dfsg-1ubuntu2.2"
MAX_CLIENT_ELF = 16 * 1024 * 1024
MOVE_LEAF = "zxs-normal-move.bin"
MOVE_A, MOVE_B, MOVE_C = (bytes([letter]) * 4096 for letter in b"ABC")
WHEELS = {"smbprotocol": "1.17.0", "pyspnego": "0.12.4"}
FILE_NOT_AVAILABLE = 0xC0000467  # Original compatible vote, but Core's FILE EX is busy.
NAME_NOT_FOUND = 0xC0000034
PATH_NOT_FOUND = 0xC000003A
NO_SUCH_FILE = 0xC000000F
NO_MORE_FILES = 0x80000006


class Refused(Exception):
    """Only program-owned, public diagnostic strings."""


def need(condition, reason):
    if not condition:
        raise Refused(reason)


def report(result, phase, **fields):
    print(json.dumps({"result": result, "phase": phase, **fields}, sort_keys=True), flush=True)


def fixed_fd(fd, owner, limit):
    need(fd >= 3, "inherited-descriptor-required")
    st = os.fstat(fd)
    flags = fcntl.fcntl(fd, fcntl.F_GETFL)
    need(stat.S_ISREG(st.st_mode) and st.st_nlink == 0 and st.st_uid == owner
         and stat.S_IMODE(st.st_mode) == 0o400 and 0 < st.st_size <= limit
         and flags & os.O_ACCMODE == os.O_RDONLY and not flags & os.O_PATH,
         "not-an-original-readonly-fixture-fd")
    seals = fcntl.F_SEAL_SEAL | fcntl.F_SEAL_WRITE | fcntl.F_SEAL_GROW | fcntl.F_SEAL_SHRINK
    need(fcntl.fcntl(fd, fcntl.F_GET_SEALS) & seals == seals, "unsealed-fixture")
    os.set_inheritable(fd, False)
    data = os.pread(fd, st.st_size + 1, 0)
    need(len(data) == st.st_size, "incomplete-fixture")
    return data


def unique_object(pairs):
    result = dict(pairs)
    need(len(result) == len(pairs), "duplicate-evidence-member")
    return result


def ns_id(value):
    return (isinstance(value, list) and len(value) == 2
            and all(type(num) is int and 0 < num < 2**64 for num in value))


def evidence(raw, mode):
    move = mode in MOVE_MODES
    v = json.loads(raw.decode("ascii"), object_pairs_hook=unique_object)
    need(isinstance(v, dict), "bad-evidence-shape")
    keys = EVIDENCE_KEYS | MOVE_KEYS if move else EVIDENCE_KEYS
    if move and UBUNTU_CLIENT in v:
        client = v[UBUNTU_CLIENT]
        need(v.get("move_case") == "stock"
             and isinstance(client, dict) and set(client) == {"package_sha256", "elf_sha256", "version"}
             and client["package_sha256"] == UBUNTU_CLIENT_PACKAGE
             and isinstance(client["elf_sha256"], str) and re.fullmatch(r"[0-9a-f]{64}", client["elf_sha256"])
             and int(client["elf_sha256"], 16) != 0
             and client["version"] == UBUNTU_CLIENT_VERSION,
             "bad-ubuntu-stock-client-evidence")
        keys = keys | {UBUNTU_CLIENT}
    need(set(v) == keys, "bad-evidence-shape")
    profile = "ZXS-ISOLATED-SMB-MOVE-1" if move else "ZXS-ISOLATED-SMB-ACCEPT-1"
    cut = "same-elf-listen-and-distinct-original-media" if move else "same-elf-listen-confirmed"
    need(v["profile"] == profile and v["observed"] == cut and v["host"] == SERVER and v["address"] == TARGET,
         "not-the-disposable-test-endpoint")
    for key, length in (("source_commit", 40), ("zxs_sha256", 64)):
        need(isinstance(v[key], str) and re.fullmatch(rf"[0-9a-f]{{{length}}}", v[key])
             and int(v[key], 16) != 0, "missing-external-source-evidence")
    need(ns_id(v["client_netns"]) and ns_id(v["outside_netns"])
         and v["client_netns"] != v["outside_netns"], "not-distinct-namespaces")
    labels = [v["data_label"]] + ([] if v["recycle_label"] is None else [v["recycle_label"]])
    if move:
        labels.append(v["target_data_label"])
        need(v["recycle_label"] is not None and v["move_case"] in ("stock", "original")
             and (not mode.startswith("move-") or mode == "move-" + v["move_case"]),
             "wrong-original-move-case")
    for label in labels:
        need(isinstance(label, str) and re.fullmatch(r"[A-Za-z][A-Za-z0-9_-]{0,31}", label), "bad-fixture-label")
    need(len(set(labels)) == len(labels), "same-fixture-labels")
    return v


def ip_json(*args):
    result = subprocess.run(["/usr/sbin/ip", "-json", *args], check=True, timeout=TIMEOUT,
                            stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, env=ENV)
    need(len(result.stdout) <= 65536, "unexpected-topology-size")
    out = json.loads(result.stdout)
    need(isinstance(out, list), "unexpected-topology")
    return out


def isolated_network(v):
    live = os.stat("/proc/self/ns/net")
    need([live.st_dev, live.st_ino] == v["client_netns"], "wrong-original-client-namespace")
    with open("/proc/self/status", encoding="ascii") as status:
        cap = next(line.partition(":")[2].strip() for line in status if line.startswith("CapEff:"))
    need(int(cap, 16) == 0, "client-has-effective-capabilities")
    links = ip_json("-details", "link", "show")
    need(sorted(row["ifname"] for row in links) == ["lo", LINK], "unexpected-interface")
    peer = next(row for row in links if row["ifname"] == LINK)
    need(peer.get("linkinfo", {}).get("info_kind") == "veth" and not peer.get("master"), "not-the-test-veth")
    nets = {"lo": ipaddress.ip_network("127.0.0.0/8"), LINK: ipaddress.ip_network("192.0.2.0/30")}
    for route in ip_json("-4", "route", "show", "table", "all"):
        ip4 = nets.get(route.get("dev"))
        need(ip4 is not None and route.get("dst") != "default"
             and not ROUTED_KEYS.intersection(route)
             and ipaddress.ip_network(route["dst"], strict=False).subnet_of(ip4), "unexpected-ipv4-route")
    for route in ip_json("-6", "route", "show", "table", "all"):
        need(route.get("dev") == "lo" and route.get("dst") in ("::1", "::1/128")
             and not ROUTED_KEYS.intersection(route), "unexpected-ipv6-route")
    route = ip_json("-4", "route", "get", TARGET)
    need(len(route) == 1 and route[0].get("dev") == LINK and route[0].get("prefsrc") == CLIENT
         and not ROUTED_KEYS.intersection(route[0]), "wrong-local-target-route")


def credentials(fd):
    raw = fixed_fd(fd, os.geteuid(), 256)
    match = re.fullmatch(rb"username = ([a-z][a-z0-9_-]{0,62})\npassword = ([A-Za-z0-9_+.!-]{24,64})\ndomain = ZXSLOCAL\n", raw)
    need(match is not None, "unexpected-disposable-account-form")
    return match[1].decode("ascii"), match[2].decode("ascii")


@contextmanager
def exact_ubuntu_client(path, expected):
    raw = os.open(path, os.O_RDONLY | os.O_CLOEXEC | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(raw, "rb") as image:
        first = os.fstat(raw)
        need(stat.S_ISREG(first.st_mode) and first.st_nlink == 1
             and first.st_uid == first.st_gid == 0 and stat.S_IMODE(first.st_mode) == 0o755
             and 0 < first.st_size <= MAX_CLIENT_ELF and os.pread(raw, 4, 0) == b"\x7fELF",
             "unqualified-ubuntu-stock-executable")
        digest = hashlib.file_digest(image, "sha256").hexdigest()
        last = os.fstat(raw)
        need(digest == expected
             and (first.st_size, first.st_mtime_ns, first.st_ctime_ns) ==
                 (last.st_size, last.st_mtime_ns, last.st_ctime_ns),
             "changed-ubuntu-stock-executable")
        yield raw


def stock(fd, move=None, ubuntu=None):
    binary = "/usr/bin/smbclient"
    opts = {"timeout": TIMEOUT * 2, "stdin": subprocess.DEVNULL, "stderr": subprocess.DEVNULL, "env": ENV}
    need(ubuntu is None or move is not None, "ubuntu-client-only-for-move")
    kept = exact_ubuntu_client(binary, ubuntu["elf_sha256"]) if ubuntu is not None else nullcontext(None)
    with kept as image:
        # A reviewed Ubuntu intake executes the actual hashed fd, not another
        # object rebound at the name. The ordinary manual path is unchanged.
        call = {"executable": f"/proc/self/fd/{image}"} if image is not None else {}
        image_fds = (image,) if image is not None else ()
        banner = subprocess.run([binary, "--version"], check=True, stdout=subprocess.PIPE,
                                pass_fds=image_fds, **opts, **call).stdout
        if ubuntu is not None:
            need(banner == ubuntu["version"].encode("ascii") + b"\n", "wrong-original-ubuntu-stock-version")
            ver = banner[:-1]
        else:
            ver = banner.strip()
            need(re.fullmatch(rb"Version (4\.22\.[0-9]{1,3}(?:[-+][A-Za-z0-9.+~-]{1,40})?)", ver),
                 "unreviewed-stock-client")
        if move is None:
            phase, command = "stock-tree-echo", "echo 1 zxs-acceptance"
        else:
            source, target = move
            phase, command = "stock-signed-rename-only", f"rename {source} {target}"
        cmd = [binary, f"//{SERVER}/ZXS", f"--ip-address={TARGET}", f"--port={PORT}",
               "--configfile=/dev/null", "--option=client min protocol=SMB3_11", "--max-protocol=SMB3_11",
               "--use-kerberos=off", "--client-protection=sign", "--name-resolve=host",
               f"--authentication-file=/proc/self/fd/{fd}", "--debuglevel=0", f"--timeout={TIMEOUT}",
               f"--command={command}"]
        rc = subprocess.run(cmd, stdout=subprocess.DEVNULL, pass_fds=(fd, *image_fds), **opts, **call).returncode
    if rc != 0:
        report("fail", phase, exit_code=rc, version=ver.decode("ascii"))
        return 1
    report("pass", phase, version=ver.decode("ascii"))
    return 0


def typed(v, user, password, phase, mode="operations", credential_fd=None):
    for distribution, version in WHEELS.items():
        need(importlib.metadata.version(distribution) == version, "unreviewed-typed-client")
    need(all(importlib.util.find_spec(name) is None for name in ("gssapi", "krb5")), "kerberos-plugin-in-ntlm-fixture")
    from smbprotocol import Dialects
    from smbprotocol.connection import Connection, HashAlgorithms, SecurityMode, SigningAlgorithms
    from smbprotocol.exceptions import SMB2ErrorResponse, SMBResponseException
    from smbprotocol.file_info import FileInternalInformation, FileRenameInformation
    from smbprotocol.header import Commands, NtStatus, Smb2Flags
    from smbprotocol.open import (
        CreateAction, CreateDisposition, CreateOptions, DirectoryAccessMask, FileAttributes,
        FileInformationClass, FilePipePrinterAccessMask, ImpersonationLevel, Open, ShareAccess,
        SMB2QueryInfoRequest, SMB2QueryInfoResponse, SMB2SetInfoRequest, SMB2SetInfoResponse,
    )
    from smbprotocol.session import Session
    from smbprotocol.tree import TreeConnect

    class ObservedConnection(Connection):
        move_replies = None
        move_request = None
        move_async_id = None
        move_bad_reply = False

        def verify_signature(self, header, session_id, force=False):
            # The upstream input worker sees replies before replacing the
            # saved response on its original Request. Record, never alter it.
            replies = None if force else self.move_replies
            try:
                if replies is not None:
                    need(len(replies) < 2 and header["command"].get_value() == Commands.SMB2_SET_INFO,
                         "unrelated-or-duplicate-move-response")
                result = super().verify_signature(header, session_id, force=force)
            except Exception:
                if replies is not None:
                    self.move_bad_reply = True
                raise
            if replies is not None:
                replies.append(header)
            return result

        def check_move(self, request, reply, *, pending):
            flags = Smb2Flags.SMB2_FLAGS_SERVER_TO_REDIR | Smb2Flags.SMB2_FLAGS_ASYNC_COMMAND
            if not pending:
                flags |= Smb2Flags.SMB2_FLAGS_SIGNED
            # 1.17.0 stores both layouts as SMB2HeaderResponse.
            async_id = reply["reserved"].get_value() | (reply["tree_id"].get_value() << 32)
            need(reply["flags"].get_value() == flags and reply["next_command"].get_value() == 0
                 and request.message["flags"].get_value() == Smb2Flags.SMB2_FLAGS_SIGNED
                 and reply["command"].get_value() == request.message["command"].get_value() == Commands.SMB2_SET_INFO
                 and reply["message_id"].get_value() == request.message["message_id"].get_value()
                 and reply["session_id"].get_value() == request.session_id and async_id > 0
                 and reply["credit_response"].get_value() in ((0, 1) if pending else (0,)),
                 "wrong-original-move-response")
            if pending:
                body = SMB2ErrorResponse()
                raw = reply["data"].get_value()
                need(reply["status"].get_value() == NtStatus.STATUS_PENDING
                     and reply["signature"].get_value() == bytes(16)
                     and len(raw) == 8 and not body.unpack(raw)
                     and body["structure_size"].get_value() == 9 and body["reserved"].get_value() == 0
                     and body["error_context_count"].get_value() == body["byte_count"].get_value() == 0,
                     "wrong-move-interim")
            else:
                need(async_id == self.move_async_id and reply["status"].get_value() != NtStatus.STATUS_PENDING,
                     "wrong-terminal-async-id")
                self.verify_signature(reply, request.session_id, force=True)
            return async_id

        # Use the upstream client for all packets and crypto. The worker's
        # default verification skips unsigned messages; acceptance must
        # explicitly reject such a reply, not just request signed traffic.
        def receive(self, request, wait=True, timeout=None, resolve_symlinks=False):
            def check(reply):
                if request is self.move_request:
                    self.check_move(request, reply, pending=False)
                    return
                command = request.message["command"].get_value()
                if command == Commands.SMB2_NEGOTIATE:
                    return
                flags = reply["flags"]
                if command == Commands.SMB2_SESSION_SETUP:
                    if reply["status"].get_value() == 0:
                        # Session.connect verifies the final signature after
                        # deriving its key, before returning to the caller.
                        need(flags.has_flag(Smb2Flags.SMB2_FLAGS_SIGNED), "unsigned-session-success")
                    return
                need(flags.has_flag(Smb2Flags.SMB2_FLAGS_SIGNED)
                     and flags.has_flag(Smb2Flags.SMB2_FLAGS_SERVER_TO_REDIR)
                     and request.message["flags"].has_flag(Smb2Flags.SMB2_FLAGS_SIGNED)
                     and not flags.has_flag(Smb2Flags.SMB2_FLAGS_ASYNC_COMMAND)
                     and reply["next_command"].get_value() == 0
                     and reply["command"].get_value() == command
                     and reply["message_id"].get_value() == request.message["message_id"].get_value()
                     and (command == Commands.SMB2_TREE_CONNECT
                          or reply["tree_id"].get_value() == request.message["tree_id"].get_value())
                     and reply["session_id"].get_value() == request.session_id, "unsigned-or-unbound-reply")
                self.verify_signature(reply, request.session_id, force=True)

            try:
                reply = super().receive(request, wait=wait, timeout=timeout or TIMEOUT, resolve_symlinks=False)
            except SMBResponseException as exc:
                check(exc.header)
                raise
            check(reply)
            return reply

    @contextmanager
    def tree(tag):
        phase[0] = f"{tag}-negotiate"
        conn = ObservedConnection(uuid.uuid4(), TARGET, port=PORT, require_signing=True)
        try:
            conn.connect(Dialects.SMB_3_1_1, timeout=TIMEOUT, preferred_signing_algos=[SigningAlgorithms.AES_CMAC])
            need(conn.dialect == Dialects.SMB_3_1_1 and conn.signing_algorithm_id == SigningAlgorithms.AES_CMAC
                 and conn.preauth_integrity_hash_id == HashAlgorithms.SHA_512
                 and conn.server_security_mode & SecurityMode.SMB2_NEGOTIATE_SIGNING_REQUIRED,
                 "wrong-dialect-or-server-signing-policy")
            phase[0] = f"{tag}-session"
            session = Session(conn, username=f"ZXSLOCAL\\{user}", password=password,
                              require_encryption=False, hostname_override=SERVER, auth_protocol="negotiate")
            session.connect()
            need(session.session_id and session.signing_required and session.signing_key and not session.encrypt_data,
                 "not-a-signed-authenticated-session")
            phase[0] = f"{tag}-tree"
            mount = TreeConnect(session, rf"\\{SERVER}\ZXS")
            mount.connect(require_secure_negotiate=True)
            need(not mount.encrypt_data and not mount.is_dfs_share, "unexpected-test-tree-profile")
            yield mount
            phase[0] = f"{tag}-tree-disconnect-logoff"
            mount.disconnect()
            session.disconnect(close=False, timeout=TIMEOUT)
        finally:
            # On a failure do not issue an implicit normal CLOSE, retry a
            # mutating request, or try to clean a potentially selected FILE.
            if conn.transport is not None:
                with suppress(Exception):
                    conn.disconnect(close=False)

    share = ShareAccess.FILE_SHARE_READ | ShareAccess.FILE_SHARE_WRITE | ShareAccess.FILE_SHARE_DELETE
    rd = FilePipePrinterAccessMask.FILE_READ_DATA | FilePipePrinterAccessMask.FILE_READ_ATTRIBUTES
    wr = rd | FilePipePrinterAccessMask.FILE_WRITE_DATA
    delete = FilePipePrinterAccessMask.DELETE

    def open_held(mount, path, access, *, create=False, directory=False, recycle_on_close=False):
        opened = Open(mount, path)
        options = CreateOptions.FILE_DIRECTORY_FILE if directory else CreateOptions.FILE_NON_DIRECTORY_FILE
        if recycle_on_close:
            options |= CreateOptions.FILE_DELETE_ON_CLOSE
        attrs = FileAttributes.FILE_ATTRIBUTE_DIRECTORY if directory else FileAttributes.FILE_ATTRIBUTE_NORMAL
        opened.create(ImpersonationLevel.Impersonation, access, attrs, share,
                      CreateDisposition.FILE_CREATE if create else CreateDisposition.FILE_OPEN, options)
        need(opened.create_action == (CreateAction.FILE_CREATED if create else CreateAction.FILE_OPENED),
             "unexpected-create-action")
        return opened

    def absent(call, *codes):
        try:
            call()
        except SMBResponseException as exc:
            if exc.status in codes:
                return
            raise
        raise Refused("unexpected-success")

    leaf = f"zxs-check-{uuid.uuid4().hex}.bin"
    path = v["data_label"] + "\\" + leaf
    content = secrets.token_bytes(4096)

    def close_acked(opened):
        # The upstream Close API returns None for local already-closed
        # state and for a received FILE_CLOSED, neither a successful CLOSE.
        reply = opened.close()
        need(reply is not None and reply["structure_size"].get_value() == 60, "close-not-acknowledged")

    def send_info(handle, request):
        mount = handle.tree_connect
        request["file_id"] = handle.file_id
        transfer = handle.connection.send(request, sid=mount.session.session_id, tid=mount.tree_connect_id)
        return handle.connection.receive(transfer)["data"].get_value()

    def number(handle):
        req = SMB2QueryInfoRequest()
        req["info_type"] = FileInternalInformation.INFO_TYPE
        req["file_info_class"] = FileInternalInformation.INFO_CLASS
        req["output_buffer_length"] = len(FileInternalInformation())
        raw = send_info(handle, req)
        need(len(raw) == 16, "unexpected-file-index-response")
        body = SMB2QueryInfoResponse()
        body.unpack(raw)
        need(body["structure_size"].get_value() == 9 and body["output_buffer_offset"].get_value() == 72
             and body["output_buffer_length"].get_value() == 8, "unexpected-file-index-response")
        result = body.parse_buffer(FileInternalInformation)["index_number"].get_value()
        need(255 < result < 2**64 - 1, "not-an-ordinary-file-index")
        return result

    def read_view(handle, expected):
        need(handle.end_of_file == len(expected)
             and handle.read(0, len(expected), min_length=len(expected)) == expected,
             "wrong-original-file-view")

    def put_view(mount, name, data, create=False):
        writer = open_held(mount, name, wr, create=create)
        need(writer.write(data, offset=0) == len(data), "short-move-fixture-write")
        need(writer.flush()["structure_size"].get_value() == 4, "move-fixture-flush-not-acknowledged")
        close_acked(writer)

    def move_exact_open(held, target):
        conn = held.connection
        mount = held.tree_connect
        rename = FileRenameInformation()
        rename["replace_if_exists"] = False
        rename["file_name"] = target
        req = SMB2SetInfoRequest()
        req["info_type"] = rename.INFO_TYPE
        req["file_info_class"] = rename.INFO_CLASS
        req["file_id"] = held.file_id
        req["buffer"] = rename
        conn.move_replies = []
        try:
            original = conn.send(req, sid=mount.session.session_id, tid=mount.tree_connect_id, credit_request=1)
            conn.move_request = original
            # receive(wait=False) would remove the original outstanding request.
            need(original.response_event.wait(TIMEOUT) and conn.move_replies, "missing-move-interim")
            conn.move_async_id = conn.check_move(original, conn.move_replies[0], pending=True)
            need(isinstance(original.async_id, bytes) and len(original.async_id) == 8
                 and int.from_bytes(original.async_id, "little") == conn.move_async_id, "wrong-client-async-id")
            final = conn.receive(original, timeout=TIMEOUT)
            body = SMB2SetInfoResponse()
            raw = final["data"].get_value()
            need(not conn.move_bad_reply and len(conn.move_replies) == 2 and conn.move_replies[1] is final
                 and len(raw) == 2 and not body.unpack(raw) and body["structure_size"].get_value() == 2,
                 "wrong-move-terminal-response")
        finally:
            conn.move_replies = conn.move_request = conn.move_async_id = None

    if mode in MOVE_MODES:
        source, target = (label + "\\" + MOVE_LEAF for label in (v["data_label"], v["target_data_label"]))
        if mode in ("seed-move-a", "seed-move-b"):
            with tree("move-seed") as seed:
                phase[0] = mode
                absent(lambda: open_held(seed, target, rd), NAME_NOT_FOUND)
                if mode == "seed-move-b":
                    old = open_held(seed, source, rd)
                    read_view(old, MOVE_A)
                    close_acked(old)
                put_view(seed, source, MOVE_A if mode == "seed-move-a" else MOVE_B, create=mode == "seed-move-a")
            report("pass", mode)
            return
        if mode == "move-stock":
            with tree("before-stock-move") as before:
                phase[0] = "stock-move-original"
                absent(lambda: open_held(before, target, rd), NAME_NOT_FOUND)
                original = open_held(before, source, rd)
                logical_index = number(original)
                read_view(original, MOVE_B)
                close_acked(original)
            phase[0] = "stock-signed-rename"
            need(stock(credential_fd, (source, target), v.get(UBUNTU_CLIENT)) == 0, "stock-rename-not-confirmed")
            expected = MOVE_B
        else:
            with tree("move-owner") as owner:
                phase[0] = "same-open-original"
                absent(lambda: open_held(owner, target, rd), NAME_NOT_FOUND)
                original = open_held(owner, source, rd | delete)
                wire_open, logical_index = original.file_id, number(original)
                read_view(original, MOVE_B)
                with tree("move-new-confirmed-view") as writer:
                    put_view(writer, source, MOVE_C)
                read_view(original, MOVE_B)
                phase[0] = "same-open-signed-move"
                move_exact_open(original, target)
                phase[0] = "same-open-old-confirmed-view"
                need(original.file_id == wire_open and number(original) == logical_index, "original-open-or-index-changed")
                read_view(original, MOVE_B)
                close_acked(original)
            expected = MOVE_C
        with tree("fresh-after-move") as after:
            phase[0] = "move-fresh-target-and-original-name"
            absent(lambda: open_held(after, source, rd), NAME_NOT_FOUND)
            original = open_held(after, target, rd)
            need(number(original) == logical_index, "move-changed-logical-file-index")
            read_view(original, expected)
            close_acked(original)
        report("pass", "move-client-observations", case=v["move_case"],
               logical_index=f"{logical_index:016x}", same_open_checked=mode == "move-original")
        return

    def check_read(opened):
        need(opened.end_of_file == len(content), "wrong-original-file-eof")
        observed = opened.read(0, len(content), min_length=len(content))
        need(observed == content, "reopened-file-bytes-mismatch")
        close_acked(opened)
        return observed

    with tree("writer-a") as a:
        phase[0] = "create"
        writer = open_held(a, path, wr, create=True)
        phase[0] = "write"
        need(writer.write(content, offset=0) == len(content), "short-write")
        phase[0] = "flush"
        need(writer.flush()["structure_size"].get_value() == 4, "unexpected-flush-response")
        with tree("competitor-b") as b:
            phase[0] = "second-writer-refusal"
            # Both opens offer all share bits; a share-mask denial alone
            # is not sufficient evidence of the real Core writer gate.
            absent(lambda: open_held(b, path, wr), FILE_NOT_AVAILABLE)
            phase[0] = "separate-reader-while-writer-held"
            check_read(open_held(b, path, rd))
            phase[0] = "original-writer-close"
            close_acked(writer)
            phase[0] = "same-competitor-reopen-after-close"
            close_acked(open_held(b, path, wr))
    with tree("reader-c") as c:
        phase[0] = "fresh-session-readback"
        confirmed = check_read(open_held(c, path, rd))
    report("pass", "create-write-flush-single-writer-readback", bytes=len(confirmed), sha256=hashlib.sha256(confirmed).hexdigest())

    recycle = v["recycle_label"]
    if recycle is None:
        report("skip", "recycle-restore", reason="no-approved-empty-recycle-fixture")
        return

    def names(mount, relative):
        d = open_held(mount, relative, DirectoryAccessMask.FILE_LIST_DIRECTORY, directory=True)
        try:
            rows = d.query_directory("*", FileInformationClass.FILE_NAMES_INFORMATION, flags=0, max_output=4096)
        except SMBResponseException as exc:
            if exc.status != NO_SUCH_FILE:
                raise
            close_acked(d)
            return []
        # Require a complete bounded scan of the small fresh fixture, not
        # just whichever page happens to contain the intended name.
        found = [row["file_name"].get_value().decode("utf-16-le") for row in rows]
        absent(lambda: d.query_directory("", FileInformationClass.FILE_NAMES_INFORMATION, flags=0, max_output=4096),
               NO_MORE_FILES)
        close_acked(d)
        return found

    with tree("recycle-d") as d:
        phase[0] = "recycle-original-empty"
        need(not names(d, recycle), "recycle-fixture-not-empty")
        phase[0] = "stable-file-number-before-delete"
        original = open_held(d, path, rd)
        before = number(original)
        close_acked(original)
        phase[0] = "normal-delete-on-close"
        close_acked(open_held(d, path, delete, recycle_on_close=True))
        phase[0] = "original-name-not-selected"
        absent(lambda: open_held(d, path, rd), NAME_NOT_FOUND)
        phase[0] = "read-exact-retained-file"
        rounds = names(d, recycle)
        need(len(rounds) == 1 and re.fullmatch(r"[0-9a-f]{32}", rounds[0]), "not-one-original-recycle-operation")
        round_path = recycle + "\\" + rounds[0]
        need(names(d, round_path) == [leaf], "unrelated-retained-entry")
        retained = round_path + "\\" + leaf
        check_read(open_held(d, retained, rd))
        phase[0] = "standalone-restore-without-replacement"
        held = open_held(d, retained, delete)
        rename = FileRenameInformation()
        rename["replace_if_exists"] = False
        rename["file_name"] = path
        req = SMB2SetInfoRequest()
        req["info_type"] = rename.INFO_TYPE
        req["file_info_class"] = rename.INFO_CLASS
        req["buffer"] = rename
        raw = send_info(held, req)
        need(len(raw) == 2, "unexpected-restore-response")
        reply = SMB2SetInfoResponse()
        reply.unpack(raw)
        need(reply["structure_size"].get_value() == 2, "unexpected-restore-response")
        close_acked(held)
    with tree("restored-e") as e:
        phase[0] = "restored-file-id-and-byte-readback"
        opened = open_held(e, path, rd)
        need(number(opened) == before, "restore-changed-logical-file-index")
        check_read(opened)
        phase[0] = "old-retained-name-not-selected"
        absent(lambda: open_held(e, retained, rd), NAME_NOT_FOUND, PATH_NOT_FOUND)
    report("pass", "recycle-restore", same_logical_file=True)


def main():
    logging.disable(1000)  # Never emit third-party credential, token or packet logs.
    warnings.filterwarnings("ignore")
    args = argparse.ArgumentParser(description=__doc__)
    args.add_argument("mode", choices=("stock", "operations") + MOVE_MODES)
    args.add_argument("--evidence-fd", type=int, required=True)
    args.add_argument("--credentials-fd", type=int, required=True)
    options = args.parse_args()
    phase = ["preflight"]
    try:
        need(sys.platform == "linux" and sys.flags.isolated and os.geteuid() != 0
             and options.evidence_fd != options.credentials_fd, "need-isolated-unprivileged-linux-runner")
        v = evidence(fixed_fd(options.evidence_fd, 0, 2048), options.mode)
        isolated_network(v)
        need(not ("LM_COMPAT_LEVEL" in os.environ or "NTLM_USER_FILE" in os.environ), "ntlm-overrides-present")
        user, password = credentials(options.credentials_fd)
        report("observed", "external-fixture-record", source_commit=v["source_commit"], zxs_sha256=v["zxs_sha256"])
        if options.mode == "stock":
            phase[0] = "stock-tree-echo"
            return stock(options.credentials_fd)
        phase[0] = "typed-client-versions"
        typed(v, user, password, phase, options.mode, options.credentials_fd)
        return 0
    except Refused as exc:
        report("fail", phase[0], reason=exc.args[0])
    except Exception as exc:
        # SMBResponseException.status is numeric in the pinned library;
        # never stringify it (which can include raw response error data).
        status = getattr(exc, "status", None)
        if type(status) is int:
            report("fail", phase[0], ntstatus=f"0x{status:08x}")
        else:
            report("fail", phase[0], reason="client-transport-or-fixture-error")
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
