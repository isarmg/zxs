# ADR 0011：提交命名空间分页根与事件／回执分段日志

状态：实验源码决策，发行格式未冻结。本决策补充 [ADR 0004](0004-paged-history-manifests.md)／[ADR 0006](0006-bounded-metadata-roots.md)，解决两者明确遗留的 `Commit.entries` 单实体增长与 events／operations 全量驻留问题；不改变 [ADR 0005](0005-incremental-publication-integrity.md) 的旧依赖免读准入，不改变单写者、固定成员发布、SQLite FULL、独立完整副本与永久回执无 TTL 规则。

## 采用门槛

本方案仍待采用验收，不能作为后续 GC 任务已接受的基础。编辑器已改为稀疏目录覆盖层和页局部 B+ 路径复制；这只是待验收实现，不是性能或采用结论。仍须验证宽目录的真实读／编码／分配预算、原有别名选择与删除事实顺序，以及固定工具链下的完整产品门禁。不得只减少最终保留的对象数宣称达标。

## 背景

- 每个 `Commit` 以 `entries: Vec<Entry>` 携带全量平铺命名空间（绝对路径、按全路径 lookup_key 排序）。每次提交构造 `NamespaceDraft::new(entries.clone())` 并写入一份完整条目表；快照把整个 Commit 实体作为一个叶值重新规范化编码。局部写的复制、编码与扫描成本随命名空间总量增长；目录改名对每个后代条目执行 `rebase` 重编码绝对路径。[ADR 0004](0004-paged-history-manifests.md) 与 [08 章](../08-full-storage-and-disk-format.md)已点名该实体为剩余增长瓶颈；[契约附件](../contracts/README.md)把 Commit/Tree 分页列为冻结门槛。
- `State.events` 为全量驻留 `Vec<Event>`，`operations`/`operation_principals` 为全量驻留 `BTreeMap`；每次变更 `state.clone()` 深拷贝全部历史事件与回执，恢复时 `resolve_raw_snapshot` 全量物化。快照内 events 以 20 位补零 ordinal 作 radix 键，追加按散列路径改写，与顺序读取无关。
- [R48 正式结论](../validation/persistent-state-r48-formal-results.md)拒绝以私有 Arc/AVL 持久容器替换驻留 State：快照获取与 held 驻留确有改善，但 file_history/graph/list_tree 多条读路径逐对回退超过 10% 门槛、事件页游标回退、端到端写收益未建立。本 ADR 据此**不更换驻留容器**，而是改变持久布局：把增长实体移出每次克隆与常驻集合，驻留端只保留有界尾部与显式上限的热缓存；借用切片的事件尾部游标语义保留在尾段上。O(1) 根克隆或路径复制本身不作为方案有效的证明；采用与否以端到端读写、恢复与内存测量为准。
- [verified-recovery-checkpoints](../architecture/verified-recovery-checkpoints.md#truly-bounded-total-startup-is-a-separate-protocol-decision)规定：带封印根的分段日志不得作为附带缓存改动引入，不得静默放宽恢复约定。本 ADR 遵守：**恢复与导入仍读取并验证全部段与全部命名空间对象**，分段只改变稳态驻留与按需读取，不改变损坏检测时机。

## 选定格式

新增四种元数据对象（规范 JSON、对象域哈希、既有 `metadata/objects/<kind>/<hash[..2]>/<hash>.json` 地址与原子写规则）：

| 对象 kind | 域 | 形状 |
| --- | --- | --- |
| `namespace-page` | `zxs:namespace-page\0` | `{"format":"namespace-page","entries":[…]}`；单目录直接子项按**组件 lookup_key** 的 UTF-8 字节序排序，键唯一 |
| `namespace-index` | `zxs:namespace-index\0` | `{"format":"namespace-index","children":[{"first_key","entries","files","ref":{kind,hash}}…]}`；同一目录的有序页索引，`first_key` 严格递增 |
| `event-segment` | `zxs:event-segment\0` | `{"format":"event-segment","events":[Event…]}`；按发布顺序 |
| `operation-segment` | `zxs:operation-segment\0` | `{"format":"operation-segment","records":[{"principal",“result”:OperationResult}…]}`；按 decision slot 顺序 |

页/段常量为格式常量，与调用时预算无关：

- namespace 页 ≤128 条且规范字节 ≤64 KiB（单条目页不受 64 KiB 限制，仍受 MAX_METADATA）；索引扇出 1..=128，`first_key` 等于其子图第一个条目键；索引可多层，同一目录各页到目录根等深，根到页最多 4 个对象（最多 3 层索引）。空 `entries` 页仅允许作为某个目录的根（空目录/空命名空间）；索引不得引用空页，children 不得为空。
- 事件段固定跨度 1024 条；唯一允许不满 1024 的是最大序号的尾段。
- 回执段封印条件：满 256 条，或规范字节首次 ≥1 MiB（逐条追加后判定，确定性）；同样仅尾段可未封印。记录的 `principal` 即原 `operation_principals` 绑定值，回执正文只保存这一份。

### 提交命名空间根

`Commit` 删除 `entries`，新增必需 `namespace: {"root":{"kind":"namespace-page"|"namespace-index","hash"},"entries":"N"}`。目录条目在页内形如 `{"entry_id","name","lookup_key","kind":"directory","entries":"M","files":"F","subtree":{kind,hash}}`，文件条目形如 `{"entry_id","name","lookup_key","kind":"file","file_id","version_id","size"}`；`name` 为组件显示拼写，`lookup_key` 必须等于 `name_policy::lookup_key(name)`，绝对路径由遍历重建并须满足既有路径预算（≤768 字节、≤200 UTF-16、≤32 层）。`entries`/`files` 是记录值，完整准入逐项重核，不作为可信分配长度。

- 局部修改只重写受影响目录的页与根到该目录的祖先链；未触及子树按 hash 共享。目录改名只改父目录页与祖先，不重编码后代。
- 同内容才共享对象；命名空间页与 state-page/历史投影页互不冒充（对象域不同）。
- Commit 仍保留 `tree_id`、`pending_file_versions`、`namespace_deletions` 原语义与稀疏编码；删除事实与路径节点（`FilePathNode`）等既有公开语义不变，目录改名仍按 [06 章](../06-history-and-branches.md)为受影响文件生成路径节点——该成本属于这些记录自身，不属于命名空间根。

<a id="page-local-editing"></a>

### 页局部编辑与确定性契约修订

**显式决策：撤回本实验先前新增的“相同逻辑树、任意编辑历史，全量构建与增量编辑必须同根同字节”要求。** 对象仍以规范 JSON 和原对象域哈希确定身份；相同节点内容仍产生相同字节与地址。完整构建对同一输入仍确定；增量编辑对同一基线与最终覆盖层仍确定，但分页边界可以依赖基线。不同根可以表达逐项相同的业务条目。业务身份、顺序、错误、提交证据链及历史均不变，不能把此决定推广到事件／回执分段或既有 radix 编码。此修订针对尚未采用的实验布局，不提供旧格式双读或迁移。

原先从最小键贪婪装满再固定扇出分组，首插／首删会平移任意多个后续边界；靠 hash 去重只隐藏了安装量，无法消除扫描与编码。比较后选择既有 page/index 形状上的标准持久 B+ 算法：

| 方案 | 局部性、空间与失败边界 | 维护取舍 |
| --- | --- | --- |
| 全局贪婪规范分页 | 构建唯一；首插删最坏 O(目录宽度) 读、分配和编码 | 简单，但不满足本任务写局部性，拒绝 |
| 有序按键 radix / 压缩 trie | 可实现历史无关分界；路径随键长度增长，需定义终止符、压缩路径及新的索引形状；长共同前缀的深度与利用率不受当前 B+ 常量约束 | 不否认可行，但会重写格式、验证器和遍历，当前没有需要全局唯一根的业务消费者，不引入 |
| 内容定义分块 | 可改善平均稳定性；边界由邻域内容决定，必须另证最坏重同步距离，不能用概率命中代替预算 | 未形成满足现有硬上限的独立方案，不引入 |
| 保持页边界的 B+ 路径复制（采用实现） | 单点仅目标页、每层至多一个相邻兄弟及祖先；内部索引构建／编辑保持半满，页按条数与字节修复；全树同层叶等深 | 保留现有格式、查找和游标，新增代码仅用于事务内脏页；代价是放弃历史无关的全树根 |

实现分为两层，不替换 R48 已拒绝的驻留 State 容器：

1. `DirectoryDraft` 只保存被访问键的原值、当前值、缺失／删除键及未打开子目录引用。首次点读经现有索引定位，重复同键操作不重新读页；读操作不标脏。新目录空根及父目录自动创建的既有业务顺序不变。
2. `seal` 先封印确实改过的子目录，把与原值不同的覆盖键交给事务内 `PageTree`；未改目录直接保留根引用。该树按需打开索引路径和目标页，同一目录批次共享已打开节点，最终才编码脏节点，故同一键改 512 次也只编码最终值。它不先编码整目录再按地址去重。
3. 插入／替换超过 128 条或 64 KiB 时，在当前页寻找合法的平衡二分；变长中间值使二分不可行时，在该页内分成最多三页。单条超 64 KiB 的既有例外保留，仍受 `MAX_METADATA`。索引超过 128 子节点时二分。
4. 删除或变短后，页若同时少于 64 条和 32 KiB，只取本父节点内相邻一个兄弟，合并后按相同边界重新分配。内部索引不足 64 子节点时同样与一个相邻兄弟合并／二分；根只剩一个子节点就提升该子节点，空根是显式空页。页不要求固定最小条数（变长条目可能无法满足）；索引 64 下限是构建器不变量，不把已有 wire 的合法 1..128 非根扇出另行改为兼容门槛。完整准入验证等深、顺序、总数与上限。
5. 最终封印前检查根到页对象数不超过 4；不是把越界页继续发布。`first_key`、条目／文件总数随脏路径重算，兄弟外的分界与引用不动。失败仍向上传播具体错误；事务局部输出不先发布可见状态。

令 `D≤32` 为路径目录数，`h≤4` 为每目录根到页对象数，`B=128` 为节点条数／扇出，`P` 为有界节点字节（普通页 ≤64 KiB，索引及单条例外 ≤`MAX_METADATA`），`k` 为最终不同覆盖键数：

| 成本 | 单点编辑 | 同一事务多个编辑／例外 |
| --- | --- | --- |
| 源对象读取 | O(Dh)，每层仅编辑路线、必要兄弟，封印深度检查至多额外一条最左路径 | O(kDh) 上界；同键原值与打开的脏页复用；不是扫描目录宽度 |
| 分配／复制与条目大小计算 | O(DhB) 条引用／条目、O(DhP) 字节；仅打开的页逐项计算规范大小 | O(kDhP) 活动写集上界；页内大小计算与最终对象编码分别计价 |
| 实际对象编码 | O(Dh) 脏页／索引，未改节点不调用编码器 | 每个最终脏节点一次；`NamespaceObjects::encoding_work` 在地址去重前统计调用与字节，不等于最终安装数 |
| 安装 | 不超过实际编码节点数，内容寻址再去重 | 不承诺只安装一次 fsync；成员数及现存对象校验规则不变 |
| 同步 | 编辑本身无 I/O 同步；发布沿现有对象 install-before-decide 和提交链 | 不改文件／目录 fsync、SQLite FULL 或独立完整副本要求 |

目录改名的命名空间改动仅移动目录条目并复用 `subtree`，不打开后代。`apply` 比较移动前后实际存储显示前缀（不是请求拼写或 lookup key）：若 UTF-8 字节数、UTF-16 单元数和组件数均不增长，准入已证明的所有后代路径预算继续成立，不读取后代；任一维增长时保留完整后代路径预算检查。后者以及删除事实、路径历史节点、绝对键侧表仍可 O(子树)，必须单列读取／分配／锁内耗时，不能把它们算作页局部编辑已消失。全量构建、准入、恢复、导出仍 O(总量)。冷／热缓存、跨容量逐出、端到端锁占用和峰值内存必须用最终提交的真实运行证据验收；单元读计数和编码计数只证明工作范围。

### 事件与回执分段

Snapshot 仍为 `format: "radix-map-roots"`，固定字段集改为：`events` 根的叶值从 Event 改为 `{"kind":"event-segment","hash"}` 引用、键为 20 位补零**段序号**；`maps` 删除 `operations` 与 `operation_principals`，新增 `operation_log`（20 位补零段序号 → `{"kind":"operation-segment","hash"}`）与 `operation_locators`（OperationId 文本 → 全局回执序号十进制字符串）。字段总数仍为 35；严格字段集与叶值形状使新旧结构互相拒绝。`PagedHistoryRoot` 的 30 个字段名不变；其 `commits` 值随 Commit 新形状变化。

- 追加 = 重编码尾段对象 + 该键的 radix 路径更新（封印边界另推进一个键）；封印段字节恒定，跨快照按同 hash 复用并保留既有同 hash 字节比较。
- 定位：`sequence` → 驻留段目录二分（每段记录 first_sequence）→ 段内二分；`OperationId` → `operation_locators` 点读（既有 `metadata_map::lookup` 路径证明规则）→ 段读取。幂等检查先查驻留尾段，再按上述路径点读；回执仍与同一 Candidate/Publication/Seal 证据链绑定，`Publication.result` 副本与 `verify_publication_seal` 的 operation_id/request_digest/sequence 对账不变，不建立第二套提交权威；`operation_locators` 只保存定位序号，不保存回执正文。
- 驻留端：State 保留尾段 Vec 与段目录（每段 hash+first_sequence）；封印段正文按需读取，进程内缓存有显式上限（命名空间页缓存 8 MiB、事件段缓存 8 段、回执段缓存 8 段、定位器叶页缓存 2 MiB），缓存非权威、可随时丢弃重读。
- 一次性冷索引与全量编码回退：磁盘读入在整段验证后附带完整 OperationId→序号索引，首次发布的全量编码消费它，决定点释放。此后正常发布沿已发布基线做增量编码；本地持久准备（保存计划、历史保存包）在发布外就地改 State 并有意作废增量基线，使下一次快照全量编码以捕获该改动——该全量编码通过已发布 `operation_locators` 根（进程内未驻留时读当前已准入快照取得）重建完整索引（O(全部回执) 页读，与全量编码同价），重建结果与驻留日志形状及驻留记录逐一对账后挂回（`attach_cold_index`），对不上即 ObjectCorrupt 关闭；索引仍只是内存定位，不是第二套提交权威。两者都无从取得时维持失败关闭（先恢复再发布）。
- 恢复/导入：读取并验证全部段（域哈希、format、段跨度/封印规则、序号与 slot 单调、事件总数连续从 0），再只保留尾段与目录驻留；降级恢复对最后一个回执的权威结果覆盖通过重编码尾段（或恰在封印边界时重编码该封印段）完成，即"尾部更新"；不改写更早段。

### 可达性与证据消费者

对象可达性：Snapshot→maps 根→state 页→Commit 值→namespace 根→namespace-index/page 闭包；Snapshot→events 根→event-segment；Snapshot→operation_log→operation-segment；Candidate→history_roots→history-manifest→commits→同一批 namespace 对象。完整恢复、降级恢复、完整巡检、检查点覆盖、修复、独立历史包导出/离线检查/导入与元数据预算全部按该闭包同步切换；历史包随 manifest 携带其 commits 可达的全部 namespace 对象。巡检对段正文与目录页在一次运行内各验证并计数一次（目录页已由 radix 遍历验证，重访只读不重计；跨快照共享的封印正文只计首次），存储安装侧对复用封印正文保持完整回读、字节校验与成员同步，所有副本均失联且正文未驻留时失败关闭——封印正文是历史，不能由驻留状态重建。拒绝旧形状（内嵌 `entries` 的 Commit、ordinal 直存 Event 的 events 叶、`operations`/`operation_principals` 字段）时保留原材料，不迁移、不重置运行池、不删历史或回执。

## 代价与不宣称

- 全量路径仍存在且如实计价：完整准入/恢复/导出 O(总量) 读验；整树分支创建、树拆分、目录分支选择等按语义 O(子树/全量)；file_contexts/file_locations/file_names 等绝对键侧表的改名成本不变。驻留 MapRoot 键/节点仍 O(keys+nodes)。不宣称端到端 O(delta)。
- 新增读路径成本：历史事件/回执与命名空间页的点读改为对象读取（缓存未命中时落盘），幂等检查新增 O(深度) 定位器点读；是否可接受以三档规模的端到端测量为准，不以结构性质替代。
- 读取失败关闭：任一页/段无有效副本时相关查询明确失败，不用投影或 SQLite 重建顶替。

## 替代方案

- 继续增大/容忍平铺 Commit：保留整表复制与 64 MiB 单实体上限风险，已被 ADR 0004/0006 点名拒绝。
- 以全路径为键把 entries 装入既有 radix map：散列序无法满足"按规范化 key 排序并先索引定位再读本页"，目录改名仍重写全部后代键。
- 驻留持久树容器（R48 候选）：已按正式配对测量拒绝，不重试。
- 事件按 ordinal 散列分页（现状）：顺序页读取跨散列叶，追加写随机叶；保留则驻留与恢复物化成本不变。

## 必要验证（分列，不互相替代）

1. 严格形状与拒绝：四种对象的字段集/排序/唯一/first_key/跨度/封印规则；旧形状拒绝且字节保留。
2. 构建器与完整 oracle：随机命名空间操作序列下，分页构建结果与平铺排序模型逐项等价；事件/回执段重放与顺序模型等价；命名空间增量与平铺 oracle 比较完整逻辑条目；同一基线和最终覆盖层比较确定性，独立节点固定字节／哈希向量继续保留。事件／回执及既有 radix 的确定性契约不变。
3. 查询：索引定位分页（首页/中间页/尾页/空目录/单页目录/跨页边界）、get/get_key、sequence/ID 定位、缓存上限与逐出后重读。
4. 发布与恢复：新池发布、重启完整恢复、检查点路径、降级恢复尾部覆盖、单副本页/段损坏修复、全部缺失失败关闭、导出/离线检查/导入。
5. 规模测量：≥3 档规模记录耗时、分配/峰值内存、读取与编码字节、恢复成本，并与平铺基线同负载对比；通过/失败/未执行分列。

关联：[任务二契约](../architecture/namespace-roots-and-segmented-logs.md)、[盘格式](../08-full-storage-and-disk-format.md)、[事务恢复](../10-transactions-and-recovery.md)、[读取缓存](../12-reading-and-cache.md)、[容量成本](../19-capacity-and-performance.md)。
