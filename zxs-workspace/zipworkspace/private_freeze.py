"""Volatile, non-linkable Linux capture. Never remove a filesystem name on cleanup."""
from contextlib import contextmanager
import os
from pathlib import Path
import stat
import sys

from .archive import read_managed_document, stamp
from .document import archive_sha, native_identity
from .model import CHUNK, MAX_TOTAL, Rejected, check_id


def mount_id(fd):
    with open(f"/proc/self/fdinfo/{fd}", "rb") as info:
        data = info.read(8193)
    ids = [s.split()[1:] for s in data.splitlines() if s.startswith(b"mnt_id:")]
    if len(data)>8192 or len(ids)!=1 or len(ids[0])!=1 or not ids[0][0].isdigit():
        raise Rejected("cannot verify the held Linux mount")
    return ids[0][0]


def local_mount(fd):
    """Qualify the actual held mount, not a user-entered path or a local-looking fstype prefix."""
    identifier = mount_id(fd)
    with open("/proc/self/mountinfo", "rb") as info:
        data = info.read((2 << 20)+1)
    if len(data)>2 << 20: raise Rejected("mount inventory exceeds the admission budget")
    rows = [line for line in data.splitlines() if line.partition(b" ")[0]==identifier]
    if len(rows)!=1: raise Rejected("held mount is missing or ambiguous")
    head, sep, tail = rows[0].partition(b" - ")
    a, b = head.split(), tail.split()
    dev = os.fstat(fd).st_dev
    if (not sep or len(a)<6 or not b or a[3]!=b"/" or b[0] not in (b"ext4",b"xfs")
            or a[2]!=f"{os.major(dev)}:{os.minor(dev)}".encode()):
        raise Rejected("fresh commit requires a verified local ext4/XFS mount; no named scratch fallback")
    return identifier


class Capture:
    def __init__(self, path, parent, identity, body):
        self.path, self.parent, self.identity, self.body = path, parent, identity, body
        self.length, self.spans = 0, {}

    def check_state(self):
        if (native_identity(os.fstat(self.parent), stat.S_IFDIR)!=self.identity or
                native_identity(self.path.stat(follow_symlinks=False), stat.S_IFDIR)!=self.identity):
            raise Rejected("private STATE no longer names the original directory")

    def append(self, block):
        if self.length+len(block)>MAX_TOTAL: raise Rejected("anonymous freeze exceeds content limit")
        view = memoryview(block)
        while view:
            try: count = os.write(self.body,view)
            except InterruptedError: continue
            if not 0<count<=len(view): raise OSError("incomplete private freeze write")
            self.length += count
            view = view[count:]

    def validate(self, tree):
        pos = 0
        for name,(start,size) in self.spans.items():
            if (start!=pos or size<0 or name not in tree.rows or tree.rows[name][:2]!=(False,size)):
                raise Rejected("invalid frozen file range")
            pos += size
        if (len(self.spans)!=sum(not row[0] for row in tree.rows.values()) or
                pos!=tree.total or pos!=self.length or pos!=os.fstat(self.body).st_size):
            raise Rejected("incomplete frozen file inventory")

    def blocks(self, name, expected):
        pos, left = self.spans[name]
        if left!=expected or pos<0 or pos+left>self.length: raise Rejected("invalid frozen extent")
        while left:
            try: block = os.pread(self.body,min(left,CHUNK),pos)
            except InterruptedError: continue
            if not block or len(block)>left: raise Rejected("incomplete frozen extent")
            left -= len(block); pos += len(block)
            yield block

    @contextmanager
    def candidate(self, operation):
        self.check_state()
        name = check_id(operation)+".zxs"
        fd = os.open(name, os.O_CREAT|os.O_EXCL|os.O_WRONLY|os.O_NOFOLLOW|os.O_CLOEXEC, 0o600, dir_fd=self.parent)
        try:
            # Close the output stream before re-reading; fd still pins its original inode.
            with os.fdopen(fd,"wb",closefd=False) as stream:
                yield name, fd, stream
        except Exception as error:
            error.add_note(f"原私有候选可能残留于 {self.path / name}，也可能尚未登记。保留现场，不自动删除或重试。")
            raise
        finally: os.close(fd)

    def verify_candidate(self, name, original, marker):
        self.check_state()
        original_id = native_identity(os.fstat(original),stat.S_IFREG)
        fd = os.open(name, os.O_RDONLY|os.O_NOFOLLOW|os.O_NONBLOCK|os.O_CLOEXEC, dir_fd=self.parent)
        try:
            with os.fdopen(fd,"rb",closefd=False) as doc:
                observed = stamp(doc)
                def check_original():
                    self.check_state()
                    named = os.stat(name,dir_fd=self.parent,follow_symlinks=False)
                    for m in (os.fstat(original),os.fstat(fd),named):
                        if (native_identity(m,stat.S_IFREG)!=original_id or m.st_nlink!=1 or
                                (m.st_dev,m.st_ino,m.st_size,m.st_mtime_ns,m.st_ctime_ns)!=observed):
                            raise Rejected("different private ZIP, link or generation at the candidate name")
                check_original()
                actual,_ = read_managed_document(doc,display_name=name)
                raw = archive_sha(doc)
                if actual!=marker: raise Rejected("original candidate marker mismatch")
                check_original()
                os.fsync(self.parent)
                # An earlier name-stat is not evidence that this barrier made
                # our actual original candidate reachable by its exact name.
                check_original()
                return raw
        finally: os.close(fd)


@contextmanager
def anonymous_capture(state):
    if not sys.platform.startswith("linux") or not getattr(os,"O_TMPFILE",0) or not hasattr(os,"pread"):
        raise Rejected("fresh commit needs native anonymous file support; no named scratch fallback")
    path = Path(state).absolute()
    parent = os.open(path, os.O_RDONLY|os.O_DIRECTORY|os.O_NOFOLLOW|os.O_CLOEXEC)
    try:
        m = os.fstat(parent); ident = native_identity(m,stat.S_IFDIR)
        if m.st_uid!=os.geteuid() or m.st_mode & 0o077 or native_identity(path.stat(follow_symlinks=False),stat.S_IFDIR)!=ident:
            raise Rejected("fresh commit requires the selected user's private STATE directory")
        selected_mount = local_mount(parent)
        body = os.open(".", os.O_TMPFILE|os.O_EXCL|os.O_RDWR|os.O_CLOEXEC, 0o600, dir_fd=parent)
        try:
            b = os.fstat(body)
            if (native_identity(b,stat.S_IFREG)[0]!=ident[0] or b.st_nlink!=0 or b.st_size!=0 or
                    b.st_uid!=os.geteuid() or b.st_mode & 0o077 or mount_id(body)!=selected_mount):
                raise Rejected("the private freeze did not produce an unnamed file on the original mount")
            capture = Capture(path,parent,ident,body)
            capture.check_state()
            yield capture
        finally: os.close(body)
    finally: os.close(parent)
