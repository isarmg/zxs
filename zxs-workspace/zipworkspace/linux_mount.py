"""Linux read-only mount policy, receipt observation, and foreground command."""
import argparse
from contextlib import ExitStack, contextmanager
import os
from pathlib import Path
import stat
import sys
import zipfile

from .document import named_document, native_identity
from .model import Rejected, check_id, new_id
from .private_freeze import local_mount, mount_id
from .snapshot_view import SnapshotView

MOUNTINFO_BUDGET = 2 << 20
READ_ONLY_MOUNT_FLAGS = frozenset({"ro", "nodev", "nosuid", "noexec"})
_RECEIPT_FLAGS = frozenset(flag.encode("ascii") for flag in READ_ONLY_MOUNT_FLAGS)


def mount_source(token):
    return "zip-workspace-ro-" + check_id(token)


def mountinfo_path(point):
    path = os.fsencode(point)
    for ch, encoded in ((b"\\", b"\\134"), (b" ", b"\\040"), (b"\t", b"\\011"), (b"\n", b"\\012")):
        path = path.replace(ch, encoded)
    return path


def mount_receipt(lines, point, token, parent_id):
    """Match the original mount and its per-mount flags without opening its FUSE path."""
    path = mountinfo_path(point)
    matches = []
    for line in lines.splitlines():
        a, sep, b = line.partition(b" - ")
        left, right = a.split(), b.split()
        if len(left) >= 5 and left[4] == path:
            matches.append((left, sep, right))
    if len(matches) != 1: return None
    left, sep, right = matches[0]
    if (not sep or len(left) < 6 or len(right) < 3
            or not (1 <= len(left[0]) <= 20 and left[0].isdigit()) or left[0] == parent_id or left[1] != parent_id
            or not (right[0] == b"fuse" or right[0].startswith(b"fuse."))
            or right[1] != mount_source(token).encode("ascii")):
        return None
    flags = set(left[5].split(b","))  # The trailing options apply to the superblock, not this mount.
    if not _RECEIPT_FLAGS <= flags or b"rw" in flags:
        return None
    return left[0]


def observe_mount_receipt(point, token, parent_id):
    """Best-effort observation in this process's mount namespace, not a durable or current state promise."""
    try:
        with open("/proc/self/mountinfo", "rb") as f: rows = f.read(MOUNTINFO_BUDGET + 1)
    except OSError:
        return None
    if 0 < len(rows) <= MOUNTINFO_BUDGET and rows.endswith(b"\n"):
        return mount_receipt(rows, point, token, parent_id)
    return None


def _ordinary_user(*, read_proc=None):
    if sys.platform != "linux":
        raise Rejected("the snapshot mount is Linux-only")
    uid = os.getuid()
    if uid == 0 or uid != os.geteuid() or os.getgid() != os.getegid():
        raise Rejected("use an ordinary Linux user, without sudo or set-id")
    if read_proc is None:
        with open("/proc/self/status", "rb") as f: data = f.read(65537)
    else: data = read_proc("status", 65536)
    caps = [line.split()[1:] for line in data.splitlines() if line.startswith(b"CapEff:")]
    if len(data) > 65536 or len(caps) != 1 or len(caps[0]) != 1 or int(caps[0][0], 16) != 0:
        raise Rejected("an ordinary user with no effective capabilities is required")
    return uid


