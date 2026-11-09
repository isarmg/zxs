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

## 同一切换事务内的原盘对象库

基于 `bcf065da` 的原盘适配仅作为最终 Backup 协调器的内部消费者，不增加公开阶段、独立报告或完成入口。`container_storage::backup_original_objects::Plan` 从协调器已验证的完整原 State 与该原盘自身完整 published State 构造；原决定、真实首笔 Publication、全体选择、原 seal/writer 租约和后续活动切换仍由同一协调器负责。

原 State 的版本 route 选择本槽 FILE，包括所有保留 terminal 版本；原纯布局规划器选择本槽实际已初始化目录，包括没有直接文件的目录。地址使用普通 `container_at` 和标准 inode locator，绝不照抄隔离 image，也不由另一目标齐全补造原盘缺失的实体。所有期望 SQLite 来自完整 published State 的既有严格生成／验证器，外来旧库先经准确 scope/格式/语义/物理校验再进入保旧安装。未知 FILE 子项、数据库旁文件及 WAL/SHM/journal 保留拒绝；目录内普通业务子项不由本适配器收编或删除。

配方绑定原根、确定相对地址、历史／实体／versions 目录 inode、原 owner／locator 文件 inode 与摘要，以及期望新库摘要。当前数据库 inode 不进入配方，因此相同决定中断恢复交换后配方不变。协调器须先持久保存所有原盘配方选择，才调用任何 `apply_preserving`；每个库都复用 `PreparedInstall` 的独立暂存、持久意图和保旧交换，旧字节与 inode 留在原历史安装材料中，无删除、回滚、正文复制或 locator 改写。

`verify` 只读两遍实际完整集合，期间持有实际数据库 FD，验证期望长度／摘要、原实体及命名身份、文件稳定戳和 I/O 租约，返回不可由磁盘 JSON 构造的内部 Completion。协调器保存其规范字节并在冷态精确比较：同字节新数据库 inode 也改变 Completion。任何本地或共同完成已存在后，协调器只能调用只读验证，禁止再调用安装；丢库、坏库、丢 locator 或换实体不能借本适配器修复。一次冷捕获可能先发现损坏而拒绝，仍不创建任何路径。

每盘最多 4,096 库／64 MiB 新镜像，沿用单库 16 MiB 限制；配方和完成记录各至多 4 MiB，目录直接子项每次最多 65,536 项。新任务按历史合计保旧配额并预留全部交换、9 MiB 完成记录余量及配置 metadata reserve；已决定恢复复用原暂存。Plan 同时持有有界集合的实体／owner／locator／旧库 FD，可能受进程 FD 上限限制并在写入前明确失败，不声称大规模已验证。最终协调器应逐盘重捕获／核配方，避免同时持有 32 份完整镜像。实际数据库双读、全状态投影和路径/locator 重检增加 CPU、内存和 I/O；未减少任何原安装同步步骤。

本片新增十一项根 Rust 夹具仅源码，覆盖完整 FILE＋初始化目录保旧、部分交换续作、完成后缺库和同字节换 inode、旁文件、损坏库、缺实体/locator、取消/容量溢出、terminal route 和 locator 替换。它们不是原生授权、全体共同选择、正文认证、后继 Publication、selector/seal、SYSTEM／Config／Current 或 writer 交接验收。按最新要求未本地编译、构建或运行测试、触发 CI；没有操作真实磁盘。此内部适配须随最终 Backup 单一事务整合，不独立宣称切换成功。

## 同一事务的 Backup SYSTEM 扩根与冷恢复入口

后续内部接线与上节原盘对象库一起交给一个最终事务，不增加公开准备阶段。`PreparedBackupBootstrap::select` 保留原 Engine writer 和全部 target handle，只允许原 Genesis Backup 决定；在改写任何原 selector 前，于 SYSTEM `backup-root-extension/plan.json` 和 `selected.json` 持久保存准确 authority、首 Candidate／Publication、registry、配置摘要和既有 bootstrap 的唯一准确 successor。原生对象仍是授权来源，SYSTEM 记录只能限制探测。

最终协调器完成全部真实 FILE／目录库与原 seal／selector，并产生私有 `ServingBarrier` 后，`publish` 重验实际全体新 selector／seal，再使用既有 `resume_root_extension` 发布同一根集合和 `complete.json`。不扩宽 Add、Exit、terminal 或 readmission 入口，不清 stops，不凭在线子集改投票集合。缺 head、未知原子残留、另一 operation、陌生目录 inode 都保留拒绝。

