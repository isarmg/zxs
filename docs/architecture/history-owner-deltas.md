# History owner closure：精确行增量

本切片以 bounded metadata format C 为基线。它不启用旧依赖跳读、不改变磁盘格式、永久收据或 SQLite FULL，也不是端到端 O(delta)／性能采用结论。

## 实际消费者与所有权

`HistoryManifest::project_state` 保留为独立全量投影和冷准入路径；`from_state`、RawState、外部导入／恢复仍执行其逻辑验证。正常发布只有完整权威发布且原 metadata epoch 有效后才把下一份 `DerivedOwners` 绑定到 `PublishedStateBase` 的精确 State allocation。索引只从该不可变 State 派生，因此后续纯成员关系复用只要求 allocation 相同；metadata epoch、配置或磁盘图权威失效不改变该纯值证明。等值的新 Arc、`Arc::make_mut` 脱离原 allocation、冷恢复均不能借旧索引。history roots、旧依赖省略及最终发布仍各自要求完整配置／epoch／原私有许可，不从成员索引取得磁盘授权。

中立私有模块 `state_projection.rs` 的穷尽 State 解构和同一 `projection_maps!` 声明连接写集、行取值、owner 计算及 graph field 名：

| 来源／关系 | 受影响投影 |
| --- | --- |
| history 自有记录 | branches、commits、versions、path/meta nodes、merge events/links、boundaries |
| repository → histories | repository scalar、directory branches、placement rules、protection policies、repository-prefix contexts |
| version → file/history（按 version ID 的精确成员集合） | file names/locations/origins/backup paths/policies/generations/notes；版本本身及 placement/content state/parents/version note |
| commit directory entry → history（引用计数）及 repository root directory | directory tree selection、directory selection generation |
| branch → selected-file keys / alias source key | file selection、alias；不是 alias target owner |
| directory branch → tree / selection keys | directory trees、directory selections，随后经 repository 的所有 histories |
| path node 与 metadata node | 同 key path note 的 owner 并集 |

版本集合与目录引用计数分别处理同 history 的多版本／多 commit 引用、多 history owner，以及最后引用移除。更新先改派生关系，再从前后权威 State 和对应索引计算受影响行；写入同值不产生 graph edits。ContentState 缺项保持既有逻辑 Available，两个 generation map 保持十进制字符串。

未知公开 mutable escape、foreign replacement 和写集上限导致的 `MapChanges::All` 才走明确 full；不把普通增删、owner 移动或 metadata 单写归入 All。没有逻辑变化的 history 不重建 manifest，不扫描其 State versions/files，也不重复全 manifest.validate。

## Graph 与信任边界

通用 `edit_map_values` 连接新增／删除／替换到同一个 radix planner。`Resolver::retain_map` 复用同一图形状／路由校验和 address 去重，只保留完整已认证对象，不另造全逻辑 manifest。新 history 经索引选出的所有真实投影行编码；Commit 自身已缩减为分页命名空间根（[ADR 0011](../adr/0011-paged-namespace-roots-and-segmented-logs.md)），其页/索引对象按内容寻址共享，不随投影行重复编码。

旧 graph 的所有真实 read/hash、原依赖及每成员 store/fsync 保留。变更字段的 radix descriptor 当前还通过 `MapRoot::read` 从已认证旧对象重建；这仍遍历该字段旧图，不能宣称整段 graph 准备只按 delta 计。catalog 仍遍历需要安装的所有旧 history；这是未开启 PGE 的 I/O 成本，不是 State owner scan。source graph acquisition 的失败仍通知既有故障 observer；仅该阶段允许从权威 State/index 重建，未改动 root 必须严格等于原绑定 hash。编码、store、决定失败直接传播。冷／import 外部语义验证没有删除。

当前 `prepare_update` 最终仍合并 reachable 旧对象和 `encoder.finish()` 新对象，再按地址去重后 full store。将来 PGE omit 必须在这个生产边界保留来源区分，不得从合并 Vec 的 hash 差反推“旧对象”：新编码对象即使撞上已存在同 hash 目标，仍需要真实精确比对及 fsync。本切片没有发放 skip capability。

## 复杂度与未关闭项

