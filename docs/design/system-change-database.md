# 系统盘全局变更库基础

[数据模型](../07-data-model.md#s07-03) · [Engine 组件](../components/engine.md)

## 本片已实现的范围

系统根既有 `index.sqlite` 新增 `system_changes` STRICT 表，按 `(kind,id)` 唯一保存回执、路径事件与命名空间目录的完整记录族，不是只保存 ID：

- `kind=0`：已准入 `State.operations` 的全部保留 `OperationResult`，含业务种类、原请求摘要、发布保护、结果正文和原序号。
- `kind=1`：已准入 `State.file_path_nodes` 的全部明确 `FilePathNode`，含文件/历史/分支/版本身份、父节点、操作、作者及旧/新外部路径。

这两个事件族跨所有仓库，不按今天的 placement、保护路由或在线盘筛旧变化。序号按 8 字节大端 BLOB 排序，完整支持 u64；同序号再按规范 ID 排序。规范 JSON 正文与其它索引、`current_state` 在同一既有 SQLite 事务提交。只改非版本元数据也更新对应回执行和序号。

这不是完整 State 数据库。以下业务 catalog 已保留完整版本、标签节点和生命周期映射，但私有认证控制和全部历史时点仍不在该投影内；后继审计族另保留原主体映射、事件数组和三项 State 代数；失败/未决定请求不属于已保留成功收据，存储的 `current_health` 不代替实时 `operation_record`。盘根当前/历史库是各自独立范围，不能将 SYSTEM 行按今天位置过滤后冒充盘根过去变化。

## 真实读取及拒绝

`Engine::system_operation_records` 和 `system_file_path_nodes` 是实际 Engine 读入口，返回完整 typed 行、固定序号及不透明续页游标。无 HTTP/Web 暴露；调用它们的应用必须自行完成全局管理读取授权，不得复用普通文件 scope 权限。

每页接受 1..=100 行，规范载荷合计最多 64 MiB。单行沿既有 metadata 编码上限；这是输出字节预算，不是整个 Engine 或编码/解码瞬时堆内存上限。可信内存索引只保留 `(sequence,id)`，不复制全部结果载荷。先构造可信有界区间，再从 SQLite 读有界键/长度，比较含下一边界键的准确顺序和数量，最后读取长度匹配的正文并逐字节对照 State。读连接另复用平台原生限制，SQLite 字符串/BLOB/编码行最多 64 MiB、SQL 64 KiB、编译 VM 操作 100,000 和 16 个参数；它们不限制初次打开解析或总堆内存，临近单行上限还可能因 SQLite 行开销被拒绝，此时显式停用而不截断内容。该页若删行、插入异常首行、改序、改载荷或替换查询表/索引会失败，不能静默少报。它不是每次全库 scrub，尚未请求的其它族/区间不在本次结论里。

游标绑定 State 序号、记录族及当前投影实例；发生新发布、重建或重开后须从首页开始。参数/过期游标报请求错误，不停用正常库。库身份、序号、模式或内容失败保留原始来源，并将现有 `index_health` 停用；后续此入口报明确 Unavailable，不回退为空页或暗中绕过损坏库。

## 全局命名空间和当前位置目录

`Engine::system_catalog_records(kind, cursor, limit)` 最初将以下十五个实际 State 映射作为完整 typed 记录纳入同一 SQLite 事务：仓库、历史域、分支、保留命名空间提交、显式目录仓库、目录分支、目录树、文件实际 owner 路径、文件各备份槽路径、版本 placement、文件分支选择、目录分支选择、目录树选择、文件写入上下文及写入来源。每条返回映射 ID 和具名枚举值；不是只返回内部 ID 或对当前界面做 JSON 摘要。

其中保留 `Commit.entries` 与 `DirectoryTree` 提供各自历史组成，包括同名但不同文件身份及目录项；可变分支、选择、owner 路径和 placement 只表示本次捕获 State 的当前事实，不能被解释成完整时序位置日志。所有族跨仓库保留，不按照现时盘位删除历史。它们按族内规范映射键排序，SQL 排序序号固定为零；页面的 `sequence` 才是完整 State 快照序号。游标不能跨这些族或与旧事件族混用。

单一声明将族、SQL discriminator、State 字段与公开类型绑定；热更新复用精确写集，替换/删除同一 ID 与冷建使用相同编码。返回完整行仍遵守 100 行/64 MiB 页预算，读取长选择键按可信区间的最大键长度限制，不能因固定 ID 长度截掉合法路径键。整个输出必须使用全局管理授权，不直接作为普通文件 scope 的接口。

### 完整业务记录族的后继

本片增加十九个生产族，继续使用相同 `SystemCatalogKind`／`SystemCatalogValue` 和实际分页入口：

| SQL discriminator | 记录族 | 保存的真实范围 |
| --- | --- | --- |
| 17 | `FileVersion` | 完整版本声明，包括正文摘要、长度、校验清单、创建盘、来源版本和 FULL 标志；不是正文 |
| 18 | `FileMetadataNode` | 完整不可变元数据节点及标签集合、父节点、作者、原操作和序号 |
| 19 | `FileDeletion` | 当前 D，保留稳定文件身份、全部冻结分支 frontier、路径与原操作；撤销后此当前行消失，原操作永久回执不删除 |
| 20–22 | `FileBranchMerge`、`FileBranchAlias`、`FileNodeParents` | 实际合并来源与结果、当前别名和完整父边；不根据同摘要另造连接 |
| 23–24 | `FilePathNote`、`FileBackupPolicy` | 当前路径节点备注及明确文件策略绑定 |
| 25–26 | `DirectorySelectionGeneration`、`FileSelectionGeneration` | 原始 u64 代数，持久正文为十进制字符串，不经 JS/浮点或截断 |
| 27–29 | `FileNote`、`VersionNote`、`FileName` | 当前完整备注和文件名，空值与不存在仍不同 |
| 30–32 | `MergeLink`、`HistoryBoundary`、`ContentState` | 完整关系线/跨域审计边界，以及 AVAILABLE/PURGED/MIGRATED/RETIRED 当前事实；不授予复活权 |
| 33–35 | `Plan`、`PlacementRule`、`ProtectionPolicy` | 已保留计划及当前放置/保护规则，含原代数、路径和备份前缀；不是新执行许可 |

全部三十四个 catalog 族与原回执/路径族仍在一次原 SQLite 事务提交，沿精确写集支持新增、同键替换、删除，metadata-only 操作也推进同一 `current_state`。全局图不因今天盘位或正文已清除而丢掉其仍保留的声明和审计信息。标签值来自真实 `FileMetadataNode`，不会由正文摘要或外部路径猜测。

同一声明生成编码、读取、SQL 族号与增量选择；通过对 `State` 的穷尽解构，将所有字符串键业务映射纳入，剩余字段明确归属原操作/路径族、下述审计/State 代数族，或明确排除的 control-auth 范围。新增 State 字段须显式作出投影归属，不依靠反射或无默认 JSON catch-all。

catalog 一律按规范 map key 排序，SQL `sequence_key=0`，即使值内有创建/发布序号也不能把该页当发生顺序流。整个结果仍要求全局管理读取授权；它不是普通文件 scope 接口，不含正文，也不直接公开认证控制状态。策略和 RETIRED 行是已准入 State 的派生副本，不能拿这些行直接执行配置或反回滚恢复。

常规打开及 `restore_from_disks*` 在原生证据准入之后，从当前 State 重建新表，SQL 闭合范围为 `0..38`。不增加旧表双读/升级来源，不从旧行重建权威。备份 SYSTEM-current 准备复用同一 `Delta::between(None, state)` 编码，所以新准备包含这些十九族；原格式已准备字节若不一致，继续按既有无覆盖规则拒绝并保留，不暗中改写历史准备材料。

本片没有写新的盘根文件或修改对象库范围，也没有自动接纳外来盘。三范围数据库仍不独立选举权威。三十四族 catalog 是当前完整业务映射及仍保留节点，不代表全部 State、完整历史时点或本盘过去归属。

## 原主体、事件数组和 State 代数

SYSTEM 后继补齐原 State 中另外三类非秘密事实，和业务 map、原回执/路径族在**同一个原 SQLite 事务**写入，不从其它记录猜作者、不重排或去重事件：

| SQL discriminator | 实际 API / 类型 | 精确身份与范围 |
| --- | --- | --- |
| 36 | `system_operation_principals` / `SystemOperationPrincipal` | 原 `operation_principals` 映射的每个规范 `OperationId` 与完整主体字符串；不是账户、密码、权限或代他人重放许可 |
| 37 | `system_audit_events` / `SystemAuditEvent` | 原 `events` 数组的每个实际位置及完整 `Event`；相同内容、相同 operation/sequence 都可处于不同位置，不按业务种类筛选 |
| 38 | `system_state_metadata` / `SystemStateMetadata` | 原 `sequence`、`business_generation`、`topology_generation` 三个 u64，来自同一已准入 State；不是当前 lease 或新的决定证据 |

主体页按规范 operation key；事件页按原始 ordinal；两者均沿既有 1..100 行、64 MiB 载荷预算与 State/投影实例绑定游标。事件 SQL ID 是二十位十进制位置，`sequence_key=0`，因此完整保存数组次序，不受有符号 SQLite INTEGER 或同序号事件影响。对外 ordinal/代数使用规范十进制 u64 字符串；事件内原 sequence/operation/kind/object_id 不改写。

读取先从可信 State/顺序构造准确有界预期，再检查真实 SQLite 页的键、数量、长度、正文及下一边界键；缺失、额外或改动行仍停用派生索引并保留具体错误。metadata API 也读取/核对 SQL 的唯一行，不直接把内存字段伪装为数据库读结果。不同族游标不能混用；调用者若跨多次获取全量材料，须保持同一 Engine 观察或明确对齐每页 State sequence。没有 HTTP/Web 新入口，调用方须授权全局管理数据读取。

主体增量复用原 tracked-map 精确键，包括替换和移除。事件仅在 `SequenceChanges::Append` 的同谱系证明有效时追加后缀；不知道谱系、发生修改/截断/重排时在当前事务清掉派生事件族再写全数组，不推测只追加。可信内存位置集合随同准确变化更新，未知谱系不遗留尾行。三代数行每次随 `current_state` 提交，metadata-only 发布不能留下旧头。

冷开、明确恢复后的索引重建及目标隔离 SYSTEM-current 编码使用同一个 `system_changes::Delta`，所以不会出现运行库有审计数据、备份编码仍只保留业务 map 的第二套过滤。旧隔离 SYSTEM-current 与当前编码不同仍保留拒绝，不能把补齐审计当作覆盖旧材料的许可。

**范围仍有限**：现在可读取当前 State 的全部非秘密业务映射、保留事件/原主体与代数；`control_auth` 明确排除。当前备注/规则/选择曾经是什么值，不可由今天映射或摘要倒推。独立原生权威、Config/代际链、真实正文、文件/目录/盘根过去各时点及跨三范围的共同恢复决定没有因此完成；不得从 RETIRED/D 行或旧主体记录授予复活、重放、选权威。

## 持久性和恢复权威

SQLite 仍是从已准入 State 构建的派生副本，不是新的发布日志、独立恢复权威或跨盘决定点。已完成发布后的 SQLite 更新失败不会把已成功业务改判失败，也不能因此撤销永久幂等收据；故障经原健康路径可见。冷启动依靠原检查点/发布证据恢复 State，再完整重建本表。缺失库不得令已有池被当成空池重新初始化；未知替换文件不获得读取或删除权限。

文件与目录内部 SQLite 的本对象、无外部路径范围不变。[盘根本盘历史](disk-local-publication-history.md)已独立实现其固定 genesis 支持范围，本片不扩展或替换它；三范围独立恢复、冲突选择或整体迁移闭包仍未完成。尤其不能以当前盘位过滤历史回执来伪造过去的本盘变化，不使用三份字节相等或多数票确定权威。

## 成本及验证要求

热更新从实际 tracked-map 写集得出增删改；精确 K 键执行 O(K log N) 比较/有序键维护，载荷仅编码发生变化的完整行。同 ID 替换、移序及删除均有对应 SQL。未知谱系按有序映射做完整比较，冷建编码全部保留行并建立 O(N) 键存储；没有把全局写路径说成常数成本。读取额外保留有界规范正文和结果解码，最坏单页载荷与比较缓冲会同时存在。

审计扩展额外保留完整主体 key 和事件位置集合，空间 O(P+E)。普通事件追加 A 项为 O(A log E) 的位置维护及 A 行写入；未知谱系按 O(E) 全族重建，不把现有 State 克隆或全局发布声称为常数成本。主体增量沿 K 个实际键，三代数为一个小行；没有复制事件正文到可信顺序集合。

复用原 FULL SQLite 事务和系统根生命周期，不增加其它磁盘写入或通用安装器。定向回归须覆盖真实多仓发布/外部改名/冷重建、metadata-only、游标实例/序号隔离、unsigned 排序、精确替换/删除、边界行完整性、畸形库保留与失败健康。实际通过项和未执行项以交付证据为准，不将新增测试源码当成运行通过。

## 当前交付状态

2026-11-08 十九族业务记录已在 `2470ab9d` 合入。本次主体、完整事件数组和 State 代数扩展基于该准确主线，并组合至 `9924665a`，只完成生产源码和文档。依用户最新要求没有审查、编译、测试、静态检查或 CI；没有把此前结果当作当前扩展通过。
