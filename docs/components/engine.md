# ZXS 存储核心

`storage-engine` 实现单写者、完整文件版本、不可变发布证明与离线恢复。对外入口由 `lib.rs` 导出；实现模块默认私有，不允许调用方绕过 `Engine` 的业务约束、操作幂等与发布流程。

| 目录 / 模块 | 职责 |
| --- | --- |
| `engine.rs` | 生命周期、业务准入、操作查询、在克隆状态上组合一次业务变更 |
| `engine/config.rs` | 拓扑与额度配置、独占操作系统锁 |
| `disk/owned.rs` | 同代磁盘根与身份所有权、盘内 FD 相对 I/O；不持 writer lock |
| `engine/publication.rs` | 正文准备、精确元数据预检、持久决定与发布屏障 |
| `engine/recovery.rs` | 仅沿已验证发布前缀回放，协调未完成决定 |
| `engine/reads.rs` | 副本验证、读租约、导出、磁盘容量与状态 |
| `engine/integrity_scrub.rs` | 独占 Engine 的有界完整巡检，分开报告 metadata 闭包与 required payload 副本 |
| `publication_evidence.rs` | 只读选择精确摘要的对象、完整快照及发布 seal；查询不执行磁盘修复 |
| `engine/workspace_storage.rs` | 工作区清单的双位置持久化与重启协调 |
| `engine/workspace_limits.rs` | 独立于持久配置的本机运行期保留证据上限 |
| `engine/index.rs` | 可重建 SQLite 投影 |
| `business/` | 仓库、命名空间、上传、文件分支、目录批量分支与历史查询 |
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

## 权威数据与投影

### 磁盘资源生命周期

`Engine` 的 `Arc<DiskResources>` 保存一次准入后的磁盘根 FD 与实际身份，独立于配置和 `_locks`。Planning、detached publication、内容读计划和 scrub 共用该代资源；盘内遍历、marker、正文/证明、对象图、journal、镜像、修复与 checkpoint 均从同一根执行相对 I/O，容量也读取该 FD。根路径改名或替换不把普通 I/O 转向新盘；缺席成员必须重开后才重新准入。资源 owner 不持 writer lock，内容读计划不会延长写者锁生命周期，写准备仍显式保留原 OS 锁。

初始化、重开和停服重配验证 held FD 的目录 device/inode 与严格 seal；不探测 mountinfo/sysfs、Btrfs UUID 或物理独立性。共享底层故障域由用户保证。真实文件读取、seal/成员证明、正文哈希和文件/目录同步不由身份代替。显式 `check`、成员 probe、操作/版本 health、磁盘状态及容量 observation 仍描述当前命名路径，不作写入准入。公开输入路径不根据前缀猜作盘内来源；内部复制显式携带来源 owner。资源与候选代际、冷准入目录身份要求、seal 时点、同步预算及外部路径边界见 [ADR 0007](../adr/0007-owned-disk-resources.md)。

持久化 `disk.json` 的 JSON 语法或 identity schema 解码失败携带 `DISK_IDENTITY_MISMATCH` 上下文，保留具体 JSON source。打开、读取和元数据字节上限失败继续保留原错误；此解码上下文不替代已有的实际身份比较或发布阶段错误。

业务操作先在 State 克隆上组合修改；分支创建只登记冻结引用，不新建空文件。实际内容写入才准备 FULL 和固定路由副本。DirectoryBranch 保持稳定 ID/名称，DirectoryTree 保存不可变组合；文件修改或子项选择只增加子树并推进 head。PREPARE 不表示可见，ACCEPT → FINALIZE → PUBLICATION 及同步收据共同决定公开状态，恢复沿原候选继续。

命名空间删除在实际执行处收集有序路径和条目种类，随不可变 Commit 保存删除时间；空效果不创建事实，移动、改名和视图选择不冒充删除。工作区与目录批量操作复用同一规则，失败不发布部分事实。删除事实不保存正文身份，不扩大历史保留根；历史包与拆分保留事实本身，活动投影不回查操作回执或父快照推测删除。

