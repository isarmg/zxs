"""Read local-only upload rules from one stopped, already qualified Linux WORK."""
from contextlib import ExitStack, contextmanager
from dataclasses import dataclass
from fnmatch import fnmatchcase
import os
import stat

from .model import Rejected, member_name
from .private_freeze import mount_id

RULE_FILE = ".zxsignore"
_MAX_BYTES = 16 << 10
_MAX_RULES = 128
_DEFAULT = ((".DS_Store",), ("~$*",), ("*.tmp",), ("*.swp",))


@dataclass(frozen=True, slots=True)
class UploadIgnore:
    rules: tuple = ()

    def matches(self, path):
        parts = member_name(path)
        if parts == (RULE_FILE,):
            return True
        for rule in _DEFAULT + self.rules:
            if len(rule) == 1:
                if any(fnmatchcase(part, rule[0]) for part in parts):
                    return True
            elif len(rule) <= len(parts) and all(fnmatchcase(p, r) for p, r in zip(parts, rule)):
                return True
        return False


def _parse(raw):
    if len(raw) > _MAX_BYTES:
        raise Rejected(".zxsignore 超过 16 KiB。")
    try:
        lines = raw.decode("utf-8-sig").split("\n")
    except UnicodeError as error:
        raise Rejected(".zxsignore 须为 UTF-8。") from error
    rules = []
    for source in lines:
        rule = source.strip()
        if not rule or rule.startswith("#"):
            continue
        if (len(rules) == _MAX_RULES or rule.startswith("!") or "**" in rule
                or "[" in rule or "]" in rule):
            raise Rejected(".zxsignore 最多 128 条，只支持普通名称、相对路径和 *、?。")
        member_name(rule.replace("*", "x").replace("?", "x"))
        rules.append(tuple(rule.split("/")))
    return UploadIgnore(tuple(rules))


def _stamp(row):
    return (row.st_dev, row.st_ino, row.st_uid, row.st_mode, row.st_nlink,
            row.st_size, row.st_mtime_ns, row.st_ctime_ns)


@contextmanager
def held_upload_ignore(root, proc, mount, uid):
    """A literal root file only; hold and recheck its bytes before the cut ends."""
    def named():
        try:
            return os.stat(RULE_FILE, dir_fd=root, follow_symlinks=False)
        except FileNotFoundError:
            return None

    observed = named()
    if observed is None:
        yield UploadIgnore()
        if named() is not None:
            raise Rejected("准备提交时，原 WORK 新出现了 .zxsignore。")
        return
    original = _stamp(observed)
    if (not stat.S_ISREG(observed.st_mode) or not observed.st_ino or observed.st_nlink != 1
            or observed.st_uid != uid or observed.st_mode & 0o7022 or observed.st_size > _MAX_BYTES):
        raise Rejected(".zxsignore 须是本人所有且不可由其他人改写的单链接小文件。")
    with ExitStack() as held:
        pin = proc.open_beneath(root, RULE_FILE, os.O_PATH | os.O_NOFOLLOW | os.O_CLOEXEC)
        held.callback(os.close, pin)
        name = os.readlink(str(root), dir_fd=proc.fds).rstrip("/") + "/" + RULE_FILE
        if (_stamp(os.fstat(pin)) != original or mount_id(pin, read_proc=proc.read) != mount
                or os.readlink(str(pin), dir_fd=proc.fds) != name):
            raise Rejected(".zxsignore 不是原 WORK 根目录中的准确本机文件。")
        # Reopen only our regular pinned object via the already verified procfs.
        fd = os.open(str(pin), os.O_RDONLY | os.O_NONBLOCK | os.O_CLOEXEC, dir_fd=proc.fds)
        held.callback(os.close, fd)

        def require_original():
            row = named()
            if (row is None or _stamp(row) != original or _stamp(os.fstat(fd)) != original
                    or mount_id(fd, read_proc=proc.read) != mount
                    or os.readlink(str(pin), dir_fd=proc.fds) != name):
                raise Rejected("准备提交时，原 .zxsignore 已被改动或替换。")

        def read():
            require_original()
            out = bytearray()
            while len(out) <= _MAX_BYTES:
                block = os.pread(fd, _MAX_BYTES + 1 - len(out), len(out))
                if not block:
                    break
                out.extend(block)
            require_original()
            if len(out) != observed.st_size:
                raise Rejected("没有读到原 .zxsignore 的准确长度。")
            return bytes(out)

        raw = read()
        yield _parse(raw)
        if read() != raw:
            raise Rejected("准备提交时，原 .zxsignore 的内容已改变。")