def _chain(stack, path, uid):
    dirs, ids = [], []
    for idx, part in enumerate(path.parts):
        leaf = idx == len(path.parts)-1
        flags = (os.O_RDONLY if leaf else os.O_PATH) | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC
        fd = os.open(part, flags, dir_fd=dirs[-1] if dirs else None)
        stack.callback(os.close, fd)
        m = os.fstat(fd)
        identity = native_identity(m, stat.S_IFDIR)
        if m.st_uid not in (0, uid) or m.st_mode & 0o022:
            raise Rejected("mountpoint ancestors must be owned by root or you and not writable by others")
        if leaf and (m.st_uid != uid or stat.S_IMODE(m.st_mode) != 0o700):
            raise Rejected("select your existing private 0700 mountpoint; no permissions will be changed")
        dirs.append(fd)
        ids.append((*identity, mount_id(fd)))
    if ids[-1][-1] != ids[-2][-1]:
        raise Rejected("will not cover an existing mount or bind mount")
    try: local_mount(dirs[-1])
    except Rejected as err: raise Rejected("the original mountpoint must be on an admitted ext4/XFS mount") from err
    with os.scandir(dirs[-1]) as items:
        if next(items, None) is not None: raise Rejected("mountpoint is not empty; nothing was hidden or removed")
    return tuple(dirs), tuple(ids)


def point_path(spelling):
    path = Path(spelling)
    if (not path.is_absolute() or path.anchor != "/" or str(path) != spelling or ".." in path.parts
            or not 2 <= len(path.parts) <= 65 or path.parts[1] in ("dev", "proc", "sys")):
        raise Rejected("give the exact absolute path of an existing dedicated mountpoint (not a device or FD path)")
    return path


def point_id(fd):
    return (*native_identity(os.fstat(fd), stat.S_IFDIR), mount_id(fd))


@contextmanager
def held_point(spelling, uid, *, leaf=None, chain=None):
    path = point_path(spelling)
    with ExitStack() as original:
        fds, ids = _chain(original, path, uid)
        if ((leaf is not None and point_id(leaf) != ids[-1]) or
                (chain is not None and tuple(map(point_id, chain)) != ids)):
            raise Rejected("the chosen mountpoint or its original directory chain changed")
        def recheck():
            with ExitStack() as now:
                if _chain(now, path, uid)[1] != ids:
                    raise Rejected("the selected mountpoint or an ancestor was replaced")
        yield path, fds, recheck


def main(args=None):
    p = argparse.ArgumentParser(prog="zip-workspace-mount-ro",
        description="Mount one verified .zxs snapshot read-only in a dedicated empty directory. "
                    "Keep the mountpoint and its ancestors stable. Recheck the reported SOURCE and ID before manual unmount.")
    p.add_argument("archive", type=Path)
    p.add_argument("mountpoint", help="existing, private 0700, absolute path; no mount/rename/sync here until unmount")
    ns = p.parse_args(args)
    try:
        uid = _ordinary_user()
        try:
            from ._fuse3 import serve
        except ImportError as err:
            raise Rejected("a working optional mount extra and system FUSE3 are required; nothing is installed automatically") from err
        source = ns.archive.absolute()
        with held_point(ns.mountpoint, uid) as (point, fds, recheck), named_document(source) as doc:
            view = SnapshotView(doc, display_name=source.name)
            parent_id = mount_id(fds[-1])
            token = new_id()
            label = mount_source(token)
            print(f"Read-only mount request for snapshot {view.commit.operation} at {str(point)!r}; "
                  f"SOURCE={label}. Not yet confirmed.", flush=True)

            def last_check():
                if native_identity(source.stat(follow_symlinks=False), stat.S_IFREG) != view.original[:2]:
                    raise Rejected("the selected archive name was replaced before mounting")
                view.check()
                recheck()

            def accepted():
                mid = observe_mount_receipt(point, token, parent_id)
                if mid is None:
                    print("FUSE accepted the request; no matching mount ID has been observed.", flush=True)
                else:
                    print("Observed this read-only mount in the kernel table: "
                          f"SOURCE={label}, ID={mid.decode('ascii')}.", flush=True)

            serve(view, point, last_check, token=token, accepted=accepted)
        return 0
    except KeyboardInterrupt:
        print("Interrupted; this does not confirm unmounting. If a mount was requested, "
              "recheck its recorded identity in the same mount namespace.", file=sys.stderr)
        return 130
    except (OSError, ValueError, RuntimeError, zipfile.BadZipFile) as err:
        print(f"zip-workspace-mount-ro: {err}", file=sys.stderr)
        return 1
