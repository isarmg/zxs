# R86 整合边界审阅与复现说明

[工程规范](../20-engineering.md) · [编码规则](../../禁止防御性编程.md) · [验证入口](../ci.md)

## 结果与适用范围

对指定整合快照的独立源码审阅未确认新的 P0/P1 阻断。确认两项 P2：历史分页反复扫描全池版本的继承热点，以及新来源回移工具的 Rust 标识符误改问题。另有一项高优先级的目录同步重试候选需先做 GitHub 故障验证。实际延迟、动态复现及掉电结果均未执行；这不是合入批准、恢复验收、性能提升或全仓合规结论。

| 对象 | 固定值 |
| --- | --- |
| 唯一目标仓库 | `https://github.com/isarmg/zxs` |
| 审阅提交 | `99f02cb978bff53f28d1ad8414037a3f52d74cd2` |
| 审阅 tree | `96d02c531530729ede98b524af819198cbdb3c8a` |
| 比较基线 | `13b664e9fd41a356ffb7ed982d21e97abb123626` |
| 整合范围 | 188 个文件，Git 差异为 36489 行新增、7722 行删除；并按需要读取未改调用方 |
| 工作副本 | 从目标仓库独立克隆；环境中的 `zxs-linux` 未修改 |
| 输出范围 | 本说明；不含生产源码、测试实现、规范契约或来源材料的修改 |

实际启动四个独立审阅代理，分别负责 Engine/存储、服务授权/SMB、Web/协议、依赖来源；主代理交叉核对两项发现与恢复/发布边界并集中交付。各方向先读取 `AGENTS.md`、根编码规则、工程规范及实际组件/领域规范。源码搜索只用于定位，结论依实际调用链、当前本地实现和测试接线建立。

## 已确认发现

### P2-A：每种历史页与字段分片重复扫描全池版本

**位置与调用链。** [history_pages.rs](../../src/server/src/engine_backend/file_manager/history_pages.rs) 的 `admit`（32–50 行）在每种读取中调用 [file_manager.rs](../../src/server/src/engine_backend/file_manager.rs) 的 `file_repo`（200–214 行）；后者以 `engine.state().versions.values().find(|v| v.file_id == file)` 查找归属版本。九类新历史读取均先在 `EngineBackend` 的 engine mutex 内复核身份，再调用该准入，随后 `Engine::file_history_scope` 又获取已有派生归属索引。主页、父边页、分支页、合并/输入/父片、字段片及旧版本出口均受影响。

**来源与已有保证。** `file_id` 在请求边界已解析；该请求持有的 State 以及由 `history_owners()` 返回的索引绑定同一已准入 `Arc<State>`。[state_read.rs](../../src/engine/src/history_package/state_read.rs) 的 189–195 行在磁盘准入拒绝一个 stable file 跨 history，已有回归位于 `test/engine/unit/state_read_tests.rs`。既有 [DerivedOwners::file_histories](../../src/engine/src/state_projection/files.rs)（128 行）可按文件查询历史归属，暖态无需扫描全池；[history_window.rs](../../src/engine/src/engine/history_window.rs) 的 `history_owners`（181 行）对冷态构建成功才缓存结果。

此扫描用于解析 `file → history → repository`，本身不重新验证磁盘、实时权限或数据散列。可以替换归属查找算法，但仍须保留 `current_protected_identity`、`repo_for_history` 中的仓库状态/权限检查、root 比较及具体对象成员检查。不要把整个 `file_repo` 当作可删除的授权层。

**影响与状态。** 目标文件在版本键序靠后或不存在时，一次准入最坏为 O(V_pool)；P 次页面/字段分片会额外重复 O(P×V_pool)，且占用全局引擎 mutex。该查找早于本次整合已存在，新增分页放大其调用次数；不是新增授权漏洞或无界新增驻留。现有 ADR 0009/容量规范披露单文件候选、全池分支、祖先与命名空间扫描，但未明确这一所有历史分片都先经过的全池版本查找。静态成本已确认，延迟与吞吐影响待测。

**最小建议。** 在核心现有派生索引上提供窄的只读文件归属查询，由服务继续执行现行授权和 root 检查；不暴露整个可变索引，不另建平行缓存。保留不存在文件的 `404 FILE_NOT_FOUND`，以及已删除但历史保留、拆分/迁出与冷恢复后的语义。若暂不优化，明确该额外成本。

