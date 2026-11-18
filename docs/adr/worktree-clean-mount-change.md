# 已提交的根级 worktree：隐藏与原实例恢复

**状态：提案，待所有者明确确认。未实施、未验收。** 第一片只让一个已经完成
[同盘已有文件整批提交](worktree-existing-content-batch.md)的实例退出 SMB 挂载，
再让管理员找回**那个实例**。没有未保存或未提交的工作；不产生 FILE 版本、目录节点、
分支或新 WorktreeId，不移动／删除原快照、工作目录和正文。

[目录 worktree 行为决定](directory-history-worktree.md#smb-名字分配与恢复冲突)已决定
保留内容、原名优先、被占时选择安全替代名。[自动命名](worktree-automatic-names.md)
并未定义删除／恢复的生产挂载权威。现有 ledger 的 `Delete` / `Restored` 帧只能证明
本地生命周期，不能撤销或放开 SYSTEM 中一个名字。此次请求确认以下**唯一**新增契约。

## 入口与授权

- 管理 HTTP：`POST /api/directory-worktrees/{worktree}/mount-changes`。请求
  `MountChange { target: WorktreeHandle, expected: WorktreeRevision, operation_id: OperationId,
  action: Hide { committed: SnapshotHandle, visible: OperationId } | Restore { deletion: OperationId, naming: RestoreNaming } }`。
  `RestoreNaming = OriginalOrAutomatic | Explicit { name: Name }`。枚举用 `kind` 内标签、
  snake_case、未知字段拒绝；不接路径、物理地址、任意 parent 或客户端的“干净”判断。
  路由 WorktreeId 必须等于完整 target 中的实例 ID。隐藏要求与原仓库显式提交相同的写权限；
  还原只准当前有该仓库访问权的管理员。每次请求及最终写者处均复查权限。既有主动查询的
  `WorktreeStatus` 只加必需字段 `visible: OperationId`：首次是创建完成回执，
  此后是最近已完成恢复的**原请求号**。相同名字／内容修订并不复活旧挂载令牌。
- `GET /api/repositories/{repository}/directory-worktrees/mount-changes/{operation}` 返回
  `{request, outcome}`；outcome 严格为 `Pending {}`、`Hidden {}`、`Restored { name: Name }`，
  POST 完成也返回同一 DTO。只向原主体且当前仍获授权者返回其原操作；
  普通 SMB / 实例列表不泄露隐藏实例、挂起操作、主体或原名。恢复命令让获授权的另一位
  管理员凭**完整原 target 与准确 deletion**发起新操作，不得冒认原主体的历史收据。
  不增加隐藏实例的广域枚举、通用搜索或 SMB 删除命令。

## 唯一的两条内部记录

沿既有 `result.snapshot_worktree` 严格封套和原生固定成员发布器。不是本地状态机、
额外结算日志或全盘扫描。字段外不得补默认；u64 沿用规范十进制字符串。

| kind / Record 标签 | 必需字段 |
| --- | --- |
| `snapshot.worktree.mount-reserve` / `reserve_clean_mount` | `request: MountChange, content_reserve: OperationId, source: SnapshotHandle, committed: SnapshotHandle, host: HostEvidence, root: SnapshotHost, instance: SparseWorktreeBinding, ledger: Binding, before: Checkpoint, prior_mount: Mount, resolved: Hide {} / Restore { mount: Mount }, publish_operation: OperationId` |
| `snapshot.worktree.mount-publish` / `publish_clean_mount` | `reserve: OperationId, reservation: MaterialCommitment, checkpoint: Checkpoint` |

两条记录均为非 FILE 节点操作，versions、版本保护均空。`Mount` 是现有
`{parent: DirectoryId, name: LookupName}`，由准入的规范名字产生。原请求摘要为原生
reserve kind、原主体及严格 request；reserve 自身沿现有
`Record::commitment` 覆盖所有决定字段。publish 操作必须为已登记的 publish_operation，
摘要只用 publish kind、同一主体和 `{reserve,reservation}`。一个动作对应一个业务决定，
前置 reserve 只有占用语义；二者不能被本地帧替代。

## 准入、安装与重试

1. 同一写者下先查原操作、主体、完整请求；再从已独立准入的实物／原收据确认：
   Initial 普通磁盘同一根、同一注册代际和父目录；原创建完成的 WorktreeHandle、实例
   与 ledger inode；原 `content_reserve` 与唯一实际 `content-publish`、准确
   committed node 及完整 FILE 结果。隐藏只接受 Active、saved 为这个 Directory
   publication 且其 revision 等于 expected，当前可见挂载就是请求的 visible，
   当前 ledger 无新的编辑、文件／命名空间
   变化及挂起操作。恢复只接受同一实例精确的 Deleted、deletion 来自已完成隐藏；
   同一已恢复或另一次删除不能冒用该 deletion。重试已完成的原操作先按原结果返回，
   不因随后同名占用或再次隐藏而建立新动作。
2. 隐藏 reserve 继续占住现挂载名，恢复 reserve 在相同根父预留唯一确切目标名。
   自动优先这次原隐藏回执的 prior_mount 原名；若按规范键冲突，在创建时原分支
   显示名的 001–999 中取最低空号。
   显式名冲突立即拒绝。普通文件／目录、全部其他实例和持久未决名字均占名；
   核对目标设备真实组件、完整可服务 SMB 路径和嵌套内容预算。超长／耗尽／离线/
   主体无权不建 reserve、不覆盖、不缩短。不另建物理同名目录或 SMB 链接。
3. 用原 `before` 和原请求号预编码**一个现有帧**：隐藏
   `Event::Delete { operation: request.operation_id, expected }`（实际 `prepare_delete`
   必须返回 Retain）；恢复 `Event::Restored { operation: request.operation_id, mount: resolved.mount }`。
   预留一个真实 ledger 帧的字节／帧数和 native 对象、各盘 SQLite / DISK 封闭预算。
   唯一业务 Candidate 绑定这份原 reserve、预定 checkpoint、原注册下的实例材料及
   prospective 可见挂载；FILE／目录历史保持逐表不变。只有原生 FINALIZE 才可在准确
   ledger 父／实例下安装该帧、同步并读验。只承认准确 before 或已安装的准确规范
   字节，不覆盖另一帧。随后才写 PUBLICATION/seal、暴露新可见关系并释放旧名字。
   列举、打开及每次旧句柄 I/O 都核对这一具体挂载的创建／恢复回执，不只比较未变的
   WorktreeId 或内容 generation。隐藏完成前不得提前释放占名；从冻结起不给旧句柄
   新的 I/O，恢复后亦须重开；不宣称能撤回已交付的字节。同名新建仍得全新实例。
4. 冷起只按原槽与原注册重开，验证原源节点、完成节点、实例／ledger 父和实体、
   先前可见状态、固定成员及原 scope。缺盘、少证明、名字归其他实例、装了另外一帧
   一概保留原意图并拒绝服务，不写“补齐”帧。既有 DISK/SYSTEM scoped 源是同一次
   native 决定，不新增另一发布者。管理页只显示已获准的具体原操作结果。
5. 原 #1 完成帧及每次隐藏／恢复都是同一 ledger 上的**历史位置**。核旧操作须
   按准确序号、前驱承诺及原字节证实其前缀；后继帧只接受同一实例、同一代际、
   由后继原生槽的确定决定逐项授权的准确字节与次序。冷起先只读核这些后继
   的原生证据，绝不见到额外帧便把它当作更新的权威。没有逐项证明的多余尾帧仍拒绝。

**后续验收：** 同节点两个真实实例；隐藏后普通列举和先前句柄不再暴露，正常新建只读原快照；
原名空闲恢复原名、被占自动换号、显式冲突与 999 耗尽不覆盖；在 reserve、原生
PREPARE/ACCEPT/FINALIZE、帧同步、回执丢失处冷重开，原 FILE/目录历史数量与内容不变。

**不批准：** 未经目录提交或有新脏项时删除（仍须独立决定 FilesBeforeDelete 与逐 FILE
收据，绝不丢稿）、新增文件/历史分叉、跨盘／deleted-data／物理退役、移动父目录、
通用重命名、Current、物理清理、后台扫描或新权威索引。本片拒绝不算完成这些事项。
