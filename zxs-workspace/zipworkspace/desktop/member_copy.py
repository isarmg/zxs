"""Save one verified member into a new local Linux file with no named staging."""
from contextlib import ExitStack
import ctypes
import hashlib
import os
from pathlib import Path
import stat
import sys
from threading import get_native_id

from ..archive import stored_member
from ..document import native_identity
from ..linux_mount import _ordinary_user
from ..model import CHUNK, MAX_CD, MAX_FILE, Rejected, member_name
from ..private_freeze import local_mount, mount_id, mount_record
from ..publish import Uncertain


def _directory(stack, path, uid, read_proc):
    # Hold each ancestor of the literal path. We never resolve a symlink or
    # continue under a newly selected directory after approval.
    descriptors, identities = [], []
    for i, part in enumerate(path.parts):
        mode = os.O_RDONLY if i == len(path.parts)-1 else os.O_PATH
        fd = os.open(part, mode | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC,
                     dir_fd=descriptors[-1] if descriptors else None)
        stack.callback(os.close, fd)
        m = os.fstat(fd)
        identities.append((*native_identity(m, stat.S_IFDIR), mount_id(fd, read_proc=read_proc)))
        if m.st_uid not in (0, uid) or m.st_mode & 0o022:
            raise Rejected("目的目录和上级目录必须属于本人或 root，且不能让组或其他人写入")
        descriptors.append(fd)
    parent = descriptors[-1]
    if os.fstat(parent).st_uid != uid:
        raise Rejected("请选择本人所有的现有目的目录")
    return parent, identities


class _OpenHow(ctypes.Structure):
    _fields_ = [(field, ctypes.c_uint64) for field in ("flags", "mode", "resolve")]


class _ProcView:
    """Use genuine kernel procfs files, never the ambient proc spelling for evidence."""
    def __init__(self, stack):
        # The Linux native LP64 x86-64/AArch64 UAPI is used for both syscalls.
        uname = os.uname()
        fstatfs = {"x86_64": 138, "aarch64": 44}.get(uname.machine)
        if (uname.sysname != "Linux" or not fstatfs or ctypes.sizeof(ctypes.c_long) != 8 or
                ctypes.sizeof(ctypes.c_void_p) != 8):
            raise Rejected("另存单个文件当前需要 Linux x86-64 或 AArch64 原生 64 位接口")
        self.syscall = ctypes.CDLL(None, use_errno=True).syscall
        self.syscall.argtypes = (ctypes.c_long,)
        self.syscall.restype = ctypes.c_long
        flags = os.O_PATH | os.O_DIRECTORY | os.O_CLOEXEC
        root = os.open("/proc", flags | os.O_NOFOLLOW)
        stack.callback(os.close, root)
        # asm-generic/statfs.h: 120-byte struct, first native word is f_type.
        fields = (ctypes.c_long * 15)()
        if self.syscall(fstatfs, ctypes.c_int(root), ctypes.byref(fields)) != 0 or fields[0] != 0x9fa0:
            raise Rejected("不能从原目录句柄核实真正的 Linux procfs")
        self.pid = os.getpid()
        if os.readlink("self", dir_fd=root) != str(self.pid):
            raise Rejected("procfs 不对应本进程的 PID")
        self.task = self._open(root, str(self.pid), flags)
        stack.callback(os.close, self.task)
        self.fds = self._open(self.task, "fd", flags)
        stack.callback(os.close, self.fds)
        _, base, path, kind = mount_record(root, read_proc=self.read)
        if (base, path, kind) != (b"/", b"/proc", b"proc"):
            raise Rejected("不是原 procfs 根挂载")

    def _open(self, parent, name, flags):
        # Linux openat2: NO_XDEV | NO_SYMLINKS | BENEATH. This excludes a
        # separately mounted lookalike even before opening/reading its files.
        how = _OpenHow(flags, 0, 0x0d)
        fd = self.syscall(437, ctypes.c_int(parent), ctypes.c_char_p(os.fsencode(name)),
                          ctypes.byref(how), ctypes.c_size_t(ctypes.sizeof(how)))
        if fd < 0:
            code = ctypes.get_errno()
            raise Rejected(f"无法在原 procfs 挂载内取得必要句柄（openat2: {os.strerror(code)}）")
        return fd

    def read(self, name, limit):
        if os.getpid() != self.pid: raise Rejected("原进程的 procfs 证据不能移交")
        if name == "status": name = f"task/{get_native_id()}/status"
        fd = self._open(self.task, name, os.O_RDONLY | os.O_NONBLOCK | os.O_CLOEXEC)
        try:
            native_identity(os.fstat(fd), stat.S_IFREG)
            with os.fdopen(fd, "rb", closefd=False) as actual:
                return actual.read(limit + 1)
        finally: os.close(fd)


