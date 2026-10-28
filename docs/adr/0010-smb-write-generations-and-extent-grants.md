# 0010：SMB 写入准入分代与有界额度授予

状态：**已采纳，当前源码设计。** 本文只改变 SMB 私有副本的单次写入准入、`Dirty` 发送频率和预算高水位增长步长；不改变核心发布阶段、提交屏障、`Flush`/`Close` 保存回执、恢复记录格式、同记录免重写复用或 HTTP spool。性能结论只以[验证记录](../validation/smb-write-admission.md)中的实测为准。

## 变更前调用链（基线 `39fbb997`）

VFS 的 `pwrite`、`pwrite_send`、`ftruncate` 都经 `write_data`/`truncate` 进入同一序列；每次写入都执行下表全部步骤。

| 步骤 | 位置 | 每次写入的固定成本 |
| --- | --- | --- |
| `PrepareWrite` RPC | [`module.rs`](../../src/smb-vfs/src/module.rs) → [`session.rs`](../../src/server/src/smb_rpc/session.rs) | 一次往返；`authorize_workspace_selection`（引擎锁）；`fstat`；`statvfs` |
| 高水位增长 | [`budget.rs`](../../src/server/src/smb_rpc/budget.rs) `Lease::before_write` → [`budget/persistence.rs`](../../src/server/src/smb_rpc/budget/persistence.rs) `grow` | 只要写入末端超过已获准末端：临时文件写入、文件 fsync、rename、目录 fsync；顺序追加时**每次写入都触发** |
| 修改 FD | VFS | `pwrite`／`ftruncate` |
| `Dirty` RPC | [`publication.rs`](../../src/server/src/smb_rpc/publication.rs) `Session::dirty` | 一次往返；恢复记录比较（同记录复用时为 `fstat`、两次身份核验、≤16 KiB 读）；`authorize_current`、`authorize_workspace_selection`、未发布代的 `authorize_upload`（各一次引擎锁）；`fstat` |

已完成、本文不重做的优化：[`recovery_record.rs`](../../src/server/src/smb_rpc/recovery_record.rs) 的 `ActiveRecord` 在同一活跃 writer 内对字节完全相同、持久身份未变的恢复记录免重写（见 [SMB 组件](../components/smb.md)）。它把同代 N 次 `Dirty` 的记录屏障降为一次，但每次 `Dirty` 仍做身份核验，预算增长仍逐次持久化。

## PrepareWrite 与 Dirty 的职责核对

| 职责 | PrepareWrite（修改前） | Dirty（修改后） | 结论 |
| --- | --- | --- | --- |
| 当前身份、工作区归属、目录／文件选择 | 有 | 有 | 每次修改前必须成立，保留在 PrepareWrite |
| 分支当前准入 `authorize_current` | 无 | 有 | 移入 PrepareWrite，修改前检查 |
| 工作区仍 Open 且租约未过期 `authorize_upload` | 无 | 未发布代有 | 未发布代移入 PrepareWrite；已发布代由随后的 Dirty 建立并授权新工作区 |
| 字节保留：末端 ≤ 句柄上限、账户／总额度、高水位持久化 | 有 | 无 | 保留，改为有界额度授予（下文） |
| 实际空间风险：`statvfs` 覆盖本次字节加恢复余量 | 有，每次 | 无 | **每次写入保留**，不因长度未增长而省略 |
| 标记 dirty 代、写恢复记录 | 无 | 有 | Dirty 只在代际转换时需要 |
| 已发布句柄切换到新工作区与新操作号 | 无 | 有 | 留在 Dirty |
| write-through 同步保存 | 无 | 有 | 留在 Dirty，每次写入都发送 |
| 文件长度 ≤ 上限 | VFS 预检＋末端检查 | `fstat` | Flush 发布前仍复核；Dirty 内检查不变 |

服务端的 dirty 状态只有 `finish_publication`（仅由 Flush／write-through／Close 的保存路径进入）会清除，只有 Dirty 会重新置位并在已发布时换代。因此“打开后或上一次 Flush 后已有一次成功的非 write-through Dirty”这一事实，在下一次 Flush 之前一直等价于服务端 writer 处于 `dirty && !committed`。

