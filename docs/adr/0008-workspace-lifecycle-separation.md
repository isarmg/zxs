# 0008：工作区生命周期与永久操作事实分离

状态：**提案（PROPOSED），尚未采纳或实现。** 本文不改变当前配额、接口、持久化格式、成功返回或恢复规则，也不表示验证已经通过。具体格式与实施范围须在下列准入问题解决后另行评审。

## 问题与当前依据

当前 `max_workspaces` 只限制 Open/Sealed；另设的 retained 工作区记录和逻辑元数据预算则覆盖所有状态。默认每主体 1024 条是累计生命周期上限，不是并发上限；Committed、Aborted 和过期记录重启后仍计费。该限制已在[容量规范](../19-capacity-and-performance.md#retained-workspace-admission)说明，并由 `aborted_records_consume_global_and_principal_budgets_after_restart` 和 `capacity_reports_retained_lifecycle_counts_and_only_selected_principal` 固定为当前行为，不能把现有测试描述成退休功能的验证。

普通 SMB 可写打开已有文件立即创建核心工作区，同时把原正文复制到独立 SMB spool。未修改时 Flush 返回 unchanged，Close 随后中止核心工作区；成功清理 spool 并不会释放核心 retained 记录额度。因此顺序执行正常打开/关闭也可能耗尽额度，不需要并发持有大量句柄。

主要源码边界：

| 边界 | 当前实现 |
| --- | --- |
| 状态、默认配额、永久操作记录 | [`model.rs`](../../src/contract/src/model.rs) 的 `Config`、`Workspace`、`State.operations` / `operation_principals` |
| 活动计数、创建、中止、提交完成 | [`business/workspace.rs`](../../src/engine/src/business/workspace.rs) 的 `create_workspace`、`abort_workspace`、`complete_workspace_commit` |
| 跨状态计费、双镜像和启动盘点 | [`workspace_budget.rs`](../../src/engine/src/engine/workspace_budget.rs)、[`workspace_storage.rs`](../../src/engine/src/engine/workspace_storage.rs) |
| SMB 工作副本、关闭与预算所有者 | [`handles.rs`](../../src/server/src/smb_rpc/handles.rs)、[`publication.rs`](../../src/server/src/smb_rpc/publication.rs)、[`budget.rs`](../../src/server/src/smb_rpc/budget.rs) |
| 原请求重放与恢复源 | [`workspace_commit.rs`](../../src/engine/src/engine/workspace_commit.rs)、[`wire.rs`](../../src/engine/src/wire.rs)、[`repair.rs`](../../src/engine/src/repair.rs) |

## 建议方向与不变项

建议区分三种生命周期，而不是取消资源上限：

1. **工作集**：仍可修改的清单、未决提交、在途准备与上传、活动绑定，以及尚未核对完的恢复材料。工作记录数、内存和准备资源继续有界。
2. **终态证据与保留正文**：工作区关闭身份、fence、原请求绑定和需要保留的完整上传代际。满足明确条件后可退出工作集，但仍可查询、核验和计入实际存储预算。
3. **永久发布事实**：原 operation ID、actor、请求摘要、结果及发布证明。继续由现有 operations 与发布协议维护，不裁剪、不设 TTL，不因工作区中止或退休而撤销。

工作区创建和中止已有自己的双镜像生命周期持久化，并不要求新增全局业务提交。**不建议为每次无修改 Close 增加永久 retirement operation、业务序号或全局发布闭包。** 退休应扩展同一工作区权威；SQLite、缓存、扫描索引或另一个日志不能成为竞争的提交权威。

原 native/public 摘要契约分别保留。native replay 当前会读取工作区、重算冻结文件/命名空间摘要，并要求确切的 `commit-request.json`；public replay 还核对原 envelope 与 workspace/repository/branch/head/fence/message 绑定。仅保留 operation ID 和结果不足以替代这些材料。

Committed→Aborted 是已有关闭语义，不是撤销发布。`closing_a_published_workspace_does_not_reopen_its_old_fence_on_restart` 明确覆盖该行为；本提案不将它改判为缺陷。发布后工作区镜像持久化失败也仍是独立错误，不能借退休设计或 SQLite 投影降级吞掉。

## 可评审的有限首批：未提交的空 Aborted

首批候选仅为明确完成正常关闭、没有核心暂存和提交关联的普通工作区；**不包含一般 Committed、历史保存工作区或所有看起来为空的 Aborted**。

核心在独占访问下核对当前 actor、ID、预期 fence、准确清单和镜像身份；要求普通 purpose、双镜像一致且已持久 Aborted、files 与 namespace operations 为空。还须盘点两个工作区目录：没有 `commit-request.json`、`wire-request.json`、`.full`、`.prepared`、`.zxs-full-*` 或未知材料。另行核对没有历史保存 plan 关联、目标 upload claim、detached commit preparation 或未决决定；复用现有准备准入与写入屏障，不以超时推断结束。缺失、损坏和 I/O 失败仍分别处理。

“核心为空”不等于“没有用户字节”：已有文件的 SMB 工作副本通常非空。当前 `cleanup_writer` 仅记录清理告警，`Lease::Drop` 在残留证据或账本不确定时保留计费，二者不能作为成功退休证明。拟议闭合边界应消费原 Writer 的所有权，使用其已持有目录、source 身份和不可克隆预算 Lease，显式完成既有成功 Close 的清理、父目录同步与预算结算；只有可返回结果的完成操作成功，才能向 Backend 交付绑定该 workspace/actor/fence 的私有关闭结果。不能新增由调用方随意填写的 `safe_to_retire` 布尔值。

失败关闭、清理告警、Detach、disconnect、尚存 reservation 或不确定账本均不得触发退休。SMB 层证明自己的资源闭合，核心层证明工作区条件；不让核心猜测外部 spool 所有权，也不复制两份预算规则。关闭结果如何在原工作区生命周期中持久表达并在崩溃后核对，是首批落地前的必答项，不能只依赖内存中的成功标记。

### 原清单归档候选与崩溃顺序

首批可先保留原 `workspace.json` 字节，不立即设计紧凑格式。候选布局是在系统盘和原 primary 各自区分 active/retired 命名空间，例如 `workspaces/<id>` 与 `retired-workspaces/<id>`、`staging/workspaces/<id>` 与 `staging/retired-workspaces/<id>`。这些是待评审的存储类示意，不是本文发布的新磁盘格式。

在关闭证明与上述准入成立的前提下，拟议顺序为：

1. 先完成原有 Aborted 双镜像持久化和明确的 SMB 清理完成边界，再回到核心核对准确基线；工作区镜像此时仍在原位置。
2. 持久创建所需归档父目录；使用已持有的安全目录身份，对一个镜像执行同文件系统、不可覆盖的目录 rename，并同步源、目标父目录。
3. 对另一镜像执行同样操作和同步。
4. 只有两边均完成，才从工作集移出并释放工作记录额度。失败保留材料与计费；关闭、退休和业务发布的结果必须分别表达，不伪造新的未知业务提交。

| 重启发现 | 必须保持的处理 |
| --- | --- |
| 两边均 active | 尚未退休，继续计费 |
| 一边 active、一边 retired | 仅在同一身份、准确终态清单及关闭证据一致时识别为未完成移动；计一次工作额度，不能提前释放 |
| 两边均 retired | 核验闭合与镜像后重建退休状态，不重复扣减 |
| 同一镜像位置重复、内容冲突、未知材料或读取失败 | 保留并明确要求核对；不选最新、第一份或较大 fence |

恢复是否继续完成移动须有显式、可重试的所有者；只读检查不能隐式改盘。缺盘和单镜像损坏沿用原有恢复准入，不降低成员要求。此候选增加 rename 和目录同步成本，不宣称减少同步次数。首批不移动含核心暂存正文的目录，不改绝对 `source_path`，不删除原工作区镜像证据。

## 退休后查询不能退化为“找不到”

当前 `workspace_ref` 返回活动 BTreeMap 的借用。移出工作集时，workspace 查询、Backend 授权、selection 检查、upload-status 与 native/public replay 必须同步改为能够取得经过核验的退休记录。候选方式是活动记录借用、退休记录受限点读并返回 owned/leased snapshot；不能为保持借用接口而建立永久增长的全量缓存。

重复 abort 当前仍会推进 fence 并持久化。若首批保留此行为，存储和安装路径必须识别退休存储类，更新退休镜像及其字节计费，不重新创建 active 目录或把记录插回工作集。把重复 abort 改成不推进 fence 的成功返回属于另一个行为改变，不在首批默认采用。

已发布 SMB 句柄在下一次 Dirty 前仍使用原工作区的选择上下文。一般 committed 退休不得删除唯一绑定材料；在途 workspace 内容读取和上传还持有 inode、租约与准备资源。首批的完整清单点读不能被当作这些后续问题已经解决。

## 有界盘点与字节预算

当前扫描在 decode 前按全部发现 ID 拒绝超额，并先 seed workspace 再恢复 journal。新设计必须把**工作集准入上限、单次扫描资源预算、归档库存**分开；不能只在计费函数中跳过终态。

拟议盘点按有限窗口读取和聚合每个 ID 的镜像，限制单清单与临时内存，分类后才计入工作集。大量可退休或已退休记录不应因旧的路径数量上限阻止核对，也不能一次装入无界 BTreeMap。完整盘点可分批继续，并明确尚未完成；达到一批预算不是库存完整的证明。退休点查、盘点与恢复源发现均须验证实际材料，不将派生索引当作权威。

已有累计超额池及系统盘恢复需要可用的受限只读库存/定点恢复入口：不能要求先将全部记录装入超额 Engine，才允许查看和处理。未满足闭合证明的旧记录仍属未退休；不能自动把所有旧空 Aborted 判为可释放。库存未核清或未退休工作集超额时保持写入拒绝，但保留诊断与核对能力，不靠修改配置锚、删除证据或降低恢复验证启动。

归档不等于免费存储。保留原清单时，`selected_*` 等字段仍可能很大，磁盘字节和枚举成本仍随历史增长。首批若仅释放工作记录名额、仍受既有累计元数据字节限制，必须明确它只改善记录耗尽；若分离归档预算，则须先定义并实现实际字节库存、准入和恢复余量，不能取消检查或只等待 ENOSPC。本文不指定新的配置数值。

完整方向需要紧凑终态事实与保留正文分别计费；仅增加 retired 目录或提高上限不是长期增长问题的完整解法。

## 落地前必须解决的问题

- 首批关闭证明的持久关联、来源所有者和重启核对方式；清理成功但退休失败的结果表达与重试归属。
- 有界库存与原 seed/recovery 顺序如何衔接；单镜像、部分移动、超额池和系统盘丢失均不能漏掉修复来源。
- 归档字节预算与工作额度的含义、诊断投影及结算失败顺序；不能通过换分类抹去资源使用。
- 一般 committed 记录保留哪些 native/public 冻结输入、精确请求与发布关联；历史保存如何绑定 Completed plan。
- SMB 已发布句柄选择上下文、读租约和上传准备的终点；旧 `.full` 代际仍可能是 `pending_payload_source` 的修复来源。
- 紧凑终态若不再包含完整 Workspace，现有查询字段如何投影；正文位置与读取契约如何保持。改变查询内容或正文保留政策需要明确决策，不能借内部压缩默认实施。

这些是实现准入问题，不是本提案已选定的新接口或格式。保留现有本地依赖策略、根 `test/` 布局、SQLite FULL 与永久收据规则，不增加旧格式兼容、迁移或 TTL。

## 必须提供的验证

现有回归提供起点，不表示退休实现已通过：

- [`retained_workspace_budget.rs`](../../test/engine/unit/retained_workspace_budget.rs)：上述跨重启计费测试及 `committed_file_context_receipt_fits_exact_reserved_metadata_budget`。
- [`smb_rpc/tests.rs`](../../test/server/unit/smb_rpc/tests.rs)：`unchanged_writable_close_cleans_up_but_disconnect_remains_uncertain`；它当前不验证核心额度归还。
- [`workspace_commit_replay.rs`](../../test/engine/integration/workspace_commit_replay.rs)：`native_commit_replay_survives_system_disk_restore`、`native_commit_replay_uses_only_exact_bound_request_copies`、`wire_replay_checks_request_message_and_fencing_against_body` 及已发布后关闭的 fence 测试。
- [`published_selection_tests.rs`](../../test/server/unit/smb_rpc/published_selection_tests.rs)：`published_handles_can_write_again_without_selection_change`。
- [`repair/tests.rs`](../../test/engine/unit/repair/tests.rs)：`recovery_uses_retained_workspace_bytes_when_all_candidate_replicas_are_lost`。

新增确定性回归应在小额度夹具中重复已有文件的无修改可写打开/关闭，核对工作额度可再次使用、业务 sequence 与版本不增加、原关闭身份可查、重复 abort 不重入工作集。分别在 spool 清理、预算结算、每个 rename 与父目录同步、运行期额度安装前后中断并重启；覆盖 cleanup warning、Detach、disconnect、未决提交、在途上传、镜像冲突与超额只读盘点。后续 committed 退休还必须验证原 operation、摘要、结果和重放不变，以及全部保留正文的恢复可达性。

所有测试继续放根 `test/` 并接入实际目标；源码阅读、编译、模拟文件故障、真实 SMB 客户端和物理掉电验证分别报告，不互相替代。

关联：[写入语义](../11-write-semantics.md)、[事务恢复](../10-transactions-and-recovery.md)、[容量规范](../19-capacity-and-performance.md)、[SMB](../components/smb.md)、[脱离锁的工作区准备](../architecture/detached-workspace-commits.md)。
