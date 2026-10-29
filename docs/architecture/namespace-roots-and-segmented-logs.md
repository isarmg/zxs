# 命名空间根与分段日志：任务二消费契约

本文是 [ADR 0011](../adr/0011-paged-namespace-roots-and-segmented-logs.md) 布局供后续任务审阅的**待验收契约**：发布根身份、可达性规则、页／段／索引关系、恢复入口，以及活跃读者如何保护其使用的对象。描述当前实验实现的接口及采用前仍需验证的约束；不为未来需求预造无人使用的接口或配置。实现入口见 `src/engine/src/namespace.rs`、`src/contract/src/segmented_log.rs` 与 `src/contract/src/model.rs`。

## 1 发布根身份

- 一次发布的唯一根身份仍是 **`Candidate` 的对象哈希**及其绑定链：`candidate.snapshot_hash` → Snapshot（`format:"radix-map-roots"`，固定 35 个 `maps` 字段 + `events` 根 + `control_auth`）；`candidate.history_roots` → 历史目录 → 每棵历史的 `history-manifest` 根。本次改动不新增任何根身份来源。
- 提交的命名空间身份是 `Commit.namespace.root`（`{"kind":"namespace-page"|"namespace-index","hash"}`，64 位小写十六进制对象哈希）。同一不可变子树在不同提交、不同历史、同一池内按内容哈希共享；**哈希相同不等于业务归属相同**，归属仍由所在 Commit 与历史准入决定。
- 事件与回执的身份是其所在段对象哈希加段内序：事件 = (段序号 = ordinal÷1024, 段内偏移)；回执 = 全局回执序号（`operation_locators` 的值），其 `OperationResult.sequence` 仍等于 decision slot。回执正文只存在于 `operation-segment`；`Publication.result` 是既有的发布证据副本，对账规则（operation_id/request_digest/sequence 与 Candidate 一致）不变。

## 2 可达性规则

从一次已发布决定出发，保留闭包为（箭头=持有引用，全部为 `{kind, hash}` 强引用）：

```text
PublicationSeal → Publication → Finalize → Candidate
Candidate ──snapshot_hash──► Snapshot
Snapshot.maps[*] ──► metadata-index / state-page（既有 radix）
  state-page 内 Commit 值 ──namespace.root──► namespace-index ──children──► namespace-page（递归，单目录内）
  namespace-page 目录条目 ──subtree──► 子目录的 namespace-index|namespace-page（递归，跨目录）
Snapshot.events ──► metadata-index / state-page，叶值 ──► event-segment
Snapshot.maps["operation_log"] ──► …叶值 ──► operation-segment
Snapshot.maps["operation_locators"] ──► …叶值（十进制序号文本，无再引用）
Candidate.history_roots ──► …叶值 ──► history-manifest ──maps["commits"]──► …Commit 值 ──► 同上 namespace 闭包
```

- 巡检、修复、检查点覆盖、历史包导出与元数据预算都按上述闭包工作；任何不在闭包内的对象不因新布局获得保留地位。
- 命名空间对象、事件段与回执段与其余元数据对象同规则：发布前在全部配置成员安装（install-before-decide），同 hash 已存在仍做真实字节比较与文件/目录同步。
- 独立历史包携带：候选闭包 + 该历史的 manifest 图 + 该历史全部 Commit 可达的 namespace 对象 + 原有 versions 正文材料。事件段与回执段是池级证据，不随单历史包导出。

## 3 页／段／索引关系

### 3.1 命名空间（每目录有序，B+ 形）

