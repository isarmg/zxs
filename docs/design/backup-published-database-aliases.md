# Backup 已发布数据库的精确范围别名

[任务清单](../../TASKS.md) · [Engine](../components/engine.md) · [原生 Backup 迁移](backup-native-generation-authority.md)

本片是最终活动数据库切换的名称消费前置，不是活动 writer、解除源围栏或完整迁移完成。按用户要求仅写源码和文档，未审查、编译、测试、静态检查或 CI，未操作真实盘。

<a id="binding"></a>

## 同一原决定，完整参与集合

原后台在全体真实原成员 Publication、完整目标 native 前缀/State/registry、Current 只读准入、全部实体正文及共同数据库副本均已重新观察后，计算唯一范围表。记录不选择另一个 authority、operation、目标集、publication 或未来策略。

| 类型化范围 | 文件类型 | 实际持有目录内的固定名称 |
| --- | --- | --- |
| SYSTEM | `SystemCurrent` | `backup-common-databases/index.sqlite` |
| 原配置盘准确 disk/slot/binding | `DiskCurrent` / `DiskHistory` | 该盘原 `metadata` 中的 `backup-common-databases/.zxs-disk.sqlite` / `.zxs-disk-history.sqlite` |
| 新选定目标准确 disk/slot/binding | `DiskCurrent` / `DiskHistory` | 该目标持有根已有的 `.zxs-disk.sqlite` / `.zxs-disk-history.sqlite` |

每项记录准确类型、完整身份、实际持有目录 dev/ino、固定相对名称、真实长度和内容散列。整个范围表还绑定原 source fence、共同 manifest 和实际首笔 Publication。原盘与新目标是不同 scope，不能因名称类似、内容相同或数量相等而互相替代。SYSTEM 散列来自已核完整 State 的同一真实编码，而非只检查 SQLite 文件存在。

`BackupDatabaseAlias` 的物理地址字段不提供公共修改入口。磁盘读回的 JSON 只与从完整证明重新计算的规范字节比较，绝不反序列化为可执行路径、权限或 writer。

<a id="installation"></a>

## 实际持久选择与原任务恢复

在任何选择记录落盘前，全部库和原 Current 证明重新检查，各参与方满足小记录与原子临时文件的余量要求。SYSTEM 先保存 `backup-preparation/published-database-aliases.json`，随后 SYSTEM、每个原盘既有 `metadata` 和每个目标持有根保存完全相同的 `backup-published-database-aliases.json`。使用原有无覆盖原子写入/同步原语，不创建硬链接或符号链接，不替换旧活动库或副本。

整个批次写后再次重建实际证明、字节散列和物理根，全部规范字节及全体记录一致后才报告 `GenerationDatabaseAliasesInstalled`。没有部分可用的别名集合；单个记录、完成 phase 或缓存报告都不能授权读取或写入。

中断后只按原 task/actor/decision/publication 重新核全体材料。存在任一个参与记录时，SYSTEM 的原精确意图必须仍在。缺失的后续选择记录可按原字节续存，异记录不覆盖；数据库、正文或原生证明缺损仍报错，不从其他 scope 补造。不删除源文件、旧 receipt、旧 Candidate 或失败临时文件。

<a id="read"></a>

## 真实消费者与数据读取

新增 `Engine::inspect_backup_database_aliases` 和 `read_backup_database_range`。两个入口都要求原 actor/task、后台无活动冲突、原 SYSTEM/盘锁和完整原/目标根；不能凭外部选择 JSON 或一个孤立目标创建。

读取请求只含 `BackupDatabaseScope`、`BackupDatabaseFile`、offset 和 length。未知身份、不存在的类型组合和越界范围明确失败，不接受磁盘路径。每个请求在完整源/目标证明和全部记录都成立后，按生产固定映射打开相应真实文件，通过同一个 fd 顺序读取并核对完整长度和散列，才释放不超过 4 MiB 的范围。零长度请求仍完整校验文件；I/O、截断、增长、撤销或内容不一致不返回部分成功。返回前再次核全部材料、记录和命名根，宿主仍须认证并授权原操作，接口尚未接普通 HTTP/SMB。

这条读取链直接消费上面安装的范围选择，不将源当前活动 SQLite、目标零号或另一副本作为 fallback。数据库仍是 native published State 的派生内容，不单独决定恢复权威。

<a id="limits"></a>

## 预算与明确未完成项

最多 32 原盘、8 明确目标和一个 SYSTEM，合计不超过 81 个别名。SYSTEM 文件最多 64 MiB，单个本盘 current/history 最多 32 MiB；描述最多 128 KiB。每个参与方检查 1 MiB 加两份描述字节的余量，这不是物理空间 reservation。容量不足时不删除旧数据或降低完整集合要求。

文件校验增量缓冲为 64 KiB，额外范围缓冲不超过 4 MiB；原 Current/共同库检查仍多次完整重放所有目标的 bounded native 前缀并持有完整状态/编码，不是 O(范围大小) 热路径，没有性能或内存峰值验收。取消仍在同步 I/O 检查点响应。

普通 Engine 的 `index.sqlite` 路径、原盘根库别名、Config/Current、目标 writer 和全池 source fence **没有改变**。这些别名当前只服务原操作的已发布 generation-local 数据观察，不能被配置成普通活动路径；本片未打通普通 generation 对 Backup 首笔 proof、serving locator 与下一笔写入的完整消费。H03 一次性外来完整链接纳仍关闭，不能借普通 Add/readmission 放行。最终活动安装必须在完整资源、原决定、全部证明和冷恢复闭包都满足后再切换，不能依据本片状态解除围栏。
