"""The fixed acceptance case inside a newly approved, no-external-NIC VM."""

from contextlib import contextmanager
import fcntl
import hashlib
from html.parser import HTMLParser
import http.client
from http.cookies import SimpleCookie
import json
import os
from pathlib import Path
import re
import secrets
import sys
import time
from urllib.parse import urlencode

INPUT = Path("/mnt/move-input/input")
sys.path.insert(0, str(INPUT))  # the original hash-checked, hypervisor-readonly seed
from guest_smb_move_foreground import (ADDRESS, GID, HOST, LOGIN, REAPED, UID,
                                       initialize, issue, require, run_server)
from guest_smb_move_isolation import isolated_link, mount_fixture, run, SETPRIV
from guest_smb_move_material import install_inputs
from guest_smb_move_media import observe_roles
from guest_smb_move_records import after_move, before_move, identify_original, snapshot
from guest_terminal_preflight import scope
from rust_smb_move_ci import PROFILE, hex_id, object_once, read_regular

AUTHORITY = ADDRESS + ":8443"
ORIGIN = "http://" + AUTHORITY
MAX_WEB = 192 * 1024
HISTORY_BIN = "/zxs/bin/move-history-test"
HISTORY_TEST = "system::reader::smb::guest_move_history::guest_move_history_record"
READY_CASES = ("stock", "original")
stage = "admission"
REQUEST_FIELDS = {"profile", "source", "server_sha256", "helper_sha256", "guest_inputs_sha256", "challenge", "case"}


def seal_digest(value):
    return hashlib.sha256((json.dumps(value, sort_keys=True, ensure_ascii=True) + "\n").encode("ascii")).hexdigest()


@contextmanager
def mem_record(value, uid=0, gid=0):
    seals = fcntl.F_SEAL_SEAL | fcntl.F_SEAL_WRITE | fcntl.F_SEAL_GROW | fcntl.F_SEAL_SHRINK
    fd = os.memfd_create("smb-move-fixture", os.MFD_CLOEXEC | os.MFD_ALLOW_SEALING)
    readonly = None
    try:
        require(0 < len(value) <= 2048 and os.write(fd, value) == len(value))
        os.fchown(fd, uid, gid)
        os.fchmod(fd, 0o400)
        fcntl.fcntl(fd, fcntl.F_ADD_SEALS, seals)
        path = f"/proc/thread-self/fd/{fd}"
        readonly = os.open(path, os.O_RDONLY | os.O_CLOEXEC)
        require(os.path.samestat(os.fstat(fd), os.fstat(readonly))
                and fcntl.fcntl(readonly, fcntl.F_GETFL) & os.O_ACCMODE == os.O_RDONLY)
        os.close(fd)
        fd = None
        yield readonly
    finally:
        if readonly is not None:
            os.close(readonly)
        if fd is not None:
            os.close(fd)


class Form(HTMLParser):
    def __init__(self):
        super().__init__(convert_charrefs=True)
        self.current = None
        self.forms = []

    def handle_starttag(self, tag, fields):
        attributes = dict(fields)
        require(len(attributes) == len(fields))
        if tag == "form":
            require(self.current is None)
            if attributes.get("action") == "/versions/commit":
                require(attributes.get("method") == "post")
                self.current = {}
        elif tag == "input" and self.current is not None and attributes.get("type") == "hidden":
            key, value = attributes["name"], attributes["value"]
            require(key not in self.current)
            self.current[key] = value

    def handle_endtag(self, tag):
        if tag == "form" and self.current is not None:
            self.forms.append(self.current)
            self.current = None

    def original(self):
        require(self.current is None and len(self.forms) == 1)
        result, = self.forms
        require(set(result) == {"op", "csrf"} and hex_id(result["op"], 32) and hex_id(result["csrf"], 64))
        return result


