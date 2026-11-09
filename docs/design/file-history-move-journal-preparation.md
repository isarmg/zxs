# 跨盘移动的原候选 PREPARE 与冷续作

[任务清单](../../TASKS.md) · [普通正文与原安装对](file-history-move-ordinary-preparation.md) · [唯一选择](file-history-move-joint-choice.md) · [单一收据生命周期](file-history-move-receipt-lifecycle.md)

> 以下记录 PREPARE-only 切片的历史边界。后继已将专用执行/冷恢复源码接到普通决定，见[最小执行路径](file-history-move-execution.md)；准备 API 本身仍不授予成功。后继未编译/测试，不能沿用这里的“拒绝所有 ACCEPT/torn tail”作为新执行协议。

原候选 `f1d77559` 基于 `24f0c31b`；本片对齐准确主线 `4a8be030`，保留新目录库恢复准入与原 `2a01b0e8`、`23621d9b` 及已发布的普通准备、冷 D 生命周期源码。新增的是**原生普通日志中的 PREPARE**，不再只记录准备侧车；仍不写 ACCEPT、FINALIZE 或 PUBLICATION。PREPARE 不是多数决定，不安装活动数据库，不发布源 D，不显露目标 namespace，不解除 choice，更不能返回 SMB rename 成功。

## 只有同一原候选可以进入日志

`prepare_file_history_move_journal` 不接收任意 Candidate、路径、快照或完成标记。调用方必须持有同一原 projection 和恢复摘要；入口从原 source State、原 choice、原时间及 operation 重新构造候选，并读取：

- 原 SYSTEM 与全体原元数据成员的恢复封套；
- 全部原 joint/native/scoped 输入、唯一 choice、完整普通 Candidate 与依赖；
- 每份独立普通 FULL 的实际正文、checksums、原复制 inode 与稳定位置；
- 源/目标全部 FILE 和参与盘 current 的原 pending old/new 安装对，以及全体冻结身份记录。

这些真实材料齐全且仍相同时才追加。报告仅给原 operation、原 slot、candidate/snapshot 摘要及实际 PREPARE 成员计数，没有转为 OperationResult 的接口，五项未完成义务全部保留。

追加调用现有普通 JournalWriter 的 PREPARE 与同步路径。首笔前消耗普通 `next_slot` 一次；原 move 的重建另取不可变 choice 中的 `decision_slot`，不能把相同操作移到下一槽，不能重新产生时间/ID 或倒退普通计数器。相同记录重试复用原 frame，不追加第二份。写入后的返回再次读取全体材料及真实日志，只有全部原元数据成员均有这一份 PREPARE 才报告完整准备。

只读 `read_file_history_move_journal` 读取实际帧，不把进程内 receipt 缓存当磁盘证明。它比较当前最后一槽与持有 writer 的尾部，并检查持有文件与其原目录中的名称身份，拒绝同内容替换 journal、截断、重复帧或另一个候选。冷重开按原字节链重新准入，不声称持有跨进程的 journal inode 证明。

## 冷态只接纳准确的未接受尾部

选定 move 后，普通恢复可以保留的新增尾部严格限于：

1. 原 `source_sequence + 1` 的唯一 `decision_slot`；
2. 原 operation、原 source snapshot、由原 choice 派生的确定 transaction；
3. 同一 candidate/snapshot，原成员原槽位，每成员最多一份 PREPARE。

候选先从产生回执的本盘读取并核成员关系，不从其它盘借来替代本盘材料。ACCEPT、FINALIZE、PUBLICATION、其它 candidate、后继槽位、重复记录和 torn tail 均明确拒绝，保留原字节，不自动截断、选新候选或回滚。此前完整发布前缀仍按原出版证明恢复源 State；未接受 move 不进入活动状态。

