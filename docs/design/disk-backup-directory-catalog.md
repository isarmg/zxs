# 盘根递归目录副本与离线历史折叠

[盘根当前清单](disk-current-catalog.md) · [本盘发布历史](disk-local-publication-history.md) · [无 SYSTEM 材料读取](offline-disk-database-material.md)

## 本片范围

盘根 `.zxs-disk.sqlite` 的 `DiskCatalogSnapshot.backup_directories` 保存**原生递归备份已经发布、在所投影 State 仍有效的目录副本**。因此空目录没有文件版本时也有本盘清单，不制造零字节 FileVersion，不借用后代文件的 placement。每行保存原仓库、历史域、稳定目录 ID、本盘 owner 路径和所选原操作 ID；盘级身份仍是完整 pool/disk/slot/binding generation。

普通 `published_backup_directories` 和盘根投影共用 `published_directories_at`，不维护两套回执解释。每一个历史 State 独立按当时的原策略与完整已发布回执选行；必须与原 replica 的完整物理登记相等。换绑后的同名槽不继承旧目录，未来策略不反写过去的路径。相同逻辑目录在不同备份路径是不同本盘行；同路径保留当前 State 下最新的有效来源代表，全部原回执仍留在 SYSTEM/原生权威中。

这里**不表示全部主盘目录、独立 Git 目录仓库内容或所有目录操作已经完整保存**。本片只收录有原生准确副本登记的递归目录材料，不能从仓库当前主槽猜测没有 binding 证明的历史归属。文件/目录实体内部 SQLite 未改动，不把这些外部 owner 路径塞入对象内部库。

## 当前库与逐发布历史

当前格式为 `disk-current-local-catalog`，闭合 schema 包含 `identity`、`versions`、`backup_directories` 和 versions 的唯一自动索引。目录表按连续 ordinal 存规范 JSON；读取还验证完整稳定 ID、规范 PortablePath、严格身份/路径排序及没有重复键。版本与目录合计最多 65,536 行、24 MiB 规范记录，SQLite 镜像仍为 32 MiB。摘要覆盖两类行，不能用只对版本的相等检查把目录改变标成未变化。

伴随历史格式为 `disk-local-inventory-changes`，在原版本事件之外增加 `BackupDirectoryEntered`、`BackupDirectoryChanged` 和 `BackupDirectoryDeparted`。同一 publication 按目录身份/路径在前、版本在后的稳定键排序。同一路径下来源回执变化形成 Changed；改到另一路径是原行 Departed 与新行 Entered，保留稳定目录 ID、旧名称及两个准确行。每条事件仍继承实际 Candidate、operation、snapshot 和序号，不额外造决定。`DiskHistoryChange::version_id()` 返回 `Option<&str>`，目录事件返回 None，不假造版本 ID。

正常发布维护检测保护策略或目录回执写集：未知谱系或可能的目录变化保守重算各个已登记盘，之后比较准确完整行；普通无关回执仍使用原文件写集跳过不相关盘。历史维护重用已认证前缀，只在实际行改变时读回并安装完整有界历史。完整冷重建、原范围 Backup 准备和 Backup 首笔新范围都走同一投影；新绑定源清单检查同时要求版本与目录为空。根库共同安装的 fold 也比较两类行，不能仅凭版本相等通过。

## 无 SYSTEM 的恢复材料

`inspect_disk_database_material` 的历史 fold 分别维护版本和目录集合，每次 Changed/Departed 必须与完整先行行相等，Entered 必须不存在。比较较早当前库时同时比较两类行；当前库缺失时，内存恢复出的 `history_inventory.backup_directories` 保留该覆盖边界的准确目录名称、身份与来源代表。它不写源盘，也不会把历史覆盖较早的目录清单标为最新。

辅助工具 `disk-databases --records` 新增 `current-backup-directory` 和 `history-inventory-backup-directory` JSONL 行，摘要分别给出目录数。全部选中材料读取成功后才输出记录，最后才给 `complete:true`。这只是可保留的本盘救援材料，不创建目录、不导出正文、不选择系统权威；`source_publications_verified`、`content_availability_verified` 和 `authority_restored` 仍为 false。

## 保全、成本与未完成事项

- 新 schema/格式明确拒绝之前只有版本的当前/历史库；不双读、不隐式升级、不删除或覆盖不支持字节。已有安装器会保留旧文件并返回具体失败，正常发布的原业务结果不因此改判。旧 Backup 准备字节与新投影不符时也保留拒绝，不暗改既有准备记录。
- 当前投影除原 placement 扫描外，还扫描本次 State 的永久操作回执，并解析对应目录 namespace；不同历史点独立计算，不缓存所有 State。最多保留有界代表行和编码/SQLite瞬态，不将单库上限称为总 RSS 上限。策略/目录回执变更会让全部登记盘进入精确比较，尚非按目标盘增量归因。
- 未增加 fsync 或独立后台线程；仍走同一安全 installer，其保留配额与同步顺序不变。当前与历史库不是跨文件原子事务，分别维护和报告错误。
- 完整代际归档、全部主目录/alias/标签历史、三范围共同权威和任意单盘正式恢复仍未实现。自摘要与本地转移闭合不能取代原 publication 认证。

2026-11-08：生产源码、helper 输出和四条回归源码已写入候选。遵照当前要求，未审查、编译、格式化、静态检查、测试或 CI，未操作真实磁盘；不能把测试源码或原有测试结论作为本片运行证据。
