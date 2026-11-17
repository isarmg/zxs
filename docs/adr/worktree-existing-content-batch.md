# 已登记 worktree：一次提交多份已有文件正文

**状态：提请确认，未实施、未验收。** 第一次只解决这个场景：从已登记的根级目录
快照打开一个实例，在原路径保存几份已有文件，然后按一次「提交」。成功后这些文件
各有一个 FILE 内容节点，目录只有一个新快照；任何读者都不能看到只成功了一半。
保存、flush、关闭句柄仍不建历史节点。不同文件的保存互不抹去。

请求批准下述**一个格式和一种发布规则**：沿用已确认的全新池唯一布局、现有日志和
[固定成员发布](0002-publication.md)。先用已有原生发布器登记一个**只占用、不发布文件**
的原请求；随后只用**同一个业务 Candidate** 发布该批的全部 FILE、目录、DISK／SYSTEM
事实及 worktree 检查点。不逐个提交 FILE，不再设一个工作区决定者。下文给出这两个
内部记录的完整字段；不改 HTTP／SMB 请求、状态 DTO，不添加版本标记、兼容读取或迁移。

## 准入范围

- 只准真实 Initial 根登记：同一普通磁盘根、同一原注册与代际、同一个可写 worktree。
  工作区源为完整已提交的 SnapshotHandle；目录当前 HEAD 必须仍是这个完整 handle。
  只接已有、可读的原始文件 occurrence，允许原快照中已登记的多级目录，不跨独立子仓。
- 实例处于 Active，未提交过目录结果；没有新增项、命名空间编辑、正在发布或未决保存。
  成功的显式保存按原快照引用和**准确原路径**替换这一项的覆盖层；同路径多次保存取
  最近的已完成保存，其他路径原样保留。每次组合覆盖表、已决定修订和真实 ledger
  必须一致；相同字节的显式保存仍算一次修改。所有脏项都参加，不接客户端自报选集。
- 每项从原 manifest 取 Container 的 FileId／VersionId／原选分支及节点；从同一文件
  容器真实登记取得当前 EntryId、路径与持有的实际父/实体。第一片要求仍在原路径、
  同一普通根，原选 FILE 节点仍在原分支头，不存在共享未提交稿。允许已建历史的
  Automatic 或 ManualClean 容器；显式工作区提交不改模式，不冒充自动保存或消费共享稿。
  不按路径或哈希查找 FileId。原快照没有声称保存普通 EntryId，这是当前容器证明，
  不是从快照文件名倒推的身份。
- 不按 FileId 合并 occurrence。若两个脏项指向同一 FileId（即使不同分支）、
  重复准入键、目标名称或持有实体冲突，全批在登记前拒绝。未改别名仍引用它自己的
  原版本。目录/FILE 已前移、本地仅有 SnapshotOnly、离线、不可读、失去实际实体，
  均不由本片降格处理；不能把一批换成串行单文件提交。已支持的独立历史分叉或新文件
  路径不作为本批失败后的回退。

## 严格持久字段

下表的结构名表示类型，不是盘上额外的字段。两条记录沿现有 snapshot/worktree
封套：原 OperationResult 的操作号、kind、主体映射、请求摘要、序号及现有结果页脚
仍由原生发布器发出。记录放在现有 `result.snapshot_worktree` 下，沿用
`Record` 的相邻标签结构 `{"kind":"表内标签","record":{表内字段}}` 及 snake_case；
没有表外字段或隐式缺省。下列
现有 Rust 类型沿用仓内各自的严格编码。u64 按既有规范十进制字符串，无前导零；
所有表按各自边界先验数量与字节，拒绝重复键、未知字段和不规范编码。

| 操作 kind／记录标签 | 必需字段 |
| --- | --- |
| `snapshot.worktree.content-reserve`／`reserve_content_batch` | `request: CommitWorktree, source: SnapshotHandle, source_binding: SnapshotNodeBinding, host: HostEvidence, root: SnapshotHost, ordinary_head: CommitId, ordinary_generation: Generation, instance: SparseWorktreeBinding, ledger: Binding, before: Checkpoint, destination_node: SnapshotNodeId, publish_operation: OperationId, rows: [Input]` |
| `snapshot.worktree.content-publish`／`publish_content_batch` | `reserve: OperationId, reservation: MaterialCommitment, prepared_node: PreparedNode, rows: [Output], committing: Checkpoint, completed: Checkpoint` |
| `Input`（一项完整物理保存及准确 FILE 基线） | `edit_operation: OperationId, edit_completion: OperationId, edit: EditRecord, original: SnapshotFile, base: DraftBase, owner: DiskId, namespace: ContainerNamespace, owner_root: RootId, owner_entity: ContainerEntity, history: HistoryTreeId, branch_base: CommitId, clean: FileWorkingState, commit: CommitId, tree: TreeId, allocations: [Allocation]` |
| `PreparedNode`（非最终名上的已分配实体） | `preparation: DirectoryId, binding: SnapshotNodeBinding, entries: [SnapshotEntry]` |
| `Output`（同一业务收据的一行，而非可独立提交的子操作） | `path: PortablePath, commit: CommitId, tree: TreeId, file: FileResult, content: PublishedContent` |

