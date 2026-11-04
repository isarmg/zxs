# 当前登记代的批量命名空间编辑

状态：源码与测试源码已实现；只完成指定文件的官方 rustfmt 和 `git diff --check`，未编译、未运行测试、模型、CI 或真实部署。本文不构成 D3、物理掉电、吞吐或端到端权限验收。既有基线见[当前磁盘命名空间](current-disk-namespaces.md)与[磁盘退出发布](disk-exit-publication.md)。

## 本次准入范围

`directory.namespace.batch` 在当前 registry 已发布后，只接纳由当前 main 拥有、已选中且选择指向命名 branch HEAD 的目录组合。Move、Copy、Rename、Delete 的来源及目标都要通过这个准入；多个当前活动子树仍必须不重叠。普通初始代保留原批量组合行为。

`NamespaceSnapshot` 和 `DirectoryNamespaceView` 只提供共享读取数据，不授予写权。新增 `current/batch.rs` 先检查 owner 和目录 scope 的可写登记及 head，再比较目录身份、完整路径拼写和整个当前子树。Entry/file/version/size 的精确相等才证明源 occurrence；全局存在同一个文件 ID 不足以导入另一冻结 occurrence。

未选中的目录 branch、选择的 tree 不等于命名 branch HEAD，以及不能与当前 owner 精确对应的组合仍拒绝。当前目录选择在投影变化时也可能合法产生一个不同于 branch HEAD 的 selection tree；这种刚选择的投影同样须先通过当前初始化或实际内容编辑物化，不能直接进入本批 batch。前两者返回 `RecoveryRequired`，已变化 head 返回 `DirectoryBranchConflict`，当前投影不一致返回 `WorkspaceChanged`。没有假称这些分支已经闭合。永久旧 receipt 仍在新操作准入之前按原请求与 principal 重放。

## 读取、准备及发布

- 业务从当前 main snapshot 和目录投影建立草稿，不再把原始 Commit/DirectoryBranch.entries 当作当前内容。目录草稿只保留可见条目的 path context、file branch、继承来源与目录引用；旧 Commit、DirectoryTree、版本及正文不改写。
- 当前目录的祖先身份也来自同一 main snapshot。来源与目标的全部基线和名称冲突在创建新正文之前完成检查；隐藏源即使有已知 entry ID 仍返回 `EntryNotFound`。
- 批量重命名先按原始映射组装最终当前视图，再调用现有当前目录冻结逻辑，避免把独立映射误作顺序重命名。发布后刷新所有受隐藏路径影响的选中子树和祖先，不让新 root 引用仍含隐藏内容的旧 child tree。
- Move/Rename 保持准确可见 occurrence 的身份；Copy 继续生成新 entry/file/version 身份并调用真实副本校验及正文准备，不能靠改名或硬链接冒充副本。同名重新创建不会使用被移除对象的身份。
- 路径节点的独立文件 branch 继续跟随这次操作的准确新路径。只复用已存在的 `relocate_namespace_file_branch`，不把同 file ID 的另一 alias 分支一并改写。
- 普通发布和恢复继续经过 `CurrentNamespaces::advance` 的 occurrence 消耗证明及不可用版本禁止集合，不增加全局 ID 白名单或恢复特例。失败不发布部分 namespace，原暂存字节和永久幂等收据保持原协议。
- 未改变真实磁盘删除、离线、登记或 I/O 关闭机制；注销只影响登记和当前独占 occurrence。没有增加清盘、恢复文件、凭据、权限或部署步骤。

## 测试源码与待执行项目

新增 [current_namespace_batch_tests.rs](../../test/engine/unit/engine/current_namespace_batch_tests.rs)，接入既有当前 namespace 的私有测试模块：

- 隐藏同名目标的 Rename、后续独立 file-branch 保存，以及原 Commit/DirectoryTree 不变。
- 递归 Copy 的新身份、真实新正文、隐藏子项不复制、当前 child refs 刷新、同名新对象后续保存。
- 两个不重叠活动 scope 间 Move、嵌套目录 Rename 与大小写重命名。
- Delete 的删除记录只含可见 occurrence；重新创建的同名对象取得新身份。
- 隐藏源、过期目标、重叠活动 scope、非活动 scope、不同于 HEAD 的选择 tree、大小/身份不符的当前投影均拒绝；旧成功 receipt 保留，精确 no-op 不物化隐藏 entries。
- 现有 workspace 正文准备期间 batch 仍返回 `CommitPreparationBusy`；损坏真实源正文不能完成 Copy。
- 反序列化的发布 State 经恢复入口复验：隐藏源或重复已保留 alias 的注入拒绝，合法 copy 可恢复。
- 独立真实生命周期源码用例 `disk_exit_current_batch_real_d3_restarts_without_reopening_or_rewriting_the_target` 走 begin → managed archive → enrollment → joint → D3 → batch Copy → `Engine::open`，覆盖 Deregister 和 PlannedOffline，检查目标绑定关闭、目标物理文件字节不变、保留正文可复制、成功 receipt 重放及旧历史不变。这是临时盘测试源码，未执行，不能称已取得真实运行证据。

统一验证者仍须编译并运行以上新测试及既有 composition、namespace、workspace、生命周期/恢复矩阵；另验证 HTTP/SMB 的对象授权和撤权、交错并发、发布故障点、真实 target stat/open 禁止、Clippy 与架构门禁。本补丁没有改变这些层的权限或同步边界，也不以 engine actor 校验代替传输层授权验收。

## 成本与同步边界

共享 snapshot 捕获不复制全历史。每个不同输入 scope 只准入一次；准确 owner 对照使用排序当前 entries 的范围查找及逐项比较，草稿上下文裁剪占用与该 scope 可见 entries 成比例的临时集合。Rename/Move 为独立映射的最终目录路径增加一次当前 main 组装，现有按活动 scope 替换的最坏扫描仍随活动 scope 数与当前 namespace 大小增长，没有声称已完成大规模性能优化。

Copy 仍按实际新正文执行已有完整副本、哈希与发布流程。没有减少 fsync、决定证明、prepare 排斥或发布线性化检查，也没有新增独立发布事务。
