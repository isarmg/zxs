"""Preview an ordinary Store ZIP. No destination is currently admitted for publication."""
from dataclasses import dataclass
from pathlib import Path
import stat
from typing import Never

from .archive import read_plain_document
from .document import archive_sha, named_document, native_identity
from .model import Rejected, member_name


@dataclass(frozen=True, slots=True)
class Preview:
    source: Path
    sha256: str
    tree: str
    count: int
    size: int


def inspect(source):
    source = Path(source).absolute()
    if source.suffix.lower() != ".zip": raise Rejected("普通 ZIP 核验只接受 .zip 文件。")
    with named_document(source) as doc:
        tree = read_plain_document(doc)
        sha = archive_sha(doc)
    return Preview(source, sha, tree.digest(), len(tree.rows), tree.total)


def observe_target(target):
    """An optional CLI name check, not an output reservation or capability."""
    target = Path(target).absolute()
    if len(member_name(target.name)) != 1: raise Rejected("请指定一个普通目录名。")
    parent = native_identity(target.parent.lstat(), stat.S_IFDIR)
    try:
        target.lstat()  # A dangling symlink is occupied too. Other I/O failures are not absence.
    except FileNotFoundError:
        occupied = False
    else:
        occupied = True
    if native_identity(target.parent.lstat(), stat.S_IFDIR) != parent:
        raise Rejected("查看名称时父目录发生变化。")
    return {"path": str(target), "occupied": occupied, "reserved": False}


class DestinationUnavailable(Rejected):
    """No protected same-filesystem staging authority is currently implemented."""


OUTPUT_UNAVAILABLE = ("当前仅支持只读预览。尚不能证明暂存目录由本应用独占且与目标位于同一文件系统；"
                      "不会提取、发布或清理任何目录。")


def require_writer() -> Never:
    # A mode-0700 directory or a native no-replace syscall is not authority over
    # the *source* entry between mkdir, the first open and rename.
    raise DestinationUnavailable(OUTPUT_UNAVAILABLE)


def execute(proposal):
    """Even a previously verified preview does not grant any write authority."""
    require_writer()