普通冷开在原限制性发现之前检查该 Backup 分支：必须有准确已发布扩根 head 与 completion，按 locator 的固定身份打开**全部**原盘及目标。每根分别读取自己的完整 archive、重建严格首 Candidate／完整 State、实际首 Publication／seal 和原／后继 registry 页；原 metadata voter 的实际 journal 必须含准确原三帧。后来普通业务 journal 后缀由现有全量重放继续处理，不能拿最新 raw State 跳过 namespace 历史。普通冷开不要求初始 SQLite inode／字节或初始所有正文永久不变，正常后继修改与派生库重建由当前 State 门禁决定。

若 SYSTEM plan 已选而扩根尚未完成，普通 open 关闭。只有显式同 operation `prepare_resume` 能接受已知原 seal／新 seal 与 selector-second 中断组合。它先在固定根内取得现存 SYSTEM writer、每盘 `.writer.lock` 和目录 reservation，绝不创建丢失锁，再逐根认证完整原生证据；此时目标仍须有首笔所需完整普通正文，原投票 journal 不得含后继业务 slot。裸 held roots 只交给同一最终协调器，按此前共同 selection 中保留的 old/new FileStamp、stage 目录和原对象完成证明补齐，不重新运行旧 seal Sources 或替换原完成库。

`PreparedBackupResume::publish` 只消费该协调器的私有 barrier，完成全根新 selector 回验和扩根后返回仍持锁的 `ResumeResources`；`into_engine` 经已有 `DiskResources::recover_held`、`DiskWriter::adopt` 和 `recovery_resources_with_locks` 转移同一 `FenceOwner`，没有释放后重新取得的窗口。之后仍由同一协调器完整重放、装 Current、重建 SYSTEM，最后处理原 fence。任一局部成功不能独立报告备份完成。

验证状态：本片仅源码和源格式化，未本地编译、测试构建、运行测试或 CI，未操作真实盘。新增八项临时根／现存 writer 锁边界夹具仅源码；不代表端到端崩溃恢复通过。依赖备份协调器同时整合的 continuation、Config 准入、common completion、Current／SYSTEM 接线，不能单独发布。仍须验证全根 record/head/selector 中断组合、原对象完成丢失拒绝、同字节新 inode、真实首笔之后写入及再冷开。当前只允许固定 Genesis Backup；已有其他 lifecycle／stop 或历史根集合不同的后继不被此入口放宽。逐根重放完整 archive／State／registry，保留既有 64 MiB 单对象、128 MiB 首 Candidate 和新增 8,192 次／128 MiB registry 读取上限，source replay 至多 2,048 笔；不声称性能或最大规模验收。

## 最终同一事务协调器（本地源码，未交付）

后续 `complete_backup_registry_placement_decision` 不再增加孤立准备报告：它从真实首 Publication、所有普通目标实物、原根和原 writer 构建一个全体选择，在 SYSTEM 与每根保存同一 `backup-serving-selected.json` 后，逐原盘安装真实 FILE/目录库，再选 Backup 专用 bootstrap 原计划。原 `disk.json` 经精确 old/new FileStamp 的保旧交换，旧 inode 留在 `backup-serving-seals/<operation>/...`，随后 no-clobber 发布对应 generation selector。任一共同完成出现之后只能核验实际整个新集合及保留旧 inode，不能补造缺原生材料。

完成全体物理读回后才产生不可反序列化的 `ServingBarrier`；bootstrap 原计划发布完成后，同一 retained DiskWriter/FenceOwner 和 I/O gate 交 Engine，完整源归档每笔重放保存原 namespace 冻结/退役/递归规则。安装真实首 State、原结果、Current、全体普通 journals 与 SYSTEM 索引后，才清这次原内存 fence；不删除历史围栏，也不能清后来新任务的阻断。后续普通 Candidate 必须携带这笔首 Publication 认证的 registry、原 generation 和固定 voters，并通过完整普通前驱/写集验证。

显式 unfinished 原操作恢复再次读取每根完整原生 archive/Publication/registry，并重新验证全目标首普通 Image（包括空目录、完整 FILE/目录 SQLite、owner/locator/inode）及原 routed Available 正文和准确首 proof。已有原对象完成逐字节和 inode 比较，绝不 apply。普通 completed 冷开则按实际后继日志重放，不把初始 SQLite 或已合法清除的版本当作永久不变；从恢复的当前 State 重建派生 SYSTEM，不从完成 JSON 生成 State。最终状态 `GenerationServingActive` 要求实际 Current 与 SYSTEM Ready 当前序列和原永久结果同时成立，早期原生 Published 收据不是成功。

