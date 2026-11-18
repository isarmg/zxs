"""Preview an ordinary Store ZIP. No destination is currently admitted for publication."""
from dataclasses import dataclass
import os
from pathlib import Path
import stat

from .archive import read_plain_document
from .document import archive_sha, named_document, native_identity
from .model import Rejected, member_name


@dataclass(frozen=True, slots=True)
class Proposal:
    source: Path
    source_id: tuple[int, int]
    sha256: str
    tree: str
    count: int
    size: int
    target: Path
    parent_id: tuple[int, int]


def inspect(source, target):
    source, target = Path(source).absolute(), Path(target).absolute()
    if source.suffix.lower() != ".zip": raise Rejected("普通导入只接受 .zip 文件。")
    if len(member_name(target.name)) != 1: raise Rejected("请指定一个新的普通目录名。")
    parent = native_identity(target.parent.stat(follow_symlinks=False), stat.S_IFDIR)
    if os.path.lexists(target): raise Rejected("目标名称已存在；请选择另一个新目录。")
    with named_document(source) as doc:
        original = native_identity(os.fstat(doc.fileno()),stat.S_IFREG)
        tree = read_plain_document(doc)
        sha = archive_sha(doc)
    if native_identity(target.parent.stat(follow_symlinks=False),stat.S_IFDIR) != parent or os.path.lexists(target):
        raise Rejected("目标父目录或名称已改变，请重新选择。")
    return Proposal(source,original,sha,tree.digest(),len(tree.rows),tree.total,target,parent)


class DestinationUnavailable(Rejected):
    """No protected same-filesystem staging authority is currently implemented."""


OUTPUT_UNAVAILABLE = ("当前仅支持只读预览。尚不能证明暂存目录由本应用独占且与目标位于同一文件系统；"
                      "不会提取、发布或清理任何目录。")


def require_writer():
    # A mode-0700 directory or a native no-replace syscall is not authority over
    # the *source* entry between mkdir, the first open and rename.
    raise DestinationUnavailable(OUTPUT_UNAVAILABLE)


def execute(proposal):
    """Even a previously verified preview does not grant any write authority."""
    require_writer()