## 决策

1. **PrepareWrite 是每次修改的唯一准入点。** 依次执行 `authorize_current`、`authorize_workspace_selection`、未发布代的 `authorize_upload`，再做字节保留与空间检查，全部在修改 FD 之前完成。撤权、工作区关闭和租约过期在修改前拒绝，而不是修改后才在 Dirty 中发现。
2. **Dirty 只表达代际转换。** 客户端 `smb_protocol::client::WriteSequence` 按句柄维护“当前代已通告”状态：打开后与每次发送 Flush 前清除；成功的非 write-through Dirty 之后置位；Dirty 失败或结果未知时保持清除，下一次修改再次发送。write-through 句柄每次修改都发送 Dirty。服务端 Dirty 的授权、记录、换代和 write-through 保存逻辑不变，重复收到也保持幂等。
3. **有界额度授予。** 写入末端超过已获准末端时，服务端一次授予 `ceiling = end + slack`：

   `slack = min(clamp(end / 4, 1 MiB, 64 MiB), max_length − end, 活跃租约剩余授予池)`，进程内活跃租约共用的授予池为 `min(1 GiB, bytes_total / 64)`。

   先用 `ceiling` 做账户、总量及“空闲空间 ≥ 全部获准额度 + 恢复余量”的准入；只有额度或空间不足时退回 `end` 准入；账本或算术错误保留原错误并拒绝，不能用较小额度掩盖。`ceiling` 在回复 `ready` 之前按原屏障持久化（临时文件、文件 fsync、rename、目录 fsync）；`ceiling` 以内的后续写入不再增长或持久化预算记录。
4. **能力位。** 握手新增必需能力 `generation-dirty-writes`。缺少该能力的旧 VFS 或旧服务端在建立会话前被拒绝，不提供逐写 Dirty 与分代 Dirty 的混合兼容路径。

不合并为单个 RPC：保留必须先于修改，dirty 代必须在字节确实改变之后建立（写入失败不能制造空代，write-through 保存必须看到本次字节）。把两者合成“修改前预标记 dirty”会让失败写入也触发一次保存或换代。

## 额度的持有者与生命周期

| 对象 | 持有者 | 计入 | 何时释放 |
| --- | --- | --- | --- |
| 未用额度 `ceiling − 已写末端` | `*.budget.json` 的 `admitted_end`（持久）与账本中该 source 的 `Charge`（内存）；主体为写句柄所属账户 | 账户额度、总量、准入时的空闲空间检查；存活租约另计入内存授予池 | 显式成功 `Close`/`Abort` 清理工作文件、恢复记录与预算记录后；Flush、断连、重启都不释放 |
| 实际暂存字节 | spool 中的私有副本文件；其恢复关联由 `*.work.json` 持有 | 存活时被已获准末端覆盖；重启盘点按 `max(admitted_end, 物理长度)` 计费 | 同上；失败清理保留证据与计费 |
| 授予池占用 | 存活 `Lease` 的最近一次授予 | 只限制活跃租约的授予份额合计，不是所有保留副本未用额度的合计上限 | `Lease` 析构（关闭、脱离、断连）时减去；重启后为空 |

崩溃重建：预算记录只增不减，`admitted_end` 总是 ≥ 服务端曾准许的任何写入末端，所以重启盘点仍按记录与物理长度的较大者计费，未用额度保守计入直到显式清理。授予池是纯内存的分配上限，丢失只会让重启后的新授予从空池开始，不影响已有计费。持久化失败或展开沿用 `Pending` 围栏：已计入的 `ceiling` 保留，封闭后续准入。

反复脱离、断连或重启可以使保留副本的未用额度合计超过 1 GiB；它们仍全部占用账户／总量预算，后续授予必须连同这些保留计费一起准入，不能靠归还活跃池绕过容量上限。这是保守计费的可用性代价：少量实际字节也可能占住较大额度，直到核对和显式清理；不是额外额度在断连时被回收，也不是文件系统针对恶意 FD 持有者的强制配额。

