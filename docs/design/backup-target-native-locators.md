# Backup 目标的普通原生恢复地址

[任务清单](../../TASKS.md) · [原生 disk.json](backup-target-seals.md) · [目标 DiskWriter I/O](backup-target-writer-io.md) · [Engine](../components/engine.md)

本片基于已发布 `ef0f35cb` 之上的本地 I/O `35ba95af` 和 seal `e2c6a07d`，三个后继均尚未发布。它继续实现目标自身的普通原生恢复文件，不把前置文件围栏批的发布批准扩大为这三个后继的批准。

本批与其余备份后继整链对齐准确主线 `79b276c7`，只解决集成冲突并准备树，交唯一协调者提交；保留主线每小时 GitHub 短 CI 安排，不自行触发 CI，也不本地编译或测试。
原任务在实际 target writer、普通 I/O gate、原生 `disk.json` 之后，在每个准确目标安装普通 Config 锚、generation locator、authority、副本独立的完整 control archive 及所有历史 Candidate 的 inclusion proof。不是只记一份路径清单或完成标记：安装代码逐文件不覆盖写入并读回，随后从该目标普通地址重放真实归档，再用已有普通本地成员读者核对首笔 Candidate 和有效 Config。

**这里只选择目标自己的恢复地址，不选择全池活动代际。** 不改原盘 seal／selector／journal、不切换 Engine Config／Current，不生成第二笔 Publication，不解除 SYSTEM 静默或来源围栏，不开放 H03。未执行上述安装流程，未操作真实盘。

八项新增临时目录边界测试仅源码，涵盖部分材料只读、无主命名空间拒绝、完成后丢失、数据目标的意外 journal、原计划不可改签、物理根不可复制改绑、固定路径边界和异字节保留。它们不是完整原生 authority／普通 Engine 启动验收。改动 Rust 源码已格式化，未编译或运行测试，未触发 GitHub CI；用户最新明确不再本地编译，尚未执行的编译／测试构建计划已取消。

## 唯一配方来自实际原决定

`target_seals::observe` 先重验同一任务的源决定、真实首笔 Publication、所有目标 native 对象、全部正文和数据库、原 writer 文件及 seal inode。然后从准确首笔 Candidate 和原 held archive 建立私有 `BackupWriterIoConfig`，读取原 native control archive，生成 `PreparedBackupTargetLocator`。

配方构造器只对这个封闭能力开放。它要求原 transition 摘要、完整 archive 摘要及原 native generation 一致，所有被授予 I/O 的新目标都不是 metadata voter。它不接受持久 JSON 提供的任意路径、调用者伪造的 Config、少数目标报告或进度相位作为权限。

每个数据目标包含同一完整配方：

- `metadata/pool-membership/config.json` 是原 Genesis Config 锚，不是新 Genesis。
- `metadata/pool-membership/generation.json` 和 `metadata/membership-generations/<authority>/authority.json` 是准确原 PortableAuthority。
- `metadata/membership-pending/<transition>/control-archive` 保存完整原生归档，含固定 Genesis 前缀、原成员真实 fenced journals 和原身份字节。
- `metadata/membership-history/<candidate>.json` 保存原归档内每笔 Publication 的 native inclusion proof，包括正文当前不可用的历史，不裁剪为当前可读子集。

目标顶层没有活动 metadata journal。归档内部的原 journals 只是原 source 证据，不能成为目标的投票资源。旧 Candidate、Publication、State、operations／principals／events、D／undo 和永久收据的字节不改。

## 先选原计划，再写四个固定命名空间

SYSTEM 的 `backup-preparation/target-generation-locators.json` 绑定源 fence、原决定／Publication、seal installation、authority／transition／archive 摘要、完整文件清单摘要、总字节数和全部准确目标 seal 身份。它先于每个目标本地选择和任何普通 namespace 文件。