- 一个目录的根 = 一个 `namespace-page`（直接子项 ≤128 条且 ≤64 KiB；单条目例外仍受 `MAX_METADATA`）或一个 `namespace-index`。索引 `children[i] = {first_key, entries, files, ref}`，`first_key` 严格递增且等于该子图第一个条目的组件 `lookup_key`；扇出 1..=128，各叶等深，根到页最多 4 个对象（最多 3 层索引）。页内条目按组件 `lookup_key` UTF-8 字节序排序且唯一。
- 条目只存**组件** `name`/`lookup_key`；绝对路径由遍历前缀重建，必须满足 `name_policy` 既有预算。目录条目携带 `subtree` 引用与记录值 `entries`（子树条目总数）/`files`（子树文件总数）；记录值供显示与预检，完整准入逐项重核。
- 定位规则（**先索引定位，再读本页**）：
  - `get(path)`：逐组件下降；在索引层以 `first_key` 二分选 child，在页层对条目二分。
  - 目录列页 `list_directory_page(dir, after_key, limit)`：在该目录的索引上二分 `after_key` 定位起始页，顺序读取至 limit；绝不全量展开后截取。游标 = 最后一条组件 `lookup_key`，快照身份 = commit_id（与既有 SMB 页约定一致）。
  - 全树 `walk` 和定域 `walk_subtree` 以完整路径 `lookup_key` 字节序合并惰性页／索引游标，与旧平铺序一致；目录项 `a` 之后先处理 `a-b`、`a.b`，再处理 `a/` 后代。未读子树以 `key/` 下界排队，索引以 `first_key` 下界排队；只有当前最小候选才能读取下一对象，回调停止后不预读。`collect_entries` 沿同一顺序收集，不再全量排序。
  - 游走内存：已打开目录流处于当前路径的祖先前缀区间，每流仅持有一页及索引高度数量的续游标；额外未读目录票据数受规范化路径键长度约束。因此遍历临时空间随路径预算／索引高度与有界页大小增长，不随目录宽度或全树条目数增长；游标持有的 `Arc` 对象另计于缓存之外，消费者自行保留的结果不在此界内。
  - 首个／最后别名消费者与旧平铺 oracle 的差分测试仍须在实际 CI 验证；删除操作内的事实顺序还需遵循编辑器的操作序契约，不能由全树顺序代替。
