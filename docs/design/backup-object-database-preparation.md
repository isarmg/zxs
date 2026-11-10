# 完整备份目标的对象数据库准备

本片基于真实 main `1b3fba43`，为 `288eefd1` 的原生控制准备补上**文件／目录仓库 SQLite 的隔离目标材料**。按用户当前指令，未编译、未测试、未审查、未运行 CI。它不是最终权威切换、完整三范围数据库迁移或外来低级链接纳完成。

## 生产入口与背景执行

`Engine::start_backup_object_database_preparation(operation_id, principal)` 只能续接同一个已经完成原生控制准备的单目标或显式二至八目标任务。actor、原操作、源 Config／pool／sequence／snapshot、整个目标集合和次序均保持；不另选子集，不重估复制份数，不把某盘失败当作减少目标的理由。

全部原目标锁、来源绑定、历史正文、原生 journal 与逐出版证明通过生产准入逻辑后，SYSTEM 先持久化不可变 `backup-preparation/object-databases.json`。后台线程从目标的完整 admitted State 编码数据库，再写所有明确目标。普通 Engine 冷开优先恢复这一更后阶段，而非退回历史或原生阶段并覆盖显示状态。源边界改变时，在重开任何原目标名字前进入 `SourceChanged`；不重新采样源，也不升级旧任务。

新终态 `ObjectDatabasesPrepared` 只表示这里列明的对象库材料。API 尚未接入 HTTP/Web 或 helper 设置流程；前置材料阶段仍需明确启动。已有 `HistoryPrepared` 和 `NativeControlPrepared` 的标记及原意义不变。

## 实际写入的数据库

每个目标的隔离目录：

```text
object-databases/
  manifest.json
  histories/<history>/files/<file>/.zxs-file.sqlite
  histories/<history>/directories/<directory>/.zxs-directory.sqlite
```

这是真正的 SQLite 字节，不是仅描述数据库的 hash 模型。文件库复用当前 FILE scope 编码器，保留文件、全部已有版本／分支、节点标签／备注、metadata、merge 以及完整 State 中可认证的永久 D/undo 与 retained-copy lifecycle 投影；不把外部路径日志塞入 FILE scope。旧 graph-only copy 若缺被导入的 lifecycle，整单拒绝，不能把缺失编码成完整空审计。

目录库复用当前目录仓库 scope 编码器，保存本仓库的分支、HEAD/index、树和所属提交，嵌套子仓库保留精确引用而不是复制子仓库今日 HEAD；每个子仓库也另有自己的对象库，所有父子和其文件都在**每个选定物理盘**的完整整池材料内。暂不实现父子原来位于不同目标盘的合流分配。

数据库从已准入的完整 State 重建，不从 live SQLite 路径随意复制，也不把 SQLite 自摘要当出版授权。本片不调用活动实体的 database installer，不移动文件容器、不替换现用 SQLite；上述是后续安装和共同决定需要的准备域。原正文仍在同盘既有 histories 材料域中，不增加硬链接、reflink 或再复制一套正文。

## 完整集合、预算与失败

整个对象库集合上限 4096 库／64 MiB SQLite，单库继续沿用 16 MiB、65536 行、12 MiB 逻辑记录限制；manifest／阶段任务与完成记录各有 4 MiB 读写上限。对象集超限整单失败，不漏掉末尾文件或子仓库来凑预算。SQLite 仅在内存连接编码或验证持有根读取的有界字节，不通过易替换的路径连接磁盘数据库。

首次目标写入前，逐盘核本盘完整待写量、独立目录／临时文件开销和保留空间。一个 peer 的空间不能抵扣另一盘。目标私有目录由持有 fd 定位；每个 SQLite 使用现有无覆盖、写入同步、目录同步原语。已有同名材料必须与当前完整投影精确相同，不覆盖冲突字节、不删除未知材料。

准备域的目录清单也是闭合集合：预期外文件、目录或链接直接阻塞并保留。中断留下的正确子集允许原任务续写，缺失与冲突分开处理。数据库写完后重新读取每个目标的全体原生证明和应保留正文，再逐库读取真实 SQLite 并核准确范围和字节；最后才逐目标写相同 `object-databases-prepared.json`。某盘先收到记录不是部分切换，其余盘未完成时整批不能通过检查。

后台诊断锁不包围磁盘同步；原 writer／目标锁持续持有，保留 stop、panic、join 和错误落盘语义。状态文件／字节计数仍是原正文统计，不把 SQLite 加成已迁移正文。数据库计数与字节由新报告逐目标列明。不承诺任意阻塞 I/O 下的硬关机时限。

## 目标独立读回

`offline_check_backup_object_databases(paths)` 要求原目标全集和次序，只读目标：

1. 逐盘认证完整 State、所有历史正文、原 journal／publication 控制前缀和原阶段标记；
2. 从**该目标自己的** admitted State 重新编码期望对象库；
3. 核准备域完整目录清单、真实 SQLite 范围／结构／全部准确字节与 manifest；
4. 核所有 peer 的共同完成记录和持有／命名物理身份。

缺失目标、缺库、坏库、伪造完成摘要、不同任务或未知条目都不能由健康 peer 掩盖。报告不是可序列化后直接授予写权限的凭证；没有 State 导入、盘注册、route 改写或切换接口。

## 未闭合的权威边界

依旧缺 SYSTEM 全局历史库和盘根本盘历史库的完整目标材料及安装、最终活动实体路径安装、membership／registry 代际归档、源增量追平／最终写围栏、真实活动多下级路由、父子旧异盘合流、helper 一次性完整低级链正式接纳和专用联合决定／冷切换。当前只支持原准备入口的单一级来源、既有末级备份边、固定 genesis 无降级前缀；其它情形明确拒绝。

**不得**用本终态放宽普通 handoff、注册待接纳盘，或将目标标记当 ACCEPT／恢复令牌。未具备整个低级链时不能成功加入。旧源、旧数据库、原永久收据均未删改。总体约束见[备份换盘 ADR](../adr/backup-target-replacement.md)，进度见根 [TASKS](../../TASKS.md)。