目标本地 `backup-target-generation-selected.json` 绑定该全体计划、原 native 安装、全部材料摘要及真实目录设备／inode。仅允许四个固定 metadata 命名空间：`pool-membership`、`membership-generations`、`membership-pending`、`membership-history`。

- 若 namespace 先已存在而没有原本地选择，即使内容相同，也保留拒绝，不采用、重命名或删除。
- 全体目标预检已存在文件的准确长度／摘要、已存在目录和原选择，再开始第一个新本地选择。
- 精确原选择下的部分安装可由原任务继续，不覆盖已有异内容文件。只读观察不修复。
- 所有新文件使用已有不覆盖原子安装、文件同步及目录同步路径；实际 writer 普通租约覆盖安装、同步、读回和失败尾部。
- 单目标归档重放和普通成员冷读完成后，写相同选择字节到 `backup-target-generation-installed.json`。该标记存在后，缺文件即拒绝，不因其余目标未完成而补造。

目录清单只接受配方内的精确文件／目录和原子写入器自己的临时文件名字；不承认任意用户路径或额外活动 journal。未知文件保留并阻止继续，不做“清理后成功”。实际数据文件全部按原配方读回，局部损坏不从另一个目标回退宣称完整。

## 全体完成与原任务恢复

目标安装完毕后释放大配方，再独立执行一轮完整全体证明观察和配方重建。全体计划必须逐字节仍相同；所有目标都须保有准确本地完成、完整 namespace、实际归档和普通成员读回。随后才在目标、原盘和 SYSTEM 写同一 `backup-target-generation-set-installed.json`，并读回全部副本。

任一参与根已有共同完成后，恢复必须先核 SYSTEM 原意图以及所有目标的本地选择、完成与实际材料。只允许给剩余参与根补写相同共同完成记录，不重建目标任何本地材料、不重新选择路径、不改原决定。

早期冻结材料读者必须能在同一个后续任务中再次观察完整 native／数据库／正文证据。原 native metadata 选择因此仅增加一个布局例外：本地 locator 选择必须与原 native stage、同一物理根和原 `disk.json` authority 一致，才允许上述四个 namespace 共存。实际文件内容仍由后续完整配方逐文件重验；这个例外不授予 writer、源权限或普通池启动能力。

`inspect_backup_target_locators` 只借用当前 Engine 保留的原任务真实 target gate，重验全体实际证据，不重新创建 gate、文件或选择。`GenerationTargetLocatorsInstalled` 仍是中间阶段，不报告迁移成功。

## 预算与剩余闭环

沿用一至八个完整目标和最多三十二个原盘。每目标最多 65,536 个文件、512 MiB 配方，文件摘要清单最多 32 MiB；SYSTEM／共同描述各最多 64 KiB，本地选择及完成各最多 8 KiB。原 native control archive 自身继续受 32,768 文件／256 MiB 材料上限约束，不复制出另一份完整归档字节再持有。

每目标容量预检为尚缺文件总字节、最大尚缺文件的原子暂存空间、每缺文件 8 KiB 开销、1 MiB 完成余量及原 metadata reserve；所有参与根也要求 reserve 加 1 MiB。预检不是空间预留。已存在准确材料不重新计算成待复制正文，但仍逐一读回。

清单和内容检查规模受固定上限约束；目录枚举上限为四倍文件上限，证明构造使用原 map 的有界路径，保留取消检查。原归档、全部正文和数据库仍需重复完整观察，不承诺增量成本、瞬时取消、峰值内存上限或吞吐改善。没有减少 fsync、数据证明或来源围栏。

仍须闭合原盘共同活动 selector／journal、活动 Config／Current 与全部数据库及 serving locator 的共同切换、正常后续 Publication、SYSTEM 解静默及安全源解围栏，再将原任务持有的资源交给普通 Engine。普通设置自动触发和完整进度也未完成，不能仅凭目标普通名字齐全就提前返回备份成功。