def _generation(m):
    return (*native_identity(m, stat.S_IFREG), m.st_size, m.st_mtime_ns, m.st_ctime_ns)


class _Positional:
    """Never change the offset shared by the displayed FD and the FUSE helper."""
    def __init__(self, fd, end):
        self.fd, self.end, self.pos = fd, end, 0

    def tell(self): return self.pos

    def seek(self, n, whence=os.SEEK_SET):
        origin = {os.SEEK_SET: 0, os.SEEK_CUR: self.pos, os.SEEK_END: self.end}[whence]
        if origin+n < 0: raise Rejected("negative ZIP offset")
        self.pos = origin+n
        return self.pos

    def read(self, n):
        if not 0 <= n <= MAX_CD or self.pos+n > self.end:
            raise Rejected("ZIP read exceeds its selected byte window")
        data = os.pread(self.fd, n, self.pos)
        self.pos += len(data)
        return data


class MemberCopy:
    """A one-use choice: prepare only observes; run writes only with explicit consent."""
    def __init__(self, selected, directory, entry, spelling):
        if sys.platform != "linux" or selected.held is None:
            raise Rejected("另存单个成员需要 Linux 上当前仍持有原件的快照")
        if (entry.kind != "file" or selected.browser.entry(directory, entry.name) is not entry or
                type(entry.size_bytes) is not int or not 0 <= entry.size_bytes <= MAX_FILE):
            raise Rejected("只能另存本次已核验的一条普通成员")
        self.path = Path(spelling)
        if (self.path.anchor != "/" or str(self.path) != spelling or ".." in self.path.parts or
                not 3 <= len(self.path.parts) <= 66 or self.path.parts[1] in ("dev", "proc", "sys") or
                len(member_name(self.path.name)) != 1):
            raise Rejected("请选择现有目录下的一个明确新文件名")
        self.selected, self.entry = selected, entry
        self.member = f"{directory}/{entry.name}" if directory else entry.name
        self.stack = ExitStack()
        self.ready = self.used = self.closed = False
        self.source = selected.held.duplicate()
        self.stack.callback(os.close, self.source)

    def close(self):
        if not self.closed:
            self.closed = True
            self.stack.close()

    def _source(self):
        if (_generation(os.fstat(self.source)) != self.selected.observed or
                _generation(self.selected.path.stat(follow_symlinks=False)) != self.selected.observed):
            raise Rejected("选中的原归档已被移走、替换或修改；不会打开同名文件或改选分支头")

    def _parent(self):
        with ExitStack() as check:
            _, ids = _directory(check, self.path.parent, self.uid, self.proc.read)
        if ids != self.parents or local_mount(self.parent, read_proc=self.proc.read) != self.parents[-1][-1]:
            raise Rejected("选中的目的目录或上级目录不再是原件")

    def _absent(self):
        try: os.stat(self.path.name, dir_fd=self.parent, follow_symlinks=False)
        except FileNotFoundError: return
        raise Rejected("目的名称已有文件、目录或链接；不覆盖")

    def review(self):
        if self.closed or self.ready or self.used: raise Rejected("请重新选择一次成员和目的文件")
        try:
            if (not getattr(os, "O_TMPFILE", 0) or not hasattr(os, "pread") or
                    os.link not in os.supports_dir_fd or os.link not in os.supports_follow_symlinks):
                raise Rejected("当前 Python/Linux 缺少匿名文件、偏移读取或原生无覆盖链接能力")
            self.proc = _ProcView(self.stack)
            self.uid = _ordinary_user(read_proc=self.proc.read)
            try: self.parent, self.parents = _directory(self.stack, self.path.parent, self.uid, self.proc.read)
            except OSError as err: raise Rejected("无法持有无符号链接的原目的目录") from err
            try: mount = local_mount(self.parent, read_proc=self.proc.read)
            except Rejected as err: raise Rejected("目的目录须位于已核验的本机 ext4/XFS 挂载") from err
            if mount != self.parents[-1][-1]:
                raise Rejected("目的目录在选择期间改变了挂载")
            self._source()
            self._absent()
            self._parent()
            self.ready = True
            return (f"原归档：{str(self.selected.path)!r}\n节点：{self.selected.browser.commit.operation}\n"
                    f"成员：{self.member!r}\n正文：{self.entry.size_bytes:,} 字节\nSHA-256：{self.entry.sha256}\n"
                    f"新文件：{str(self.path)!r}\n\n仅保存这一项的字节，需容纳一份正文。"
                    "核完这一成员并读回新文件后才公开名称；不覆盖、不创建工作树、不打开结果。"
                    "提交这次确认后须等待结果，不能从窗口中途取消。"
                    "不会复制权限、时间或来源元数据；内容校验不代表来源可信。")
        except BaseException:
            self.close()
            raise

    def _output(self, fd, links, size, name=None, *, follow=False):
        m = os.fstat(fd) if name is None else os.stat(name[1], dir_fd=name[0], follow_symlinks=follow)
        generation = _generation(m)
        if (generation[0] != self.parents[-1][0] or m.st_uid != self.uid or stat.S_IMODE(m.st_mode) != 0o600 or
                m.st_nlink != links or m.st_size != size or (name is None and mount_id(fd, read_proc=self.proc.read) != self.parents[-1][-1])):
            raise Rejected("未核到原目的卷上本人所有、0600 的普通新文件")
        return generation

    def _copy(self, body, fd):
        size, sha = self.entry.size_bytes, self.entry.sha256
        digest = hashlib.sha256()
        left = size
        while left:
            data = body.read(min(left, CHUNK))
            if not data or len(data) > left: raise Rejected("成员没有达到原核验长度")
            digest.update(data)
            left -= len(data)
            view = memoryview(data)
            while view:
                written = os.write(fd, view)
                if not 0 < written <= len(view): raise Rejected("未能写完匿名文件")
                view = view[written:]
        # ZipExtFile checks CRC upon EOF, including empty members.
        if body.read(1) or digest.hexdigest() != sha: raise Rejected("成员与原核验的长度或 SHA-256 不符")

    def _verify(self, fd):
        size = self.entry.size_bytes
        before = self._output(fd, 0, size)
        os.fsync(fd)
        digest, read = hashlib.sha256(), 0
        while read < size:
            data = os.pread(fd, min(CHUNK, size-read), read)
            if not data: raise Rejected("匿名副本读回提前结束")
            read += len(data)
            digest.update(data)
        if os.pread(fd, 1, size) or digest.hexdigest() != self.entry.sha256 or self._output(fd, 0, size) != before:
            raise Rejected("匿名副本读回不等于原成员")
        return before

    def run(self, *, confirmed=False):
        if confirmed is not True or not self.ready or self.closed or self.used:
            raise Rejected("只允许明确确认一次已核验的成员和目的文件")
        self.used = True
        may_be_linked = False
        try:
            with self.stack, ExitStack() as temp:
                if _ordinary_user(read_proc=self.proc.read) != self.uid:
                    raise Rejected("不再是确认这次另存的普通用户")
                self._source()
                self._parent()
                self._absent()
                reader = _Positional(self.source, self.selected.observed[2])
                size = self.entry.size_bytes
                with stored_member(reader, reader.end, self.selected.browser.commit, self.member, size) as body:
                    # O_EXCL would forbid linking this O_TMPFILE; no named fallback.
                    out = os.open(".", os.O_TMPFILE | os.O_RDWR | os.O_CLOEXEC, 0o600, dir_fd=self.parent)
                    temp.callback(os.close, out)
                    self._output(out, 0, 0)
                    self._copy(body, out)
                sealed = self._verify(out)
                self._source()
                self._parent()
                name = str(out)
                if (not stat.S_ISLNK(os.stat(name, dir_fd=self.proc.fds, follow_symlinks=False).st_mode) or
                        self._output(out, 0, size) != sealed or
                        self._output(out, 0, size, (self.proc.fds, name), follow=True) != sealed):
                    raise Rejected("procfs 中没有核验通过的原匿名文件")
                may_be_linked = True
                try:
                    os.link(name, self.path.name, src_dir_fd=self.proc.fds, dst_dir_fd=self.parent, follow_symlinks=True)
                except FileExistsError as err:
                    may_be_linked = False
                    raise Rejected("目的名称已被占用；没有覆盖") from err
                published = self._output(out, 1, size)
                if published[:4] != sealed[:4]:  # link changes ctime, not identity/size/mtime
                    raise Rejected("新名称与已核验的文件不符")

                def recheck():
                    self._parent()
                    if (self._output(out, 1, size, (self.parent, self.path.name)) != published or
                            self._output(out, 1, size) != published):
                        raise Rejected("正式名称或原成员副本在确认时改变")

                recheck()
                os.fsync(self.parent)
                recheck()
            return f"已另存并核验：{str(self.path)!r}（{size:,} 字节）"
        except Exception as err:
            if may_be_linked:
                raise Uncertain(f"无法确认原目的文件是否完整登记：{str(self.path)!r}。完整副本可能已经出现；"
                                f"请先检查原位置，不自动重试或删除。\n{err}") from err
            raise
        finally:
            self.closed = True
