# Private embedded guest for the fixed Debian 13 amd64 Python/Samba capsule.
# Importing it (or running this source as a script) never provisions anything.
# A future host launcher must positively establish authorization, fresh exec,
# exclusive originals, namespace/file-descriptor isolation and the signed runtime.
# This module can produce a nonsecret native observation, NEVER an authority.
import hashlib
import os
import stat
import sys
from contextlib import ExitStack

NAME = b"ZXSLOCAL"
SOURCE_CONFIG = "4f5cc8579cb37cbbf9ccdbc23a46ff40d319765336bf74b33ca4abd0a9160abb"
ROOT = "/instance"
PRIVATE = ROOT + "/private"
CONF = ROOT + "/domain-bootstrap.conf"
_entered = False


class Refused(Exception):
    """Do not add native exceptions, paths, TDB records or raw output to a receipt."""


def _require(ok):
    if not ok:
        raise Refused("dedicated fresh Samba provision not established")


def _is_just(directory, names):
    # No unbounded list or user filename is collected or printed.
    remaining = set(names)
    status = lambda st: (st.st_dev, st.st_ino, st.st_nlink, st.st_mtime_ns, st.st_ctime_ns)
    before = status(os.fstat(directory))
    # os.scandir(fd) may duplicate the same open-file description. Open "."
    # independently each time so an earlier EOF cannot mask any later entry.
    stream = os.open(".", os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=directory)
    try:
        if status(os.fstat(stream)) != before or before[2] == 0:
            return False
        with os.scandir(stream) as it:
            for row in it:
                if row.name not in remaining:
                    return False
                remaining.remove(row.name)
        return not remaining and status(os.fstat(stream)) == before and status(os.fstat(directory)) == before
    finally:
        os.close(stream)


def _same_fd_and_name(parent, name, fd):
    a = os.fstat(fd)
    b = os.stat(name, dir_fd=parent, follow_symlinks=False)
    return ((a.st_dev, a.st_ino, a.st_uid, a.st_mode, a.st_nlink)
            == (b.st_dev, b.st_ino, b.st_uid, b.st_mode, b.st_nlink))


def _store_at(private, uid, dev, expected=None):
    # Samba owns the open database. Do not independently open or inspect its
    # content; compare the inode behind the fixed name while that handle lives.
    mark = os.stat("secrets.tdb", dir_fd=private, follow_symlinks=False)
    identity = (mark.st_dev, mark.st_ino)
    _require(stat.S_ISREG(mark.st_mode) and stat.S_IMODE(mark.st_mode) == 0o600
             and mark.st_uid == uid and mark.st_dev == dev and mark.st_nlink == 1)
    if expected is not None:
        _require(identity == expected and mark.st_size > 0)
    return identity


def _handoff(result_socket, private, uid, dev, store, raw):
    # Only on the private host channel. A descriptor in flight keeps the
    # original inode alive across guest exit; no TDB payload is ever read.
    import array
    import socket
    _require(result_socket.family == socket.AF_UNIX
             and result_socket.getsockopt(socket.SOL_SOCKET, socket.SO_TYPE) == socket.SOCK_SEQPACKET)
    _store_at(private, uid, dev, store)
    fd = os.open("secrets.tdb", os.O_PATH | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=private)
    try:
        m = os.fstat(fd)
        _require((m.st_dev, m.st_ino) == store and stat.S_ISREG(m.st_mode)
                 and m.st_uid == uid and stat.S_IMODE(m.st_mode) == 0o600 and m.st_nlink == 1 and m.st_size > 0)
        _store_at(private, uid, dev, store)
        public = b"ZXS-SAM/NEW-1\0" + NAME + raw
        n = result_socket.sendmsg([public], [(socket.SOL_SOCKET, socket.SCM_RIGHTS, array.array("i", [fd]))],
                                  socket.MSG_NOSIGNAL)
        _require(n == len(public))
    finally:
        os.close(fd)  # O_PATH only, not a second open of the TDB data stream


def _domain(s):
    # 's' in the native path is the rendering of upstream's actual dom_sid.
    prefix = "S-1-5-21-"
    _require(isinstance(s, str) and s.startswith(prefix) and len(s) <= 48)
    parts = s[len(prefix):].split("-")
    _require(len(parts) == 3)
    values = []
    for part in parts:
        _require(1 <= len(part) <= 10 and part.isascii() and part.isdecimal())
        n = int(part)
        _require(str(n) == part and n <= 0xffffffff)
        values.append(n)
    _require(any(values))
    return bytes((1, 4, 0, 0, 0, 0, 0, 5, 21, 0, 0, 0)) + b"".join(n.to_bytes(4, "little") for n in values)


