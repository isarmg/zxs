# Backup 目标的普通 serving 实物

[任务清单](../../TASKS.md) · [原盘控制材料](backup-original-generation-control.md) · [Engine](../components/engine.md)

本地后继基于已发布 `24f0c31b`，续接 `55f3d092` 原盘控制材料。原生 writer、seal、目标与原盘控制归档齐全后，目标的 `published-serving-image` 仍在隔离地址；不能直接把它当普通 Engine 的容器、对象库和版本地址。

本片在每个目标真实原 DiskWriter 租约下，安装普通 `histories/<history>/...` 内完整容器、独立正文、FILE／目录 SQLite 和普通 inode locator，并通过实际普通读取器逐项核回。**仍不修改原盘 selector／seal／journal、活动 Config／Current、SYSTEM 静默或来源围栏；没有第二笔 Publication，也没有开放 H03 或宣称迁移完成。** 未执行安装或操作真实盘。

## 来源与地址

必须先重验原决定、真实首笔 Publication、全部目标及原盘完整控制材料、普通 SYSTEM／盘根库和普通 native 对象。每目标配方使用该目标自身完整已准入 State 和 registry，不能借另一个目标的 State／数据库替代缺件。实际全体 writer、root inode 和原 source fence 始终保留。

- 已初始化目录采用既有纯目录布局规划器，完整父子森林位于该历史下 `directory-repositories/<根仓库 ID>/<原直接上游前缀>/...`。每个真实目录 ID 都有标准 `.zxs-layout/directories` inode locator，父子目录和所属完整文件仍在同一目标内，既有普通缺 locator 库存扫描仍能找到这些实体，不会因换到未知命名空间而生成第二个容器。
- 未托管的完整文件采用该已发布 State 的准确目标 `file_backup_paths`，位于普通 `file-containers/<file>/namespace/<目标路径>`；命名空间及容器 owner marker 使用既有格式。已托管文件使用普通 directory FileAddress 指向同一森林。
- 每个保留文件都有普通 `.zxs-layout/files` locator、完整 FILE 数据库和版本目录，包括所有正文不可用的保留历史。正文只为 Available 版本独立复制到容器的 `versions/<storage_name>`，不为 Purged／Retired／Migrated 伪造正文。
- 已初始化目录都有其完整目录 SQLite；未初始化的空目录按准确 outer repository 和 hop 保存，不伪装为新初始化仓库。
- 旧 native package、`activation-image` 和 `published-serving-image` 全部保持原样。复制不移动源正文、不做 hardlink、reflink 或去重；原任务中断后仍能按旧路径重验原材料。

locator 配方来自已准入 State，不从磁盘记录提取新路径或权限。标准有限地址解析器核真实容器 owner、目录 inode、精确名字和位置。这里不会调用普通布局修复来把未知材料收编，也不会以全局默认值补缺失所有权。

## 选择、安装与恢复

SYSTEM `backup-preparation/ordinary-serving.json` 绑定原 fence、决定、Publication、原盘控制完成摘要和全部目标配方／物理根。预检所有目标后，SYSTEM 意图先行，每目标 `backup-ordinary-serving-selected.json` 全部落盘，再开始第一个普通实体写入。

选择之前，任何现存 `backup-entities`、`file-containers`、`directory-repositories` 或 `.zxs-layout` 都拒绝采用，即使为空或同字节。只遍历已准入历史的上述固定命名空间；未知文件、未知目录、链接及特殊文件保留拒绝，不清理另一历史或原 package。选择之后可以续原配方的未完成部分，不覆盖异内容文件。

所有普通材料使用既有 no-clobber 原子写入／独立验证复制与同步。先完成实际目录和文件，再保存绑定实际 inode 的普通 locator。重试继续相同记录和同步尾部，不替换原实体身份。正常单目标验证要求：

1. 配方目录及元数据完整，内容逐字节等于该目标已发布 State 的原配方；
2. 全部 locator 经标准读者解析为准确物理地址和 owner；
3. Available 正文通过普通 `content_path_at` 指向本片容器，而不是 package fallback，再由 `open_payload_at` 和普通完整校验读取器验证实际正文与校验表；
4. 从同一实际文件描述符验证的内容、设备／inode及稳定文件戳一致，命名路径仍指向它。

`backup-ordinary-serving-installed.json` 保存配方摘要与全部实际目录、文件、locator 的设备／inode。该记录一旦存在，缺件、改字节、丢 locator、同字节新 inode 均拒绝，不因其它目标尚未完成而重建。只读观察不创建材料或修复。

全部目标本地完成后，释放旧计划并重新观察原完整证据，再构造相同全体计划和核验全部实际普通实体。只有原计划不变且每目标原 inode 完成仍完整，才向全部目标、原盘和 SYSTEM 写入共同 `backup-ordinary-serving-set-installed.json`。任一共同记录存在后，只可补其它相同共同记录，不重造本地材料。

公开 `inspect_backup_ordinary_serving` 仍需要 Engine 所持原任务和实际 target writer；它返回全体实际读回报告，不返回可写 State、FD 或普通 Current。`GenerationOrdinaryServingInstalled` 只是普通目标实物阶段。

## 预算、验证与未闭合部分

沿用最多 32 原盘、1 至 8 完整目标。每目标至多 100,000 个目录／文件／locator，元数据及路径累计 96 MiB，配方与 inode 完成记录各 32 MiB；全体意图／完成各 64 KiB，本地选择 8 KiB，临时原子文件最多 1,024 个。未知材料不计成可用实体。

新不可逆决定前增加一份完整普通正文及最大正文原子暂存的保守上界（合计最多两倍总正文预算）、96 MiB 元数据、两份 32 MiB 清单、每条 8 KiB 开销和 16 MiB 余量。已有决定续作按缺失正文／材料、最大缺件原子暂存、inode 清单、目录开销及 metadata reserve 检查；已有本地完成只读验证，不重新要求整份正文空闲空间。容量检查不是空间预留，ENOSPC 仍保留准确未完成任务。

这会增加完整独立复制及反复全量校验的磁盘、内存和 CPU 成本，不声称增量复杂度或性能优化，不减少 fsync。按用户要求，本次仅源码开发，未本地编译、运行测试或触发 CI。新增十二项临时目录／标准 locator 边界夹具仅源码，不是原生 Publication、SQLite 语义、实际正文整链或最终活动切换验收。

仍需原盘对象库与后继普通布局维护、全体活动 selector／seal、活动 Config／Current 和 writer 所有权交接、原决定冷恢复、后继 Publication、SYSTEM 解静默及安全来源解围栏的共同闭环。不能仅凭本片普通名字齐全就向用户报告迁移成功。

本片原候选基于 `24f0c31b`，本次对齐 `8560cb66`；保留主线数据库与跨盘 move 后继。组合仅源码整合与格式化，未本地编译、运行测试或触发 CI，不沿用历史验证结论。
