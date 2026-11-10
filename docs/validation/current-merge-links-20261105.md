# 当前注册表下的关系连线准入

集成基线：`348ff9e89`。本轮整合原基线 `9ae138d0` 的历史关联候选，并沿用本次统一冷恢复前缀证明。范围是原有 `merge-link.create`，不是文件分支合并或目录合并。关系只固定两个已有历史节点，不改变 HEAD、内容、选择或物理名称，也不提交已有工作区。

## 证明与边界

- 保留原生 actor、幂等摘要、不同分支、同一历史、相同文件作用域和第一父链端点判断。历史节点可不是 HEAD，正文不可用不等同于历史节点不存在；这不会授予读取、恢复或写入正文的权限。
- 通用 `require_operation` 白名单不增加关系类型。独立 planner 在候选离开规划前拒绝 payload，并重建原生 `OperationResult`，验证其 actor、操作 ID、摘要、历史 ID、固定端点和精确结果字段。
- `namespace_merge_link` 绑定当前 registry、activation sequence、mask、仓库和历史身份，以及两端的 scope/head、fork、generation、state、kind、file/node ID 和选定提交。两条第一父链逐项核对真实键/ID/历史；缺失、跨历史、循环、Migrated 分支或另一旁支的提交不构成合法来源。
- 只有一个全新 `MergeLink`、一个永久回执、一个主体记录和一个标准事件可以增加；sequence 前进且 business generation 精确加一。穷尽解构 `State`，保留其余所有字段，包括 auth、topology、分支、提交、版本、路由、内容状态、文件/目录选择、备注、物理名称和位置。
- 已有关系、历史回执、主体和关系事件不得在后续 unchanged-HEAD 快路被修改或删除。缺失回执、改 kind 或移除来源字段都不能掩盖新增关系。未来若准入历史拆分，须为其关系重映射建立独立证明。
- `CurrentNamespaces::advance` 在 unchanged-HEAD 快路前重验。统一 recovery preflight 在认证 snapshot/history 之后、publication discovery、候选副本修复、payload/name 投影和 publication 补写之前做纯 advance；成功完成后仍由原路径安装可见状态，不提前安装。
- 将 `merge-link.create` 纳入主线 `verify_current_candidate_receipt` 的空 `Candidate.versions` 门禁。带版本证据时，在 `discover_publication` / published barrier 之前读取原始认证快照中的 receipt 并拒绝；保留后续 State 的完整证明，不能以共享门禁代替它。
- 原始无 registry 回执仍由原生幂等路径原样返回，不补写来源、不生成新关系。未准入的 pending registry 在执行 change 闭包前拒绝。

## 回归代码

`test/engine/unit/engine/current_merge_link_tests.rs` 新增 12 个测试函数，其中 1 个需 `fault-injection`，覆盖以下场景：

1. 两次真实 D3 后新增历史端点关系，检查隐藏项、共享历史、所有分支 HEAD、未提交工作区和永久结果；两次冷开不重新发布。
2. registry 之前的旧回执经历 D3、新关系和冷开后仍精确重放。
3. 仓库所属 data-only 根停用或删除登记后，关系保留 metadata-only 语义，不打开已停止根。
4. 空 actor、同分支、同历史另一旁支端点、跨历史、文件/目录及不同文件作用域拒绝；同文件分支关系正例冷开保留。
5. Archived 历史端点与 Sealed 工作区不因画线重新取得写权限或丢失暂存。
6. 46 类 receipt、主体、关系、来源、历史与封闭写集攻击，同时验证 tracked State 和序列化恢复 State。
7. 无新关系回执时拒绝旧关系、actor、receipt、event 变化。
8. 将新关系包装成另一原生 operation kind 仍拒绝。
9. 12 类来源身份/第一父链不完整、循环或跨域输入拒绝。
10. 专用规划正例与摘要、来源、额外字段、payload、缺失关系、代数攻击；普通白名单和 pending registry 保持拒绝。
11. 包含真实 D04 data-only 既有版本的夹带 payload 攻击：State 与原生 receipt 仍完全有效，仅改 Candidate.versions；在 Accept-only 与已 Published 两路用 inotify 检查版本目录及其直接子文件的 OPEN/ACCESS 无事件，覆盖 publication discovery 的 local-proof 读取。此断言不等于启动过程中从未获取根目录句柄。
12. 真正写入 Prepare/Accept，及可选完整 Finalize/Publication/Seal，再移除候选专属的一个 snapshot 副本。合法对照应修复并完成；重哈希的来源、actor、第一父链和物理位置攻击必须在副本修复/投影前拒绝。结构、固定成员和所有哈希仍真实有效，缺失对象继续缺失，SYSTEM、全部存活 metadata/data 根、停止目标的文件字节不变。
13. Finalize、容器名称投影、首个 Publication 边界故障后的冷恢复与幂等。