**最小验证提案。** 在根 `test/server/unit/engine_backend/file_manager/history_pages_tests.rs` 的真实 Engine 夹具或既有基准中，固定目标文件为一个版本、相同 root、相同字段片；逐级增加其他文件的 1k/10k/100k 个版本。预热派生索引后重复请求目标首页/字段片，并请求不存在的文件。记录版本规模、重复次数、mutex 持有时间及分布，不以单个时间阈值判通过。优化后的正确性测试覆盖删除后历史保留、拆分/迁出、冷恢复、同根续读及撤权。需要复杂度计数时使用独立基准/测试，不能在生产函数插入计数 hook。

### P2-B：回移工具将嵌套 use 与 Unicode 标识符后缀误认作 crate 根

**位置与调用链。** [source_backport.py](../../tools/rust-sources/source_backport.py) 的 `_crate_root`（238–249 行）仅以相邻字节和后续 `::` 判断路径根；`map_identifiers`（252–274 行，尤其 258 行）用 ASCII 标识符边界匹配。`_plan → build_document → merge_files`（586–587 行）对 upstream base/fixed 都调用它；`_apply`（1108 行）和 `check_replay`（696 行）重用相同变换。来源验证经 `proof_legacy_maintenance.Maintenance._verify_live → check_replay` 检查重放，因而同一误改也可能自洽地通过重放。

**静态最小反例。** 使用当前已登记的真实映射 `futures_core → futures`，basis 为 `consolidation:eafb0b4185a7c210506e15a28d87995a7ff4898a240281ec013b11ec70a2acd3`：

| 输入源码 | 按当前实现推导的错误输出 | 应有结果 |
| --- | --- | --- |
| `use crate::{futures_core::Thing};` | `use crate::{futures::Thing};` | 原字节不变，replacements 为空 |
| `use αfutures_core::Thing;` | `use αfutures::Thing;` | 原字节不变，replacements 为空 |

第一项的模块明确位于 `crate::` 下，但 `{` 遮住了 `_crate_root` 对直接前置 `::` 的判断。第二项不存在独立的 `futures_core` token，UTF-8 前置字节却满足 ASCII 正则的负向前瞻/后顾边界。这些输出由源码逐分支推导，尚未运行工具复现。

**来源与已有保证。** `validate_identifier_map`（277 行起）证明映射的 from/to 与登记的 rename/consolidation 一致；它不证明源码出现位置属于外部 crate 根。[来源身份规则](../architecture/source-naming.md) 要求按真实命名空间匹配，不能改别的私有模块。工具及[维护链规范](../tools/rust-sources.md)没有把源码限定为 ASCII 或扁平 `use`，也未拒绝上述语法。因此是边界解析不足，不能通过删除来源/路径/字节校验解决。

**影响与状态。** 后续接纳的上游 hunk 含这些形态时，工具可能生成错误 Rust 源码，导致编译失败，或在另一个同名模块存在时改变行为。人工 `review.diff` 与显式收据登记仍是独立门禁；此问题不等于无需审阅即可合入。固定 HTTP 补丁的两个生产文件和一个抽取测试文件未见该触发形态，本项不构成它们当前已被误改的证据，也不作为当前整合阻断。

**最小建议。** 匹配完整 Rust token，跟踪 `use` 树前缀；对不能确定真实归属的语法停止并要求人工处理，不能仅因后接 `::` 自动变换。保留现有重命名、整合和许可证策略，无需引入通用重构框架。

**最小验证提案。** 在根 `test/tools/rust-sources/test_source_backport.py` 的 `Identifiers` 类加入 `test_nested_use_paths_preserve_local_module_segments` 与 `test_unicode_identifier_suffix_is_not_a_crate_token`，断言表中输入加 LF 后逐字不变且无 replacements；保留顶层 `futures_core::Thing`、`::futures_core::Thing`、`use futures_core::Thing` 的正向变换。再用既有 `Synthetic` 夹具添加一个 upstream FIXED 的嵌套 `use` 新 hunk，贯通 plan/apply/replay；不能只检查变换与自身重放相等。

## 已核对并应保留的边界

