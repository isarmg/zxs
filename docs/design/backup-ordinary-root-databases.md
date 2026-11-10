# Backup 普通盘根数据库的保留式安装

[任务清单](../../TASKS.md) · [范围别名](backup-published-database-aliases.md) · [Engine](../components/engine.md)

本片把原来仅在 generation-local 目录保存的本盘 current/history 副本，实际安装到每个原盘的普通 `.zxs-disk.sqlite`、`.zxs-disk-history.sqlite` 名称。它不是最终迁移完成：SYSTEM 的活动 `index.sqlite`、Engine Config/Current、目标 writer、下一笔业务 publication 和源围栏没有切换。按要求未审查、编译、测试、静态检查或 CI，未实际操作盘。

<a id="proof"></a>

## 原决定与完整参与集合

同一显式 registry/placement 后台任务只有在真实全体原成员 Publication、所有目标完整 native 前缀、正文、对象 SQLite、Current 只读准入、共同范围库和已有范围别名再次成立后，才进入普通盘根安装。新描述绑定原 source fence、实际 Publication、完整别名集合和每个原盘准确身份及实际 dev/ino；记录内容只与重新推导的规范字节比较，不反序列化为路径或执行权。

数据来自该原盘自身 `metadata/backup-common-databases` 的已发布完整作用域，不从另一盘替代，不把目标自己的数据库改名为原盘数据库。新目标普通根库已由前一阶段独立写入，本片对它们继续全体验存，不重新赋予它们投票或 writer 权限。旧被替代末级盘仍在完整原配置集合内，数据库反映真实已发布的本盘义务变化，而不是删除历史。

父子仓库同层同盘、不可拆分的文件/目录历史单位和显式多下级目标仍由前序共同决定及全部实体证明约束。本片不选择新的目标或副本数，不接外来 H03、不生成占位链。

<a id="exchange"></a>

## 真正交换普通名称，旧对象保留

先对所有原盘两库执行严格有界 SQLite 解码、准确身份核对，以及 history 从空库存完整折叠到同序 current 的比较。旧库若属于未知格式、错误身份、未来序号或同一序号的不同内容，保留并拒绝。任一普通库旁存在 WAL、SHM 或 rollback-journal 都拒绝，不删除、忽略或把其中内容与另一 main 文件混用。

全体预检通过后，SYSTEM 首先保存 `backup-preparation/root-databases.json`。随后 SYSTEM、全部原盘根及每个目标保存同一 `backup-root-databases-selected.json`。所有原盘的选择标记都持久存在后，才开始首个普通库名称交换。

实际写入复用生产 `PreparedInstall`：在该原盘的 `.zxs-layout/database-installs` 下独立写入完整新字节、同步并保存精确 inode/摘要封套，再无覆盖创建或 exchange 普通名称；被替换的旧库保留在原安装尝试内，绝不 unlink、回滚覆盖或清理旧 receipt。current 与 history 是两次交换，不声称跨文件或跨盘原子 rename。任何失败保留原决定和已发生的交换，原任务可在相同来源及完整参与集合下继续。

<a id="recovery"></a>

## 部分完成时不得读取为活动库

普通 Engine 的本盘 current/history 查询与维护入口在触碰数据库前识别选择标记并返回 `RecoveryRequired`。该标记的存在只增加拒绝，不选择 State 或授予任何权限；损坏记录也不能放行。因而旧 Engine、正常冷开追平和显式旧状态重建不会把已经部分交换的新库覆盖回旧 State，也不会把一新一旧的两库报成完整。

原后台继续使用保留的原 State、真实日志、原锁和目标证明。冷开仍由原 fenced 任务恢复，不从新 SQLite 选权威。所有普通两库实际读回相同后，重新核全部原生/目标证明，才向全体保存 `backup-root-databases-installed.json` 并报告 `GenerationOrdinaryRootDatabasesInstalled`。任何一个完成标记已经存在时，全部选择标记和全部普通库都必须仍然完整；缺失或损坏不借该标记重新生成。只缺后续参与方的完成标记可按原精确字节续存。

`Engine::inspect_backup_root_databases` 是原 actor/operation 的只读观察：后台必须已退出，保留全部锁并重新核实际选择、普通库和全体证明。它不修复、不接普通 HTTP/SMB，也不返回最终迁移成功或解围栏许可。取消仍只在同步 I/O 检查点响应，中断不承诺回滚。

<a id="limits"></a>

## 预算与尚未闭合的活动切换

最多 32 个原盘和 8 个明确目标。每个本盘库最多 32 MiB，逐盘仅处理一对输入，不把所有数据库同时复制到安装计划；SQLite 解码及既有完整 native 重放仍有额外内存，不宣称热路径或峰值已优化。全体描述最多 128 KiB，各根先保留 1 MiB 加四份描述余量；新操作合计检查一对数据库的现有 installer 256 MiB/8192 项保留预算以及配置的 metadata reserve。已决定恢复可复用自身封套，不因原尝试重复计费而自动丢弃旧数据；实际磁盘容量不足仍阻塞，空间检查不是物理 reservation。

SYSTEM 活动库存在独立 SQLite 连接、WAL 和保留读快照，不能只复制 main 文件后接旧 WAL。因此本片**没有交换 SYSTEM 活动别名**。完整 writer 还需要普通 generation 对 Backup 首笔证明的真实消费、活动 Current/serving locator、资源生命周期和下一笔发布闭包。现有 Backup 拒绝分支、目标隔离和源全池停写保持；盘根完成后普通 catalog 暂不可用，这是未完成切换的显式状态，不伪装成可用或隐藏故障。

后继 [SYSTEM 静默安装](backup-ordinary-system-database.md) 已增加真实普通 `index.sqlite` 的保留式改名，但仍未开放活动查询/Current/writer 或源解围栏；本页所述原盘独立阶段不因此获得这些权限。
