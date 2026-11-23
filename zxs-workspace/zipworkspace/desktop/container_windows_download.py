"""Dormant desktop plan for one selected immutable Windows remote node."""
from contextlib import ExitStack
import sys

from .._transfer import _BeforePublication
from ..container_https import HttpsExchange
from ..model import Rejected
from .container_connection import OpenConnection
from .container_management import Branch, Detail, _node


class WindowsContainerNodeCopy(_BeforePublication):
    """Review holds both selected directories; write only after confirmation.

    The future GUI owner must check that the original snapshot is still selected
    before queuing this plan. No GUI state is accessed from its worker.
    """

    _cancel_note = "已取消另存，未发布正式文件；私用暂存可能保留。"

    def __init__(self, connection, detail, branch, node, destination_parent, output_name, *, staging_parent):
        if (sys.platform != "win32" or type(connection) is not OpenConnection or not connection._listed
                or type(detail) is not Detail or type(branch) is not Branch or branch not in detail.branches
                or not _node(node) or node not in branch.nodes or node.length >= 1 << 63):
            raise Rejected("请选择当前连接中已核验的原节点。")
        self._exchange = connection._exchange
        self._authorization = connection._authorization
        if type(self._exchange) is not HttpsExchange or self._exchange._authorization is not self._authorization:
            raise Rejected("请选择本窗口认证的原连接。")
        self._identity, self._origin = self._exchange.destination_identity, self._exchange._location
        self._container, self._branch, self._node = detail.container, branch.id, node
        self._destination, self._staging, self._name = destination_parent, staging_parent, output_name
        self._native = None
        self.closed = self._reviewed = self._used = False
        self._init_cancellation()

    def _original_source(self):
        self._checkpoint()
        if (self._exchange._location != self._origin or self._exchange.destination_identity != self._identity
                or self._exchange._authorization is not self._authorization):
            raise Rejected("原服务器连接已改变。")
        self._authorization.require_live()

    def review(self):
        from .._windows_node import WindowsNodeOutput

        with self._publication_lock:
            if self.closed or self._reviewed:
                raise Rejected("请重新选择原节点和两个本机目录。")
            self._reviewed = True
        try:
            self._original_source()
            with ExitStack() as held:
                native = held.enter_context(
                    WindowsNodeOutput(self._destination, self._name, staging_parent=self._staging)
                )
                self._original_source()
                final = native._destination.name(native._output)
                partial = native._staging.name(native._stage)
                with self._publication_lock:
                    if self.closed or self.cancelled.is_set():
                        raise Rejected("原另存计划已关闭。")
                    self._native = held.pop_all()
                    self._output = native
            return (f"服务器：{self._origin}\n容器：{self._container.name!r} · {self._container.id}\n"
                    f"分支：{self._branch} · 节点：{self._node.id}\n"
                    f"正文：{self._node.length:,} 字节\nSHA-256：{self._node.sha256}\n"
                    f"私用暂存文件：{partial!r}\n新文件：{final!r}\n"
                    "通过核验才发布正式名称，不覆盖，不打开结果。")
        except BaseException:
            self.close()
            raise

    def run(self, *, confirmed=False):
        with self._publication_lock:
            if confirmed is not True or self.closed or self._used or self._native is None:
                raise Rejected("请先确认这次原节点、暂存和正式目的文件。")
            stack, self._native = self._native, None
            output, self._used = self._output, True
        try:
            with stack:
                self._original_source()
                receipt = output.publish(
                    self._exchange, self._container.id, self._node.id, length=self._node.length,
                    sha256=self._node.sha256, confirmed=True, checkpoint=self._original_source,
                    begin_publication=self._begin_publication, progress=self._bytes_processed, phase=self._phase,
                )
                return f"已另存并核验：{receipt['new_file']!r}（{receipt['bytes']:,} 字节）"
        finally:
            self.close()

    def close(self):
        with self._publication_lock:
            if self.closed:
                return
            if not self.publishing:
                self.cancelled.set()
            self.closed = True
            held, self._native = self._native, None
        if held is not None:
            held.close()
