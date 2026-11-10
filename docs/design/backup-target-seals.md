# Backup 目标原生 disk.json 的任务内安装

[任务清单](../../TASKS.md) · [目标 DiskWriter I/O](backup-target-writer-io.md) · [writer 文件围栏](backup-target-writer-fencing.md) · [Engine](../components/engine.md)

本 seal 切片保留原 `0abfcb64`，曾在已发布 `ef0f35cb` 上对齐为本地 `e2c6a07d`，随后与 I/O／locator 整链对齐 `79b276c7`，由唯一协调者发布为 `90450cf0`。不改写原冻结候选，也不把单目标 seal 安装视作全池活动切换。

本片新增实际目标 `disk.json` 的持久化代码，不再只在 `DiskHandle` 内保有 seal。它消费原任务已持有的真实 native DiskWriter，以独占不覆盖方式写入生产 `MembershipGeneration` 格式，并保存准确文件设备／inode 绑定。没有执行该流程或操作真实磁盘。

**目标身份安装仍不等于活动池切换。** 不写 `metadata/pool-membership/generation.json`、普通 Config 锚或代际 journal，不改 Engine Config／Current，不开放第二笔 publication、SYSTEM 查询或安全源解围栏。正常启动的 generation／registry／promotion 门禁没有放宽；不能把孤立 `disk.json` 作为普通池、外来盘或 H03 接纳许可。

对齐真实父后已格式化改动的 Rust 源码，没有编译、运行测试或触发 GitHub CI；用户最新要求不再本地编译，取消尚未执行的验证。八项临时目录测试仅源码，涉及精确重试、未拥有 seal、两个创建中断点、已绑定文件丢失／同字节替换、物理根替换及不接受其它 native stage／Genesis 格式。未运行端到端、真实掉盘或断电验收，不能据此声称这些行为已经验证。

## 从真实资源取得唯一 seal 配方

安装先执行既有 `writer_io::observe`：重新观察原生源决定和实际首笔 Publication、全部目标完整正文／数据库／native 对象、writer 文件 inode 以及已有普通 I/O gate。进度、SYSTEM JSON、目标本地意图或此前完成报告都不能单独生成安装权限。

seal 由原 `Target` 所保有的 `DiskHandle` 返回，读取时仍取得该目标准确 admission plan 的普通租约。没有从保存的 Config 创建新 Genesis，没有重开／重锁 writer，也不更换任何原盘 seal 或 I/O epoch。

SYSTEM 的 `backup-preparation/target-seals.json` 绑定源 fence、原决定、实际 Publication、完整 writer admission hash、native 安装 hash 和所有目标的准确 seal。它先于第一个目标本地意图及实际文件写入。

## 精确不覆盖安装与恢复

每个目标先保存 `backup-target-seal-intent.json`，包含全体 seal plan、原 writer admission、原 native 安装及完整物理 seal。随后用已有原子不覆盖写入和文件／目录同步流程安装普通 `disk.json`，读回实际文件身份后保存 `backup-target-seal-binding.json`。这整个写入、同步、读回和失败尾部都由原目标普通 I/O 租约覆盖。

- 没有准确本地意图的 `disk.json`，即使字节恰好相同，也不采用、不覆盖。
- 只有原意图而尚无 seal，可由显式原任务续写；只读检查不创建文件。
- 原意图和准确 seal 已存在，但尚无文件绑定，可在同一原任务完成同步和原 inode 绑定。
- 文件绑定一旦存在，seal 缺失或被同字节新 inode 替换，都保留拒绝，不能重建或改绑。
- 任一全体完成记录存在后，所有目标意图、seal 和本地文件绑定均须齐全；仅允许补齐后续相同全体完成记录，不允许重造上述任何材料。

全体 seal 安装后再次执行完整原生／正文／数据库／writer gate 观察，并重比准确原计划，才在目标、原盘和 SYSTEM 写共同 `backup-target-seals-installed.json`。该记录绑定全体 seal hash 和实际文件设备／inode，不只是“完成”布尔值。

## 冻结读边界，不是假活动 selector

原任务冷恢复和既有冻结读者仍须先读取同一目标材料。普通 metadata 库存继续仅允许 `objects` 和 `publication-seals`，拒绝 pool-membership、活动 journal 及未知名字。本片只允许一个狭窄例外：已选 native 安装 hash 与本地 seal 意图一致，物理根和准确 seal 字节一致，已有 inode 绑定仍有效。

这个本地检查只让冻结材料继续被读取，不授权写盘，不构造 DiskWriter，不选择 State 或 Config。所有写入仍须回到原源能力、全体证明和实际已有 writer gate；脱离原任务的读者也不能仅凭这个意图绕过独立持有的原决定和 Publication。

目标身份可以已写完、池选择尚未完成。普通启动仍要求本 seal 阶段尚未安装的共同 generation 材料；重新解析配置也不能把 `MembershipGeneration` seal 当成初始 `ManagedDirectory` 盘。活动 selector／locator 的后继实现须单独闭合全体原盘、目标、SYSTEM 和普通 Current，不能从这里直接放开来源。

`inspect_backup_target_seals` 仅观察当前 Engine 保留的原任务 gate、全部实际材料、普通 seal 字节及 inode 绑定，不创建 gate 或修复文件。状态 `GenerationTargetSealsInstalled` 也不报告最终迁移成功。

## 预算与剩余工作

沿用原决定最多八个完整目标、最多三十二个原盘；全体 plan／completion 各上限 64 KiB，每目标本地意图、seal 和绑定各上限 8 KiB。所有参与根预检原 metadata reserve 加 1 MiB，不将预检当作已预留容量。每目标增加三个小文件的原子安装／同步及共同完成记录，保留原全部持久化屏障。

全体观察仍重放原归档并读取所有正文及数据库，不承诺增量成本、瞬时取消或吞吐提升。后继[目标原生恢复地址](backup-target-native-locators.md)仅继续安装目标自身普通 generation／Config 锚与独立归档，不给原池切换。最终全池活动 selector／Config／Current、第二笔普通 publication、SYSTEM 解静默、安全源解围栏、设置自动触发及完整进度仍缺；原永久操作／主体／事件／D／undo、完整单元与低级完整链约束不变。

后继[原盘独立原生控制](backup-original-generation-control.md)在不改变旧 selector／seal 的情况下，继续补每原盘完整归档并绑定已存在的真实首笔 journal；该后继仍仅本地，不计作活动全池。