这里 `Binding`、`Checkpoint`、`EditRecord` 与 `FileResult` 分别是现有 ledger/
projection 类型；`PublishedContent` 是 FILE 生产者的 `{version,placement,allocations}`。
不把临时路径、OS 绝对地址或可选 JSON 塞进字段。

1. 原请求号 = `request.operation_id`，原意图摘要只用
   `Engine::native_request_digest("snapshot.worktree.content-reserve", principal, request)`。
   请求中的 target 同时绑定创建完成回执及 WorktreeId，expected 是整实例的修订。
   reserve 的完整内容承诺沿用现有 `Record::commitment`：
   BLAKE3(ASCII `zxs:snapshot-worktree-reservation` 后接零字节，再接整条 Record 的规范 JSON)，
   包括分配结果。publish 的操作号必须是 reserve 预先分配的 `publish_operation`；
   其请求摘要以同一主体、publish kind 及严格对象 `{reserve,reservation}` 计算。
   publish 数据中不反向包含 Candidate／Snapshot 自身摘要，避免循环引用。
2. 两个 rows 都以原快照 `PortablePath.key()` 严格升序；Input 的键是
   `edit.path`，Output 的 `path` 和 `file.path` 必须与对应 Input 保留完全相同
   的正式拼写。两表个数和键序必须精确一致。行号不授予权限，身份是
   `(source_binding.reference(), edit.path)`。`edit_operation/edit_completion`
   必须是这一路径最后完成保存的原始保留和完成回执；重放所有保存与替换重建完整覆盖表。
3. `original` 是原条目，`base` 绑定原 BranchId、FileNodeId、VersionId 和实证的
   选择代数。`namespace` 是当前准确 ContainerNamespace，带 EntryId；连同 owner、
   RootId、实体和原生目录父链逐项重开。无普通父、移动、退役或含稿的行不准入。
   `edit.material` 是已保存的独立 VersionId、FileId、大小、完整哈希；
   `edit.leaf/entity` 是原稀疏实体。`clean` 必须是上述两种干净态，模式和工作稿
   修订不由本次历史提交修改。FILE 选择、历史和 placement 仍按真实 FILE 构造器闭合。
4. reserve 一次分配所有 FILE CommitId／TreeId、目录 SnapshotNodeId 和 publish 操作号；
   核对现存完整身份、短名与实际目标组件、同批重复，不截断或覆盖。每个 FILE 提交
   的 operation_id 是同一 `publish_operation`；永久逐项结果的键为原操作与原
   occurrence，不伪造 N 个可分别成功的 FILE 操作。FILE 父提交为 `branch_base`，
   父版本为 original.version_id，新 FILE node 为 `FileNodeId::Version(edit.material.version_id)`，
   留在 `base.branch_id`。每个 Output 的 `commit/tree/file/content` 与相应真实
   FILE State、版本证据和落点一一对应。普通根当前投影也可能需要额外 Commit/Tree
   或惰性分支：从冻结的 ordinary_head/generation 开始，以行序在同一 prospective
   State 上运行真实 FILE 规划器，将每次调用的完整 Allocation 有序轨迹保存在 Input。
   再执行必须一次性精确耗尽，不可新增/遗漏；Output.content.allocations 必须逐项相同。
5. 新目录在原分支继续，父节点为 `source`，完整 handle 中
   `publication=publish_operation`。`entries` 由全量原 manifest 逐路径替换这批结果
   后用唯一 manifest 构造器得出：空目录、没改的路径／拼写／版本及 FILE 选择原样保留。
   `binding` 的 repository/node/files/worktrees 实体和 reference 必须与实物及
   上述 entries 重算结果完全一致。ledger 的 Commit 与 Completed 两个既有 Frame
   按 `before`、原操作、原 HEAD、目录目的地及全量 FileResult 预编码，逐个计算
   `committing/completed`；不可按最新本地帧补选。core 的 directory/worktree
   local-material 引用绑定这份节点与完成后检查点；它不产生第三份业务决定。

## 步骤、落盘及中断

这些是动作顺序，不是另存的一张可自行推进的 phase 表：