- 局部写 = 受影响目录的新页（及其目录内索引）+ 根到该目录的祖先链新对象；其余子树按 hash 复用。目录改名只产生父目录链的新对象。实际显示前缀的字节、UTF-16 单元、组件数均不增长时不扫描后代路径；有增长时保留 O(子树) 路径预算验证，不能把命名空间对象共享误称为整个业务改名恒定读取。
- 实现约定：完整构建对排序后的输入确定，索引尾组重新均分以维持非根内部节点 64..128 的构建不变量。编辑使用稀疏键覆盖层和按需打开的 B+ 页；只在封印时编码最终脏节点，首插／首删不平移其它页。叶页按条数／字节分裂，低填充时只合并／重分配一个相邻兄弟；内部索引半满修复，单子根收缩。同一目录各页等深，根到页最多 4 个对象。非根 wire 扇出仍允许 1..128，完整准入不以构建器占用率取代完整性检查。
- **确定性范围已显式修订**：对象规范字节和对象域哈希不变；相同逻辑命名空间经不同历史可以有不同分页根，不能由物化条目重建并声称取得原根。导出、测试固定夹具及其它根身份消费者须保留实际可达 namespace 对象，逻辑等价另与独立平铺 oracle 比较。决定、替代方案和读／分配／编码／安装／同步预算见 [ADR 0011](../adr/0011-paged-namespace-roots-and-segmented-logs.md#page-local-editing)。


### 3.2 事件段

- 固定跨度 1024：事件 ordinal k 在段 ⌊k/1024⌋、偏移 k mod 1024。事件总数 = 封印段数×1024 + 尾段长度；只有最大段序号允许 <1024 条。
- 段目录 = Snapshot `events` 根（radix，键 = 20 位补零段序号，键必须从 0 连续）。段内事件 `sequence` 单调不减，跨段首尾衔接；最后一条 ≤ `state.sequence`。
- `events_after(after_sequence)`：驻留段目录（每段 `first_sequence`）二分 → 读该段（缓存→成员对象，哈希验证）→ 段内 partition_point → 迭代，跨段续读；尾段直接读驻留 Vec。页上限 1000 与 `INVALID_CURSOR` 语义不变。

### 3.3 回执段与定位器

- 回执按 decision slot 顺序追加；段封印条件 = 满 256 条或规范字节 ≥1 MiB（先到者，逐条判定，确定性）。记录 = `{principal, result}`；`principal` 即原 `operation_principals` 值。
- `operation_log`（键 = 20 位补零段序号，从 0 连续）与 `operation_locators`（键 = OperationId 文本，值 = 全局回执序号十进制文本）同为 Snapshot maps 字段。定位器是**定位信息**，不是回执正文，也不是第二套权威。
- ID 定位：驻留尾段线扫 → `operation_locators` 点读（既有 radix 路径证明规则，区分"不存在"与"路径损坏"）→ 段读取 → 核对 `operation_id`。幂等检查即该路径；digest/principal 比对规则不变。sequence 定位：段目录按 `first_sequence` 二分 → 段内二分。
- 永久回执不裁剪、无 TTL；旧 ID 永不复用为新幂等键。
- 一次性冷索引：磁盘读入在整段验证后附带完整 ID→序号索引，首次全量编码消费、决定点释放。此后的全量快照编码（本地持久准备在发布外改 State、有意作废增量基线时发生）通过已发布 `operation_locators` 根重建该索引，与驻留日志形状和驻留记录逐一对账后挂回；对不上或两者皆无从取得时失败关闭。索引始终只是内存定位，不是第二套权威。

### 3.4 缓存上限（进程内，非权威）

| 缓存 | 上限 | 丢失后的行为 |
| --- | --- | --- |
| 命名空间页/索引 | 8 MiB（按对象规范字节计，LRU） | 从成员对象重读并验证哈希 |
| 事件段 | 8 段 | 同上 |
| 回执段 | 8 段 | 同上 |
| 定位器/段目录 radix 叶 | 2 MiB | 同上 |

缓存键含对象 kind 与哈希；逐出只丢弃副本，不影响正确性。客户端分页上限沿用既有值（目录 200/500、SMB 256、事件 1000）。

## 4 恢复入口

- 唯一入口不变：成员日志从字节 0 全量扫描 → 候选绑定 → `journal::resolve_snapshot` → `RawState` → `TryFrom<RawState>`。新布局下该入口额外执行：
  1. 事件段与回执段**全部读取并验证**（域哈希、`format`、跨度/封印规则、键连续、序单调、事件总数从 0 连续），然后仅保留尾段与段目录驻留；
  2. 每个 Commit 的 namespace 闭包**全部读取并验证**（域哈希、形状、排序/唯一、first_key、路径预算、记录计数重核），随后按历史语义准入（`HistoryManifest` 校验走同一分页视图）。
  每次完整快照准入或单候选历史图校验可在该同步调用内复用已认证、已完成结构解码的同 kind/hash namespace 节点；临时缓存按规范字节计最多 8 MiB，逐出后重新读取认证，不跨候选、恢复调用或健康查询。规范字节预算不等于活跃堆上限，游走持有的 `Arc` 另计。各路径的深度、前缀、数量与归属仍分别核验，失败不缓存。
  快照准入同时保留各历史的目录 ID 集合，供紧接着的准确候选图对账复用；事实与私有不可变 State 同生共灭，消费 State 时丢弃，不代表磁盘当前健康。checkpoint 已覆盖候选的独立图准入不使用这份目录事实。所有可达段和页仍须在本次对应验证范围内读验，分段不改变跨调用的损坏检测时机。
- 检查点仍只加速重放，不成为权威；其覆盖散列继续包含 `metadata/objects` 下的新对象种类文件。
- 降级恢复的"尾部更新"：以 `Publication.result` 覆盖最后一个回执时，重编码尾段（或该回执恰为封印段末条时重编码该段）并更新对应段目录键；不触碰更早段。逐对象修复以 (对象域, hash) 为键，覆盖四种新对象；任一对象全部副本缺失时失败关闭，不用多数投票、SQLite 或另一个根顶替。
- 新布局自给自足：不依赖任务二即可发布、重启恢复并读取完整永久历史。

## 5 活跃读者如何保护其使用的对象

- 读者以 `Arc<State>` 克隆固定一份已发布状态（既有规则）；其中段目录与所有根哈希随之固定。页/段对象是不可变、内容寻址的已发布元数据：**运行时没有已发布不可变元数据的 GC**（见 [integrity-scrub](integrity-scrub.md)），因此被固定根可达的对象在池内持续存在；缓存逐出只影响副本，不影响可读性。
- 因此活跃读者的保护 = 持有 `Arc<State>`（或引擎当前状态）期间，按其根哈希读取；读取始终验证对象域哈希，失败关闭。任务二若引入任何回收/归档协议，必须先给上述可达闭包建立显式租约或等效保留证明，才允许移动或回收对象；本任务不提供也不预留回收接口。
- 工作区、内容墓碑与正文读租约的既有保护边界不变（[12.8](../12-reading-and-cache.md#s12-08)）。

## 6 本任务明确不包含

- 不改变驻留 `MapRoot`/`Tracked` 容器实现（R48 结论）；State 其余映射的克隆成本不变。
- `DirectoryBranch`/`DirectoryTree` 实体仍内嵌其子树条目表；目录分支子树分页另行立项。
- `file_contexts`/`file_locations`/`file_names` 等绝对键侧表与 `FilePathNode` 语义及其改名成本不变。
- 不提供任何段/页回收、归档或跳读配置。
