# 当前登记下的目录分支创建与选择

状态：源码实现与测试源码，尚未编译或运行。本文不是生产部署、真实磁盘停用或 CI 通过记录。

## 准入范围

`directory.branches.create`、`directory.branches.select`、`directory.default.select`、`directory.default.set` 使用已正式安装的当前 registry view。当前目录身份由所属 main 的准入 entries 解析；同名重建目录只能使用新 entry ID。preview 与 branch summary 同样使用当前目录身份及目录分支 head 的准入 entries，统计不再包含隐藏文件。

创建先从当前 owner 捕获源目录与已选后代。非根目录创建还从同一 owner 刷新当前根组合，保证新源和祖先 references 一致，而不是从未物化的 raw commit 回填隐藏项。新命名分支仍继承父树，不复制正文、不创建占位文件；路径 context 只增加本分支的延迟写入意图。

选择必须同时满足：当前 owner 可写、源目录分支可写、源 head 与准入 scope 相同、稳定目录身份仍在当前 owner。源树的每条保留 occurrence 精确匹配 entry/file/version/size/path；其子树与源父 scope 的完整 occurrence 绑定须一致。每个保留文件取该路径原有的 frozen context，不从全局同名文件或其它路径补造。

子树只能沿该已准入 head 的冻结 references 投影。不存在任意 tree ID 的当前写入口，也不把全部历史树统一过滤或置空。不闭合的子树关系返回明确错误，不能把另一条当前分支的 head 当作该冻结引用的替身。

## 不可变树与当前选择

旧 Commit、DirectoryTree、源 DirectoryBranch.head 均不改写。当前 entries、contexts 或后代引用与原树不同时，选择只增加不可变投影树，并通过 `directory_tree_selections` 记录当前选择。新树以原树为 parent；未变化的节点复用原树。`directory_tree_selections` 因此可以不同于所选分支 head，这是已有 State 表达能力，不是格式迁移。

main 的新提交只替换准确目录 scope，保留当前投影中的兄弟。新当前 path contexts、file references 和 selected descendant graph 全部来自同一源投影。隐藏上下文不被带回；历史查询仍保留原树与旧引用。再次选择完全相同的投影不新增目录树。

发布前及恢复时，`advance` 从已认证 previous State 与其 current view 重建 exact splice、contexts、file references 与新树 lineage。操作回执只定位 source branch，不能凭 receipt bool、原 head 字符串或全局保留 ID 获得准入。新 fork 另核对来源树的稳定目录身份、当前 owner entries、所有路径 context 和后代图。通用 unavailable-version 与历史 identity guard 继续执行。

旧成功 receipt 在 planner gate 前按原 operation/request 重放。新 default set 仍要求准确 expected head/tree；普通发布失败不安装候选 State 或当前 view。

## 成本与边界

设当前 owner 有 N 条 entries，源图展开后的总 entries 为 S，引用节点数为 D。创建刷新当前 owner 的选中组合，最坏随当前目录嵌套展开量增长，不扫描或重写全部历史树。投影按路径二分及 ID/引用有序索引匹配；有序 map 操作为对数成本，entry/context/tree 的工作量与 S 相关。不可变源树保留 Arc，复制所需投影图与上下文，额外内存 O(S)，另有 owning namespace 的 O(N) splice 与排序成本。每条引用还核对命名分支 parent 链，成本随所访问的链长度与分支 map 查找增长；创建沿用现有全分支名称冲突扫描。现有 State 共享 map 首写、历史序列化和永久 receipt 成本不在此改变。

最多每个发生投影变化的引用节点新增一个 tree，没有正文复制、额外独立业务发布或删除既有 fsync 屏障。本次没有运行分配、吞吐或大规模基准，不能据此声称性能达标。

历史 import/merge、显式旧 tree 写入和未闭合的跨 scope 操作仍按各自 gate 拒绝。当前 batch 若要求 selected tree 等于 branch head，会拒绝刚建立的独立 selection projection；应先完成该 batch 的独立来源证明，不能仅删除等式。本实现不扩大真实部署、凭据、权限或不可恢复删除范围。

## 待统一执行的测试

`test/engine/unit/engine/current_directory_branch_proof_tests.rs` 使用临时登记目录，执行实际 `begin_disk_exit → enrollment → joint decision → activate_pending_disk_exit` 后覆盖：

- Deregister 后 preview、root/subdirectory 选择、创建、旧 receipt、不可变旧树/commit，以及重开恢复。
- 嵌套 refs、隐藏 contexts、兄弟保留、同一选择的无新树结果。
- 删除后同名新目录不冒用旧目录身份，旧 source branch 不能重获当前入口。
- 重放时篡改 splice、path context、新选择树或旧 source tree 均拒绝；新 fork 的跨 scope 身份、隐藏项、伪造 materialized 或 context 变更拒绝。
- stale expected tree/head 不发布 receipt 或改动 State。
- journal prepare/sync 故障不发布选择，重开重试；PlannedOffline 保留登记与目录数据。
- D3 后临时目标目录逐文件字节保持不变。

这些是待运行的源码断言，不替代逻辑 I/O 计数、并发准备交错、各断电屏障、HTTP/SMB/UI 验收或真实设备测试。本分支只执行官方 rustfmt 与 `git diff --check`，不运行 cargo、测试、clippy、模型、CI，也不推送远端。
