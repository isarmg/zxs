# R3 只读目录、详情与分支历史

既有桌面客户端的读取契约。写入、原回执和未来 Range/会话接口不由分页授权。[现有服务路由](../../../zxs/src/workspace_protocol/mod.rs)和[旧桌面投影](../../zipworkspace/desktop/container_management.py)共同核对。

## 容器列表

请求 `GET /zxs/v1/containers`，带 `Zxs-View: compact`。每页最多 256 项，只返回 `container_id`、`sequence` 和 `name`。用上页 `next_after` 原样作下一页的 `Zxs-After`；`null` 结束，满页后可能有空末页。

只有所有页都完整核验才更新列表或恢复列表。各页反映各次查询，不是同一时刻的快照。分支和标签另读详情。

## 所选容器详情

请求 `GET /zxs/v1/containers/<id>`，带 `Zxs-Detail: paged`。各页保留同一容器 ID、版本、名称和标签；最多 256 个按分支名排序的头。

续页使用原版本 `Zxs-Detail-Sequence` 和上一页的 `Zxs-Detail-After`。满页后可能有空末页。版本不一致或任一页不完整就不显示这次详情；历史读完后再核原详情第一页。

## 分支历史

请求 `GET /zxs/v1/containers/{id}/branches/{branch}/nodes`。每页都带原头节点 ID `Zxs-History-Head`；续页另带 `Zxs-History-Next`，即上一页末节点的父节点，不重复上一页末节点。

响应为 `head`、`nodes`、`next`；每页最多 256 节点，`next: null` 到根。服务端每页核原头并沿其父链找续页起点。客户端只有整链验证并重新核原详情一致后才显示。

[旧版来源](https://github.com/isarmg/zxs/blob/85a8988c386baad1305f4f81ce2c57786df6e436/zxs-workspace/docs/container-management.md#分页容器列表)保留；面向使用者的连接、另存和结果未知见[操作指南](../guides/remote-files.md)。
