# Backup 原盘的独立原生代际控制材料

[任务清单](../../TASKS.md) · [目标恢复地址](backup-target-native-locators.md) · [Engine](../components/engine.md)

原候选 `4bd46a46` 对齐 `44c9d6e6` 后，已由唯一协调者发布为 `55f3d092`；目标 DiskWriter I/O、inode 绑定 seal 和普通目标 locator 三片已在 `90450cf0` 合入。本片继续为实际全池切换补齐原盘自身的控制材料，发布不等于编译、测试通过或全池切换完成。

原首笔 Backup 的真实 ACCEPT／FINALIZE／Publication 已在每个原 metadata 成员的普通 `metadata/membership-generations/<authority>/journal.bin`。不能为了准备活动资源，再写一个空 journal 或从 receipt JSON 重造这些帧。新目标上的完整归档也不能代替每个原盘自己的独立恢复材料。

本片在原任务真实源能力、全部目标 locator、正文、数据库和首笔 Publication 重验后，为全部原盘不覆盖安装同代 `authority.json`、完整 native control archive 及所有历史 inclusion proof。实际原 journal、原 Genesis anchor、原 seal 和源目录身份全程保持不变。

**没有安装原盘 `generation.json` 或更换原盘 seal，没有改变 Engine Config／Current、来源 I/O epoch、SYSTEM 静默或来源围栏，也不允许第二笔 Publication 或 H03。** 只有这些实物准备完成，后续共同活动选择才有各根独立可消费的控制材料。未执行本安装流程或操作真实盘。

## 真实首笔文件，而非可重放配方

`BackupFirstJournal::published_binding` 从原持有根和普通 I/O binding 只读打开实际 journal，最多读取 16 KiB。保留同一文件描述符，检查单链接、设备／inode 和固定长度，使用生产 native 帧读取器核完整三帧及准确首笔 Candidate，再逐字节对应实际 Publication seal 中该原成员的最后 receipt。

该观察不创建、截断、追加、同步或修复 journal，不返回 JournalWriter。完整文件摘要和实际设备／inode进入全体计划；少帧、撕裂尾部、第四帧、错误 Candidate、错误的原成员根、替换 inode 或 Publication 不符均不能生成完成报告。原 data-only 盘若有该代际 journal，保留拒绝，不升级为投票成员。

随后每次原根检查都重新读取该真实文件并比较准确绑定。SYSTEM 原意图或本地选择存在后，原 journal 即使被同字节新 inode 替换，也不能凭新的现场观察重签原计划。

## 独立副本与持久选择

配方复用已封闭的原生 target locator 构造器，但原盘文件集合明确剔除 `metadata/pool-membership/config.json` 和 `metadata/pool-membership/generation.json`。来源 I/O 许可只来自原 `Sources` 所持目录、writer 和普通租约，不从新目标 Config 或本地选择 JSON 获得。

每个原盘安装：

- `metadata/membership-generations/<authority>/authority.json`；
- `metadata/membership-pending/<transition>/control-archive` 内完整固定 Genesis 前缀、原 fenced journals、原成员身份及 native objects；
- `metadata/membership-history/<candidate>.json` 的完整原历史 inclusion proof，包括正文不可用的历史。

原盘的 Config anchor、seal、root identity、角色和真实首笔 journal 绑定加入 SYSTEM 的 `backup-preparation/original-generation-control.json`。全体 plan 还绑定源 fence、原决定／Publication、已完成的目标 locator、完整 archive 及文件清单摘要。

全部原根预检先于 SYSTEM 原意图；SYSTEM 意图先于所有本地 `backup-original-generation-selected.json`；所有本地选择先于第一个原盘材料写入。未拥有的现存配方文件即使完全同字节也不采用。既有原 journal 是唯一另行严格认证、仅保留不写的例外。

已有根的其它 namespace 不被遍历成新权限或清理；本次 authority 目录、准确 archive 目录和历史 proof 目录则做有界闭集枚举。未知文件、错误目录、意外 journal 全部保留拒绝。每个预期文件不覆盖安装并读回，随后从该原盘自身普通 archive 地址执行完整生产归档重放，不借其它盘补足缺件。

## 完成、冷续作与观察

单个原盘实际归档重放成功后写 `backup-original-generation-installed.json`，内容与原本地选择相同。只读检查从不补写。原任务可续未完成的匹配部分写入，但已存在本地完成后缺失／损坏材料不能降级为部分任务后再修复。

原盘全部完成后释放大配方，再独立重验来源、所有目标及真实首笔 Publication，重建的全体计划必须仍相同。所有原盘的本地完成、完整材料、实际旧 anchor／seal 和原 journal 绑定再次成立，才在目标、原盘、SYSTEM 保存同一 `backup-original-generation-set-installed.json` 并读回。

任一共同完成记录存在后，全部原盘都必须保有原本地选择、完成和完整实物。显式原任务只可补剩余相同共同记录，不能重造 journal、变更角色、移动根或修复已完成副本。旧原盘 selector 和 seal 不改变，因此仍通过原冻结 Engine 的同一任务恢复，不提前把冷开导向尚未闭合的普通 Backup 激活路径。

`inspect_backup_original_control` 借用当前 Engine 保留的原任务资源，做全体只读再观察；`GenerationOriginalControlInstalled` 只是原盘控制实物阶段，不代表新池启动、Current 安装或迁移成功。

## 上限与验证状态

沿用最多 32 个原盘及 1 至 8 个完整目标；每原盘最多 65,536 个材料文件／512 MiB，native archive 本身仍受 32,768 文件／256 MiB 限制。全体记录最多 64 KiB，本地选择及完成最多 8 KiB，摘要清单最多 32 MiB。journal 的既有三帧不计作可复制材料或新投票。

**新决定之前**，原根容量预检在现有 common-database 预算外，额外计入 512 MiB 材料、256 MiB 最大 archive 文件原子暂存、每文件 8 KiB 分配余量及 1 MiB 完成余量，再加 metadata reserve。这个保守检查不预留磁盘空间；已存在决定不重新强制该最坏预算，只按实际缺失字节、最大缺件、文件数和 reserve 检查。ENOSPC 仍保留准确未完成任务，不删除旧数据腾空间。

全部实际正文、数据库、归档和第一笔证据仍需重复完整观察，不声称增量运行、瞬时取消、低峰值内存或性能提升。没有删除旧数据、减少独立副本或减弱 fsync。

新增八项临时目录测试仅为库存／保留边界的源码夹具，不冒充 native Publication 或端到端切换验收。按用户最新要求没有本地编译或运行测试、没有手动触发新 CI；原候选仅格式化修改的 Rust 源码，本次对齐后的组合没有重新编译或运行测试。实际原 journal 绑定、新阶段的中断续作、全体组合、真实掉盘／断电和最终活动切换尚未验证。

后继[目标普通 serving 实物](backup-ordinary-serving-entities.md)接普通容器／对象库与实际读取闭包；尚不安装活动 Current 或解除围栏。