统一核心任务接口保存一次完整 source/target/topology 请求。`current_backup_transaction_request` 可在首 worker 尚未创建的中断后只读定位原 op；同 op 改请求或主体拒绝。普通 Continue 只推进非决定材料，stop、不可逆 decide 与最终 activate 各需明确动作。此处不复制服务端鉴权，管理入口另片共同整合；在完成前不能对用户报告成功。

本节仅实现源码并以 rustfmt 整理语法，未本地编译、运行任何测试或 CI。新增五项根 integration 源覆盖完整 API 顺序、同决定成功重试、普通后继写/冷开、原请求替换拒绝和 completed 缺 selector 不修复、bootstrap 发布后末完成标记前的原操作恢复，以及缺原 writer 锁不补造；它们尚未执行。目标普通后继布局和实际 HTTP/Web 已同批接入本地，所有安全边界、最大规模、FD、崩溃与真实浏览器组合均未验。共同选择/完成各 256 KiB；每原盘对象计划按已有 64 MiB 限制逐盘重捕获，不同时留存 32 份；仍承担全量原生重放及正文、数据库和 inode 多次读回成本，不称增量优化。

### 后继普通写入的原选择延续（本地源码，未验证）

活动 Current 只从实际原首 Publication 持有完整的新 route 和 hop prefix。旧终端仍登记且保存原数据，不再用全 Config 的数字副本数在其与新目标之间猜选。首状态中原仓库/未变化策略保护完整低级链的设置，沿同一原生选择映射到完整目标数；后来新建或改变的设置不凭相同数字自动继承，普通显式数字仅能选唯一低级链或已明确选择的完整 route。新文件路径补原前缀，已有 owner path 不被写入器静默重写；恢复时按同一 Current 重算新 payload 路由和路径。

这不是递归策略重新签名、后续任意拓扑变更或跨代 lifecycle 授权；旧 recursive capture 的独立拒绝仍在。上述源码与真实目标后继布局及管理员入口须同批整合；未编译、未运行测试或 CI。

## 活动 Backup 后继的普通目录布局

普通 `shared_layout::prepare` 现在从完整原生首 Publication continuation 读取 `Generation::backup_layout_targets` 的固定 slot／上游 prefix，再以当前已准入 State 为每个目标生成同一套真正布局 goal。不从裸 Config 的分叉、现存 locator 或 `file_backup_paths` 猜一个活动目标；未完成 native continuation 的 Backup 仍拒绝，普通非 Backup 主布局保持原行为。

目标沿用首安装的 `target_directory` 几何，将初始化森林置于 `directory-repositories/<根 ID>/<固定 prefix>/...`。当前目录重命名／移动仍按同一目录 ID 和现有 inode 更新普通 Batch／locator；目标文件只纳入其准确 binding generation 的实际版本 route，完整历史版本继续属于同一文件容器。目标不再因原 planner 仅生成 primary goals 而在下一 Publication 被当作退役目录，并送回 stable fallback。`file_backup_paths` 原值不改写；未托管文件仍用其已准入普通稳定地址，不把旧 terminal 或任意分叉追加为新目标。

新初始化森林所需的固定 prefix 父目录仅作内部命名空间基础目录准备，先解析所有既有 locator 后才补这些空父目录，不创建实际目录实体、正文或 locator，不采用丢失实体。实际名字移动、真实目录数据库保旧安装和同步仍沿用原 Finalize 后 Batch 及数据库安装路径，不增加独立准备报告或切换阶段。goals／路径沿用 1,000,000 项与 16 MiB 上限；每个完整目标会增加相应 goal、目录数据库和布局 I/O 成本。

五项新增临时源 Engine／纯 goal 夹具仅源码，覆盖首目标几何、目录改名时保持原 backup path 元数据、后继提交保留全部版本、错 binding 拒绝和非 Backup 不推断目标。未本地编译、构建测试、运行测试或 CI；没有实际目标搬移／崩溃恢复／后继冷开验收。本片依赖同一事务的 native `backup_layout_targets` 接线，不扩宽旧递归策略 capture 的重签规则。
