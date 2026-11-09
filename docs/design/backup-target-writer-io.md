# Backup 目标专用 DiskWriter I/O

[任务清单](../../TASKS.md) · [目标 writer 文件围栏](backup-target-writer-fencing.md) · [普通原生对象](backup-ordinary-native-metadata.md) · [Engine](../components/engine.md)

原目标 writer-file 围栏批已发布为 `ef0f35cb`；本 I/O 切片由原 `7ace5287` 独立保全、对齐为 `35ba95af`，随后与 seal／locator 整链对齐 `79b276c7`，由唯一协调者发布为 `90450cf0`。本文件记录该 I/O 切片自身范围，不把实际 I/O 资源视作全池完成。

本片把原任务已经持有的真实目标 writer owner 接入生产 `DiskHandle`、`DiskIoBinding` 和 `DiskWriter`。目标普通文件操作开始使用原生 Backup 代际的准确 I/O epoch，并由真实普通租约覆盖写入、读取及同步尾部。不是重新解析 Config 后把目标当成新的 Genesis，也不把文件锁当作 Current 授权。

**范围仍是原任务内的目标资源，不是最终活动池。** 不修改 `Engine.config`、活动 selector、`disk.json`、Engine Current 或原盘 I/O authority，不开放第二笔普通 publication，不解除来源围栏，不开放 SYSTEM 普通查询及 H03。只读来源、目标完整单元、原 voters 和永久回执的规则不变。

本 I/O 后继仅源代码开发；对齐真实父后已格式化改动的 Rust 源码，但没有编译或运行测试，也未触发 GitHub CI。用户最新要求停止本地编译，尚未执行的验证已取消。未操作真实盘。新增三项测试源码未运行：普通 Add 不能生成 Backup writer 投影、真实 I/O 尾部保留目录／writer 两把锁、最终 drain 关闭两把锁但不删文件。它们不是完整 Backup 端到端或断电验证。

## 唯一原生来源，不从诊断授予权限

新阶段先重新观察全部来源的实际首笔 Publication、每个目标完整正文和数据库、普通不可变对象／registry／Current 重放，以及已完成的 writer inode 绑定。进度 phase、共同完成记录、prospective State 内的 PUBLISHED 字样和孤立 writer 锁都不能构造 I/O 能力。

私有 `BackupWriterIoConfig` 消费准确 `PreparedBackupFirstPublication` 与同一首笔 Candidate，再从已经持有的目标 control archive 重放原完整 State 和历史，并核对原首笔配方。它保留原 Genesis 控制上下文和固定 metadata 索引，不经过序列化／反序列化生成一份新 Genesis 权限；实际 `io_disks` 仅包含原决定选定的新目标，不能给来源原盘更换 I/O authority。

每个新目标的 epoch 是 native authority 和完整登记身份的专用摘要。来源原盘仍保留旧 epoch；新目标的无代际 `None` 生命周期不能被另行解析的配置重新创造，已存在的不同生命周期必须先真实关闭。这些语义不改变原 Candidate、Publication 或 authority 的不可变字节；此前没有被支持的 Backup 活动 epoch 需要兼容或迁移。

## 同一资源的标准 I/O admission

任务把同一个持有目录交给 `DiskHandle::new`，用准确配置计算内存中的物理 seal，再用原 `FenceOwner` 调用 `DiskWriter::adopt`。没有重开 `.writer.lock` 或释放后重锁，也不把这个内存 seal 写成可被普通冷启动选择的 `disk.json`。

根目录的共享 binding 一旦建立，之前保留的同根别名也走同一普通 admission，失败时不能降级为无 binding 的 bootstrap 操作。任务保留真实 handle 和 writer，错误或后台退出不会丢弃已完成的 admission。相同原任务复用同一 plan、根、seal 和 writer；不同 plan 不能替换已有资源。

还把目标目录的 `AdvisoryLock` 交给同一 `FenceOwner` 保留。真实 I/O 租约因此同时保住 writer FD 和目录预约，即使最后一个任务／writer 包装已被释放，正在进行的读写和清理尾部也不会提前允许另一初始化者取得目录。最终 closure 按已有 gate 计数释放它们；不删除 writer 文件或目标材料。

## 落盘屏障与冷恢复

SYSTEM 先保存 `backup-preparation/target-writer-io.json`，绑定前置 writer-fencing hash、真实 Publication／authority／Candidate、完整 Config hash、registry 和全部目标 epoch。全体目标 admission 后再次通过实际 native／数据库／正文／writer 门禁，才写各根共同的 `backup-target-writer-io-admitted.json`。

目标完成记录必须通过已绑定 handle 的普通 I/O 租约写入、同步和读回；并非只在内存构造 `DiskWriter` 后写一个不受管理的缓存。普通对象、正文和数据库的再观察也经过已绑定根。SYSTEM 和来源根继续使用原持有能力，绝不调用全体源盘的 generation promotion。

冷恢复本来就必须重新创建内存 gate。它只能在已重新锁住相同真实 writer inode，并重新成立全体 native 证明后这样做；旧完成记录不替代这一门禁，不允许补造缺失 writer／绑定／原生材料。只缺后续相同完成记录时，显式原任务可继续写它们。异内容记录一律保留拒绝，无旧格式双读、清理或回退。

`inspect_backup_writer_io` 仅对当前 Engine 已保留的原任务资源做完整观察。它要求真实的现有 task-local DiskWriter，不在只读查询内构造新 gate 或修复文件。任务之外没有导出裸目录 FD、writer 或可自行选择目标的 Config。

## 预算与剩余边界

仍限原准确的一至八目标及既有三十二原盘。新描述上限 64 KiB，每个参与根要求原 metadata reserve 加 1 MiB；这是预检而非容量预留。增加每目标一个标准 gate／handle／DiskWriter，共享原 writer 和目录锁，额外记录逐根原子写入与同步，不降低持久化屏障。

native Generation 保留完整来源 State／历史以支持准确首笔语义，构造和再观察仍遍历完整归档、正文与数据库。复用现有 archive reader 的硬上限与校验；整段归档重放只有前后取消检查，不承诺瞬时取消、低峰值、增量成本或性能提升。

最终活动 Current／Config 与全池 selector、正常后续 publication、SYSTEM 解静默及安全源解围栏仍未实现。目标 `disk.json` 和普通原生恢复地址由后继本地切片继续开发，不算本片独立完成或已发布。这些要共同闭合后才能将任务内资源交给普通 Engine，不能仅凭本片真实 DiskWriter 已存在就释放旧池或报告迁移成功。

后继本地候选续接[目标原生 seal 的任务内安装](backup-target-seals.md)，只完成准确 `disk.json` 的不覆盖写入、持久文件绑定与原任务恢复。另有[目标原生恢复地址](backup-target-native-locators.md)后继，为目标安装独立完整归档及普通冷读名称。两片仍没有全池活动 selector、普通池切换或源解围栏，已随 `90450cf0` 发布，仍未完成全池切换。

后继[原盘独立原生控制](backup-original-generation-control.md)在不改变旧 selector／seal 的情况下，继续补每原盘完整归档并绑定已存在的真实首笔 journal；该后继仍仅本地，不计作活动全池。
