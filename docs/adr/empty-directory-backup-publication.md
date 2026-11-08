# ADR：递归备份的空 scope 目录发布

状态：有限实现，发行格式未冻结。原入口实现以前明确拒绝的“选定文件集合为空”的目录 scope；有文件的既有递归配置、文件正文及其收据表示保持原逻辑。[显式全目录快照入口](#mixed-directory-snapshot)另行选择同时发布非空目录的文件与目录行，不默认扩大旧请求。Web 配置和备份列表消费同一已发布目录快照；后续 native rename/move 以新收据发布精确跟随结果，不改写旧收据。

## 持久权威与决定点

目录身份、各目标身份和完整逐级路径放在新空 scope 收据的 directory_namespace 中，由原生共享规划器从已准入 CurrentNamespaces、原前驱、冻结 RecursiveBackupCapture 及实际策略路由导出。严格新发布和冷重放重算完整收据，所以删字段、改目录身份/路径或换目标绑定都会拒绝。本片不增加 State/HistoryManifest 字段或用空文件版本伪装目录，不删除历史收据，不加入旧格式回退或迁移。

新空目录目标目前必须既是已启用登记盘，又是元数据成员。规划器检查原登记角色，准备和决定前再检查当前配置角色/身份、目录条目与空间预算。每个目标因而已有现行 Candidate/Snapshot/Publication 全成员日志和持有 seal 的闭包，即使 Prepared 文件列表为空也成立。仅数据角色的目标明确 OperationUnsupported；不能只因写了一个目录就说该目标具有发布证明。原先可用的单文件、有文件 scope 和数据盘专用文件备份不受这项空 scope 门禁改变。

正常路径在严格 State/Prepared 验证、容量预检与所有 Prepare 之后进入原决定流程；Finalize 完成后，才沿已准入持有 DiskRoot 创建 histories/<history>/directory-backups/<operation>/namespace/<published-path>，并以无覆盖原子写保存 record.json。祖先 mkdir/fsync 与文件/目录同步复用现有原语。物化失败保持原本 InDoubt 和决定证据，不能提前交换可见 State 或返回成功。成功收据的文件正文保护仍为 NotApplicable，不虚报 COMPLETE_VERIFIED 文件副本。

## 冷恢复与读取

只有完整发布前缀严格重放完成且当前恢复代固定配置的全部元数据成员在线、成员切换未冻结、恢复尾部未阻塞时，恢复流程才依据“仍与现行策略完全相等”的永久收据重建当前目录投影。缺成员或冻结的只读冷开不创建派生目录，也不写 record.json；读到已发布收据本身不授予磁盘修复权限。恢复写权限后，正常冷开仍重建缺少的派生目录；不以缓存目录/record.json 或同名字节建立发布权威。它不逐个重放所有过期历史物理目录，不重开已停止盘，不把旧身份材料投到换绑的新盘；冲突 record.json 保留并明确报错，不能覆盖冲突材料。

Engine::published_backup_directories(slot) 提供目录 ID、仓库/历史域、冻结路径和操作 ID。返回值描述已发布且仍是现行策略的目录快照，不证明盘当前健康，也不自动采样新增目录。已捕获目录的移动/改名由下述独立 native 语义发布。Server 在持锁捕获阶段消费此 API，锁外与文件节点共同进行全局冲突验证，之后才分页。真实目录保留其仓库/历史域/目录 ID，合成祖先仍按槽与规范化路径标识；合成祖先可与真实目录合并，不同真实目录占同路径以及文件/目录冲突必须拒绝。回锁时继续验证 listing snapshot、仓库和 namespace witness。响应保持 backup/read_only，不伪造主盘 location、文件正文或版本；HTTP GET 不修复物理投影。

已准入的初始池不可能发布 Native 目录，因此目录读取返回空行，原文件备份树仍可读。这是明确 Initial 状态，不是不支持错误的回退；AwaitingPublication 等准入错误仍传播，槽仍验证。当前登记的永久证据读取和物化规则不变。

Web 移除 file_count 为零的硬拒绝，沿现有 scope、capture、会话和永久操作回执流程提交；文案明确只支持独立空 scope、元数据成员目标，已捕获身份的改名/移动会跟随；新增目录与盘名前缀仍需重新保存。目录浏览使用同一备份树列表接口。实际浏览器和 WASM 交互验收仍单列。

每个已配置份数逐级走既有准确来源路由；当前 scope 的独立子策略保留，目录条目经现有继承判定筛选。本片不声称解决全部历史视图混合份数、后续策略变化的自动目录传播、外来盘导入或数据盘专用目标的本盘证明。

## 成本与验证边界

准备时只扫描已准入范围条目，保留目录记录，工作量约 O(D * route_length)，metadata canonical bytes 受 MAX_METADATA 限制；每目标目录数受 max_entries 限制。每个目录创建使用现有全祖先持久屏障，未削减同步。空间预检计入记录、目录组件估算及 metadata reserve，实际 I/O 仍可失败；该估算不是内核空间预留。

读取和冷恢复当前会扫描永久 operations，再解析空 scope 的候选收据；不 clone 全 State，仍为 O(operations + selected_directories)，这是明确剩余性能成本。物化投影不承担权威，不新增另一张并列表。

[公开回归](../../test/engine/integration/recursive_backup_empty_directories.rs)覆盖双跳真实空目录、冷重开、永久幂等、后续发布后的缺投影冷重建、决定后真实路径阻塞导致 InDoubt 并冷恢复、数据盘专用目标拒绝，以及不新增文件版本和不扁平化路径。缺元数据成员和成员切换冻结两例比较文件字节并断言指定目录投影保持缺失；前者还验证成员恢复后可写冷开重建。文件字节快照不是完整文件系统元数据无写证明。

[递归备份回归](../../test/engine/integration/recursive_backup_paths.rs)继续覆盖既有文件正文、独立来源链、软删除和停止来源拒绝；[私有规划与回放用例](../../test/engine/unit/engine/current_recursive_backup_tests.rs)覆盖目录行、目标身份和缺失材料的拒绝。测试定义与实际执行结果分别记录，源码接线不能代替运行。

[HTTP 定向测试](../../test/server/integration/recursive_empty_backup_http.rs)定义双跳空目录身份、热读不修复、冷开与原请求重试、策略替代后的旧游标拒绝、分页前真实目录/文件冲突，以及初始池文件读取。Web 的[递归配置测试](../../test/web/unit/files/backup/recursive_tests.rs)覆盖空 scope 仍携原始来源捕获和范围身份。

物理掉电、真实 fsync EIO、浏览器/WASM 和生产硬件测试另列，临时目录 I/O 回归不能代替它们。

## 后续 rename / move 的独立 native 语义

`file.namespace.recursive-directories` 和 `directory.namespace.batch.recursive-directories` 仅用于新的 rename/move。旧 `file.namespace`、`directory.namespace.batch` 及先前 file-only recursive kind 保留原验证和永久重试语义，不迁移或修改旧收据。新 kind 同时要求精确 `recursive_follow` 与 `recursive_directory_follow`，不能借旧 kind 携带新目录写入能力。

派生索引只从已完整验证的空范围配置建立：绑定策略、原来源捕获、原操作 ID、每级完整根及捕获的稳定目录 Entry ID。移动只更新这些 ID 的前后逻辑路径，不按新名字重采样，不把后来创建的空目录混入原快照。每级仍以直接上游盘名为前缀；盘名与仓库名来自原捕获。受影响策略 generation 递增，原快照退出当前读取，新收据保留每个策略的来源 operation、精确 namespace 和新策略。独立子策略分别更新；同请求限定同仓库/历史域，按目标合并物理计划，整份新收据在该 operation/target 下只写一次 record。新范围后来出现的文件，在新 kind 中也沿同绑定根更新已有文件备份路径；现有文件正文和软删除冻结协议不变。

新写预检仍检查现行启用身份、准确路由、元数据成员角色、空间与条目预算；全成员 Prepare/Finalize 后才物化新 operation 投影。旧 receipt/cache 保留，不自行物理 rename 旧缓存。热发布与冷重放共用原请求、真实 namespace 变化和精确目录/策略计划验证；reader 与最终冷修复逐策略选取现行 generation，读取不授予修复权。缺成员、fenced、blocked 等恢复门禁不变。新缓存失败保留 InDoubt；冷恢复从已决定证据完成，不以目录缓存作为权威。

同 ID 自定义策略覆盖会撤销派生跟随资格。被删除但没有本次稳定 ID path move 的历史捕获行保持原快照语义；本片不把删除、全量重新选择、盘名修改、数据盘专用目录目标或目录垃圾回收混入 rename/move。

新增热路径按已变化 owner 分支的 Entry 集合发现捕获 ID，索引反查受影响策略；首次 Arc 写复制仍为 O(P + D)，不声称纯 changed-key 常数开销。读取/冷修复仍扫描永久 operations。目录行完整新快照的 canonical metadata、条目及物化预算保持原上限。新增公开/私有定向定义位于 `recursive_backup_empty_follow.rs` 和 `current_recursive_backup_tests.rs`，实际运行结果另列，定义不代表通过。

逻辑读取与同目标物理预检按 `(repository, history, directory_id, normalized_path)`（目标内投影省略共同的仓库/历史）合并重复行后计条目预算。不同身份的同路径不合并，仍交既有冲突检查；隐式祖先仍由 Server 的逻辑节点预算覆盖。`PublishedBackupDirectory::operation_id` 是相同逻辑行的最新 sequence 发布代表，不是唯一来源证明，所有被合并的永久 receipt 保留不动。完整 receipt canonical bytes 仍受 metadata 大小上限，不删除资源保护。

后续按实际保留路由的 rename/move 使用 `*.recursive-routes` 新 kind，并完整继承本文目录快照验证。前述 `*.recursive-directories` 仍是旧收据的固定语义，不被追溯改变；详细区别见[路径跟随 ADR](recursive-namespace-follow.md#retained-route-follow)。

<a id="mixed-directory-snapshot"></a>

## 显式混合目录快照

`Engine::configure_recursive_backup_tree` 接受原有冻结 `RecursiveBackupConfigurationRequest`，但仅接受目录 scope，使用独立原生 kind `backup.configure-recursive-tree`。它在同一完整副本事务中回填所选文件的可用保留版本，并捕获当前 scope 内的目录 Entry ID 与各层完整路径，包含非空目录下的空子目录。独立子策略继续拥有自己的目录快照与路由，不被父 scope 吞并；未选定历史视图中的目录及发布后新增目录不会自动加入快照。每一级仍只按直接上游磁盘名加前缀，再继承上一层完整路径；不按全局绝对挂载点拼接。

保留 `configure_recursive_backups` 和 `backup.configure-recursive` 的原语义是兼容性决定，而不是自动回退。原文件模式的非空 scope 仍可使用数据专用备份目标；新全树模式的目录目标必须有现行元数据成员发布证明。任一目标不支持时整笔明确拒绝，不可悄悄省略目录；文件 scope 也明确拒绝。相同 operation ID 在两种模式间切换属于不同原生请求，成功后的永久重试必须使用原模式、原捕获和原参数。

新 kind 复用现行 typed 规划器、精确 State/Prepared/Candidate 验证、容量门禁和全成员决定流程。目录行、目录 ID、目标绑定与所需文件 payload 都是闭合证明的一部分；目录证据不能代替文件正文复制，旧 kind 不能通过携带目录字段获得新能力。决定后 I/O 失败仍保持 InDoubt，冷恢复在原有可写准入门禁后完成文件与目录投影，不改旧 receipt，不新增 State/HistoryManifest 表，也不迁移旧格式。

已验证的新配置进入现有稳定 ID 索引，后续 `*.recursive-routes` 以新收据共同跟随已捕获的文件与目录。最终冷修复和 `published_backup_directories` 继续只消费现行策略快照。Server 备份读取可复用这些目录行，活动分类认识新 kind；本片不新增 HTTP 写入路由或 Web 全树选择，现有界面仍保持原文件/空 scope 入口，不将 Engine 能力冒充端到端已接线。

定向定义见[混合目录公开回归](../../test/engine/integration/recursive_backup_mixed_directories.rs)与[精确原生证明回归](../../test/engine/unit/engine/current_recursive_tree_tests.rs)：前者覆盖保留正文、双跳真实目录、旧模式、独立子策略、数据专用目标/文件 scope 拒绝、改名后冷修复和永久重试、决定后真实 I/O 阻塞；后者覆盖目录伪造/遗漏、模式降级以及缺少或改变文件 payload。测试定义不代表运行通过，临时目录 I/O 不代表生产硬件或物理掉电认证。
