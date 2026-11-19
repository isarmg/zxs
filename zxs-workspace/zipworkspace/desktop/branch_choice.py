"""Fork the displayed, verified checkout node; do not read the current working files."""
import os
import stat

from .. import worktree
from ..archive import stamp
from ..document import named_document, native_identity
from ..model import Commit, Rejected, new_id
from ..publish import Conflict
from .commit_choice import CommitChoice


class SnapshotBranch(CommitChoice):
    purpose = "branch"

    def __init__(self, live):
        self.branch_id = new_id()  # Fixed before review; a collision never selects a different branch.
        super().__init__(live)

    def _prepare(self):
        old = self.live.checkout_marker
        self._prepare_base(required=(old.repo, old.branch, old.generation, old.operation, self.live.checkout_sha))
        if self.base_marker != old or self.base[-1] != self.live.checkout_sha:
            raise Rejected("本工作树的当前基线不再是窗口所选原节点；不能从另一个节点建分支")
        if self.branch_id == old.branch: raise Rejected("新分支号与所选原分支冲突")
        self.proposed = Commit(old.repo, self.branch_id, 1, self.operation, old.operation, old.tree)
        if str(self.live.source_path) == str(self.live.store/self.name):
            self.source = self.original  # Reuse the already verified exact canonical file.
        else:
            self.source = self.stack.enter_context(named_document(self.live.source_path))
        self._selected_source()
        self.check_empty_slot()
        if self._idle_base() != self.base: raise Rejected("读取期间原基线变化；未制作候选")
        return (f"所选归档：{self.live.source_path}\n仓库：{old.repo}\n原分支：{old.branch} · 第 {old.generation} 代\n"
                f"父节点：{old.operation}\n原归档 SHA-256：{self.live.checkout_sha}\n"
                f"新分支：{self.branch_id} · 第 1 代\n新节点 / 本次操作号：{self.operation}\n"
                f"新正式槽：{self.live.store/self.proposed.filename}\n"
                f"本工作树：{self.live.work}\n原 STATE：{self.live.state}\n\n"
                "本操作复制所选原节点的完整快照，不读取工作目录里的编辑。确认才创建私有候选并向上述 STORE 发布；"
                "不更新原分支，不覆盖任何已有槽。仅在原操作完成后本工作树切到新分支，磁盘上的编辑仍保留。")

    def _selected_source(self):
        s = self.live.source_stamp
        m = self.live.source_path.stat(follow_symlinks=False)
        if (stamp(self.source) != s or native_identity(m, stat.S_IFREG) != s[:2]
                or (m.st_size, m.st_mtime_ns, m.st_ctime_ns) != s[2:]):
            raise Rejected("窗口原先所选归档已被替换或改变，不能用同名文件另建分支")

    def check_base(self):
        super().check_base()  # The canonical original remains in the retained STORE.
        self._selected_source()  # The displayed name remains bound to the held input used for the copy.

    def check_empty_slot(self):
        self.store.check_store()
        try: os.stat(self.proposed.filename, dir_fd=self.store.fd, follow_symlinks=False)
        except FileNotFoundError:
            self.store.check_store()
            return  # Only an observation; the actual publish is no-replace.
        raise Conflict("新分支第 1 代名称已被占用；不会覆盖、改号或自动重试")

    def run(self):
        if self.closed or not self.ready or self.used: raise Rejected("本次原节点分支决定不可重用")
        self.used = True
        try:
            return worktree.branch_from_snapshot(self.live.state, selected=self)
        except Conflict:
            self.conflict = True
            raise
        finally:
            self.close()
