# 备份目标上的原生围栏控制归档

[Engine](../components/engine.md) · [任务表](../../TASKS.md) · [路由快照](backup-generation-routing-snapshot.md)

## 本阶段范围

已决定的后台 registry/placement 任务在 `GenerationInputsPrepared` 后，新增 `GenerationControlPrepared`。它复制并独立重放**每个原元数据成员的实际配置锚点、封印和截至原 MembershipFence 的完整日志字节**，不是用证书里的日志哈希或计划 receipt 合成日志。目标自身已有的完整业务 native 前缀提供原 Candidate、Publication、Finalize、快照、历史及实际 publication seals。

这些材料转换为已有 `membership-control-archive` 原生格式：成员 map、publication seal map、原 transition、真实 journals 及全部引用对象。每个目标的 `backup-generation-control/archive/` 都有独立完整字节。普通 native `verify_generation_archive_observed` 从该目标自己的持有目录重放，核对原来决定绑定的完整成员日志；不从旧源或其他健康副本补读，不把目标变成旧投票者。

此片代码未审查、编译、测试或运行 CI。下面描述实现契约，不表示验证通过，也不是部署或操作真实磁盘的指令。

## 持久化与中断

1. 已存在的原决定先核所有原成员的真实不可替换选择槽。现有目标正文、实体、路由及未发布 State 材料仍须通过自己的准入。
2. 在原 I/O 租约和 writer 锁存活期间读取真实原成员字节，并再次核实际决定/封印/日志；每个目标独立提供其自己的已认证 native 前缀。
3. 在首个新写入前，全部目标计算出相同原生控制闭包并满足各自完整空间预留。目标间不借用可用空间。
4. 材料经原 no-clobber 文件同步及目录同步原语安装。原字节不同、未知条目或损坏时保留拒绝；不替换旧数据。该新隔离命名空间保留最多 1,024 个合法原子写临时文件，但不采纳、晋升或删除临时文件。
5. 全目标都完成实际字节比较和正常 native 回放后，才逐个保存 `generation-control-prepared.json`。再次观察所有原选择。单个 completion 不是整个批次的完成证据。

崩溃后现有原任务重开仍须通过原 Engine 围栏准入及实际选择。它重新计算同一闭包，无覆盖补缺；不能换操作、目标、原成员、祖先链或放置策略。缺源时不猜原日志尾、不用业务前缀替代围栏日志。

## 离线读取

已有 `open_decided_backup_targets` 要求独立保存的可信决定摘要和全部原目标目录。新增 `inspect_generation_control()`：从每个目标自身的固定原成员地址读取保存字节，核证书已绑定的封印/日志摘要，再用该目标自己的原业务前缀重建精确预期归档并运行正常 native 回放。丢失一个目标、缺原生页、损坏日志、错误 fence、异决定 completion 都不能由其他目标掩盖。

它只返回原控制归档根及诊断计数，不暴露可变 State、认证秘密或目标 writer；不依赖源/SYSTEM 在线，不补写、不创建新决定。现有 `resume_generation_preparation` 仅恢复此前的路由快照材料，**不会**在源丢失时伪造本阶段缺失的原成员字节。

## 资源与保留边界

- 继续限定完整固定 Genesis、原成员全集和明确的一至八个全量目标；不支持的前驱代际仍拒绝。
- 每原成员 journal 上限 16 MiB，原成员 config/seal/journal 累计 128 MiB。
- 每份 native control 材料最多 32,768 个文件和 256 MiB，外层清单最多 4 MiB；每目标另留目录开销和 16 MiB 余量。
- 目标 native 回放最多 512 MiB 观测读取、131,072 个观测步骤和 32,768 个对象；重复引用仍计读取预算。调用既有 native 前缀读取时继续受其自身 128 MiB/32,768 步限制。
- 构造最多保留原成员字节、一份最终归档和当前目标重算，不同时积累八个目标全量图；真实 I/O/fsync 仍可能阻塞，检查点不等于任意故障下硬关机时限。

## 仍未完成

归档根是**前驱控制证据**，不是新的 `PortableAuthority::Backup`、第一笔业务 Candidate/Publication、活动拓扑或可服务新盘封印。新绑定的完整数据库历史范围、serving locator 安装、未来策略延续、目标 writer 激活和源围栏安全解除仍缺。没有把 Config 单链限制偷偷改成扇出，没有把普通 Add/Abort 用作 Backup。

完整文件/目录仓库、父子对应层同盘、多下级显式选择、一次性完整低级链规则继续保留。复杂原分叉、异盘父子合流及外来正式接纳仍拒绝/未完成。旧源、旧正文、永久 operations/principals/events/D/undo/退役证据均不删除、不提前重写；源池仍可能停写，已选 Backup 后原 Abort 也可能不可用。
