# ZXS 存储核心

`storage-engine` 实现单写者、完整文件版本、不可变发布证明与离线恢复。对外入口由 `lib.rs` 导出；实现模块默认私有，不允许调用方绕过 `Engine` 的业务约束、操作幂等与发布流程。

| 目录 / 模块 | 职责 |
| --- | --- |
| `engine.rs` | 生命周期、业务准入、操作查询、在克隆状态上组合一次业务变更 |
| `engine/config.rs` | 拓扑与额度配置、独占操作系统锁 |
| `engine/disk_registry.rs`、`engine/disk_lifecycle/plan.rs` | 借用当前已准入登记与冻结业务图，生成不执行注销的分页影响预览 |
| `disk/owned.rs` | 同代磁盘根与身份所有权、盘内 FD 相对 I/O；不持 writer lock |
| `engine/publication.rs` | 正文准备、精确元数据预检、持久决定与发布屏障 |
| `engine/backup_preparation/`、`engine/membership_transition/backup.rs` | 完整目标后台准备与[原生源围栏/认证取消](../design/backup-source-fence.md)；无最终目标切换，仍保留原数据 |
| `engine/recovery.rs` | 仅沿已验证发布前缀回放，协调未完成决定 |
| `engine/reads.rs` | 副本验证、读租约、导出、磁盘容量与状态 |
| `engine/integrity_scrub.rs` | 独占 Engine 的有界完整巡检，分开报告 metadata 闭包与 required payload 副本 |
| `publication_evidence.rs` | 只读选择精确摘要的对象、完整快照及发布 seal；查询不执行磁盘修复 |
| `engine/workspace_storage.rs` | 工作区清单的双位置持久化与重启协调 |
| `engine/workspace_limits.rs` | 独立于持久配置的本机运行期保留证据上限 |
| `engine/index.rs`、`engine/index/system_changes/` | 可重建 SQLite 历史索引及系统全局回执/路径行投影 |
| `directory_conversion/` | [磁盘向导普通目录转换](engine/src/directory_conversion.md)：只读预览、非覆盖导入和显式当前视图导出，不接纳外来 ZXS 历史 |
| `foreign_import/migration/` | [外来历史离线迁入包](engine/src/foreign_migration.md)：精确来源分组、完整独立副本、非覆盖发布和断点恢复；不是活动池接纳 |
| `business/` | 仓库、命名空间、上传、文件分支、目录批量分支与历史查询 |
| `business/history_copy.rs`、`engine/disk_registry/current/history_copy.rs` | 单文件在线历史 fork、精确写集重放及独立正文来源／候选向量准入；范围见[写入语义](../11-write-semantics.md) |
| `business/namespace.rs` | 路径校验、父目录约束、命名空间改动与工作区冲突规则 |
| `business/namespace/draft.rs` | 工作区提交内的临时路径索引与保序条目草稿，不用于读取或持久缓存 |
| `business/ancestry.rs` | 基于子节点索引的提交后代查询，供拆分与历史包校验复用 |
| `journal.rs` / `metadata_budget.rs` | 哈希对象、日志帧、证明与实际序列化大小限制 |
| `state_projection.rs` / `state_projection/` | history 与容器共同消费的 State 派生成员及精确写集闭包，不是编码或持久化权威 |
| `container_storage.rs` | 普通版本文件、真实容器改名、继承引用缓存与格式准入 |
| `historical_save.rs` / `maintenance.rs` | 历史另存分支、计划、清理与历史拆分 |
| `history_package.rs` | 历史包证明、精确图对象选择与修复、离线校验、导出及恢复导入 |
| `history_package/manifest.rs` | `HistoryManifest` 的历史域 State 投影与逻辑闭包校验；不执行存储 I/O |
| `history_package/graph.rs` | 确定性分页根、索引与叶页编码和解析；校验图形状与引用闭包 |
| `repair.rs` / `degraded_recovery.rs` | 修复原始候选与显式永久丢失恢复 |
| `wire.rs` / `protection.rs` | 冻结上传清单、请求摘要、后续版本放置与保护规则 |

## 显式逐盘格式切片

`disk_authority.rs` 与 `disk_authority/` 为独立新格式提供真实持久日志、文件版本链、来源 checkpoint 备份和可重建系统投影，写入调用方是 `zxs-helper authority`；管理面的独立盘预览只读消费已验证来源图和本盘保留版本清单。它不接入当前 Engine 的池 State、命名空间或在线写者准入。边界、命令、恢复与未切换消费者见[逐盘权威切片](../design/independent-disk-journal.md)。

## 权威数据与投影

### 磁盘资源生命周期

`Engine` 的 `Arc<DiskResources>` 保存一次准入后的磁盘根 FD 与实际身份，独立于配置和 `_locks`。Planning、detached publication、内容读计划和 scrub 共用该代资源；盘内遍历、marker、正文/证明、对象图、journal、镜像、修复与 checkpoint 均从同一根执行相对 I/O，容量也读取该 FD。根路径改名或替换不把普通 I/O 转向新盘；缺席成员必须重开后才重新准入。资源 owner 不持 writer lock，内容读计划不会延长写者锁生命周期，写准备仍显式保留原 OS 锁。

