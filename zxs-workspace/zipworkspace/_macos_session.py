"""One explicitly requested, *unmounted* macFUSE session on known synthetic data. No host mount API."""
import hashlib
import os
from pathlib import Path
import plistlib
import stat
import sys
from types import SimpleNamespace

from . import macos_read
from ._macos_copy_evidence import apfs, native
from .comparison import Folder
from .macos_snapshot import selected_callbacks
from .model import Commit, Manifest, Rejected

NAME, BODY = "probe.txt", b"Workspace snapshot\n"
BUNDLE = "/Library/Filesystems/macfuse.fs/Contents/Info.plist"


def synthetic_marker():
    tree = Manifest()
    tree.add(NAME, False, len(BODY), hashlib.sha256(BODY).hexdigest())
    return Commit("1"*32, "2"*32, 1, "3"*32, None, tree.digest())


def _installed_release():
    # Version is an eligibility check, not a substitute for an independently trusted install.
    fd = os.open(BUNDLE, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC)
    try:
        before = os.fstat(fd)
        if not stat.S_ISREG(before.st_mode) or not 0 < before.st_size <= 65536:
            raise Rejected("无法确认已装 macFUSE 的发布版本")
        with os.fdopen(fd, "rb", closefd=False) as f: data = f.read(before.st_size + 1)
        after = os.fstat(fd)
        if (len(data) != before.st_size or after.st_size != before.st_size or
                (after.st_mtime_ns, after.st_ctime_ns) != (before.st_mtime_ns, before.st_ctime_ns) or
                plistlib.loads(data).get("CFBundleShortVersionString") != "5.5.0"):
            raise Rejected("此资格步骤仅针对已由操作者核实的 macFUSE 5.5.0")
    finally:
        os.close(fd)


def _empty_point(point):
    fd = point.check()
    info = os.fstat(fd)
    if info.st_uid != os.getuid() or stat.S_IMODE(info.st_mode) != 0o700 or info.st_flags:
        raise Rejected("需要本人已有的 0700 私有空目录；不创建或修改权限")
    native().no_acl(fd)
    volume = apfs(fd)
    parent = os.open("..", os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=fd)
    try:
        if (apfs(parent) != volume or os.path.samestat(info, os.fstat(parent)) or
                not os.path.samestat(info, os.stat(point.evidence.path.name, dir_fd=parent, follow_symlinks=False))):
            raise Rejected("不能把卷根或换名后的目录作为所选空目录")
    finally:
        os.close(parent)
    with os.scandir(fd) as rows:
        if next(rows, None) is not None: raise Rejected("所选目录不是空目录；不隐藏或删除内容")
    return volume


class _Dispatch:
    """Translate four vendor high-level callbacks to the *same* retained snapshot data plane."""
    def __init__(self, ops, checkpoint):
        self.ops, self.checkpoint = ops, checkpoint

    def attr(self, path):
        row = self.ops.getattr(path)
        return tuple(row[key] for key in ("st_ino", "st_size", "st_mode", "st_nlink", "st_uid", "st_gid"))

    def open(self, path, flags):
        fi = SimpleNamespace(flags=flags)
        self.ops.open(path, fi)
        if not fi.direct_io or fi.keep_cache: raise Rejected("数据面未拒绝缓存")
        return fi.fh

    def read(self, path, count, offset, ino):
        return self.ops.read(path, count, offset, SimpleNamespace(fh=ino))

    def release(self, path, ino):
        return self.ops.release(path, SimpleNamespace(fh=ino))


def observe(selected, point_path, stop, *, consent=False):
    """One synchronous owner; stop is a caller-owned threading.Event. Never a mount receipt."""
    if consent is not True or sys.platform != "darwin":
        raise Rejected("仅限明确同意在合成数据上核对已装 SDK 的无卷会话")
    if not isinstance(point_path, str) or str(Path(point_path)) != point_path or not Path(point_path).is_absolute() or ".." in Path(point_path).parts:
        raise Rejected("请选择明确的原空目录，不接受路径别名拼接")

    def checkpoint():
        if stop.is_set(): raise InterruptedError("已取消无卷会话；没有挂载")

    checkpoint()
    if selected.browser.commit != synthetic_marker():
        raise Rejected("此步骤仅接固定的单成员合成快照，不接私人归档")
    with macos_read.no_materialization():
        point = Folder.choose(point_path)
        try:
            original_volume = _empty_point(point)
            with selected_callbacks(selected, checkpoint=checkpoint) as ops:
                def recheck():
                    checkpoint()
                    if _empty_point(point) != original_volume: raise Rejected("原空目录的卷已改变")
                    ops.view.check()
                recheck()
                _installed_release()
                # Trusted, operator-built qualification extension, not in the project's build.
                # Neither the mountpoint's spelling nor any device descriptor enters native code.
                from . import _qualification_macos_session
                hooks = _Dispatch(ops, recheck)
                if _qualification_macos_session.observe(hooks) is not True:
                    raise Rejected("没有得到此 SDK 实例完成读取并释放原通道的结果")
                recheck()
                return "本次无卷 SDK 会话已从原快照读取 probe.txt 并释放；未挂载。"
        finally:
            point.close()
