# History owner closure：精确行增量

本切片以 bounded metadata format C 为基线。它不启用旧依赖跳读、不改变磁盘格式、永久收据或 SQLite FULL，也不是端到端 O(delta)／性能采用结论。

## 实际消费者与所有权

`HistoryManifest::project_state` 保留为独立全量投影和冷准入路径；`from_state`、RawState、外部导入／恢复仍执行其逻辑验证。正常发布以 `PublishedStateBase` 的同一 State allocation、配置与 metadata epoch 为边界保存 `DerivedOwners`，只有完整发布及索引成功后才发布下一份索引。

`history_package/delta.rs` 的穷尽 State 解构和同一 `projection_maps!` 声明连接写集、行取值、owner 计算及 graph field 名：

| 来源／关系 | 受影响投影 |
| --- | --- |
| history 自有记录 | branches、commits、versions、path/meta nodes、merge events/links、boundaries |
| repository → histories | repository scalar、directory branches、placement rules、protection policies、repository-prefix contexts |
| version → file/history（引用计数） | file names/locations/origins/backup paths/policies/generations/notes；版本本身及 placement/content state/parents/version note |
| commit directory entry → history（引用计数）及 repository root directory | directory tree selection、directory selection generation |
| branch → selected-file keys / alias source key | file selection、alias；不是 alias target owner |
| directory branch → tree / selection keys | directory trees、directory selections，随后经 repository 的所有 histories |
| path node 与 metadata node | 同 key path note 的 owner 并集 |

引用计数同时处理同 history 的多版本／多 commit 引用、多 history owner，以及最后引用移除。更新先改派生关系，再从前后权威 State 和对应索引计算受影响行；写入同值不产生 graph edits。ContentState 缺项保持既有逻辑 Available，两个 generation map 保持十进制字符串。

未知公开 mutable escape、foreign replacement 和写集上限导致的 `MapChanges::All` 才走明确 full；不把普通增删、owner 移动或 metadata 单写归入 All。没有逻辑变化的 history 不重建 manifest，不扫描其 State versions/files，也不重复全 manifest.validate。

## Graph 与信任边界

通用 `edit_map_values` 连接新增／删除／替换到同一个 radix planner。`Resolver::retain_map` 复用同一图形状／路由校验和 address 去重，只保留完整已认证对象，不另造全逻辑 manifest。新 history 经索引选出的所有真实投影行编码；新增大 Commit.entries 本身仍是实际 S 字节成本，没有拆分大 entity。

旧 graph 的所有真实 read/hash、原依赖及每成员 store/fsync 保留。变更字段的 radix descriptor 当前还通过 `MapRoot::read` 从已认证旧对象重建；这仍遍历该字段旧图，不能宣称整段 graph 准备只按 delta 计。catalog 仍遍历需要安装的所有旧 history；这是未开启 PGE 的 I/O 成本，不是 State owner scan。source graph acquisition 的失败仍通知既有故障 observer；仅该阶段允许从权威 State/index 重建，未改动 root 必须严格等于原绑定 hash。编码、store、决定失败直接传播。冷／import 外部语义验证没有删除。

当前 `prepare_update` 最终仍合并 reachable 旧对象和 `encoder.finish()` 新对象，再按地址去重后 full store。将来 PGE omit 必须在这个生产边界保留来源区分，不得从合并 Vec 的 hash 差反推“旧对象”：新编码对象即使撞上已存在同 hash 目标，仍需要真实精确比对及 fsync。本切片没有发放 skip capability。

## 复杂度与未关闭项

