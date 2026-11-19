"""Operate on exact private ZIPs beneath the selected, held CLI STATE directory."""
from contextlib import contextmanager
import hashlib
import os
from pathlib import Path
import stat
import zipfile

from .archive import read_managed_document, stamp
from .capacity import buffered_output, close_output_fd, note, writing
from .document import archive_sha, native_identity
from .model import CHUNK, Rejected, check_id


def generation(meta):
    return meta.st_dev, meta.st_ino, meta.st_size, meta.st_mtime_ns, meta.st_ctime_ns


class CandidateDirectory:
    def __init__(self, path, parent, identity):
        self.path, self.parent, self.identity = path, parent, identity

    def check_state(self):
        if (native_identity(os.fstat(self.parent), stat.S_IFDIR) != self.identity or
                native_identity(self.path.stat(follow_symlinks=False), stat.S_IFDIR) != self.identity):
            raise Rejected("private STATE no longer names the original directory")

    def check_file(self, name, fd, expected=None):
        self.check_state()
        held = os.fstat(fd)
        observed = generation(held) if expected is None else expected
        named = os.stat(name, dir_fd=self.parent, follow_symlinks=False)
        for m in (held, named):
            native_identity(m, stat.S_IFREG)
            if m.st_nlink != 1 or generation(m) != observed:
                raise Rejected("different private ZIP, link or generation at the candidate name")
        return observed

    @contextmanager
    def candidate(self, operation, *, editable=False, mixed_source=False):
        self.check_state()
        name = check_id(operation) + ".zxs"
        access = os.O_RDWR if editable else os.O_WRONLY
        with writing("STATE",self.path/name,"独占建立私有命名候选，未据此受理",operation,self.parent):
            fd = os.open(name, os.O_CREAT | os.O_EXCL | access | os.O_NOFOLLOW | os.O_CLOEXEC,
                         0o600, dir_fd=self.parent)
        try:
            with buffered_output(os.fdopen(fd, "w+b" if editable else "wb", closefd=False),
                                 "STATE", self.path/name, operation, self.parent) as stream:
                self.check_file(name, fd)
                yield name, fd, stream
        except Exception as error:
            # The shared copy helper can read a different volume. It annotates its actual output writes.
            if not mixed_source:
                note(error,"STATE",self.path/name,"制作或同步原私有命名候选；它可能保留且尚未登记",operation,self.parent)
            error.add_note(f"原私有候选可能残留于 {self.path / name}，也可能尚未登记。保留现场，不自动删除或重试。")
            raise
        finally:
            close_output_fd(fd, "STATE", self.path/name, operation, self.parent)

    def verify_candidate(self, name, original, marker):
        observed = self.check_file(name, original)
        fd = os.open(name, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC,
                     dir_fd=self.parent)
        try:
            with os.fdopen(fd, "rb", closefd=False) as doc:
                def check_original():
                    self.check_file(name, original, observed)
                    if os.fstat(fd).st_nlink != 1 or stamp(doc) != observed:
                        raise Rejected("different private ZIP, link or generation at the candidate name")
                check_original()
                actual, _ = read_managed_document(doc, display_name=name)
                raw = archive_sha(doc)
                if actual != marker: raise Rejected("original candidate marker mismatch")
                check_original()
                os.fsync(self.parent)
                check_original()
                return raw
        finally:
            os.close(fd)

    def fork(self, old, expected_sha, new):
        """The receipt identifies the original bytes. Pin the actual selected input from here."""
        name = check_id(old.operation) + ".zxs"
        self.check_state()
        fd = os.open(name, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC,
                     dir_fd=self.parent)
        try:
            with os.fdopen(fd, "rb", closefd=False) as src:
                before = self.check_file(name, fd)
                if archive_sha(src) != expected_sha or read_managed_document(src, display_name=name)[0] != old:
                    raise Rejected("original frozen candidate is unavailable")
                self.check_file(name, fd, before)
                return self.copy_with_marker(src, expected_sha, new, lambda: self.check_file(name, fd, before))
        finally:
            os.close(fd)

    def copy_with_marker(self, src, expected_sha, new, check_source):
        """Copy exact authenticated bytes and replace only the logical commit, on the exclusive output fd."""
        check_source()
        length = stamp(src)[2]
        src.seek(0)
        with self.candidate(new.operation, editable=True, mixed_source=True) as (name, fd, dst):
            left, digest = length, hashlib.sha256()
            while left:
                block = src.read(min(left, CHUNK))
                if not block: raise Rejected("incomplete copy of the original frozen candidate")
                try: count = dst.write(block)
                except OSError as error:
                    note(error, "STATE", self.path/name, "复制已核原包为私有分支候选", new.operation, self.parent)
                    raise
                if count != len(block): raise Rejected("incomplete copy of the original frozen candidate")
                digest.update(block); left -= len(block)
            if src.read(1) or digest.hexdigest() != expected_sha:
                raise Rejected("original candidate changed while copying")
            check_source()
            with writing("STATE", self.path/name, "写入并完整核验新分支候选", new.operation, self.parent):
                dst.flush()
                self.check_file(name, fd)
                with zipfile.ZipFile(dst, "a", allowZip64=True) as z: z.comment = new.encode()
                dst.flush(); os.fsync(fd)
                dst.close()  # The actual fd is retained through complete marker/tree/byte verification.
                raw = self.verify_candidate(name, fd, new)
            check_source()
            return raw


@contextmanager
def held_candidates(state):
    if (os.name != "posix" or not all(hasattr(os,n) for n in ("O_DIRECTORY","O_NOFOLLOW","O_NONBLOCK","O_CLOEXEC"))
            or os.open not in os.supports_dir_fd or os.stat not in os.supports_dir_fd):
        raise Rejected("cannot prove private candidate names on this platform")
    path = Path(state).absolute()
    parent = os.open(path, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC)
    try:
        m = os.fstat(parent)
        if m.st_uid != os.geteuid() or m.st_mode & 0o077:
            raise Rejected("use the selected user's private STATE directory")
        area = CandidateDirectory(path, parent, native_identity(m, stat.S_IFDIR))
        area.check_state()
        yield area
    finally:
        os.close(parent)
