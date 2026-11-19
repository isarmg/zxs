"""Volatile, non-linkable Linux capture. Never remove a filesystem name on cleanup."""
from contextlib import contextmanager, nullcontext
import os
from pathlib import Path
import stat
import sys

from .capacity import close_output_fd, note, writing
from .document import native_identity
from .model import CHUNK, MAX_TOTAL, Rejected
from .private_candidates import CandidateDirectory, held_candidates


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


class Capture(CandidateDirectory):
    def __init__(self, area, body):
        super().__init__(area.path, area.parent, area.identity)
        self.body, self.operation = body, None
        self.length, self.spans = 0, {}

    def append(self, block):
        if self.length+len(block)>MAX_TOTAL: raise Rejected("anonymous freeze exceeds content limit")
        view = memoryview(block)
        while view:
            try: count = os.write(self.body,view)
            except InterruptedError: continue
            except OSError as error:
                note(error,"STATE",self.path,"写入匿名冻结正文；匿名体在最后一个句柄关闭后释放",self.operation,self.parent)
                raise
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
def anonymous_capture(state, *, held=None, operation=None):
    if not sys.platform.startswith("linux") or not getattr(os,"O_TMPFILE",0) or not hasattr(os,"pread"):
        raise Rejected("fresh commit needs native anonymous file support; no named scratch fallback")
    with (held_candidates(state) if held is None else nullcontext(held)) as area:
        if area.path != Path(state).absolute(): raise Rejected("not the selected private STATE")
        area.check_state()
        parent, ident = area.parent, area.identity
        selected_mount = local_mount(parent)
        with writing("STATE",area.path,"取得匿名冻结文件，尚未建立本次待决",operation,parent):
            body = os.open(".", os.O_TMPFILE|os.O_EXCL|os.O_RDWR|os.O_CLOEXEC, 0o600, dir_fd=parent)
        capture = None
        try:
            b = os.fstat(body)
            if (native_identity(b,stat.S_IFREG)[0]!=ident[0] or b.st_nlink!=0 or b.st_size!=0 or
                    b.st_uid!=os.geteuid() or b.st_mode & 0o077 or mount_id(body)!=selected_mount):
                raise Rejected("the private freeze did not produce an unnamed file on the original mount")
            capture = Capture(area,body)
            capture.check_state()
            yield capture
        finally:
            close_output_fd(body, "STATE", area.path, capture.operation if capture else operation, parent)