初始化、重开和停服重配验证 held FD 的目录 device/inode 与严格 seal；不探测 mountinfo/sysfs、Btrfs UUID 或物理独立性。共享底层故障域由用户保证。真实文件读取、seal/成员证明、正文哈希和文件/目录同步不由身份代替。显式 `check`、成员 probe、操作/版本 health、磁盘状态及容量 observation 仍描述当前命名路径，不作写入准入。公开输入路径不根据前缀猜作盘内来源；内部复制显式携带来源 owner。资源与候选代际、冷准入目录身份要求、seal 时点、同步预算及外部路径边界见 [ADR 0007](../adr/0007-owned-disk-resources.md)。

磁盘观察的 `sample()` 分开保存绑定/逻辑账本与容量 `Result`；失败保留 slot、`Seal / Capacity` 阶段和具体 source。seal 通过但容量读取失败时仍表示身份在线，容量未知；seal 失败仍可作为离线成员显示，不能误报主机挂载点容量。`disk_status()` 的既有 `Result` 在任一容量未知时返回首个类型化错误，而非成功返回 0；需要部分成员状态的消费者使用 `sample()`。成功的 0 字节样本与不可用不同。此观察不改变写入准入、held-root I/O、同步屏障或原有 metadata epoch 撤销条件；普通容量 syscall 错误不被扩大为 metadata 身份故障。

持久化 `disk.json` 的 JSON 语法或 identity schema 解码失败携带 `DISK_IDENTITY_MISMATCH` 上下文，保留具体 JSON source。打开、读取和元数据字节上限失败继续保留原错误；此解码上下文不替代已有的实际身份比较或发布阶段错误。

业务操作先在 State 克隆上组合修改；分支创建只登记冻结引用，不新建空文件。实际内容写入才准备 FULL 和固定路由副本。DirectoryBranch 保持稳定 ID/名称，DirectoryTree 保存不可变组合；文件修改或子项选择只增加子树并推进 head。PREPARE 不表示可见，ACCEPT → FINALIZE → PUBLICATION 及同步收据共同决定公开状态，恢复沿原候选继续。

命名空间删除在实际执行处收集有序路径和条目种类，随不可变 Commit 保存删除时间；空效果不创建事实，移动、改名和视图选择不冒充删除。工作区与目录批量操作复用同一规则，失败不发布部分事实。删除事实不保存正文身份，不扩大历史保留根；历史包与拆分保留事实本身，活动投影不回查操作回执或父快照推测删除。

未决操作查询保留证据成员 seal 拒绝的首个具体原因及 disk/slot 上下文。健康成员能提供精确候选、快照与操作页时仍可回答；否则不能把缺席或损坏成员当作操作不存在。只有可信扫描无目标且没有未解决的读取错误才返回 `OPERATION_NOT_FOUND`。已发布内存收据仍不读取无关未决证据。历史包导出、墓碑清理及工作区完成核对保持原成功条件和 `RECOVERY_REQUIRED` 分类，证据不足时附带成员拒绝来源。此选择不授权写入、不修改磁盘，也不增加同步次数。

恢复读取 candidate/publication 时逐份验证日志绑定的精确摘要，不因首份对象缺失或损坏就忽略健康副本。仅用验证通过的对象修复安全的普通文件或缺失目标；所有副本失效时停止，不用多数票替代摘要，也不覆盖符号链接或硬链接目标。

SQLite、容器 container.json 及当前名称是可重建投影，不能替代发布记录和不可变证明。新建分支不生成占位文件；检测到 legacy 或 pending-branches 旧容器结构时返回 `FORMAT_UNSUPPORTED`，不删除或迁移原材料。活动文件分支、工作区和读租约固定正文；当前清除准入固定所有保留的目录分支 HEAD 中的文件正文，包括未选中及尚未完成的分支。切换目录选择或完成物化本身不解除这些 HEAD 引用；所有相关保留 HEAD 推进后，旧目录树的存在本身不永久固定旧正文。

读取继承冻结父版本，范围编辑保留父未改字节；零字节或同摘要首写仍建立独立版本。历史另存只为 staged 文件生成 FULL。Branch/Commit 空映射及新增目录 map 按当前稀疏编码省略序列化；FULL 证明仍使用确定性编码。语义、目录子树与默认视图见[分支说明](../design/branch-system-logic.md)。

## 派生索引不可用与发布结果

权威日志、证明及 State/快照安装完成之后，SQLite 更新失败不改变原 `OperationResult`、永久幂等收据或已完成成员元数据的 epoch。`QueryProof` 用单一 `Unbuilt / Ready / Unavailable` 状态持有连接或首个具体故障；故障保留 Read/Update 阶段、当时发布序号和完整 source，关闭失败附在原失败之后，不能覆盖首要原因或授权隔离替换。停用后不再执行增量 SQL／差分，原历史候选读取从 State 扫描；新增系统变更库读取明确返回 `SystemChangeReadError::Unavailable`，不能静默变成空页或其它数据源。完整重建成功才回到 Ready。失败状态不是内容副本的 `current_health`，也不授予写权限。