1. **登记。** 在同一 Engine 写者下核对权限、已存在的原请求、完整覆盖层、真实
   磁盘/容器和所有分支头。先核对原操作、主体、完整请求才看新输入；
   无可提交脏项或任一不满足时，不登记、不动材料。原生 reserve 只有占用语义：
   锁定准确 worktree、所选 FILE 分支、同批更新的普通根分支、目录分支及目标身份；
   所有前台写入口和冷重开须执行同一占用，既不推进 HEAD，也不移动
   稀疏文件。已登记的原操作永久可查询为 Pending；不发布后改主意的替代分支。
2. **准备。** 按 reserve 的唯一计划准备各 FILE 独立全量正文、原生版本证据和同一
   prospective State。在原 `.zxs` 父下用 EXCL 创建临时组件
   `batch-<preparation 的完整 DirectoryId 文本>`；其中仍是现有 manifest、files 和
   worktrees 布局。准备所有未改的不可变正文 COPY；dirty 目标叶留空，从原实例持有
   /重开精确 inode，核对全长、哈希并同步。此时没有 MOVE，原稿仍完整。临时组件
   不出现在历史或 SMB 名字空间；真实新实体及所有父须读验。准备期取消/崩溃不抹
   任一输入。若独立原生日志证明从未形成待恢复的 publish Candidate，原 reserve
   可用新的**临时** preparation 重新准备同一批指定身份；不采用未知临时目录，
   不重新分配任何 FILE／节点／操作身份。若已写 PREPARE 或结果不明，先走原
   槽位的现有恢复，不另开一次 publish。
3. **决定。** 重获写者，在现有线性化点复核权限、原冻结占用、所有根/父/清洁
   基线、FILE 分支和目录 HEAD、候选预算及精确物理证明。一个原生 Candidate
   同时绑定目标 Snapshot、所有真实 VersionEvidence、FILE/DISK/SYSTEM scope 闭包、
   被绑定的临时 node 实体及永久 publish 结果。原 PREPARE／ACCEPT／FINALIZE 的
   成员、代际和门槛不变。查不到确定决定时不做 MOVE，也不释放冻结。
4. **安装和可见。** 只有原生 FINALIZE 证明允许在该槽的既有物理安装阶段对每个脏项
   做 NO REPLACE MOVE：源是 `instance.worktree_entity + edit.leaf + edit.entity`，
   目标是 `prepared_node.binding.files_entity` 和由最终 manifest 的该 VersionId
   唯一派生的已有 snapshot payload 名。原节点不动。复核两端父和 inode、同步文件及
   两个父；核完精确全量 node 后，将同一实体从上述临时名 NO REPLACE 安装为
   **既有** `layout::snapshot::node(destination_node)`，同步并经最终名再完整读验。
   依序同步准确 ledger Commit／Completed 两帧，完成 FILE/各盘安装后，才允许原
   PUBLICATION／seal、活动 State 和对外成功。FILE、SMB、管理页、备份读与冷开
   不得绕过在途安装看到混合结果；缺盘时保留原操作，不能以 SQLite 或本地帧开锁。
5. **重试。** 对已决定的 MOVE，只承认原 inode 唯一在准确源或准确目标，目标已装则
   复核并同步，源已装则继续。两处都有、都无、实体不同、权限或父变化时保留并阻止
   服务，不覆盖、不用同哈希文件重建。临时/最终 node 的安装遵循同一规则。已写一帧
   或两帧只接受原检查点精确字节；丢回执后核对同一 publish 收据/闭包，不重建节点。
   同号异主体或异请求始终拒绝，曾登记但未完成绝不返回成功。

## 有界性、验收与不包含项

沿用实际配置和既有最小上限，不提高门槛：完整 manifest 最多 2048 项／512 个版本、
4 MiB 编码及既有路径和逻辑正文预算；ledger 最多 1024 帧、每帧 4 MiB、全表 64 MiB。
预留本次两个帧，另核原生元数据、数据库、正文、物理 staging 占用和 FD 预算；
任何一个超限在决定前拒绝，不拆批伪装原子成功。FILE 工作区逐一提交不算实现。

须以两个及多个真实原始文件、嵌套路径、反复保存其中一个、相同字节的显式写、
未改别名、两个实例争一分支验证。分别在 reserve 前后、临时节点准备、PREPARE、
ACCEPT／FINALIZE、各 MOVE、node 安装、两个 ledger 帧及回执丢失处冷重开；
核对整批 FILE 和目录都未显现或都已闭合、未改材料不变，另验真实 Samba/浏览器。
这些是后续验收条件，本文未运行构建、测试或 CI。

**本次不批准**命名空间改名/隐藏、同 FileId 的多脏路径合并、历史多文件分叉、
首次导入/新增文件、软删除/恢复、已移入 deleted-data 的容器与物理退役证明、
跨盘复制/移动、Current 登记、节点改成「15 位提交身份－目录名」、增量后台合成
或新增 HTTP／SMB 协议。上述需求继续单列，不用本片的拒绝或一次 ledger 回放冒充完成。
