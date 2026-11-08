# 跨盘移动的单一原生操作准备

本片对齐 `main 39404157`，承接 `1b3fba43` 的[共同决定输入](file-history-move-decision-inputs.md)，新增一个真实持久化及冷重建切片：用一个 `file.history.move` 操作，表达目标完整历史导入和源完整 frontier D 的共同归属。

**仍未决定、未发布，也不是成功收据。** 公开 API 返回 `FileHistoryMoveNativePreparation` 观察报告，不返回 `OperationResult`。内部原生操作模板明确使用 `STAGING`、`NOT_APPLICABLE` 和 `UNKNOWN`，不预填 `PUBLISHED`、`TARGET_MET` 或健康成功。没有 PREPARE/ACCEPT、源 SMB 隐藏或目标在线编辑。

## 一次操作，两端归属

原投影已经固定目标全部独立身份和源 D。此片增加严格字段集的原生操作体：准确 move 请求、原稳定源 file ID、原 sequence/记录摘要、冻结 route/时钟、全部版本和分支映射、目标 commit/branch/history、完整旧审计，以及原恢复与共同输入 digest。

模板 State 只在原完整拟议 State 上新增：

- 一项 `file.history.move` operation；
- 同一 operation 的原 principal；
- 一项同 sequence、同 operation、指向目标 arrival commit 的事件。

它不会伪造两个 operation，不借用 copy 的 `history_copy_request` 作为 move 请求，不修改原历史永久收据。完整源 D 在这个原生操作的 `deletion_node` 中；映射后的历史 D/undo 在同一操作的 `retained_lifecycle` 中。源 D 不变成目标活动删除，源历史及物理文件仍保留。

owner 派生索引现在能够将这一操作分别归到源文件的 deletion membership 和目标文件的 retained-history membership。FILE 派生读取器据此分别提取源的原生 D 审计、目标的完整旧审计。**这只是派生归属，不是操作准入授权。** 普通已发布 State 的删除收据检查、冷态完整 State 准入和当前 registry 的允许操作集合均没有被放宽。

## 用同一原生操作重建两个文件库

构造原生模板后，不再使用专门的“没有收据”源/目标 FILE 投影，改为从这个私有模板 State 的普通 receipt-owner 路径独立生成两份逻辑记录。两份 records hash 必须分别等于已经在所有参与根独立读验的源 D 后 FILE SQLite 和目标 FILE SQLite。

这将两端审计归属与原有真实文件材料连接起来：漏掉源 D、把源 D 导入目标、漏掉旧 undo、弄错任一 file/history/operation，都不能得到同一套合法记录。没有新建正文或修改源容器。

`version_protection` 记录的是本次已经重新读验的全部原目标路线材料观察，仍不构成发布时间保护事实。模板 header 的 `NOT_APPLICABLE` 指向尚未发生的发布；之后真正的 Candidate/PUBLICATION 必须重新建立所需证明，不能把模板字段或准备报告复用为成功证据。

## 实际保存与原操作冷重建

`prepare_file_history_move_native_operation` 首先验证原恢复副本、所有目标 FULL 及 inode/marker、全部共同决定输入及两端文件库。缺任一输入时不自动创建原封套、不补目标复制，也不跳过共同输入阶段。

在原 `file-history-move-decision-inputs/` 下追加：

- `native-operation.json`：明确标为 `file-history-move-unaccepted-native-operation` 的原生 STAGING 模板和两个 FILE 逻辑摘要，保存到系统、全部元数据和原源/目标路线盘。
- `native-state.json`：原完整拟议 State 加上这一项原生操作/principal/event，仅保存到系统和原元数据成员，不下放仅正文盘。

原 `inputs.json`、`unaccepted-state.json` 和原 digest 不改写，不进行格式升级或兼容替换。native digest 共同绑定两份规范字节。没有字段将原准备槽标成已发布。

所有写入沿用持有根、实际磁盘 seal/I/O gate、准确原 manifest、无覆盖原子写、文件与父目录同步。配置容量纳入已知目标 FULL、原决定输入及新增 native 文件；继续保留物理元数据 reserve。已有文件逐字核对与同步，缺失文件只在同一原 manifest 下显式补齐，损坏或不同内容原样保留。没有自动清槽。

`read_file_history_move_native_operation` 是完整只读入口。冷进程先按原恢复封套恢复投影，再独立重新构造同一原生操作，比较所有参与根的准确文件，最后再次验证共同输入和全部目标 FULL。它不从存盘模板安装 State，不信任自摘要、已保存的 STAGING 字段或先前进程的观察。

## 不能进入普通发布

通用 `prepare_publication` 的第一步新增原生 move 硬围栏。在任何 payload/name I/O 之前，拒绝：

- 结果 kind 声称是 native move；
- 新增或更改 native move State 操作，即使结果换成别的 kind；
- 将已有 native 操作改名或删除以绕过判断。

该围栏不是完整执行器，不能被模板内容解除。只有后续真正建立私有三范围数据库/原子恢复准入能力后，才能替换为具体证明消费点。当前既有软删除的全写集验证、当前 registry 准入和冷态源 D 收据验证继续拒绝这个未闭合模板。

**仍缺**系统与盘根完整历史数据库、统一永久成功收据的实际协议准入、唯一 Candidate/N 成员共同决定及原决定恢复、双端读取围栏。报告保留原五项未闭合义务；这片只推进其中“一操作、双端审计、精确重建”的实现，不把 STAGING 当永久结果，不开放跨盘 SMB rename。

## 资源与验证状态

native 操作和 State 各受 12 MiB 编码预算，State 在规范化分配前计数；两份 FILE 继续使用已有有界完整行验证器。按根顺序保存，成本随完整 State、原文件历史、目标版本/路线和重复完整读验增长，不声称恒定复杂度或吞吐改进。

新增八项根集成测试源码与三项发布围栏单元测试源码：单 operation/principal/event、完整 State 范围隔离、旧 D/undo 的双端归属、两种配置冷重建、缺一副本的只读拒绝与显式续作、篡改模板保留、原输入/取消前置、来源变化/目标损坏，以及直接通用发布、遗漏 State 行、改名/删除绕过拒绝。另将上一片测试中的开发盘位误写 `B02` 改为实际配置的 `B01`；不改变生产盘位规则。

按用户要求未进行代码审查、编译、格式化、测试或 CI。以上是生产与测试源码范围，不是执行通过、真实 SMB、断电恢复或完整原子 move 验收。

## 后继 scoped 数据库准备

[原完整时间线与未接受数据库](file-history-move-scoped-databases.md)增加实际历史 SYSTEM 索引、参与盘原 current/history 和同一 native 提议 current，按原成员全量读验及冷续作。它不更改本文件定义的 native 字节，也不把 STAGING 升为成功；共同决定、原子普通库安装和双端读取围栏仍缺，五项义务保持。