workspace 加载完成后、可重建 SYSTEM 索引写入前，再通过新增的**只读原封套重建路径**恢复原 projection，并重新核对完整普通正文、所有原安装对和全部实际日志。该路径不调用会同步/补副本的显式 `resume_file_history_move_recovery`。缺少一份 envelope、正文、原安装或候选依赖即停止，不能凭一份合法 PREPARE 填补它们。

所有原参与资源均需可用。允许一个或几个元数据成员尚无 PREPARE，但不允许缺失原成员、本盘原候选或其它准备材料。冷启动只观察实际计数，不补缺失的票。持有原 writer 的普通开启动作仍可能同步已打开文件/目录，workspace 的既有恢复规则也保留；这里不宣称整个 Engine::open 是零 I/O 或零写入入口。

## 部分写入与明确续作

| 观察到的情况 | 本片行为 |
| --- | --- |
| 首笔前取消或缺少完整材料 | 不写 PREPARE，保留原 choice/准备与源数据 |
| 已写部分准确 PREPARE | 返回带原错误来源的 RecoveryRequired；不把计数当决定 |
| 冷开只有部分原 PREPARE，但其余原材料齐全 | 恢复旧 source State 与原 slot，报告实际数量，继续普通写入围栏 |
| 操作者显式用同一原 projection/摘要调用 prepare | 再核全体原材料，只补缺少的相同 PREPARE |
| 全体 PREPARE 齐全后重试 | 读取/复用原帧，不产生新 operation、时间或槽位 |
| 已登记正文/安装对/封套缺失、变更或 I/O 失败 | 保留剩余材料并拒绝，不复制造出替代 inode，不清理源或 target stage |

在首笔前已设置进程内保留状态但尚未写出任何帧时失败，显式重试仍只针对同一原选择；冷重开按实际零帧恢复原下一槽。没有增加自动中止、清除 choice、回收空间或废弃操作的 API。任何不确定写入后都不能根据“取消”宣称回滚。

## 成本与尚缺闭环

每成员追加至多一条受现有 journal 编码限制的 PREPARE，写前另检查 4 KiB 增量及 metadata reserve；既有普通材料、完整独立正文和数据库保留预算不放宽。幂等重试不重复追加。冷/显式读会全量重读准备闭包与各成员日志，本片目前还有多次候选/正文重建；日志扫描成本随真实历史增长，不是有界尾部优化。未测量峰值内存、吞吐或 fsync 总数，未提供性能或硬件耐久结论。

**仍缺实际共同决定及决定后恢复**：SYSTEM 活动安装、盘根 history 新 move 行、FILE/current/history 三范围共同交换、两端读取围栏、真实 ACCEPT/FINALIZE/PUBLICATION、准确永久成功收据与 choice 解除。单一 move 的冷 D 生命周期已经有准入逻辑，但普通发布的 native gate 不因此放开。完整目标/源 D 的最终可见性仍必须由同一决定控制，不能借本片 PREPARE 单独发布任何一端。

## 实际验证范围

遵照用户“本地也不再编译，直接提交”的最新要求，本片未本地编译、测试构建、运行测试或触发 CI；没有操作真实数据。本次对齐没有源码冲突，保留原实现，仅更新文档/TASKS；未创建 commit/ref，只准备准确 GitHub tree。新增八项根 `test/` 集成测试仅源码，涵盖全员幂等、冷原槽位/正文 inode、部分 PREPARE 不自动补票、缺原件拒绝、torn tail 保留、同内容 journal 替换、取消以及合法编码的 ACCEPT/重复/其它候选/后继槽拒绝。临时根故障构造不是物理断电证据。

历史 `04af3778` 父组合的 lib check 通过发生在 ENOENT 修正之前；随后十项所选普通准备集成全在首次普通 commit 夹具失败，**0 通过、10 失败、77 过滤**，没有进入 move。目录探测修正、后来发布的组合及本后继均未复验。格式整理不等于编译或运行通过，完整 SMB/真实盘/掉电验收仍未执行。