`Engine::index_health` 是不做 I/O 的类型化观察。HTTP `/readyz` 在引擎和管理模式都投影独立 `history_index` 状态及安全错误码，不输出内部路径、SQL 或完整错误；仅派生索引失效不把已有可写服务说成不可用。停用转换保留内部具体错误供应用诊断；核心不在库内新增请求日志。重复写、查询或健康轮询不重复访问故障索引，也不重复创建失败记录。

正常更新没有新增文件/目录同步，仍执行原 SQLite FULL 事务；失效后省去索引差分及 SQL，State 扫描成本不变，不能据此声称吞吐提升。启动／重开／显式恢复的重建错误继续直接传播，只有已识别 SQLite 损坏才按原规则隔离，不将权限、I/O 或不安全路径伪装成可替换损坏。工作区终态镜像失败是独立错误窗口，具体限制见[发布证据与错误边界](../10-transactions-and-recovery.md#s10-10)。

## 系统全局变更库基础

`Engine::system_operation_records` 与 `system_file_path_nodes` 分页读取系统盘 `index.sqlite` 的完整保留结果和明确文件路径事件，范围不按当前磁盘位置过滤。每页最多 100 行、64 MiB 规范正文；游标绑定当前 State 序号、记录族和本次已准入投影实例，不能跨重建/重开复用。调用者负责外部授权，本片未增加 HTTP/Web 入口。完整行、严格模式、可信有序键及边界行一起校验，丢行/插行/错序/载荷替换均拒绝；读取失败共享现有索引健康状态，保留首个具体来源。

`system_catalog_records` 现包含三十四个完整业务 map 族，新增十九族保留版本声明、标签/元数据节点、D、合并/父边/别名、备注、u64 选择代数、内容退役状态与计划/规则。正常增量、冷重建和隔离备份 SYSTEM-current 复用同一声明；现有当前行删除不裁剪永久操作回执，catalog 页不是时间线，也不含私有认证控制。新增代码按要求未编译/测试/审查/CI。

`system_operation_principals`、`system_audit_events` 和 `system_state_metadata` 将原操作主体、保持完整位置的事件数组与三项 State 代数接入同一 SQLite 事务、可信精确分页和冷重建。事件同序号/同操作不折叠，只有原 tracked append 证明才更新后缀；其它谱系重写该派生族。目标 SYSTEM-current 编码共用声明，控制认证秘密不输出；新增源码未运行验证。

永久性属于 State 中原有回执保留契约，不属于可丢弃 SQLite 本身。`OperationResult.current_health` 是存储快照，不替代实时 `operation_record`。文件路径行保留外部旧/新路径，但不是所有内容/目录/分支事实的总日志。复杂度和恢复边界见[系统盘全局变更库基础](../design/system-change-database.md)。

## 完整巡检（过渡实现）

`Engine::scrub` 实读每个配置元数据成员的已准入 journal/发布证明与全部保留元数据闭包，
并逐 required route 验证当前 AVAILABLE 正文。它不替换 `check` 的磁盘目录诊断，
不修复、不发布 State，也不删除普通写校验。当前整次保持独占 Engine，并拒绝已准入的
锁外 commit preparation；它不是磁盘快照或最终后台减锁方案。起止时间、取消/失败、
有限问题记录、去重和资源上限见[巡检契约](../architecture/integrity-scrub.md)。

## 事务元数据的记录级共享

State 对 `commits`、`directory_trees` 与 `versions` 使用 `SharedRecords<T>`：克隆时共享有序映射及记录值，借用读取仍返回原记录类型。首次修改一个仍被共享的映射时，复制全部键和记录句柄，成本随记录数增长；取得某条共享记录的可变引用时，再复制该条记录及其嵌套条目。候选修改不会影响已发布基态或其他候选，失败或放弃候选不会发布局部修改。拆分中丢弃旧提交使用不取回记录值的删除路径，批量扩展也不为丢弃的替换值复制载荷。

永久操作结果与主体映射 `TrackedOperations<T>` 复用 `SharedRecords<T, OperationId>` 的值级共享；已有收据的 owned JSON 结果不会在每次新增操作时随整张映射深复制。借用查询仍返回 `&T`，需独立拥有的单条回复仍按原 API 克隆该条结果；替换值或取得可变引用只复制实际涉及的共享值。操作 ID 排序、规范编码、对象哈希、永久幂等及事务写集语义不变。首次插入仍需 O(N) 的旧键和记录句柄复制，并新增每条值的 Arc 分配与间接访问；事件序列及其他状态字段的全量工作未在此步骤消除，不表示整体事务为 O(写集)。

HistoryManifest 的 `commits` 与 `directory_trees` 也使用同一逻辑记录容器：构建时按历史筛选提交、按目录分支所属仓库筛选目录树，扫描源映射并仅复制所选键与记录句柄；恢复时通过 `extend_shared` 合并，不重复复制记录载荷。源状态、清单及恢复候选的后续修改仍各自隔离，不增加跨事务缓存。清单的 `versions` 仍使用普通有序映射；提交与版本字段仍必填，空目录树仍省略。

版本差分在前后 `versions` 引用同一底层映射时跳过比较；`FileVersion` 无内部可变字段，因此相同映射可证明记录未变。精确匹配前一容器谱系的 `TrackedRecords` 写集只比较改动键的 file/history/sequence 投影，净删除产生 DELETE，净新增或投影改变产生 UPSERT；同值和只改非投影字段不产生 SQL。初始重建、外来容器、无法匹配的谱系或写集超限仍执行完整有序差分，不能把继承自更早事务的写集当成本次差分。`SharedRecords::get_key_value` 借用已存储键，差分不复制 ID 或记录。这不跳过 SQLite 文件身份、序号核对、事务提交或可信查询证明的健康与序号检查，也不依赖操作名称或人工维护的变更清单。精确写集为 K 个键时，差分查找成本 O(K log N)，不改变 State 克隆、元数据持久化和全局写路径的其它成本。

共享仅改变这些映射的内存表示，不改变持久编码、规范字节或对象哈希，也不涉及读取资源所有权。每条记录增加独立分配、引用计数及读取间接访问成本。其他 State 字段仍按原方式克隆，历史仍完整驻留，序列化仍扫描记录；历史图编码仍完整展开 JSON Value，并进行条目大小计算及对象编码，未消除这些分配与扫描；不能把整个 `State::clone` 或更新操作描述为 O(1)。确定性测试覆盖载荷克隆次数、候选隔离与回滚、共享映射零比较和空差分的序号检查，固定发布样本继续逐字节验证快照和历史对象及其哈希。

## 正文准备的有界并行

正文准备按文件分组，同文件版本和各版本的保护路由保持串行；每次准备最多四个作用域工作线程，完成一组即领取下一组，不再等待同批慢文件。发现错误或捕获工作 panic 后停止新领取，已经领取的组仍完成或遇到自身错误；全部线程结束后才返回，按原分组索引选择首个失败，不按完成先后选错。慢组尚未暴露失败前可能已准备更多后续组，因此失败路径可能增加无效 I/O 和未发布准备材料；不改变既有保留规则、额度、正文缓冲或发布屏障，也不表示已有真实设备吞吐保证。

## 工作区提交的命名空间草稿

普通工作区提交使用事务内的 `NamespaceDraft`：`BTreeMap<lookup_key, slot>` 为冲突检查、暂存文件替换和父目录查找提供路径索引，`Vec<Option<Entry>>` 保存条目次序。草稿仅在这次提交组合期间存在，不保存授权、读取结果或跨事务缓存，也不改变通用 `namespace::find` 对未排序中间向量的支持。

源条目只做一次明确准入：检查路径、显示名称、规范化键和种类的一致性，拒绝重复键、缺失父目录或非目录父项；不要求输入已排序，也不把损坏来源隐式规范化或按首项/末项去重。索引按路径而非文件 ID 建立，同一文件的不同路径别名仍是独立条目。查询沿用 Unicode 规范化与完整大小写折叠，等价查询不变成改名，也不放宽新名称校验。

替换时清空旧槽并在末尾追加，新父目录按原顺序追加；因此与原有 `retain` 后 `push` 的可见次序一致。暂存文件循环结束后只展开一次，再执行原有命名空间操作、删除事实收集和最终 `validate_entries`。目录选择、文件上下文、来源版本、正文构建和最终发布规则均不变；本优化的资源及测试边界见[容量与性能说明](../19-capacity-and-performance.md#s19-04)。

### 业务查找复用既有权威关系

主命名空间查询沿 `Branch.history_tree_id → HistoryTree.repository_id → Repository` 查找，再判断当前 `main_branch_id`；这是现有 State 所有权关系，不新增反向缓存。已准入分支缺少历史或仓库属于内部不变量破坏，不能解释为普通侧分支。历史快照各自保留绑定，显式 SMB 重绑定只改变当前候选；对象授权、分支状态、工作区选择围栏与恢复校验仍由原入口执行。按分支、历史、仓库数量 B/H/R，单次查找为 O(log B + log H + log R)，不是扫描全部仓库；提交内已有仓库引用的地方直接复用。

工作区的已写文件身份只用于成员判断，使用事务内 `BTreeSet`；W 个写入身份的构造为 O(W log W)，P 个 pending 项的已写判断为 O(P log W)，不改变别名的文件身份、首次写入、未写来源保留或条目顺序。元数据请求的 `FileLocation` 在保留原路径准入、文件与仓库归属条件后，复用 `NamespaceTree` 做 O(log N) 的规范化键查找；前置版本归属查找仍扫描版本，不能将整个请求描述为对数复杂度。

路径节点回执继续完整扫描 `file_path_nodes`，按 operation 与 sequence 筛选并保持键顺序，成本为 O(N)。当前历史导入准入没有建立“已有节点的序列号必然早于下一次操作、操作 ID 必然已登记”的保证，不能仅凭事务写集排除未改动的匹配节点；此处不采用增量写集优化，也不新增校验或兼容分支。既有操作重放仍返回永久收据，不重新生成回执。上述已采用的局部查找与集合调整不改变锁、I/O、同步次数或发布屏障，不构成吞吐提升实测证据。

## 目录批量操作的类型边界

`DirectoryNamespaceBatch.kind` 使用独立的 `DirectoryBatchKind`，只表示 `move`、`copy`、`delete`、`rename`；不与工作区命名空间动作混用。HTTP 请求在构造批量请求时严格解析字符串，拒绝未知值及对象、数组等非字符串形状；库调用者直接传枚举，Web 将 UI 动作映射到该共享类型，HTTP 客户端直接序列化类型化请求。引擎不再重复检查动作字符串白名单，仍检查非空与条目上限、目标是否必需、路径、冻结基线和业务冲突。四个既有 wire 字符串、请求摘要和发布语义不变，不增加格式迁移。

## 共享规则与扩展位置

- 名称与路径只调用 `name-policy`，固定 Unicode 16.0 NFC / case folding。显示拼写可以不同，祖先关系必须按规范化键判断；移动子树时按路径组件处理显示名称，不能用另一种拼写的字节长度截断 UTF-8。
- 新写入备注调用 `storage_contract::validate_note`，最多 `MAX_NOTE_BYTES = 128` UTF-8 字节，空串清空，拒绝控制字符。历史包读取同样验证当前备注格式，不隐式重写不匹配的证明。
- 校验块大小由 `storage_contract::SUPPORTED_CHECKSUM_CHUNK_BYTES` 统一提供，编码器、配置和历史包使用同一集合。
- 新业务操作在 `business/` 或相应维护模块组合状态，经 `mutate` / `mutate_digest` 发布；不得自行写日志、提交 SQLite、改写既有版本正文。
- 目录分支选择按替换子树后完整仓库树检查 `Config.max_entries`，目录和文件均计入；单个冻结子树合法不代表与当前兄弟子树组合后仍合法。超额返回 `RESOURCE_LIMIT`，不改变选择、提交、代际、操作记录或发布日志；根目录选择替换整棵树，不叠加旧树条目。
- 新上传检查放在不可变代次复制之前，流式复制同时受到文件额度和剩余工作区额度约束。失败应保留已接受代次，避免留下一个没有清单引用、又阻止合法重试的代次。
- 工作区提交重试共用目标业务状态、所属主体及已发布请求摘要校验。原生请求使用相同冻结输入重算摘要后，从系统盘/数据盘寻找完全匹配的请求；不接受另一份可解码请求。恢复仅凭完整请求绑定协调工作区状态，wire 正文也双位置持久化；缺少旧正文证据时保留原工作区状态，不猜测或清除暂存字节。

运行期保留证据额度通过显式 Engine 构造参数传入，不修改配置锚或发布证明；正常打开、恢复与提交规划快照使用同一有效上限。运维参数与限制见[容量规范](../19-capacity-and-performance.md#runtime-retained-workspace-limits)。

## 验证

故障注入只属于非默认 `storage-engine/fault-injection` feature：关闭时，Engine
没有故障字段、注入 API、failpoint 调用或测试点名称格式化。engine 自身及 server 的
`dev-dependencies` 显式启用它，因此下面的普通测试命令仍运行全部发布/恢复故障回归，
不以 `cfg` 跳过测试。工作区 resolver 2 不把 dev feature 带入正常构建；发行构建不得
使用 `--all-features` 或显式开启 `fault-injection`。CI workspace suite 的
`check-native-consumer-features.py` 分别检查实际消费者的 normal/build 与 dev 图，拒绝
生产图中的故障注入并要求 engine/server 测试图保留它。测试构建产物不能当作发行包。

CI 和 release 共用的 workspace suite 还显式运行两个默认 ignored 的
`disk::admission_tests`：`immutable_existing_bind_leaf_is_rejected_before_read_or_sync`
与 `cold_device_admission_does_not_borrow_a_replacement_mount_identity`。测试环境需要
`/usr/bin` 下的 util-linux `unshare`、`mount`、`umount`，以及允许非交互调用的
`sudo` 和 `env`。普通构建与整个 suite 仍以 runner 用户运行；只有这两个 exact
测试的子进程通过 `sudo -n -- /usr/bin/env -i` 以 root 启动独立 mount namespace，
用 `unshare --mount --propagation private` 隔离挂载及其传播；不创建只映射 root 的
user namespace，以免丢失对 runner 所属私有构建路径的访问权。子进程仅执行
临时目录内的 bind/tmpfs 挂载及 lazy unmount。清空子进程环境后仅显式传入
对应测试的递归 marker，不继承 `LD_PRELOAD` 等变量，也不保存或输出凭据。
测试不改宿主安全设置、不访问真实数据盘、不把 sudo/namespace/挂载失败当跳过。
这些工具和权限仅为回归环境要求，生产功能不依赖它们。


```sh
cargo test --locked --offline -p storage-engine -p storage-contract -p name-policy --lib
cargo test --locked --offline -p storage-engine --test directory_selection_limits
cargo check --locked --offline -p storage-engine
```

测试覆盖发布屏障、幂等、恢复、读租约、受支持的历史树拆分及完整包恢复。新分支回归检查无正文复制或空 stub、真实空首写、缓存/改名/旧 inode、来源清除、目录组合和不可变子树闭包、共享文件身份、选择代际、目录 head 固定与历史清理；含目录组合、文件节点或独立文件分支的历史拆分明确拒绝。目录额度回归覆盖两个合法四条目状态组合为六条目时的原子拒绝、释放额度后同操作 ID 重试，以及根组合在额度边界切换并重启恢复。所有 fixture 使用临时目录，不代表物理设备故障或发行验收。

打开已有池时，缺失根目录或权限安全且完全为空的根目录视为不可用成员，可由剩余配置元数据成员的多数证明恢复只读服务；不会在这些未验证目录创建锁或恢复文件。非空目录缺少 disk.json、身份冲突、符号链接和不安全权限仍拒绝启动；配置多数门槛不会随失联成员缩小。

一般 history owner 增量、实际写集与未关闭成本见[owner closure](../architecture/history-owner-deltas.md)。

### 读取资源异常析构

当前核心读租约从 `prepare_content` 登记起持续到持有者释放，以版本引用计数阻止清除或拆分，没有期限、续约或到期排空机制。存在活跃租约时维护返回 `ContentPinned`；租约登记异常时失败关闭。当前清除通过重新检查引用后发布 `Purged`，尚未实现持久化 `PURGE_PENDING` 及禁止新租约／续约的屏障。这不降低[清除规范](../13-retention-and-deletion.md#s13-04)与[有限租约目标](../19-capacity-and-performance.md#s19-10)：交付该协议仍需一致的计划代际、续约身份／内容检查及进行中 I/O 排空，不能仅给引用计数增加 TTL 后删除正文。

读取租约析构只释放已登记的正计数；缺失或零计数会使该进程的租约登记表失败关闭，后续读取准入、purge 与拆分检查返回 `RecoveryRequired`，不把异常解释成无活跃读取。中毒登记表不恢复、不清空；析构记录异常且不二次 panic，正常业务仍拒绝使用中毒状态。历史范围快照独占的 SQLite 连接允许在析构时从中毒容器取出，仅用于关闭，不重用于查询；已有读取失败与关闭失败保留诊断。

### 磁盘观测故障验证

`cargo test -p storage-engine --lib engine::observations` 覆盖损坏 seal、未知/成功零容量区分及错误投影；零容量用 Linux nsfs 的实际 `fstatvfs` 成功样本，不伪装设备已满。独立错误测试位于同一测试模块，但默认忽略：先用 `cargo test -p storage-engine --lib --no-run` 构建，再将 Cargo 输出的测试二进制路径传给 `bash test/engine/unit/engine/observations/run-io-faults.sh <binary> <output-dir>`。runner 以进程局部 read interposer 注入 seal EACCES，以 strace 注入实际 fstatfs EIO，并检查 trace 的 INJECTED 证据；不修改生产业务函数或系统 ptrace 策略。仅在原本允许 ptrace 的 Linux CI/宿主运行；环境拒绝时明确记为未完成，不能算测试通过。容量 syscall 失败保留原 metadata epoch，seal 失败撤销旧 epoch；两者不是写入准入测试或物理掉盘验收。

CI/机械入口为 `python3 .github/scripts/check-disk-observation-io.py`：从本轮 Cargo JSON 中只选择唯一且 build-finished 成功的 storage-engine libtest，不按 target 文件名猜旧二进制；构建警告、丢失/重复 artifact、空测试、少/多注入命中或 ptrace 拒绝均失败，不降级跳过。runner 验证一次 seal EACCES 命中和两次容量 EIO 命中（sample 与 status 各一次），日志输出独立测试结果及 syscall trace。该入口需要宿主已有 `cc` 和 `strace`；机械负测试为 `python3 -m unittest discover -s test/ci -p test_disk_observation_io.py`，不执行真实 ptrace。

### 目录历史只读查询

`capture_directory_history` 捕获共享业务元数据与精确 history manifest root，`DirectoryHistoryRead::page` 在锁外按稳定目录身份筛选真实树/分支，`recheck_directory_history` 在返回前校验当前 root 与选择。复用文件历史的有界 stable-key selector；节点只包含显式用户提交，沿 `parent_tree_id` 跨过内部暂存检查点解析前一次用户提交，不把 composition 的子目录引用画为祖先。pending 分支共享来源 head，工作 head 与最近已提交 head 分别返回。当前目录未初始化时读取不产生 mutation；rename/delete/recreate 按 entry 身份隔离。参见[目录只读历史窗口](../15-api-and-errors.md#目录只读历史窗口)。

历史文件节点合并复用单次 mutation 与 `FileBranchMerge`，显式 `MergeResultPolicy::AlwaysNewNode` 保证新 `fmeta` 节点继承选择的正文与标签，所有原节点父关系不变；当前-head `/merge` 保留 `ReuseChosenInput` 规则。无需新增持久格式或迁移。


新增历史合并祖先验证共用 `business/file_ancestry`：同快照 scoped 图一次构建，merge provenance 扫描一次，BFS visited 去重，按唯一节点/边计费。来源解析先做明确作者/所有候选 head 的廉价证明，再多起点搜索；写入按 reviewed head 分组，多来源共享遍历，全部预验证先于任何派生。全局扫描 header、原始父记录、图容量和累计搜索工作均有独立硬上限；取消覆盖读侧每步。chosen 标签继承也借用单一 base parent 并共用写预算。此优化不改变旧 `/merge` 祖先规则或其资源策略，不把命名、分支派生、健康检查、发布成本声称为常数。数值与失败语义见[历史节点合并](../15-api-and-errors.md#历史节点合并)。

## 可恢复注销的纯影响预览（P1）

`Engine::preview_disk_deregistration` 只回答假设移除一个准确登记后，哪些版本及冻结命名空间 occurrence 仍有其他登记持有者。它不会注销、写恢复入口、停止 I/O、修改 Config/State/索引或把备份升级为主盘。接口是受信本地域 API，应用仍负责当前管理员认证；预览不是执行权限或“可安全删除”结论。实现与后续边界见[纯影响规划](../design/disk-deregistration-preview.md)。

修复与永久损失分类的 I/O 尾部回归源码及后续 GitHub 执行入口见[测试说明](../testing/repair-io-tails.md)。说明中的命令不代表本批已经执行。

<a id="目录暂存与用户提交"></a>

## 目录暂存与用户提交

完整权威与边界决策见[独立目录仓库](../design/directory-git-repositories.md)。

目录默认未初始化。`initialize_directory_git` 使用已审阅的主命名空间 HEAD 创建 `DirectoryGitRepository`，并登记初始无提交 HEAD 的分支。`directory_git_repositories` 在 State 快照、元数据投影、历史包分页、导入及恢复中均有显式覆盖；它随既有权威材料写入数据盘根目录，系统盘保存对应索引，已激活目录仓库还保存作用域受限的 `.zxs-directory.sqlite` 派生副本。目录内副本不是独立恢复权威。当前格式不增加旧格式迁移。

文件保存、改名、移动、文件分支选择等继续即时发布文件版本与工作组合。原不可变 DirectoryTree 仍是恢复及并发工作检查点，不自动显示为用户目录历史。已初始化仓库的自身文件修改为待提交内容，未初始化子目录递归归最近父仓库；初始化独立子仓库则改变所有权边界，使父暂存令牌失效，必须显式重新暂存。`directory_git_status` 区分工作树、Git HEAD、index generation、自有内容的 staged/unstaged、待确认边界、子仓库 dirty、未暂存引用变化和已暂存引用。

`commit_directory_changes` 同时核对 `expected_tree_id`、`expected_index_generation`、非空说明及永久操作 ID。一次提交仅冻结 `index_tree_id` 固定的自有条目与明确暂存的子仓库提交 ID，只新建一个 `directory_commit`，不推进工作树、不改主命名空间，也不替子仓库提交。该事实通过 `Commit.provenance` 精确绑定新树；树的父边和 commit 父边精确对应前一次用户提交。初次提交可以是独立根，但导入/恢复必须验证反向 marker、身份、分支继承、全部 index/HEAD/child 边及完整无环覆盖，不能靠 kind 字符串放行断开图。

`stage_directory_git_child` 固定用户审阅的子 HEAD，不自动跟随之后的子提交。`stage_directory_git_owned` 确认当前边界，并从已准入当前命名空间固定暂存快照；后续保存不改变该快照，提交后也不自动暂存这些后续更改。退出磁盘后的隐藏旧条目不能借提交重新进入当前视图。父提交之后子仓库仍可继续修改和提交，父只出现尚未暂存的引用变化。正常路径读取当前工作字节；显式历史快照预览读取对应不可变树。

分支登记要求本仓库自己干净；checkout 检查自身待提交状态并只替换自己拥有的工作范围，独立子仓库的挂载条目、工作字节、选中分支与 HEAD 原样保留。创建、切换、初始化、暂存、提交在当前登记模式下均由可重放的闭合元数据计划证明，公开 operation kind allowlist 不授予这些操作通用权限。永久回执、发布前持久化和冷恢复沿用原协议，不削减同步阶段。

单目录差异使用路径有序索引，成本随该目录工作树和已提交树增长；子仓库 dirty 检查访问其所有权树。当前登记完整计划验证包含全 State 比较，因此显式元数据操作存在随历史增长的成本。普通文件保存未增加新的 Git 元数据发布或同步阶段。没有声称内部检查点停止增长，也没有增加自动历史清理。

系统派生库增加 `system_catalog_records`，读取完整仓库/历史/分支/命名空间组成以及当前位置、placement和选择映射，见[系统变更库](../design/system-change-database.md)。它不含账户凭据，不是盘根过去归属日志或恢复权威；本次后继未执行编译/测试/CI。

盘根维护API `rebuild_disk_catalog` / `disk_catalog_snapshot` 复用安全installer在正式启用盘保存、读取严格当前范围的 `.zxs-disk.sqlite`，见[当前盘目录](../design/disk-current-catalog.md)。正常发布/正常冷启动现在维护当前库，按前后精确 binding 刷新并保留逐盘错误；不用于恢复或外来准入，本次未运行编译/测试/CI。

盘根历史维护 API `rebuild_disk_history` / `disk_history_observation` 从完整固定 genesis 发布链的相邻 State 重新计算本盘版本/owner 路径变化，保存到独立历史伴随索引，见[本盘发布变更副本](../design/disk-local-publication-history.md)。不从当前 route 推断旧归属，未支持代际归档、全部标签/目录审计或独立恢复；本片未编译/测试/审查/CI。

`disk_catalog_statuses` 是无 I/O 的逐盘维护事实，不是健康准入。当前库刷新失败与已发布业务结果分离；离线 PreservePending 不自动写库。历史伴随库新增 `disk_history_statuses` 和正常冷开/发布连续维护：完整来源建立前缀证明，后续从准确前后 State 追加本盘差异；不相关发布不重写，但持久覆盖和进程内观察覆盖分别显示。失败保留原来源，不回滚权威回执；此后继也未编译、测试、审查或运行 CI。

离线 `inspect_disk_database_material` 不创建 Engine、不打开 SYSTEM，只在明确选定的盘根及准确登记下解析/折叠当前和历史材料；本地自洽不等于已认证来源或恢复权威。原 `restore_from_disks*` 则在真实权威恢复完成后维护这两份派生索引并保留各自错误；辅助工具不再把该维护误称全部三范围恢复。见[离线数据库材料](../design/offline-disk-database-material.md)。本片未编译、测试、审查或运行 CI。

### 源围栏后隔离目标实体

`start_backup_target_installation` 在完整原生源围栏后后台复制并读回所有准确目标的 FILE 正文／数据库和目录仓库森林，冷开只续原任务；完成相位仍不授权目标，也不解除全池写停。范围、预算和硬中止临时文件拒绝边界见[目标实体安装](../design/backup-target-entity-installation.md)。本片无编译／测试／审查／CI。

`start_backup_registry_placement_decision` 在同一 source fence 的原成员共享槽内选择完整目标身份与实体地址，并将实际全成员凭据保存至全目标，冷开仅重建原任务。`RegistryPlacementDecided` 仍不释放全池停写；足够原选择后认证取消可能不再可达，不能作为普通设置的隐式阶段。准确范围见[共同 registry/placement 决定](../design/backup-registry-placement-decision.md)。本片未审查、编译、测试或 CI。

### 已决定备份目标恢复

`inspect_backup_registry_decision` 读取原成员真实共同选择和全部完整目标，供宿主独立保存决定摘要；`open_decided_backup_targets` 仅在该摘要与完整原目标集合均满足时提供有界、同次全正文校验的冻结恢复读取。原决定恢复不补缺票，读入口不生成 active Config/Generation、不开放 writer、不解除源围栏。见[具体边界](../design/backup-decided-target-recovery.md)。本片未审查、编译、测试或 CI。

### 已选完整 writer 路由材料

同一 registry decision 后台 worker 在重新读取全部原真实选择后，生成并在全部目标持久化完整扇出与整单位路由，冷开仍只续原任务。`WriterRoutesPrepared` 表示 `backup-writer-routes` 两份材料已读回，不是活动 writer；`BackupDecidedTargets::{inspect_writer_routes,file_writer_route,directory_writer_route}` 逐次重核全目标证据并解析完整明确目标集合，不按线性 `.find` 任选一个下级。父子目录和整文件所有版本共享完整目标集合。活动 Config/Current/Generation、普通写入口与源围栏不变。见[路由材料边界](../design/backup-selected-writer-routes.md)。本片未审查、编译、测试或 CI。

### Backup generation 前的路由与激活边界

真实 `PlanningContext` 和 current backup 规划现共用结构路由解析：副本数只能表达唯一链前缀，遇分叉不得任取一个盘或自动扩大副本数；显式路由材料要求完整祖先集合；旧按副本数的历史 copy 冷态准入必须重建同一唯一前缀和准确 Enabled binding，不能用结果向量伪造分叉选择。递归前缀按每个目标自身父链生成，不把兄弟盘当相邻跳。原已决定准备集合也使用同一闭包。普通 Add promotion 与普通首个 Candidate 显式拒绝 Backup 选择，认证 Abort 恢复仍保留。活动 Config 的单链约束没有放宽，没有新 Backup Generation、目标 writer 或源解围栏。见[准确范围](../design/backup-generation-route-admission.md)。本片未审查、编译、测试或 CI。

### 已决定备份的 native 路由快照材料

[未发布路由快照](../design/backup-generation-routing-snapshot.md)将已决定完整目标集的下一步写集编码为真实 native snapshot/map 对象，原后台任务无覆盖补缺；每个目标由自身完整源快照推导，保留所有旧 receipt/生命周期/私有 State 字段。显式离线原决定续作可在原源/SYSTEM 不可用时仅补隔离材料。`GenerationInputsPrepared` 不是已发布 generation：不发成功回执、不启用目标 writer、不清源围栏；真正 Backup authority、首个 publication、新范围历史库与 serving locator 仍缺。按要求本片未编译/测试/审查/CI。
