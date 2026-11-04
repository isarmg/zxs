# 当前登记代的批量命名空间编辑

状态：源码与测试源码已实现；只完成指定文件的官方 rustfmt 和 `git diff --check`，未编译、未运行测试、模型、CI 或真实部署。本文不构成 D3、物理掉电、吞吐或端到端权限验收。既有基线见[当前磁盘命名空间](current-disk-namespaces.md)与[磁盘退出发布](disk-exit-publication.md)。

## 本次准入范围

`directory.namespace.batch` 在当前 registry 已发布后，只接纳由当前 main 拥有、已选中的目录组合。选择树可以是目录选择刚生成的不可变投影，也可以是已选父组合冻结引用中的子树，不必等于命名 branch HEAD。Move、Copy、Rename、Delete 的来源及目标都要通过这个准入；多个当前活动子树仍必须不重叠。普通初始代保留原批量组合行为。

`NamespaceSnapshot` 和 `DirectoryNamespaceView` 只提供共享读取数据，不授予写权。`current/batch.rs` 返回字段私有、不可由调用方构造的 `CurrentBatchScope`，将命名 branch HEAD 基线与实际 selected tree 分开。它先检查 owner 和目录 scope 的可写登记及 head，再验证稳定目录身份、树所属分支继承链、当前整个子树的精确 occurrence 和完整后代 refs。Entry/file/version/size/路径拼写逐项相等才证明源 occurrence；全局存在同一个文件 ID 不足以导入另一冻结 occurrence。

未选中的目录 branch、不能与当前 owner 精确对应的历史 tree 仍拒绝；不提供任意历史 tree ID 的写入口。请求的 `expected_tree_id` 继续比较真实命名 branch HEAD，不能用 selected projection ID 代替，过期基线返回 `DirectoryBranchConflict`。非活动或历史选择返回 `RecoveryRequired`；身份、上下文、refs 或准入投影损坏返回 `WorkspaceChanged`。D3 后仍选择原 HEAD 的已有路径复用目录投影证明，不要求先额外初始化。永久旧 receipt 仍在新操作准入之前按原请求与 principal 重放。

## 读取、准备及发布

- 业务消费 `BatchScope::Current(CurrentBatchScope)`，普通初始代的冻结组合保留独立类型分支。所有当前草稿以真实 owner 的当前路径表达；选中投影不能因 selected tree 不等于命名 HEAD 而误走 inactive 保存。
- selected tree 的每个后代 refs 必须指向该目录身份的实际选择，后代完整 entries 必须与父树对应范围重定位后相等，子图 refs 必须与根图一致。未选 child HEAD 的较新或较旧版本都不能替代真实选择。
- 完整 occurrence 相等之后，path context 从同一当前 owner 的精确路径取得，并验证 file/history/node 身份。父目录选择可向子目录赋予继承写意图而不推进 child HEAD；批量编辑保留该当前意图，不能用 child 冻结上下文覆盖，也不按全局 file ID 借用另一个 alias 的 context。公开 file selection 镜像只在 occurrence 已证明后收集。旧 Commit、DirectoryTree、版本及正文不改写。
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
- 隐藏源、过期目标、重叠活动 scope、非活动 scope、未选历史 tree、大小/身份不符的当前投影均拒绝；旧成功 receipt 保留，精确 no-op 不物化隐藏 entries。
- 现有 workspace 正文准备期间 batch 仍返回 `CommitPreparationBusy`；损坏真实源正文不能完成 Copy。
- 反序列化的发布 State 经恢复入口复验：隐藏源或重复已保留 alias 的注入拒绝，合法 copy 可恢复。
- 独立真实生命周期源码用例 `disk_exit_current_batch_real_d3_restarts_without_reopening_or_rewriting_the_target` 走 begin → managed archive → enrollment → joint → D3 → batch Copy → `Engine::open`，覆盖 Deregister 和 PlannedOffline，检查目标绑定关闭、目标物理文件字节不变、保留正文可复制、成功 receipt 重放及旧历史不变。这是临时盘测试源码，未执行，不能称已取得真实运行证据。

新增 [current_selected_batch_tests.rs](../../test/engine/unit/engine/current_selected_batch_tests.rs)，直接接入 `current/batch.rs` 的私有测试模块。六项源码用例走真实临时目录 Deregister/D3 后再选择冻结 root，覆盖：

- root selection tree 与 root HEAD 不同，递归 Copy 只复制当前选中正文，entry/file/version 全部新建，隐藏文件不复制。
- child selection tree 来自父组合旧 refs，而 child HEAD 含未选的新版本；跨不重叠 scope 的 Move 保留所选旧版本及父选择的继承写意图。
- Rename 占用隐藏同名路径但保留准确源身份；重新保存原路径取得新身份；旧 HEAD 失败和成功 receipt 幂等重放。
- 批量 Delete 的目录项递归删除，删除事实不含隐藏文件；同名重建目录不重新准入旧目录身份。
- inactive branch、隐藏源、把 selected tree ID 当作 expected HEAD 的请求均原子拒绝。
- 选中树的 hidden entry、未选 child HEAD、refs、缺失 context 或目录身份篡改均拒绝。

Copy、Move、DeleteTree 场景包含 `Engine::open`，Copy 另验证完整历史不变、目标目录文件字节不变和重启后 receipt。测试未运行，不代表恢复闭环已获运行证据。

统一验证者仍须编译并运行以上新测试及既有 composition、namespace、workspace、生命周期/恢复矩阵；另验证 HTTP/SMB 的对象授权和撤权、交错并发、发布故障点、真实 target stat/open 禁止、Clippy 与架构门禁。本补丁没有改变这些层的权限或同步边界，也不以 engine actor 校验代替传输层授权验收。

## 成本与同步边界

共享 snapshot 捕获不复制全历史。每个不同输入 scope 只准入一次；准确 owner 对照使用排序当前 entries 的范围查找及逐项比较，来源只展开实际已选子图。设该子图各节点 entries 总量为 S，类型构造与 refs/context 检查占用 O(S) 临时存储并含有序集合/map 的对数开销；复用现有目录投影的 HEAD 路径保留原投影成本，不扫描未选历史树。Rename/Move 为独立映射的最终目录路径增加一次当前 main 组装，现有按活动 scope 替换的最坏扫描仍随活动 scope 数与当前 namespace 大小增长，没有声称已完成大规模性能优化。

Copy 仍按实际新正文执行已有完整副本、哈希与发布流程。没有减少 fsync、决定证明、prepare 排斥或发布线性化检查，也没有新增独立发布事务。
