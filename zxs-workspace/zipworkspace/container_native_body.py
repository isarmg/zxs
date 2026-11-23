"""Held local bodies for exact remote node reads. No export, ZIP or mount."""
from contextlib import contextmanager, ExitStack
import os
import stat
import sys
import tempfile

from .model import Rejected


def _stream(fd, mode):
    try:
        return os.fdopen(fd, mode)
    except BaseException:
        os.close(fd)
        raise


def _regular(fd):
    row = os.fstat(fd)
    if not stat.S_ISREG(row.st_mode) or not row.st_ino:
        raise Rejected("node staging is not an ordinary held file")
    return row


@contextmanager
def _darwin():
    from .macos_read import local_object, no_materialization

    flags = os.O_NOFOLLOW | os.O_CLOEXEC | os.O_NONBLOCK
    with tempfile.TemporaryDirectory(prefix="workspace-node-") as home, ExitStack() as held:
        with no_materialization():
            parent = os.open(home, os.O_RDONLY | os.O_DIRECTORY | flags)
            held.callback(os.close, parent)
            directory = os.fstat(parent)
            if directory.st_uid != os.geteuid() or stat.S_IMODE(directory.st_mode) != 0o700:
                raise Rejected("node staging requires its original private directory")
            local_object("node staging directory", parent)
            leaf = "body"  # Private generated directory, never a remote name.
            wf = os.open(leaf, os.O_RDWR | os.O_EXCL | os.O_CREAT | flags, 0o600, dir_fd=parent)
            writer = held.enter_context(_stream(wf, "wb"))
            rf = os.open(leaf, os.O_RDONLY | flags, dir_fd=parent)
            reader = held.enter_context(_stream(rf, "rb"))
            local_object("node staging body", rf)
            w, r = _regular(wf), _regular(rf)
            n = os.stat(leaf, dir_fd=parent, follow_symlinks=False)
            stamp = lambda row: (row.st_dev, row.st_ino, row.st_uid, row.st_nlink)
            if stamp(w) != stamp(r) or stamp(r) != stamp(n) or r.st_nlink != 1 or r.st_uid != os.geteuid():
                raise Rejected("node staging did not retain the exact original file")
            # Remove only our own newly created name before receiving any data.
            os.unlink(leaf, dir_fd=parent)
            if _regular(rf).st_nlink != 0:
                raise Rejected("node staging still has a published name")
        yield writer, reader


@contextmanager
def _windows():
    import ctypes as c
    import msvcrt
    from pathlib import Path
    import secrets
    from ._windows_output import ATTR, CREATE, DELETE, FILE_ONLY, READ, SYNC, WRITE, PinnedParent
    from .windows_nt import Handle, P

    def take(owner, mode, flags):
        fd = msvcrt.open_osfhandle(owner.value, flags | os.O_BINARY | os.O_NOINHERIT)
        owner.value = None
        return _stream(fd, mode)

    with PinnedParent(Path(tempfile.gettempdir()), output=True) as parent, ExitStack() as held:
        api = parent.api
        leaf = "remote-" + secrets.token_hex(16)
        # Exclusive FILE_CREATE; sharing 0; delete the staging object on final
        # close. The parent is a held, fixed local NTFS directory; no native
        # name is opened a second time.
        DELETE_ON_CLOSE = 0x1000
        original = api._request(parent.handle, leaf, SYNC | ATTR | READ | WRITE | DELETE,
                                0, CREATE, FILE_ONLY | DELETE_ON_CLOSE)
        held.enter_context(original)
        src = original.value
        api.require_local_ntfs(src)
        process, target = api.current_process(), P()
        # Downscope to read data+attributes+synchronize, no inheritance.
        if not api.duplicate_handle(process, src, process, c.byref(target), SYNC | ATTR | READ, 0, 0):
            raise c.WinError(c.get_last_error())
        if not target.value:
            raise Rejected("no read-only duplicate of the original staging handle")
        clone = Handle(api, target.value)
        held.enter_context(clone)
        reader = held.enter_context(take(clone, "rb", os.O_RDONLY))
        writer = held.enter_context(take(original, "wb", os.O_RDWR))
        _regular(reader.fileno())
        yield writer, reader


def stage_pair():
    if sys.platform == "darwin":
        return _darwin()
    if sys.platform == "win32":
        return _windows()
    raise Rejected("native held remote downloads in this adapter require macOS or Windows")