## 成本与验证状态

新增来源校验只读已认证 State，第一父链长度为 L 时使用 O(L log L) 的有序访问集和 O(L) 临时引用空间。写集验证使用已有 tracked keys；冷 State 无跟踪时比较相关完整映射。不增加数据根探测、正文读写、锁、fsync 或 quorum 操作；不将启动既有日志同步描述为零 I/O。

本次已执行：候选按所负责文件的 `git apply --check`、两份 merge 补丁的逐字节相同比较、`git diff --check`，以及固定 Rust 1.99.0 的 rustfmt；均通过。现有 `current_namespace_tests` 的通用操作拒绝回归改为直接调用通用 planner，继续保证 `branch.create` 与 `merge-link.create` 不能凭 kind 绕过专属证明执行闭包。

原环境缺少 Rust/Cargo，初次测试命令以退出码 127 终止；随后补齐固定 Rust/Cargo 1.99.0，统一核心 libtest 编译通过。测试使用 `TMPDIR="$PWD/target/test-tmp"`，启动时所在文件系统约有 56 GiB 可用空间，没有调整生产容量限制。

通过：

- `target/native/debug/deps/storage_engine-4fb75adcb35977be engine::disk_registry::current::merge_links::tests --test-threads=2 --nocapture`：12/12，299.57 秒。包括真实 D3、故障注入、Accept-only／Published 认证伪造与 inotify 正文访问检测；日志 `target/validation/merge-links-tests-verified.log`。等价 Cargo 入口为 `cargo test --locked --offline -p storage-engine --lib engine::disk_registry::current::merge_links::tests`，不能仅用文件名过滤而漏跑测试。
- 同一 libtest 的 `engine::disk_registry::current::tests`：70/70，99.05 秒，包含 batch、initialization、rename、workspace overlap、placement 等；日志 `target/validation/current-namespace-regressions.log`。
- 三项 `--exact` 既有回归分别通过：`business::history::replay_tests::merge_link_retry_keeps_original_result_after_split_and_restart`、`maintenance::split_tests::split_merge_links_have_local_branch_stubs_and_explicit_crossing_boundaries`、`tests::merge_link_endpoints_must_belong_to_the_selected_distinct_branches`；日志 `target/validation/merge-links-existing-regressions.log`。
- 修改后的 `engine::disk_registry::current::tests::membership_registry_current_generic_imports_never_invoke_the_mutation_planner` 单独 1/1 通过；日志 `target/validation/merge-links-generic-gate.log`。
- 最后合入 Tags 的 Merge 来源收紧与备份来源修补后，重新编译的 libtest 复跑两项关键 `--exact` 测试：`current_merge_link_preserves_projection_history_workspaces_and_permanent_results` 1/1（90.45 秒）、`current_link_proof_rejects_tracked_and_cold_closed_write_set_forgery` 1/1（26.74 秒）。分别保留双 D3／重启正例与全部 46 类冷热伪造拒绝，日志为 `target/validation/merge-links-final-combination.log`、`target/validation/merge-links-final-write-set.log`。

首轮 9/12，3 项失败均已修复并在上述 12/12 中复验：普通仓库写入尚未创建独立文件选择，测试须先调用真实文件分支 API；删除不存在选择键不是有效伪造，现改为实际插入，并逐项断言全部 46 个攻击确实改变 State；`D04` 是 `slot_id`，不能与生成的 `disk_id` 比较。原始失败日志保留于 `target/validation/merge-links-tests.log`。未通过放宽生产证明来修改期望。

这些是组合工作树中的核心定向证据，不代替后续最终组合代码的完整 CI、严格 Clippy、真实设备掉电或吞吐测量。物理盘掉电与吞吐测量未执行；本机故障注入不代表硬件验收。