class Web:
    def __init__(self):
        self.cookie = None

    def request(self, method, target, fields=None):
        require(target in ("/", "/login", "/versions/commit")
                or re.fullmatch(r"/versions\?file=[0-9a-f]{32}", target))
        connection = http.client.HTTPConnection(ADDRESS, 8443, timeout=12)
        headers = {"Host": AUTHORITY, "Origin": ORIGIN}
        if self.cookie is not None:
            headers["Cookie"] = self.cookie
        data = None if fields is None else urlencode(fields).encode("ascii")
        if fields is not None:
            headers["Content-Type"] = "application/x-www-form-urlencoded"
        try:
            connection.request(method, target, body=data, headers=headers)
            response = connection.getresponse()
            raw = response.read(MAX_WEB + 1)
            require(len(raw) <= MAX_WEB)
            return response.status, response.headers, raw.decode("utf-8", errors="strict")
        finally:
            connection.close()

    def login(self, password):
        status, _, html = self.request("GET", "/login")
        require(status == 200 and 'action="/login"' in html and 'name="password"' in html)
        status, headers, _ = self.request("POST", "/login", {"login": LOGIN, "password": password.decode("ascii")})
        raw = headers.get_all("Set-Cookie", [])
        require(status == 303 and headers.get("Location") == "/" and len(raw) == 1)
        cookie = SimpleCookie(raw[0])
        require(set(cookie) == {"zxs-http"} and hex_id(cookie["zxs-http"].value, 64)
                and cookie["zxs-http"]["httponly"] and cookie["zxs-http"]["samesite"] == "Strict")
        self.cookie = "zxs-http=" + cookie["zxs-http"].value
        status, _, html = self.request("GET", "/")
        require(status == 200 and all(label in html for label in ("Source", "Target", "ZXS")))

    def commit(self, file, ordinal):
        status, _, html = self.request("GET", "/versions?file=" + file.hex())
        require(status == 200 and "zxs-normal-move.bin" in html)
        form = Form()
        form.feed(html)
        form.close()
        original = form.original()
        status, _, html = self.request("POST", "/versions/commit", original)
        require(status == 200 and "正式版本已提交" in html and file.hex() in html and f"第 {ordinal} 个版本" in html)
        return bytes.fromhex(original["op"])


LOWER = (SETPRIV, "--reuid", str(UID), "--regid", str(GID), "--clear-groups", "--no-new-privs", "--")
PYTHON = ("/usr/bin/python3", "-I", "-B")


def client_run(peer, fixture, mode, evidence, password):
    private = b"username = " + LOGIN.encode("ascii") + b"\npassword = " + password + b"\ndomain = ZXSLOCAL\n"
    public = json.dumps(evidence, sort_keys=True).encode("ascii")
    with mem_record(public) as facts, mem_record(private, UID, GID) as cred:
        # Only these two exact versioned read-only wheels and the original
        # pinned test source enter the isolated interpreter. No import install.
        code = "import runpy,sys; sys.path[:0]=sys.argv[1:3]; sys.argv=sys.argv[3:]; runpy.run_path(sys.argv[0],run_name='__main__')"
        raw = peer.unprivileged((*PYTHON, "-c", code, *(str(w) for w in fixture.wheels), str(fixture.client),
                                 mode, "--evidence-fd", str(facts), "--credentials-fd", str(cred)), fds=(facts, cred))
    rows = [json.loads(line, object_pairs_hook=object_once) for line in raw.splitlines()]
    require(2 <= len(rows) <= 3 and rows[0] == {"result": "observed", "phase": "external-fixture-record",
                                                "source_commit": evidence["source_commit"],
                                                "zxs_sha256": evidence["zxs_sha256"]})
    if mode.startswith("seed-"):
        require(rows[1:] == [{"result": "pass", "phase": mode}])
    else:
        if evidence["move_case"] == "stock":
            require(len(rows) == 3 and rows[1] == {"result": "pass", "phase": "stock-signed-rename-only",
                                                  "version": evidence["ubuntu_smbclient"]["version"]})
        else:
            require(len(rows) == 2)
        final = rows[-1]
        require(set(final) == {"result", "phase", "case", "same_open_checked", "logical_index"}
                and final["result"] == "pass" and final["phase"] == "move-client-observations"
                and final["case"] == evidence["move_case"]
                and final["same_open_checked"] is (evidence["move_case"] == "original")
                and hex_id(final["logical_index"], 16))
    return rows


def observer(request, binding, file, facts, operation=None):
    pool, left, right, nodes = binding
    challenge = request["challenge"]
    placement = facts["location"][0]
    a, b = nodes
    payload = "\n".join(("SMB-MOVE-HISTORY-1", challenge, pool.hex(), left.hex(), right.hex(), file.hex(),
                         a.hex(), b.hex(), "-" if operation is None else operation.hex(), "")).encode("ascii")
    with mem_record(payload) as fd:
        code = ("import os,sys; os.dup2(int(sys.argv[1]),3,inheritable=True); "
                "os.execv(sys.argv[2],sys.argv[2:])")
        output = run((*LOWER, *PYTHON, "-c", code, str(fd), HISTORY_BIN, "--ignored", "--exact", HISTORY_TEST,
                      "--test-threads=1", "--nocapture"), fds=(fd,), capture=True, timeout=28)
    rows = [line.decode("ascii").split() for line in output.splitlines() if line.startswith(b"SMB-MOVE-HISTORY ")]
    require(len(rows) == 1 and b"1 passed; 0 failed" in output)
    row, = rows
    require(len(row) == 17 and row[:6] == ["SMB-MOVE-HISTORY", challenge, file.hex(), placement[3].hex(),
                                           placement[1].hex(), str(placement[2])])
    allowed = {"source"} if operation is None else {"pending", "retired"}
    require(row[6] in allowed)
    for index, parts in enumerate((row[7:12], row[12:17])):
        node = facts["node"][index]
        require(parts == [nodes[index].hex(), node[5].hex(), str(node[7]), "4096",
                          hashlib.sha256(bytes((b"AB"[index],)) * 4096).hexdigest()])
    return row


