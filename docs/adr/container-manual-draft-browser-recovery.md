# 手动共享稿的浏览器显式提交恢复

## 范围与当前证据

已有 Web 设置入口默认显示实际 automatic/manual 状态；dirty 时不允许隐式切回自动。
显式提交独立调用 prepare，冻结 PublicCommit 请求，再提交原请求；普通 SMB flush/close
不取得此许可。已提交的首版 UI 仅在根状态中保留请求，能跨页面导航，不能据此声称跨刷新恢复。

本后继实现浏览器持久恢复，但尚未完成远端编译、测试或真实浏览器刷新验收。
新增纯解析回归也未在本机执行。工程实现存在不等于已通过运行验证。

## 恢复协议

复用现有 bounded browser recovery Store，不保存账号凭证。按已验证主体分区，每个主体
最多两条、每条最多 64 KiB：不可覆写的原始 intent 与不可覆写的完整 frozen envelope。
原 intent 在 prepare 前写入；envelope 在任何 commit POST 前写入并读回核对。
记录保留原 operation、文件、调用权限 context、稿 generation、稿 operation 与完整请求体。

刷新或进程重启后，登录同一主体才恢复其记录。完整 envelope 只能查询或重发原请求，
不能重新 prepare 或换 operation。只有 intent 的记录允许用户显式重试原 prepare。
恢复不信任本地 published 声明；必须取得匹配 operation、kind、request digest 的现场收据。
已有 envelope 恢复为未知结果并查询；不会在恢复过程中自动提交。

终态后先删 intent 再删 envelope。中断留下的 envelope-only 仍保存所有原始字段，
只能按原请求查询/重发。损坏、冲突、超预算或存储错误保留原材料并明确阻塞新写入，
不会删除记录后从头准备。跨账号或新会话拒绝旧异步回调；已终态请求拒绝迟到回调。
取消只允许尚无 envelope 的提交准备，不丢弃共享稿。

## 尚待真实验收

- prepare 前后、envelope 落盘后及 commit 响应丢失时分别刷新，确认只复用原操作。
- 同账号重启后的查询/重试与异账号隔离；原权限撤销后应由服务端拒绝。
- localStorage 禁用、满额、损坏与清理中断时保留证据并阻塞，而非静默重建请求。
- 与服务端共享稿冷开、无历史节点的 SMB flush、显式提交清稿保持闭包。

目录提交及未提交 worktree 软删仍沿既有约定，本后继不修改其语义。
