# 待决定：让一块盘能核验整个目录快照仓库，而不另建分支注册表

**决策：2026-11-18 03:53:31 UTC，所有者批准了 [02d97634 的本页](https://github.com/isarmg/zxs/blob/02d97634a45f028bcdab5cbae78606a7ce70637e/docs/adr/directory-repository-local-material-proposal.md)。批准只覆盖下述目录局部材料；不是已完成、已合并或已测试声明。** [已批准的三范围格式](three-scope-production-authority.md)
给目录材料留了 `native{root_hash}`，但没有定义一个能代表**整个目录仓库**的本地原生根。
本提案只定义这个引用的内容，及首次仅有预约时如何诚实登记。不是升级、第二种
Snapshot 格式、另一个写者、可变 HEAD 文件或新的表决。既有目录操作/HTTP/SMB 不变。

## 为什么不能指向已有摘要

以既有原生实现 [f7c0f4e6](https://github.com/isarmg/zxs/tree/f7c0f4e661187bd226649cb22fbe45d40c8e93d1/src/engine/src) 为准：

- `working_tree/snapshot/store.rs::SnapshotNodeBinding` 和 `SnapshotReference` 只绑定一个节点及其原清单。
  空目录、同正文的不同路径和 FILE 选择在清单中；支线、兄弟节点、稀疏实例和待处理预约不在。
- `snapshot_worktree/registry.rs::ReceiptIndex::capture_inner` 及 `ordered_records` 在验证过的原生 SYSTEM 前缀上
  知道各分支、已完成/保留对象及未完成预约。`registry/history.rs` 的业务历史页只遍历已完成节点；
  `worktree_for_commit` 也不会把未创建完成或有待处理编辑的实例当可写实例。这个内存索引不是所有参与盘已有的可单独读取物料。
- 单个 worktree 的原生 `ledger::Binding` 和 `Checkpoint{sequence,commitment}` 能验证自己的原始链，不能证明同仓库没有别的实例。
  对正在移动的正文，`commit/content/scope.rs` 的实际 held 源与 `recovery/workspace_receipts/content.rs`
  的原 Candidate 切点能区分预约、部分移入和已完成；扫目录或取当前 HEAD 不能。
- 因此，沿用某个节点/ledger 的摘要、以 SYSTEM State 哈希充当本盘图、或对目录列表求哈希，都无法在一块非元数据盘上核验整个原仓库。

## 请求批准的唯一增量

1. **一个不可变物料，没有单独发布。** 对每个实际宿主上的 `(SnapshotRepositoryId,DirectoryId)`，
   在**同一原生决定**准备本盘清单时生成 `directory-repository-material`。沿用现有私有内容寻址对象目录、
   `state-page`/ `metadata-index` 和规范 JSON；不在业务目录建立服务、HEAD、SQLite 或旁路日志。
   固定头只有 `repository_id,host,initialization_operation_id,rows`。其中
   `host={registration,root_id,directory_id,host_entity,namespace}`，`namespace` 是已有普通仓库/分支的
   `NamespaceHostAddress`；`rows` 是带原生 page/index 类型的引用。可复用且未变的根不因无关序号而重写。
   在已批准的 `Material{scope:directory,identity,logical_root,representation,body_inventory}` 中，
   `identity={repository_id,directory_id}`，`representation=native{root_hash}`。
   `root_hash` 是上述固定头的域摘要。`logical_root=root_hash`：此种原生物料本身就是唯一规范的
   逻辑投影，不容许另一种拼接或字节编码。`body_inventory` 另外绑定本盘所有已要求的真实正文 occurrence；
   它不能反过来代替仓库图，也不能将两个局部节点的表当作全仓库表。

2. **一次派生，精确到原始 occurrence。** 下表是 `rows` 唯一容许的行族。键为所列有类型元组的
   规范 JSON 字符串；无歧义、区分 stable ID/原操作/ordinal。值的每个已有类型沿用原生严格 codec，
   不接受任意 JSON/任意路径字符串。每行的来源一律是已独立选定的原操作
   `{sequence,operation_id,request_digest}`；本次操作只用该稳定元组，绝不引用包含自己的 Candidate/Snapshot。
   接受原生变更时只更新相应键；不能仅从次序、显示名、最高序号或扫描结果拼出这些行。

| 行及唯一键（在本仓库内） | 取自哪里，保存什么 |
| --- | --- |
| `host_step(ordinal)` | 已验证 `HostEvidence.directories` 的原顺序及每个原 `NamespaceDirectoryBinding`。本盘登记、根、管理分区和实际宿主实体仍独立核验；无嵌套时确认为空。 |
| `branch(branch_id)` | 原生分支稳定 ID、原分支名和建立操作；状态仅为 `pending{reservation}` 或 `head{SnapshotHandle}`。重复/分叉的正向声明拒绝。该指针只能由原生分支转换重算，不是另一个可改的 HEAD。 |
| `node(node_id)` | **全部**已发布且仍归属此仓库的节点（非 HEAD 也在）：完整四元 `SnapshotHandle`、父 `initial{}` 或 `node{handle}`、原预约、原 `SnapshotNodeBinding`。按该绑定实际读 MANIFEST、files 及所需正文；每个原 `SnapshotEntry`（含空目录、路径拼写、FILE 选择）及不同 occurrence 保留。 |
| `instance(worktree_id)` | 全部仍须保留的实例：完整 `WorktreeHandle`、创建预约、原源节点 handle、已确定的 mount、`SparseWorktreeBinding`、`ledger::Binding` 与原决定选择的完整 `Checkpoint`。由该实例的真实 Genesis→Checkpoint 链重算修订、阶段及稀疏正文；已提交/不再可见不自动删除。没有原生终态证明不得消去义务。 |
| `pending(reserve_operation)` | 已知仍未决的原预约：原生 `Record` 的严格 kind、`Record::commitment`、原操作元组、其所属本仓库/分支/节点/实例的原 stable ID 及原预计完成操作。区分 `not_bound{}`（此**原决定**还没有绑定物理实体）与 `bound{binding}`（已有原生决定选定的原 node/instance/ledger 实体绑定）。区别不取决于目录是否存在。这一行不是可执行的恢复配方。 |
| `pending_item(reserve_operation,role,source_ordinal,item_ordinal)` | 仅原生预约的本地义务，按原顺序：`node_entry` 为原 `SnapshotEntry`，用原生成器重算预约的 `SnapshotReference`；`allocation` 为原生分配项；`edit` 为原编辑路径、FILE 材料、leaf/entity 与原编辑/完成操作；`file_input` 为原 FILE/历史、owner/root/entity、选定源/目标及原 branch/commit/tree；`file_result` 为原 `FileResult`、原 commit/tree；`checkpoint` 为原 ledger 及所选前/后 Checkpoint。一个角色只有对应的现有类型，不填伪空值。大批量按原路径/ordinal 展开，分配/结果数组逐项，不把整批或整份 OperationResult 塞进单页。未参与此预约的角色没有行；独立原生输入才能证明该角色为空。 |

原提交人、自由文本、认证资料、任意回执信封不另行复制。预约摘要是回指原决定的链接，
**不是**单盘重建完整请求、继续写入或证明正文的依据。只有已批准的原生阶段可投影；
新的未知角色或缺失原来源必须报不完整，不以兼容分支猜测。实际可读原节点/ledger 的规范字节仍在**已有位置**，不做第二份档案。

3. **首次预约不捏造仓库 inode。** 若原决定只分配了仓库/分支/节点，尚无一个已绑定的
   `.zxs` 仓库，允许逐盘 `inventory.object` 增加**一种**严格内部变体
   `directory_repository_reservation{repository_id,directory_id,reserve_operation,material}`。
   该行的 `entity` 明确是**已登记的普通宿主目录**，`location=anchor{root_id,directory_id}`，
   `ownership=original{registration}`，`operation_id` 是该原预约；生命周期为此预约所处的已批准原生待决态。
   它不声明目录仓库实体存在，不占用 backup/FILE/退役类型。通过上面物料验证已知的预约条目与
   预定 SnapshotReference，但只报告 Pending。首次真实节点完成后，同一次原生转换以
   `directory_repository{repository_id,directory_id,material}` 和自己的实际实体取代该 pending 行；
   旧前缀原样保留。已有仓库内的新预约用 `pending` 行，不增加逐盘对象类型。
   如果连已登记普通宿主也不能认证，则拒绝该源，不本地初始化。

4. **准备和冷读的边界。** 准备必须同时具备上一原始已验 scope/原生前缀、原实际写集、同一操作的
   before/after 原生输入及各变更宿主的 held 实体，预算通过后才在该登记盘准备不可变页，
   把唯一根接回既有 Candidate→Snapshot→DISK 清单。沿用原成员/法定票数/FINALIZE；根本身不是票。
   冷读先从独立选定的原前缀取得本盘 material，然后验完整页、登记/父目录/本盘 MANIFEST/ledger 链/
   正文和计数；同文件不同路径/实例不能合并。即使仅一盘可读，也只报告这一盘所选前缀的已验物料。
   无有效原决定时只能查看/报告，不能由当前文件自行组装权威。Pending、不可达、未绑定、未知或
   证明丢失都不等于空/退役；不把孤儿同名文件纳入，不凭较新的本地后缀续写。部分移动的判断
   只能用同一原操作的既有持票 FINALIZE/Publication 切点；未来两帧在决定前不声明已经存在。

5. **界限与反证。** 头 8 KiB、叶至多 128 行/64 KiB、元数据总包 64 MiB、盘数及各原生
   节点/ledger 原有限额不放宽。分解的是原来就可逐项核对的数组，不拆一个不合格单项以逃避上限。
   覆盖旧支线/同名不同实例、初始预约无 inode、4 MiB 多条预约清单、未决编辑/部分移动、删除
   任一页/路径/帧、同字节替换、串盘、换前缀、超预算、一盘隔离只读和改权威失败。上述是待实现的
   检查，不是测试通过声明。

**不在本决定内：** 另一份 SYSTEM/全池恢复、H05、密钥/口令副本、Native FILE 新 ID、Foreign 或 H07；
不改变公共 API、不迁移盘、不新增后台进程。不批准时保持明确缺口，不发布一个冒充完整仓库的根。