恢复读取 candidate/publication 时逐份验证日志绑定的精确摘要，不因首份对象缺失或损坏就忽略健康副本。仅用验证通过的对象修复安全的普通文件或缺失目标；所有副本失效时停止，不用多数票替代摘要，也不覆盖符号链接或硬链接目标。

SQLite、容器 container.json 及当前名称是可重建投影，不能替代发布记录和不可变证明。新建分支不生成占位文件；检测到 legacy 或 pending-branches 旧容器结构时返回 `FORMAT_UNSUPPORTED`，不删除或迁移原材料。活动文件分支、工作区和读租约固定正文；目录只固定选中及尚未完成分支的当前 head，不永久固定旧子树。

读取继承冻结父版本，范围编辑保留父未改字节；零字节或同摘要首写仍建立独立版本。历史另存只为 staged 文件生成 FULL。Branch/Commit 空映射及新增目录 map 按当前稀疏编码省略序列化；FULL 证明仍使用确定性编码。语义、目录子树与默认视图见[分支说明](../design/branch-system-logic.md)。

## 派生索引不可用与发布结果

权威日志、证明及 State/快照安装完成之后，SQLite 更新失败不改变原 `OperationResult`、永久幂等收据或已完成成员元数据的 epoch。`QueryProof` 用单一 `Unbuilt / Ready / Unavailable` 状态持有连接或首个具体故障；故障保留 Read/Update 阶段、当时发布序号和完整 source，关闭失败附在原失败之后，不能覆盖首要原因或授权隔离替换。停用后不再执行增量 SQL／差分，历史读取从 State 扫描；完整重建成功才回到 Ready。失败状态不是内容副本的 `current_health`，也不授予写权限。

`Engine::index_health` 是不做 I/O 的类型化观察。HTTP `/readyz` 在引擎和管理模式都投影独立 `history_index` 状态及安全错误码，不输出内部路径、SQL 或完整错误；仅派生索引失效不把已有可写服务说成不可用。停用转换保留内部具体错误供应用诊断；核心不在库内新增请求日志。重复写、查询或健康轮询不重复访问故障索引，也不重复创建失败记录。

正常更新没有新增文件/目录同步，仍执行原 SQLite FULL 事务；失效后省去索引差分及 SQL，State 扫描成本不变，不能据此声称吞吐提升。启动／重开／显式恢复的重建错误继续直接传播，只有已识别 SQLite 损坏才按原规则隔离，不将权限、I/O 或不安全路径伪装成可替换损坏。工作区终态镜像失败是独立错误窗口，具体限制见[发布证据与错误边界](../10-transactions-and-recovery.md#s10-10)。

## 完整巡检（过渡实现）

`Engine::scrub` 实读每个配置元数据成员的已准入 journal/发布证明与全部保留元数据闭包，
并逐 required route 验证当前 AVAILABLE 正文。它不替换 `check` 的磁盘目录诊断，
不修复、不发布 State，也不删除普通写校验。当前整次保持独占 Engine，并拒绝已准入的
锁外 commit preparation；它不是磁盘快照或最终后台减锁方案。起止时间、取消/失败、
有限问题记录、去重和资源上限见[巡检契约](../architecture/integrity-scrub.md)。

## 事务元数据的记录级共享

State 对 `commits`、`directory_trees` 与 `versions` 使用 `SharedRecords<T>`：克隆时共享有序映射及记录值，借用读取仍返回原记录类型。首次修改一个仍被共享的映射时，复制全部键和记录句柄，成本随记录数增长；取得某条共享记录的可变引用时，再复制该条记录及其嵌套条目。候选修改不会影响已发布基态或其他候选，失败或放弃候选不会发布局部修改。拆分中丢弃旧提交使用不取回记录值的删除路径，批量扩展也不为丢弃的替换值复制载荷。

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
