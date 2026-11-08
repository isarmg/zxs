# 跨盘移动的联合恢复准备封套

本候选对齐 `main 308e894bd3fef34039ff81fd7d332ba58d7225c8`，承接已发布的 `c53da226` 源捕获与 `7b214fd7` 共同元数据投影。它把原 operation 的准确联合准备保存到真实系统目录和全部配置元数据成员，提供显式冷重建／原准备补齐；**没有建立 move 决定，不发送 ACCEPT，也不发布源 D 或目标 namespace。**

## 原始输入不变

`save_file_history_move_recovery` 消费仍有效的共同投影。严格当前格式 `file-history-move-joint-recovery` 固定：

- 原 operation、actor、池、精确已发布 snapshot、配置摘要及显式 Initial／Activated 配置代。
- 稳定源 file ID、两端 namespace 请求、源 sequence、完整逻辑文件库摘要、所有分支 frontier、去重 parents 和完整版本集合。
- 原 proposed decision slot、冻结时间、完整目标路由和 binding generations、固定且排序的全部元数据成员身份。
- **联合投影摘要**，由完整目标逻辑 HistoryManifest、目标导入的旧 D/undo 审计和本次源 D 一起计算。它不是只对源 HEAD、两端路径或封套自身做摘要。

联合投影先按 12 MiB 编码上限计数，再进行规范编码／摘要；保存的恢复封套上限 2 MiB，各集合仍受 65,536 项限制。没有省略字段的旧格式兼容、默认空前缀或 source-intent 自动升级。

## 实际持久化范围

系统根和每个配置元数据盘根各保留一个 `file-history-move-recovery/preparation.json`。先保存原系统封套，再按固定 disk ID 顺序同步全部成员，最后逐成员独立读回比对准确封套并重验原系统材料。它们的字节相同；成员身份也在冻结封套中，不允许以另一个可用盘、同 slot 新 binding 或较小成员组代替。

系统根必须仍是原持有目录与原 `writer.lock` inode。成员使用现有已登记 DiskHandle、I/O admission／running gate、原 disk seal 和准确根目录身份。路径不是权限；目录 fd、文件 inode 与持有字节在同步前后重验。记录采用独占新私有目录和 no-clobber 原子写入，文件、准备目录及原根目录同步后才完成该副本；重复保存相同原始记录也补齐同步屏障，不因“文件已存在”略过。

每根保留元数据恢复空间。新创建期间失败，或系统已保存后任一成员出错／取消，都明确保留原准备，报告 `RecoveryRequired` 及来源；可能只有部分副本存在，但源和目标业务状态尚未改变。这里的取消不可能取消一个已经成立的 move 决定，因为此路径根本不写决定。

所有副本保留**单槽且不可替换**：同 operation／同记录只重验同步；同 operation 不同输入拒绝；另一 operation 不覆盖。缺失整个成员准备目录可以在原请求核对后独占重建；已有但不完整的目录、未知临时文件、损坏封套、不同内容或无法读取的副本必须保留并报错，不能用系统自摘要覆盖它们。

## 冷重建与原成员补齐

`inspect_file_history_move_recovery` 只检查系统封套。结果中的成员数是**预期副本数**，不是在线健康报告、quorum、决定或移动成功。

`resume_file_history_move_recovery` 要求原 operation、原 actor 和已查看的准确封套摘要。它先核对当前已发布 snapshot／配置代／完整成员集和原 proposed slot，再通过当前 Engine 独立重新捕获整个来源，沿保存的原时间／route／slot 重建共同目标图与源 D。重新计算完整 RecoveryRecord，逐字段与原记录相等后，才允许补齐**同一准备**缺失的成员目录。

因此，文件标签／备注、未选分支、D/undo、目标变化、配置／成员变化、slot 推进或未提交写工作区都不被两个未变的 namespace HEAD 掩盖。即使攻击者改写某字段并重算封套自摘要，独立来源及联合投影重建仍必须匹配。自摘要不提供用户授权、设备身份或内容可用性证明。

冷重建返回新的 `FileHistoryMoveProjection`，使其 Weak State／双端 namespace witness 绑定当前 Engine，而不是恢复旧进程指针。再保存或重建仍使用原 operation、D ID、目标对象身份、时间和 slot，不重新采样时钟或重新规划。

## 仍未实现，禁止提升为成功

该封套是**恢复准备依据**，不是业务 journal 的 PREPARE／ACCEPT／FINALIZE／PUBLICATION，不预留 State operation 或 decision slot，不占据全池写者的决定槽，也不声称 N 份文件等于共识。其它合法业务变化仍可使本准备过期。

仍缺目标全部 FULL 副本和三范围数据库／标签的独立验收、一个永久 move 收据的双端审计归属、该收据在热 Prepared／冷 Candidate 中的完整重建，以及与实际 N 成员 journal 决定、源／目标读取围栏和原操作恢复的一体衔接。**缺目标闭包时，没有执行接口能够把本封套变成源隐藏、目标可编辑或 SMB rename 成功。** 不能先独立 copy 成功再 unlink，也不能用已落盘准备假装已经决定。

当前恢复还依赖原系统目录可读与原 State 能正常重开。即使成员都有准备副本，也没有新增系统盘丢失时的自动选主、投票或任取一份接纳入口。没有自动清槽、取消／终态归档、多个并行 move 槽位、后台监视或主服务启动自动推进；源变化后原准备保持可诊断但不能再执行。旧未合入 `25722d90` 仅系统来源计划不是本格式的前驱或迁移入口。

## 验证状态

本片新增八项根 `test/` 集成测试源码，使用临时 Engine 池：全部元数据成员相同字节与原操作冷重建、冷开补缺失副本、伪造自摘要拒绝、来源变化不覆盖、成员残缺目录保留、损坏副本不覆盖、actor／operation／取消拒绝、不支持格式不升级。新测试还在已激活配置与初始配置两种夹具上约束原 D／时间／映射不变及业务 State 无改变。

按用户要求未做代码审查、编译、格式化、测试或 CI。没有断电或跨机器故障证明，测试源码不表示已通过；仍未验证真正 move 决定恢复、完整目标安装或真实 SMB。代码路径有实际新持久化行为，但不等同于已验收的持久性。