| 分区 | 实际调用链/保证 | 审阅判断 |
| --- | --- | --- |
| 发布阶段 | `publication/member_protocol.rs::run_member_stage → stages.rs`；每成员独占 journal，最多八工作线程，阶段 join 后继续；panic 先 join 并围栏再传播 | 失败不是前缀集合；保留 allN/quorum、决定后 IN_DOUBT、AppendMode 与失败围栏，不能为了少锁/同步合并阶段 |
| 正文扇出 | `disk/fan_out.rs::SourceReader → ChunkStream/Agreement → copy_target`；源完整 EOF 才公布证明，参与者 guard 先于 spawn，提前退出关闭通道，全体封存后才 rename | Arc 块复用只证明输入；目标独立完整文件与后续实际读回/散列、inode 身份、目录同步均必须保留；超包络走串行不是产品文件上限 |
| 恢复与历史根 | `recover_mode`、普通发布、degraded recovery 安装 `HistoryCatalog`；目录绑定精确 State 分配，本地 plans-only 更新有专门重绑定 | 缺 catalog 返回 RECOVERY_REQUIRED，冷对象读取仍验完整性；根缓存不是跨盘内容永久可信证明 |
| namespace 准入 | `HistoryManifest::from_state/validate → validate_namespaces → admit_namespace` 先验证外部图，再进行目录投影 | 保留深度、资源、路径、图闭包校验；不把持久化数据当作内部永远可信 |
| 服务鉴权 | `AuthorizedBackend::current → EngineBackend` mutex 内 `current_protected_identity → file_repo/repo_for_history → root/scope`，九类读取均接入 | 排队期间可能撤权，外层与锁内复核具有不同线性化意义；本次未找到漏接入口 |
| HTTP 读预算 | raw query 128 KiB/8 字段 → 重复/未知参数拒绝 → typed request/cursor → 有界页编码/UTF-8 分片 | 游标不是授权凭据；file/history/root/context 与边界 node 的检查是网络边界，保留字节预算及非末页前进语义 |
| 响应资源 | `http_file_history::page` 的 backend permit 经 blocking job、Body、运输 chunk 的 Arc lease 延续；`DrainStream` 记录实际 socket 写进度 | Body EOF 不等于发送完成；保留 socket 身份 capability、idle/total deadline 和最多 64 KiB chunk，后者避免小尾块钉住整页 |
| HTTP/SMB 错误 | `error_registry` 实际将 HISTORY_CHANGED 映射 HTTP 409；SMB 当前没有历史分页 RPC，新增 code 纳入既有 EIO 分组 | 未凭抽象错误名建议改 errno；`ConnectInfo<SocketAddr>` 仍为真实连接 peer，不改变可信代理/认证来源 |
| Web 页/动作 | `request_main → HistoryReads/Fence → cancellable_get → wire parse → complete_main`；辅助页校验 file/history/root；草稿保留捕获 heads/root | 四槽取消和迟到响应围栏不能删；根变化不更新 expected head、不恢复旧写依据；网络 DTO 严格校验不是内部重复解析 |
| Web 本地实现 | `Snapshot::PartialEq` 使用 `Arc::ptr_eq`；本地 Memo 以等值短路，page owner cleanup 释放 observer/frame/controller | 未把上游响应式行为当本地事实；有意保留一份 Loading 旧页不等于泄漏；未见心跳重复下载完整历史 |
| HTTP 来源回移 | `Receiver::poll_close` 排空 mpsc、`Dispatcher::pending_result` 跨 Pending 保存结果，按本地 Tokio cooperative/send 竞态核对；upgrade 单独移交 | 是真实异步边界，不把关闭/排空检查作为多余防御删除 |
| 来源证明 | schema 2 连续 parent、完整 before/after、认证生产路由/外测切片、固定收据与失败恢复链 | 许可、origins、包身份、旧收据不能改；`consumer_tests.executed = {}` 不表示动态测试通过；重放、消费者与安全公告扫描证据分别判断 |

已披露的单文件冷扫描、全池 branches 过滤、首父链/namespace 查找、冷重建索引仍有成本，本审阅不重报为新缺陷。`content_state` 缺项代表 LIVE 是既有稀疏状态语义；健康 Unavailable 可表示有意义的观察结论，不能机械归为吞错。Commit.kind 开放字符串、Plan.input Value、根 test 接线与明确 fault-injection、永久收据无 TTL、独立副本和不支持旧格式迁移均按既定产品例外保留。

## 待动态验证与既有责任项

### R1：失败创建目录的重试可能缺少祖先持久化屏障

**状态。** 高优先级恢复候选；原语继承自基线，不是已确认的新并行发布回归。本审阅静态确认下面的同步调用缺口，但没有执行故障注入，也没有证明实际确认后掉电丢失，不把它计作已确认 P1。