def case_evidence(request, fixture, mounts):
    global stage
    ids = {key: secrets.token_hex(16) for key in ("actor", "init", "init_approval", "instance", "bind", "setup", "setup_approval")}
    require(len(set(ids.values())) == len(ids))
    password = secrets.token_hex(24).encode("ascii")
    stage = "init"
    subject, init = initialize(ids, mounts)
    stage = "credential"
    setup = issue(ids, request, fixture.providers, subject, init, password)
    case = request["case"]
    stage = "local-link"
    with isolated_link() as peer:
        stage = "startup"
        with run_server(ids, init, request["server_sha256"]) as (_, ready):
            stage = "web-login"
            web = Web()
            web.login(password)  # actual same origin, after both PREPARED and lowered process checks
            evidence = {"profile": "ZXS-ISOLATED-SMB-MOVE-1", "observed": "same-elf-listen-and-distinct-original-media",
                        "source_commit": request["source"], "zxs_sha256": request["server_sha256"],
                        "client_netns": peer.identity, "outside_netns": peer.outside, "host": HOST, "address": ADDRESS,
                        "data_label": "Source", "target_data_label": "Target", "recycle_label": "Recycle", "move_case": case}
            if case == "stock":
                evidence["ubuntu_smbclient"] = fixture.ubuntu_smbclient
            stage = "seed-and-commit"
            client_run(peer, fixture, "seed-move-a", evidence, password)
            file = identify_original()
            commits = [web.commit(file, 1)]
            client_run(peer, fixture, "seed-move-b", evidence, password)
            commits.append(web.commit(file, 2))
            previous = snapshot(file)
            binding = before_move(previous, file)
            require(binding[0].hex() == init[0] and commits == [n[3] for n in previous["node"]]
                    and observe_roles(UID, GID, empty=False) == mounts)
            stage = "source-history"
            source_bytes = observer(request, binding, file, previous)
            stage = "signed-move"
            moved = client_run(peer, fixture, "move-" + case, evidence, password)
            index = moved[-1]["logical_index"]
            stage = "retirement"
            until, move_id = time.monotonic() + 25, None
            while True:
                finish = snapshot(file)
                operation, retired = after_move(previous, finish, file, case, index)
                require(move_id is None or move_id == operation)
                move_id = operation
                if retired == "retired":
                    break
                require(time.monotonic() < until)
                time.sleep(0.2)  # read-only observation of the original cleanup, never retry MOVE
            stage = "target-history"
            target_bytes = observer(request, binding, file, finish, move_id)
            require(target_bytes[6] == "retired" and observe_roles(UID, GID, empty=False) == mounts)
            stage = "reap"
    require(observe_roles(UID, GID, empty=False) == mounts)
    def sql_hex(value):
        if isinstance(value, bytes):
            return value.hex()
        if isinstance(value, (list, tuple)):
            return [sql_hex(item) for item in value]
        if isinstance(value, dict):
            return {k: sql_hex(v) for k, v in value.items()}
        return value
    return {"pool": init[0], "file": file.hex(), "mounts": seal_digest(mounts), "init": seal_digest(init),
            "setup": seal_digest(setup), "ready": seal_digest(ready), "client": seal_digest(moved),
            "system": seal_digest(sql_hex((previous, finish))), "versions": seal_digest((source_bytes, target_bytes)),
            "retirement": seal_digest((target_bytes[6], move_id.hex())), "reap": seal_digest(REAPED)}


def main():
    global stage
    require(not sys.argv[1:] and Path(__file__).absolute() == INPUT / "guest_smb_move_case.py" and sys.flags.isolated)
    request = json.loads(read_regular(INPUT / "request.json"), object_pairs_hook=object_once)
    require(set(request) == REQUEST_FIELDS and request["profile"] == PROFILE and request["case"] in READY_CASES
            and hex_id(request["source"], 40) and hex_id(request["challenge"], 32)
            and all(hex_id(request[key], 64) for key in ("server_sha256", "helper_sha256", "guest_inputs_sha256")))
    scope()  # exact pinned x86_64 image, initial PID 1 view, no other interface
    os.umask(0o022)
    # All seed raw/source/interface receipts precede private guest placement.
    stage = "offline-inputs"
    fixture = install_inputs(INPUT, request)
    require(fixture.case == request["case"])
    stage = "mounts"
    mounts = mount_fixture()
    result = case_evidence(request, fixture, mounts)
    print("SMB-MOVE-CASE " + json.dumps({**request, "evidence": result}, sort_keys=True), flush=True)


if __name__ == "__main__":
    try:
        main()
    except Exception:
        # Never echo raw terminal captures, third-party responses or secrets.
        print("SMB-MOVE-CASE-FAILED " + stage, file=sys.stderr, flush=True)
        raise SystemExit(1) from None
