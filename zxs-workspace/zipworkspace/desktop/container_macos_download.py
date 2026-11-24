"""Native consent plan for one immutable remote node. Independent of FSKit."""
from pathlib import Path
import sys

from .._transfer import _BeforePublication
from ..container_https import HttpsExchange
from ..macos_node_export import MacNodeOutput
from ..model import Rejected
from .container_connection import OpenConnection
from .container_management import Branch, Detail, _node


# Maintainer release gate, not a preference/environment variable. A headless
# APFS test that substitutes the human click does not qualify the packaged GUI.
SAVE_AS_RELEASED = False


class MacNodeSaveAs(_BeforePublication):
    _cancel_note = "已取消另存，未发布正式文件；私有暂存可能保留。"

    def __init__(self, owner, detail, branch, node, spelling):
        if (sys.platform != "darwin" or type(owner) is not OpenConnection or not owner._listed
                or type(detail) is not Detail or type(branch) is not Branch or branch not in detail.branches
                or not _node(node) or node not in branch.nodes or type(owner._exchange) is not HttpsExchange
                or owner._exchange._authorization is not owner._authorization):
            raise Rejected("请选择当前原认证连接中已核验的节点。")
        if type(spelling) is not str: raise Rejected("请选择原目的目录和一个全新文件名。")
        path = Path(spelling)
        if str(path) != spelling or not path.is_absolute():
            raise Rejected("请选择原目的目录和一个全新文件名。")
        self._path, self._exchange, self._container, self._node = path, owner._exchange, detail.container.id, node
        self._authority = owner._authorization
        self._origin, self._identity = self._exchange._location, self._exchange.destination_identity
        self._output, self._reviewed, self.closed = None, False, False
        self._init_cancellation()

    def _source(self):
        self._checkpoint()
        if (self._exchange._authorization is not self._authority or self._exchange._location != self._origin
                or self._exchange.destination_identity != self._identity):
            raise Rejected("原服务器连接已改变。")
        self._authority.require_live()

    def review(self):
        """Preflight only; no file, request or consent until the native GUI decision."""
        with self._publication_lock:
            if self.closed or self._reviewed:
                raise Rejected("请重新选择要另存的原节点。")
            self._reviewed = True
        self._source()
        output = MacNodeOutput(str(self._path.parent), self._path.name)
        try:
            self._source()
            with self._publication_lock:
                self._checkpoint()
                if self.closed: raise Rejected("原另存计划已关闭。")
                self._output = output
        except BaseException:
            output.close()
            raise

    def approve(self):
        self._source()
        with self._publication_lock:
            output = self._output
            if self.closed or output is None: raise Rejected("还没有核对原目的目录。")
        node = self._node
        permit = output.approve_save_as(self._exchange, self._container, node.id, length=node.length, sha256=node.sha256)
        self._source()
        return permit

    def run(self, permit):
        with self._publication_lock:
            if self.closed or self._output is None: raise Rejected("原另存计划已结束。")
            output, self._output = self._output, None
        try:
            with output:
                self._source()
                result = output.publish_save_as(permit, checkpoint=self._source,
                        begin_publication=self._begin_publication, progress=self._bytes_processed, phase=self._phase)
                provenance = ("系统已返回并核对本次下载地址。" if result.system_reported_url
                              else "系统保留下载隔离标记，但没有返回来源地址。")
                return f"已另存并读回核验：{str(result.path)!r}（{self._node.length:,} 字节）。{provenance}"
        finally:
            self.close()

    def close(self):
        with self._publication_lock:
            if self.closed: return
            self.closed = True
            if not self.publishing: self.cancelled.set()
            output, self._output = self._output, None
        if output is not None: output.close()