## 保持不变的保证

- Flush 与最终 Close 仍在完整上传及核心事务提交后才返回成功；Close 不改为后台持久化；不提前返回成功。
- 每次写入前仍检查末端上限与实际空间风险（`statvfs` 覆盖本次字节与恢复余量）；覆盖写、稀疏写不因文件长度不增长而跳过。
- 预算增长仍在许可修改 FD 之前完成全部持久化屏障；Flush 不回收额度。
- 恢复记录格式、`descriptor_changes_unconfirmed: true`、同记录复用、断连保守保留、丢回执核对与永久操作收据不变。
- 核心发布阶段与屏障（任务 3 范围）未修改。

## 代价

- 每次增长的提前额度为 `min(clamp(end/4, 1 MiB, 64 MiB), max_length − end, 活跃池剩余额度)`。小文件也可能获得 1 MiB（例如末端为 1 字节且其余限制充足时），不能把成本写成至多末端的 25%。额度内写入不降低持久高水位，截短也不回收；崩溃或脱离后仍随记录计费，直到人工核对和清理。
- PrepareWrite 每次多两次引擎锁内的内存检查。
- 撤权或工作区关闭现在在修改前拒绝。旧实现下修改已经写入 FD、随后 Dirty 才失败；新实现下该次修改不执行。
- VFS 每个打开记录的 I/O 互斥锁改为保护 `WriteSequence` 状态，不另加锁。

## 不采用的方案

| 方案 | 不采用原因 |
| --- | --- |
| VFS 缓存物理字节额度，额度内写入跳过 PrepareWrite | 把空间风险检查和撤权线性化点从每次修改移到授予时刻；外部进程在更长窗口内竞争同一文件系统；服务端崩溃后 VFS 仍可凭本地额度修改 FD |
| 文件长度不增长时跳过空间检查 | 覆盖写在稀疏区或写时复制文件系统上仍分配块，硬约束禁止 |
| Flush 时把高水位收回到实际长度 | Samba 仍持可写 FD，已获准的在途写入可能完成；与“Flush 不是清理”冲突 |
| Close 先回复再后台持久化 | 违反保存回执语义，硬约束禁止 |
| 固定大步长或按句柄静态上限预留 | 小文件预占过多账户额度；本方案按末端比例增长并受全局池约束 |

## 验收设计

- 协议单测（`test/smb/unit/client/write_sequence.rs`）：首次修改发送 Dirty，同代后续修改不发送；Flush 前清除且 Flush 失败也清除；Dirty 失败后下次重发；write-through 每次发送；零字节写不发送；PrepareWrite 失败时不修改。
- 预算单测（`budget_tests.rs`）：授予公式、池上限、额度不足时退回精确末端、授予在回复前持久化、重启按记录计费、关闭释放池、失败围栏保留授予。
- 会话回归（`write_generation_tests.rs`）：撤权与工作区关闭在 PrepareWrite 拒绝且 FD 未修改；同代省略 Dirty 后 Flush/Close 仍发布全部字节；Flush 后修改重新开启新代；write-through 每次保存。
- 计数：`WriteSequence` 直接调用同进程 `Session::dispatch` 和真实核心引擎／spool，不经过 socketpair、RPC 编解码或 Samba。按 64 KiB 顺序写统计每 GiB 的请求次数（按类型）、预算记录写入与恢复记录替换；文件／目录同步及 Flush／Close 延迟的方法和历史证据限制见验证记录。请求次数可描述协议序列变化，但进程内耗时不是实际 RPC 或客户端吞吐。所述基线为 `39fbb997` 上未提交的旧 VFS 序列测量补丁，交接不含该补丁与原始日志。

## 关联规则

[编码规范 §2、§6、§7、§11](../../禁止防御性编程.md)；[事务与恢复](../10-transactions-and-recovery.md)；[写入语义](../11-write-semantics.md)；[SMB 组件](../components/smb.md)；[ADR 0008](0008-workspace-lifecycle-separation.md) 保留的 PrepareWrite/Dirty 授权承诺在本文中前移到修改之前，没有削弱。