**精确路径与来源。** [disk/owned.rs](../../src/engine/src/disk/owned.rs) 的 `private_relative_dir`（190–235 行）在 `mkdirat` 成功时设 `created=true`，仅该分支调用 `directory.sync()`、`parent.sync_all()`（231–234 行）；两者失败均返回错误且保留已创建目录。下次遇 EXIST 则跳过同步，且父目录可打开时不递归重走祖先。

当前发布链 `publication.rs::complete_candidate_verified` 的 Publication stage（751 行）经 `PreparedMetadataObject::store_on → journal/object_storage.rs::write_object_bytes_at`（176–185 行）调用 `DiskRoot::write_atomic`（owned.rs:238–248）。对象路径为 `metadata/objects/<kind>/<hash前两位>/<hash>.json`。最终平台 [atomic_write](../../src/platform/src/fs_safety/unix.rs)（362–431 行）同步文件和当前 shard 目录，不补祖先屏障。

**最小候选交错。** `engine.rs` 初始化（320–329 行）只建立 objects，没有预建所有 kind/shard。在 `metadata/objects` 已持久化但 `publication` kind 首次创建时，让 `mkdirat(publication)` 成功，随后 `fsync(objects)` 一次性 EIO。kind 留下、调用失败；决定阶段经 IN_DOUBT/blocked 返回，重新 `Engine::open → recover_mode → complete_candidate` 恢复同一候选。创建或复用 shard 时 kind 的父已可打开，后续最多同步 shard 和 publication，未找到必须再次同步 objects 的路径。若该次重试最终给出成功确认，则仍需证明 publication 的名字在 objects 中已持久化。journal 重开与 checkpoint 同步的是 metadata，seal 写入 publication-seals，Names 同步容器目录，均不能直接替代 objects 的这一父目录屏障。选择纯元数据的首次建仓操作可避开正文证明；不依赖其他 kind 的偶然创建来补齐父同步。

这是实际 I/O 失败后的持久化边界，不是内部类型已证明的存在检查；0700、持有 FD 或目录当前可打开都不能证明创建名称曾成功同步。最小整改方向应由故障结果决定：保留/补完失败创建的祖先持久化依据，或在缺此依据时拒绝成功完成。不要机械给所有读取加 fsync，也不要删除当前文件/目录同步。

**最小 GitHub 验证提案。** 复用根 `test/engine/unit/history_package/immutable_io/` 和 `test/platform/unit/fs_safety/atomic_io.c` 的隔离子进程/interposer模式，以 FD 的实际 inode/路径绑定目标 objects 目录，只让目标一次 fsync 返回 EIO，不真正执行底层 fsync。记录 mkdir/fsync/rename 的 FD 身份，第一调用断言失败且目录保留，移除注入后重试同 kind 的不可变对象写入，检查成功返回前是否存在 objects 的成功 fsync。分别在单对象原语与真实 `CompletePending` 恢复链复现；若真实链另有补同步，应将候选关闭并记录该调用。扩展到首次多级目录时逐级验证，不给生产加测试计数 hook。此测试证明调用顺序；真实掉电结论仍需获授权的一次性持久文件系统环境，tmpfs/重开成功不证明断电持久性。

### Web 生命周期覆盖提案

Web `inline::Draft::start/cancel` 维护 `files.open_editors`，但尚未证明 owner 销毁能留下非零计数的生产可达链。普通 blur 取消、`write_allowed` 的 loading 准入、发布终态清零及 context Effect 都阻止了简单复现。可在根 Owner 保留 State、子 Owner 初始化并 start 编辑后，在取消 Effect 消费前销毁子 Owner，再挂载并观测 `begin_main(Latest)`；即使该生命周期测试失败，也需继续建立不经过 blur/显式清零的真实 UI 场景。本项只是覆盖提案，不计确认缺陷。

### 原责任任务的整合风险

主任务已知项继续归原责任人，不登记为新发现：

- `R86 run 40830312132` 的六个 Clippy 项（stage 大 Err、两处 `&work`、八参数项）。
- 三项对抗测试只检查浅层 source；已知改动把深度错误控制到四次读取，但多一层 HistoryPackageInvalid。合入后要核对完整错误链和读次数，不能删除必要图准入。
- publication local proof 同调用重复 hash 已有独立补丁；合入时检查其资源身份/恢复验证边界。
- 操作协调不应扫描整个提交的 health，以及回执/健康分离由另一实现任务负责；永久结果不能被当前不可用观察改写，Web 终态与轮询需同步核对。