- 冷 owner 构建 O(State records + commit entries + 实际 owner 投影行数)；warm 分类访问确切 MapChanges keys、前后相关记录及反向引用，非全 versions/commits。branch/history 的 owner 改变可能真实影响大量行 M，按 M 处理。
- 索引所有无界容器复用 `SharedRecords`，没有每代 overlay、外部缓存权威或第二套树。**本 C 基线 SharedRecords 是 Arc<BTreeMap>，首写仍复制该 map 的全部 keys/handles**；最终 path-copy 门禁必须与统一 AVL 候选底层组合后验证，不能以本切片宣称已达到。
- 每个真正修改／新增的大 record 的 clone/compare/encoding 仍按其 S 计；新 commit 引入所有 directory references 也是实际输入成本。
- 命名捕获的后续源码切片复用现有 `PublishedStateBase` 的精确 State allocation 谱系：冷／失配时仍在 closure 前完整捕获，closure 后仅访问真实变化的 main HEAD；精确 warm 元数据写不再遍历无关 repositories 的 entries。它只初始化缺失物理身份，不能拿历史 alias 重新定位容器；All／foreign／overflow 的全量边界、私有写入契约及独立 oracle 见[命名捕获说明](../components/engine/src/container_storage.md#name-capture-lineage)。源码切片不代表该组合已通过性能或完整 CI 门禁。
- 业务 `set_file_note` 仍通过 versions.values().find 定位文件／版本，这是独立于 history 投影的已有查找成本；未将发布专用索引暴露成第二业务权威。
- `container_storage::changed_files` 按自身真实字段谓词消费 MapChanges；容器位置及描述符从同一 State 派生关系选择对应文件的成员。它们不把 HistoryEdits 当作名称写集，也不改变已有候选谓词。冷／All／foreign 输入仍全量构建一次关系，不对每个文件重扫所有 versions。当前源码变更尚待独立 GitHub 验证，不能据此宣称命名发布整段已为 O(delta)。
- 已收窄实际已知 bulk writers：restore 的 `TrackedRecords::extend_shared` 逐 incoming key 记录；政策 `retain_keys` 只授予不可变 predicate 并精确记录删除；alias retarget、split 的政策与边界先确定候选 key，再 get_mut。筛选仍扫描各自 map，split 仍扫描相关 commit entries，不隐去这些成本。未知公开 mutable iterator／retain 仍保留 All。

## 验证与交付状态

根 `test/engine/unit/history_package/delta/tests.rs` 以独立完整 `HistoryManifest::project_state`、完整 graph 编码及每个对象 bytes/hash 为 oracle，覆盖全 map 增删／同值、owner 迁移、多引用最后移除、新增／移除 history、联合修改、连续多代、All/foreign/overflow。多 owner fixture 故意超出当前外部准入条件；真实 save／file note／version note／backup policy 测试额外断言必须产出精确 delta；所有真实发布测试仍有 full encoder oracle，不放宽准入。

radix tests 增加空 map、absent deletion、新路径、collapse、混合增删改的 full bytes 对照。源实现分支保留 `work/history-owner-deltas`；实际组合验证在 frozen typed-C `fc5e9f3c01f921858d699d12f7cf818f7c6d6684` 的独立 `audit/history-owner-on-typed-c` 分支执行，使用 C 原 worktree 的唯一 `targets/bounded-format-check`，jobs=2、dev/test debug=0、incremental=0、locked/offline。原 `172e`、`fc5e` refs 不移动。

实际 `c604441d3c0fe46720f2f2fac6771d988efef874`：lib/all-target check 通过；delta 7、radix 16、真实 save/metadata/backup、旧依赖 incremental 11、独立 history oracle 6 通过；完整 engine 703 passed/10 ignored、contract 107 passed/2 ignored。第一轮 strict Clippy 仅新测试的 `manual_is_multiple_of` 失败。修复后的 `3274bf96f45c342caf0f7b1433264f3cc919b7dc` 再次通过 lib/all-target check、engine+contract lib/tests strict Clippy `-D warnings`、完整 engine 703/10 ignored、contract 107/2 ignored。原始结果在任务根 `history-owner-typed-matrix-01.log`、`history-owner-typed-matrix-02.log`，target 实测 1.1 GiB（预算 2 GiB）。

保留的初始失败不是产品通过证据：check01 发现 export 仍需完整 `bound_history_graph_observed` 的生产可见性，已恢复；check02 修正 Note owned 参数及旧 test oracle 宏可见性。focused01 中旧测试直接破坏私有 mutator 契约后要求 warm 全复验，与本次边界变更冲突；保留坏引用负例于 All/冷 RawState 准入，并新增真实公开 missing-node 操作拒绝及 State 不变断言。未放宽外部逻辑验证。独立旧 closure oracle 已移入根 `test/`，生产只保留合法模块接线。

## 容器成员共享切片（源码状态）

`DerivedOwners` 已从 history_package 编码子模块迁到 `state_projection`，真实调用方直接使用中立模块，无兼容别名。file→history→version-ID 集合替换原 version 引用计数；非空 history 桶同时提供原 file owner 闭包，不再维护另一份计数。目录／仓库计数仍保留各自语义。

同一派生对象还维护容器实际需要的 file→pending branch、path node、metadata node、merge event、context key 与 alias source 成员；branch→context 是 branch.file_id 变更的精确反向依赖。pending 包括普通目录历史分支的 pending_file_versions；context 与 alias 保持文件级而非 history 级过滤，alias 取源分支归属。关系只保存 ID，实际字段仍从不可变 State 读取。

`ProjectionTransition` 在 publication 准备边界生成一份前后关系，history 编码、路径准入与名称准备共同消费。沿用 PublishedStateBase 的 allocation/config/metadata epoch 绑定，以及完整权威决定完成后的安装屏障；SQLite 更新失败只停用其独立候选加速器。恢复从已验证 State 冷构建，不从 container.json 或 SQLite 恢复关系权威。All/foreign 继续完整语义准入；失败不发布派生对象。

容器位置仍并集旧／新全部版本的真实 routes，包括 PURGED/MIGRATED 与只存在于旧 State 的位置。实际容器存在后才构造 call-local 文件描述符和 NodeProjection；版本／路径排序、事件 ID 顺序、备份位置、单对象限额、匿名暂存、实际 bytes/hash/同步/重放均不改变。每个文件的 history 描述符只构造一次后供该 history 的各副本编码；标签 memo 不跨发布保存。

本切片消除的是局部选择中的 changed-files × 全域记录扫描；仍需选中文件的完整历史、实际引用和输出字节成本。SharedRecords 当前是 Arc<BTreeMap>，首次共享写仍复制所触及 map 的 keys/handles；未验证的持久树候选不能作为本分支的复杂度承诺。

新增根 test 的独立全扫描 membership/精确 container bytes oracle、旧路由、inode/body、预算拒绝及共享存储测试；原 history full graph oracle 保留。仅执行指定文件 rustfmt 与 diff 检查，未在本地执行 Cargo、编译、测试或性能测量；本段不是 CI 通过记录。
