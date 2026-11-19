"""Private desktop helper. Uses only inherited read descriptors; not a public command."""
import fcntl
import json
import os
from pathlib import Path
import re
import socket
import stat
import sys
import threading

if __name__ == "__main__" and not __package__:
    # -I -B with this concrete script, not a module chosen from cwd or another installation.
    from importlib.util import module_from_spec, spec_from_file_location
    root = Path(__file__).resolve().parent
    spec = spec_from_file_location("zipworkspace", root/"__init__.py", submodule_search_locations=[str(root)])
    if spec is None or spec.loader is None or "zipworkspace" in sys.modules: raise SystemExit(2)
    package = module_from_spec(spec)
    sys.modules[spec.name] = package
    spec.loader.exec_module(package)
    __package__ = spec.name

from .archive import stamp
from .document import archive_sha, native_identity
from .linux_mount import _ordinary_user, held_point
from .model import Commit, Rejected, check_id
from .private_freeze import mount_id
from .snapshot_view import SnapshotView

LIMIT = 16 << 10


def mount_receipt(lines, point, token, parent_id):
    # /proc mountinfo escapes precisely these bytes; no stat/open of our own waiting FUSE mount.
    path = os.fsencode(point)
    for ch, encoded in ((b"\\", b"\\134"), (b" ", b"\\040"), (b"\t", b"\\011"), (b"\n", b"\\012")):
        path = path.replace(ch, encoded)
    matches = []
    for line in lines.splitlines():
        a, sep, b = line.partition(b" - ")
        left, right = a.split(), b.split()
        if len(left) >= 5 and left[4] == path:
            matches.append((left, sep, right))
    if len(matches) != 1: return None
    left, sep, right = matches[0]
    if (not sep or len(left) < 6 or len(right) < 3 or b"ro" not in left[5].split(b",")
            or not left[0].isdigit() or left[1] != parent_id or
            not (right[0] == b"fuse" or right[0].startswith(b"fuse.")) or
            right[1] != ("zip-workspace-ro-" + check_id(token)).encode("ascii")):
        return None
    return left[0]


def receipt(point, token, parent_id):
    try:
        with open("/proc/self/mountinfo", "rb") as f: rows = f.read((2 << 20) + 1)
        if len(rows) <= 2 << 20: return mount_receipt(rows, point, token, parent_id)
    except OSError: pass
    return None


def mount_selected(req, doc, chain, authorize, notify, *, driver=None):
    uid = _ordinary_user()
    if not 2 <= len(chain) <= 65: raise Rejected("invalid mountpoint chain")
    if fcntl.fcntl(doc.fileno(), fcntl.F_GETFL) & os.O_ACCMODE != os.O_RDONLY:
        raise Rejected("an inherited read-only archive is required")
    native_identity(os.fstat(doc.fileno()), stat.S_IFREG)
    expected = tuple(req["observed"])
    if stamp(doc) != expected: raise Rejected("原窗口归档已经改变")
    marker = Commit.decode(req["marker"].encode("ascii"))
    digest, token = req["sha"], check_id(req["token"])
    if digest is not None and not re.fullmatch(r"[0-9a-f]{64}", digest):
        raise Rejected("invalid original archive digest")
    if driver is None:
        try: from ._fuse3 import serve as driver
        except (ImportError, OSError) as err:
            raise Rejected("同一环境缺少可用的 FUSE3 / mount 可选依赖；未自动安装") from err
    with held_point(req["point"], uid, chain=chain) as (point, fds, check_point):
        original_parent = mount_id(fds[-1])
        view = SnapshotView(doc, display_name=req["name"])
        if (view.original != expected or view.commit != marker or
                (digest is not None and archive_sha(doc) != digest)):
            raise Rejected("原包、所选节点或历史原件摘要不一致")
        view.check()
        check_point()
        authorize()  # Blocks before init. No later substitution of source/point or fresh HEAD lookup.
        def last_check():
            view.check(); check_point()
            notify(b"I")
        def accepted():
            mid = receipt(point, token, original_parent)
            notify(b"M" + mid if mid is not None else b"A")
        driver(view, point, last_check, token=token, accepted=accepted)


def await_decision(link, ready, go, abandon):
    try: answer = link.recv(2)
    except OSError: answer = b""
    if answer != b"G" or not ready.is_set():
        # Pre-GO this private process is read-only. The kernel releases its FDs, even on GUI loss.
        abandon(0)
        return
    go.set()


def seal_handoff(ids):
    if not 4 <= len(ids) <= 67 or min(ids) < 3 or len(set(ids)) != len(ids):
        raise Rejected("invalid original descriptor handoff")
    # pass_fds survives this one exec only. libfuse may itself invoke a system helper:
    # it must not inherit our GUI control channel, selected archive or directory authority.
    for fd in ids: os.set_inheritable(fd, False)


def main(argv=None):
    ids = [int(x) for x in (sys.argv[1:] if argv is None else argv)]
    seal_handoff(ids)
    link_fd, source, *chain = ids
    attempted = False
    with socket.socket(fileno=link_fd) as link, os.fdopen(source, "rb") as doc:
        def notify(msg):
            nonlocal attempted
            if msg == b"I": attempted = True
            try: link.send(msg)
            except OSError: pass  # After consent the dedicated service survives a lost GUI.
        try:
            if link.family != socket.AF_UNIX or link.type != socket.SOCK_SEQPACKET:
                raise Rejected("expected a private local control channel")
            packet = link.recv(LIMIT+1)
            if not packet or len(packet)>LIMIT: raise Rejected("invalid desktop request")
            req = json.loads(packet)
            ready, go = threading.Event(), threading.Event()
            threading.Thread(target=await_decision, args=(link, ready, go, os._exit),
                             daemon=True, name="mount-consent").start()
            def authorize():
                ready.set(); notify(b"R"); go.wait()
            mount_selected(req, doc, chain, authorize, notify)
            return 0
        except Exception as err:
            message = str(err).encode("utf-8", errors="replace")[:500]
            notify((b"U" if attempted else b"E") + message)
            return 1
        finally:
            for fd in reversed(chain): os.close(fd)


if __name__ == "__main__":
    raise SystemExit(main())
