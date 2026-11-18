"""Ordinary Store ZIP → new ordinary directory. No catalog, registration or branch."""
from contextlib import ExitStack
from dataclasses import dataclass
import os
from pathlib import Path
import stat

from .archive import read_plain_document
from .destination import HeldOutput, held_parent
from .document import archive_sha, named_document, native_identity
from .freeze import observe_held
from .model import Rejected, member_name
from .publish import rename_new_at, require_native_writer


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


def execute(proposal):
    require_native_writer()
    dest = proposal.target
    with held_parent(dest.parent,proposal.parent_id) as area, ExitStack() as staging:
        if area.occupied(dest.name): raise Rejected("目标名称已存在；没有覆盖。")
        os.fsync(area.fd)
        with named_document(proposal.source) as doc:
            if (native_identity(os.fstat(doc.fileno()),stat.S_IFREG) != proposal.source_id
                    or archive_sha(doc) != proposal.sha256):
                raise Rejected("所选普通 ZIP 已替换或更改。")
            stage, original, fd = staging.enter_context(area.stage())
            manifest = read_plain_document(doc,output=HeldOutput(fd))
        if manifest.digest() != proposal.tree or observe_held(fd,original).rows != manifest.rows:
            raise Rejected("暂存目录的实际内容与所选 ZIP 不符；未确认导入。")
        area.check()
        if area.child(stage.name) != original: raise Rejected("原暂存名称已更换。")
        rename_new_at(area.fd,stage.name,dest.name,expected_source=os.fstat(fd))
        os.fsync(area.fd)
        if (area.occupied(stage.name) or area.child(dest.name) != original
                or observe_held(fd,original).rows != manifest.rows or area.child(dest.name) != original):
            raise Rejected("未核实目的目录；保留已存在的内容，不自动重试或清理。")
        return manifest