def _fixed_config(directory):
    # The host holds the original read-only artifact throughout the native call.
    n = "domain-bootstrap.conf"
    before = os.stat(n, dir_fd=directory, follow_symlinks=False)
    _require(stat.S_ISREG(before.st_mode) and 0 < before.st_size <= 4096 and before.st_nlink == 1)
    flags = os.O_RDONLY | os.O_NONBLOCK | os.O_NOFOLLOW | os.O_CLOEXEC
    with os.fdopen(os.open(n, flags, dir_fd=directory), "rb", buffering=0) as f:
        _require(os.fstatvfs(f.fileno()).f_flag & os.ST_RDONLY)
        _require(_same_fd_and_name(directory, n, f.fileno()))
        data = b""
        while len(data) <= before.st_size:
            more = f.read(before.st_size + 1 - len(data))
            if not more:
                break
            data += more
        _require(len(data) == before.st_size and hashlib.sha256(data).hexdigest() == SOURCE_CONFIG)
        _require(_same_fd_and_name(directory, n, f.fileno()))


def _provision_original():
    """Only called from the fixed supervisor's fresh, pre-authorized sandbox."""
    global _entered
    _require(not _entered)
    _entered = True  # secrets.c caches db_ctx; a failed attempt cannot be retried in-process.
    _require(sys.version_info[:2] == (3, 13) and sys.flags.isolated and sys.flags.no_site and sys.dont_write_bytecode)
    _require(os.getuid() == os.geteuid() != 0 and os.getgid() == os.getegid() != 0 and os.getcwd() == "/")
    _require(os.statvfs("/").f_flag & os.ST_RDONLY)
    flags = os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC
    with ExitStack() as held:
        def directory(name, parent=None):
            fd = os.open(name, flags, dir_fd=parent)
            held.callback(os.close, fd)
            return fd
        base = directory(ROOT)
        private = directory("private", base)
        state = directory("state", base)
        uid = os.geteuid()
        pm = os.fstat(private)
        sm = os.fstat(state)
        _require((pm.st_dev, pm.st_ino) != (sm.st_dev, sm.st_ino))
        _require(all(s.st_uid == uid and stat.S_IMODE(s.st_mode) == 0o700 and s.st_nlink > 0 for s in (pm, sm)))
        _require(os.fstatvfs(base).f_flag & os.ST_RDONLY and os.fstatvfs(state).f_flag & os.ST_RDONLY)
        _require(not (os.fstatvfs(private).f_flag & os.ST_RDONLY))
        def originals():
            _require(_same_fd_and_name(None, ROOT, base) and _same_fd_and_name(base, "private", private)
                     and _same_fd_and_name(base, "state", state))
            _fixed_config(base)

        originals()
        _require(_is_just(private, ()) and _is_just(state, ()))
        # No sitecustomize, .pth, PYTHONPATH or user modules. The eventual host
        # launcher supplies this single signed Debian dist-packages tree in an
        # otherwise immutable interpreter runtime. No host /etc, /var or sockets.
        sys.path.append("/usr/lib/python3/dist-packages")
        import samba
        from samba.samba3 import passdb, param
        _require(samba.version == "4.22.11" or samba.version.startswith("4.22.11-"))
        originals()
        _require(_is_just(private, ()) and _is_just(state, ()))
        passdb.set_smb_config(CONF)       # fixed bytes expressly select PRIVATE and standalone
        ctx = param.get_context()  # Samba3's actual globals; not a separately parsed configuration.
        expected = {"server role": "standalone server", "netbios name": NAME.decode("ascii"),
                    "workgroup": "ZXSWORK", "private dir": PRIVATE, "state directory": ROOT + "/state",
                    "passdb backend": "zxs_sqlite", "map to guest": "Never", "usershare max shares": 0}
        _require(all(ctx.get(key) == val for key, val in expected.items()))
        originals()
        _require(_is_just(private, ()) and _is_just(state, ()))
        passdb.set_secrets_dir(PRIVATE)  # intentional CREATE of this original instance's secrets.tdb
        store = _store_at(private, uid, pm.st_dev)
        originals()
        _require(_is_just(private, ("secrets.tdb",)) and _is_just(state, ()))
        raw = _domain(str(passdb.get_global_sam_sid()))
        originals()
        _require(_is_just(private, ("secrets.tdb",)) and _is_just(state, ()))
        _store_at(private, uid, pm.st_dev, store)
        # stdout is the one-shot supervisor socket, never a terminal or a log.
        # A receiver obtaining its EOF still needs a real, separate successful
        # child/monitor/reap result; EOF alone never authorizes synchronization.
        import socket
        with socket.socket(fileno=os.dup(1)) as output:
            _handoff(output, private, uid, pm.st_dev, store, raw)


if __name__ == "__main__":
    raise SystemExit("private embedded provision guest; no standalone command")