## GitHub 验证方案

以下命令与新增测试提案均未在本云工作区执行，本次也未手动触发远程 workflow。报告分支的普通 push 可能触发既有 CI，其结果须单独读取，不能视为本审阅动态验收。由主任务在可信 GitHub 验证环境 checkout 准确候选提交，按根固定工具链与离线依赖初始化后执行。先跑对应筛选；核对真实测试数量，禁止零测试作为通过。命令中的筛选是现有模块/测试名，不承诺未来修改后仍对应相同数量。

```sh
cargo test --locked --offline -p storage-engine --lib engine::publication::member_fault_tests
cargo test --locked --offline -p storage-engine --lib engine::publication::parallel_preparation_tests
cargo test --locked --offline -p storage-engine --lib disk::fan_out::tests
cargo test --locked --offline -p storage-engine --lib state_projection::first_commit_tests
cargo test --locked --offline -p storage-engine --lib metadata_adversarial
cargo test --locked --offline -p storage-server --lib completed_account_revocation_precedes_all_engine_admissions
cargo test --locked --offline -p storage-server --lib backend_file_history
cargo test --locked --offline -p storage-server --lib http_shutdown::tests
cargo test --locked --offline -p storage-server --test http_engine history_pages
cargo test --locked --offline -p web-app --lib
cargo test --locked --offline -p smb-protocol
python3 -m unittest discover -s test/tools/rust-sources -p 'test_source_backport.py' -v
python3 -m unittest discover -s test/tools/rust-sources -p 'test_legacy_http_maintenance*.py' -v
python3 .github/scripts/check-http-transport.py
bash .github/scripts/check.sh rust-sources
bash .github/scripts/check.sh contracts
```

重点运行现有 `first_middle_and_last_member_failures_preserve_every_stage_barrier`、`simultaneous_member_failures_preserve_non_prefix_results_and_the_same_decision`、fan-out spawn/reader/目标故障与损坏读回、深度最小反例与最大合法图；不能仅跑成功路径。发布、恢复和源证明的内存/文件系统模拟不代替物理设备掉电、真实 ENOSPC 或吞吐测试。

真实 TCP 继续核对 `history_send_lease_and_idle_deadline_cover_actual_unread_tcp_queues_after_body_eof`、`completed_history_send_cancels_its_deadline_and_keeps_healthy_connection_alive`。补充组合交错可在同一 fixture 用 backpressure 保留运输尾块，取消客户端并请求唯一 backend slot，验证旧任务/块释放之前不准入，后续 keepalive 不被旧 timer 关闭。

浏览器走既有 browser-smoke 的 `history_pages` 场景，使用真实 fetch gate 延迟 branches/parents/field，再换 root/文件并释放旧响应，观测旧 panel/写控件不复活、observer/DOM 有界及 AbortController 行为。服务端真实编码器捕获的历史夹具与 contract-check 联验；纯宿主测试不替代 WASM/DOM 实测。最终相关 suite 与文档门禁结果由主任务绑定提交归档。

## 执行证据与交付边界

| 状态 | 内容 |
| --- | --- |
| 已完成 | 独立目标克隆；Git commit/tree/基线与 188 文件范围核对；四代理源码/规范/调用方及现有测试静态阅读；集中发现交叉复核 |
| 已通过的静态命令 | `git diff --check 13b664e9 99f02cb9` 退出 0，无输出；仅证明整合差异没有该命令识别的空白错误 |
| 报告静态检查 | 新增文件的 `git diff --cached --check` 通过；自包含链接检查确认报告的本地文件目标存在且均在独立目标仓库内，未执行仓库文档检查程序 |
| 未执行 | 仓库脚本、构建、测试、文档/来源自动门禁、动态复现、性能测量、浏览器、Samba SDK/真实客户端及硬件验收；这是任务明确限制，不记为通过 |
| 未变更 | 生产源码、schema/OpenAPI、测试实现、来源/许可收据、main、`zxs-linux`、凭据/权限、运行池 |
| 交付建议 | 优先验证 R1 目录同步重试；两项 P2 可独立分配；已知阻断仍按原责任任务解决。合入批准取决于候选提交的真实验证，不由本说明代替 |

本说明遵循文档链接/编码示例门禁的现有形式；自动 `contracts` 门禁仍须在 GitHub 执行。没有本地测试通过、全仓符合规范或性能提升的声明。