- 冷 owner 构建 O(State records + commit entries + 实际 owner 投影行数)；warm 分类访问确切 MapChanges keys、前后相关记录及反向引用，非全 versions/commits。branch/history 的 owner 改变可能真实影响大量行 M，按 M 处理。
- 索引所有无界容器复用 `SharedRecords`，没有每代 overlay、外部缓存权威或第二套树。**本 C 基线 SharedRecords 是 Arc<BTreeMap>，首写仍复制该 map 的全部 keys/handles**；最终 path-copy 门禁必须与统一 AVL 候选底层组合后验证，不能以本切片宣称已达到。
- 每个真正修改／新增的大 record 的 clone/compare/encoding 仍按其 S 计；新 commit 引入所有 directory references 也是实际输入成本。
- `planning::plan_new_mutation` 仍在业务 closure 前后调用 `capture_names`，遍历 repositories 的 main-head entries。该函数只初始化缺失物理身份，不能拿历史 alias 重新定位容器；本切片未跳过这些扫描。
- 业务 `set_file_note` 仍通过 versions.values().find 定位文件／版本，这是独立于 history 投影的已有查找成本；未将发布专用索引暴露成第二业务权威。
- `container_storage::changed_files` 仍前后遍历多个 map、versions 和 branches；`projection_locations` 仍为每个受影响 file 遍历 versions。它们未消费本次 HistoryEdits，下一命名投影切片必须按自身真实字段语义接 MapChanges 和 file→version 反向引用，不能把 history closure 的完成当作名字投影已增量。
- 已收窄实际已知 bulk writers：restore 的 `TrackedRecords::extend_shared` 逐 incoming key 记录；政策 `retain_keys` 只授予不可变 predicate 并精确记录删除；alias retarget、split 的政策与边界先确定候选 key，再 get_mut。筛选仍扫描各自 map，split 仍扫描相关 commit entries，不隐去这些成本。未知公开 mutable iterator／retain 仍保留 All。

## 验证与交付状态

根 `test/engine/unit/history_package/delta/tests.rs` 以独立完整 `HistoryManifest::project_state`、完整 graph 编码及每个对象 bytes/hash 为 oracle，覆盖全 map 增删／同值、owner 迁移、多引用最后移除、新增／移除 history、联合修改、连续多代、All/foreign/overflow。多 owner fixture 故意超出当前外部准入条件；真实 save／file note／version note／backup policy 测试额外断言必须产出精确 delta；所有真实发布测试仍有 full encoder oracle，不放宽准入。

radix tests 增加空 map、absent deletion、新路径、collapse、混合增删改的 full bytes 对照。源实现分支保留 `work/history-owner-deltas`；实际组合验证在 frozen typed-C `fc5e9f3c01f921858d699d12f7cf818f7c6d6684` 的独立 `audit/history-owner-on-typed-c` 分支执行，使用 C 原 worktree 的唯一 `targets/bounded-format-check`，jobs=2、dev/test debug=0、incremental=0、locked/offline。原 `172e`、`fc5e` refs 不移动。

实际 `c604441d3c0fe46720f2f2fac6771d988efef874`：lib/all-target check 通过；delta 7、radix 16、真实 save/metadata/backup、旧依赖 incremental 11、独立 history oracle 6 通过；完整 engine 703 passed/10 ignored、contract 107 passed/2 ignored。第一轮 strict Clippy 仅新测试的 `manual_is_multiple_of` 失败。修复后的 `3274bf96f45c342caf0f7b1433264f3cc919b7dc` 再次通过 lib/all-target check、engine+contract lib/tests strict Clippy `-D warnings`、完整 engine 703/10 ignored、contract 107/2 ignored。原始结果在任务根 `history-owner-typed-matrix-01.log`、`history-owner-typed-matrix-02.log`，target 实测 1.1 GiB（预算 2 GiB）。

保留的初始失败不是产品通过证据：check01 发现 export 仍需完整 `bound_history_graph_observed` 的生产可见性，已恢复；check02 修正 Note owned 参数及旧 test oracle 宏可见性。focused01 中旧测试直接破坏私有 mutator 契约后要求 warm 全复验，与本次边界变更冲突；保留坏引用负例于 All/冷 RawState 准入，并新增真实公开 missing-node 操作拒绝及 State 不变断言。未放宽外部逻辑验证。独立旧 closure oracle 已移入根 `test/`，生产只保留合法模块接线。
