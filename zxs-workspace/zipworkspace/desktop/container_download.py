"""Export one selected immutable R3 node, not a checkout or an archive."""
import os
import sys

from .._transfer import CancelledCopy
from ..model import Rejected
from .container_connection import OpenConnection
from .container_management import Branch, Detail, _node
from .member_copy import _AnonymousCopy


class ContainerNodeCopy(_AnonymousCopy):
    """Use the existing held Linux publisher without selecting a ZIP source."""

    def __init__(self, connection, detail, branch, node, spelling):
        if (sys.platform != "linux" or type(connection) is not OpenConnection or not connection._listed
                or type(detail) is not Detail or type(branch) is not Branch or branch not in detail.branches
                or not _node(node) or node not in branch.nodes):
            raise Rejected("请选择当前连接中已核验的原节点。")
        self._connection, self._exchange = connection, connection._exchange
        self._destination = self._exchange.destination_identity
        self._container, self._branch, self._node = detail.container, branch.id, node
        self._set_output(spelling, node.length)

    def _source(self):
        if (self._connection._exchange is not self._exchange
                or self._exchange.destination_identity != self._destination):
            raise Rejected("认证的服务器连接已改变。")
        try:
            self._connection._authorization.require_live()
        except Rejected as error:
            raise Rejected("原服务器连接已断开。") from error

    def _origin_label(self):
        return f"服务器：{self._exchange._location}\n容器：{self._container.name!r} · {self._container.id}\n"

    def _source_details(self):
        return f"分支：{self._branch} · 原节点：{self._node.id}\n"

    def _selection_details(self):
        return f"正文：{self.size:,} 字节\nSHA-256：{self._node.sha256}\n"

    def _before_link(self):
        self._source()

    def _payload(self, create):
        fd = create()
        self._phase("正在下载所选原节点到匿名文件", self.size)
        with os.fdopen(fd, "wb", closefd=False) as sink:
            def write(block):
                self._checkpoint()
                self._source()
                count = sink.write(block)
                self._bytes_processed(count)
                return count

            try:
                self._exchange.read_node_into(self._container.id, self._node.id,
                                              length=self.size, sha256=self._node.sha256, write=write)
            except CancelledCopy:
                raise
            except Rejected as error:
                raise Rejected("未能完整核对所选节点；没有发布文件。") from error
            self._checkpoint()
            self._source()
            sink.flush()
        # The transport already checked extent and SHA; the shared publisher
        # syncs and reads back these same destination bytes before linking.
        return fd, self._node.sha256
