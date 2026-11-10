# 当前注册表下的文件节点标签准入

集成基线：`348ff9e89`。从交接目录的标签候选逐块接入当前主线，原候选基于 `9ae138d0`，复用 `fc2af0bf` 的独立标签规划。范围是既有 `file.tags.update`，不改变标签产品语义、HTTP 认证、正文访问权限、磁盘退出或恢复协议。

## 证明与写集

- 普通 `require_operation` 白名单不增加标签操作。独立 planner 在闭包前拒绝未准入 registry，规划后拒绝任何 Prepared payload；实际发布和冷 replay 使用同一纯计划重建，不能只靠操作名通过。
- 输入严格反序列化，保留原生 actor、完整摘要（包括显式空 location）、批量来源、精确 expected node、只读别名、标签名称及 16 项容量检查。分支、仓库、历史、head/fork、节点和正文的真实键/ID/历史域相互绑定；原生分支归属与继承标签规则保留。
- 当前节点推进必须来自准入分支和 owner 的相同 entry ID、文件 ID、路径及正文尺寸。历史节点仍可生成不读正文的标签子节点，但不能把较新的当前正文或分支头回退；隐藏、Detached 或 Disabled 目标不会由标签重新开放。
- `namespace_tags` 固定 registry、activation sequence、mask、认证源 snapshot 和 sequence。operation/ordinal 派生的新 ID、元数据时间、原始标签集合与增删集合共同重建唯一计划，不在 replay 读取时钟、随机源或正文。
- 写集仅包含计划中的 immutable metadata children、确实匹配的分支和目录后继、对应 context/selected tree，以及一个原生 receipt、actor 和 event。穷尽解构 State；正文 versions、placements、物理名称/位置、路由、选择意图代数、备注、计划、auth 与 topology 不得夹带变化。
- 旧 metadata 子节点、标签 receipt、actor 和 event 保持不可变。移除来源标记或换 kind 不能把新 Tags 子节点隐藏起来。新 Merge 子节点必须有新合并来源记录、同主体／操作／sequence 的新原生收据和 `file_metadata` 提交，被存活分支的新 HEAD 实际采用，且正文和标签来自所选旧节点。此证明先于 unchanged-HEAD 快捷路径；历史合并的 `AlwaysNewNode` 与普通合并的现有节点复用规则分别保留。
- 统一冷恢复前缀先绑定原始 snapshot/history，对准确前驱执行纯 `advance`，再验证原生收据和 Candidate 的版本差分，之后才允许 Accept-only publication discovery 或 published barrier 读取 local payload proof。共享 `verify_current_candidate_receipt` 同时拒绝原生标签候选的非空 Candidate.versions；更改 kind、移除标签标记或伪装成 no-op 也不能绕过前缀证明。

## Named HEAD 与 selected tree

旧候选把 selected composition 放进以 branch ID 索引的源表，再无条件写回 named HEAD，可能在标记旧正文时回退未匹配的较新目录头。本次分开两条证明：

1. Named HEAD 只从 `directory_projection` 的已认证保留图重写。每个 frozen descendant 来自该父图的真实 tree，不借用同名子分支的新 HEAD；未匹配的 named HEAD 不变。
2. 实际选择从 `batch_scope` 认证其精确 selected graph 与当前 owner occurrence、继承意图。只更新对应 selected tree，允许它继续不同于 named HEAD。
3. 同一精确投影和已重写子引用共享本次计划的缓存；不按可变 branch ID 混用不同组成。旧 commit/tree 永不改写，新树保留确切 parent tree 与 commit 来源。

磁盘注销只移除 ZXS 登记及目标独占当前 occurrence，不擦除原文件，不创建备份或恢复文件；登记下线保留登记和数据。本次标签操作不改变这些规则。

## 定向回归源码

`test/engine/unit/engine/current_file_tags_tests.rs` 包含 18 个测试函数：

- 真实 D3 注销/下线后的当前标签、不可用历史正文标签、no-op、永久幂等、未选目录、继承意图和只读别名。
- 精确节点/正文/历史身份、缺失 occurrence、预期头、位置、容量和旧工作区拒绝；tracked 与序列化恢复 State 的 actor、receipt、事件、目录 context 和封闭写集攻击。
- 真实 freeze root → 更新正文 → D3 → 选择旧 frozen composition → 标签 → 两次冷开。所选旧正文获得标签，较新 named child HEAD 和文件分支 HEAD 保持不变；再次编辑标签也不得回退。
- 同一批次的历史 no-op 与当前变更、双来源移除标签，以及第二目标冲突时整批不发布。
- Tags 伪装成 Merge 并附伪造合并来源记录，但不推进 HEAD 的负例；真实非复用普通合并的正例，以及只篡改其新 Merge 节点标签的负例。tracked 与冷解码 State 都须执行同一来源证明。
- 32 个真实 accepted/published 冷 fixture（两种决定 × 当前/历史来源 × valid/actor/source/context/location/payload/disguised_payload/disguised_noop_payload）。真实 snapshot/history、Prepare/Accept 和可选 Finalize/Publication/Seal 后移除一个候选 snapshot 副本。合法对照应修复；攻击应在修复/投影前拒绝并保持 SYSTEM 和所有 metadata/data 根字节不变。
- 原生 payload 变体保持完整 State/receipt 证明合法，仅向 Candidate 夹带已有 D04 data-only 版本。另有两个变体更改 kind 并移除标签信封，其中 no-op 不留下新标签子节点，必须由精确版本差分拒绝。三个变体均以 inotify 验证其版本目录及直接子文件无 OPEN/ACCESS。此断言不声称启动从未获取合法根目录句柄。

定向验证用 `cargo test --locked --offline -p storage-engine --lib <filter> -- --test-threads=1` 执行以下模块，检查标签与既有选择/合并交互；这些范围不替代全 workspace 测试：

- `engine::disk_registry::current::file_tags::tests`
- `business::file_tags::tests`
- `engine::disk_registry::current::batch::tests`
- `engine::disk_registry::current::file_branches::tests`

上述回归与统一冷恢复前缀一并编译执行；32 个冷 fixture 均包含在下列最终标签测试结果中。

## 成本与实际验证

Live 使用原 Config::max_entries。Replay 由已准入 occurrence 最大数和目标数推导有限重建上界，不使用 usize::MAX；候选解码继续受原 MAX_METADATA 约束。计划只读取认证 State，遍历含目标文件的 named scope 和相关 selected graph；成本取决于各源图累计展开的 occurrence/ref 数，嵌套目录或多个冻结别名可能重复验证共有后代。临时缓存只合并完全相同投影的新对象，不声称全图线性或常数成本。tracked 写集按变化键检查，冷 State 比较相关完整映射。统一前缀会在所有 registry 候选的正文证明读取前认证原始 snapshot/history 并验证 State 转移；合法标签候选的 versions 为空。本次未增加数据根探测、正文复制、锁、fsync 或 quorum。

已执行：独占文件补丁 `git apply --check`、集成后的 `git diff --check`、`python3 .github/scripts/check-architecture.py`、固定 1.99.0 rustfmt 和核心 libtest 编译。初次编译发现并修复写集验证闭包的临时借用生命周期错误；Clippy 发现的规划参数数量问题已通过 `TagPublication` 归并解决，测试的单元素克隆切片改为借用切片，没有放宽 lint。

首次 16 项标签测试为 14 通过、2 失败，失败源于旧夹具把 `disk_id` 当成 `slot_id`，以及未反映当前独立历史分支准入顺序，均已修正。新增伪装 Merge 回归在修补前实际失败，确认仅凭 provenance row 会绕过 unchanged-HEAD 快捷路径，随后补入上述采用与来源证明。

最终以 `cargo test --locked --offline -p storage-engine --lib --no-run` 生成 libtest，再直接运行对应过滤器和 `--test-threads=2 --nocapture`。四组共 40 个测试全部通过：

- `engine::disk_registry::current::file_tags::tests`：18/18，334.29 秒，包括 32 个 accepted/published 冷 fixture、两项新增 Merge 来源回归和 selected/named HEAD 冷开回归。
- `business::file_tags::tests`：4/4，1.47 秒。
- `engine::disk_registry::current::batch::tests`：6/6，57.00 秒。
- `engine::disk_registry::current::file_branches::tests`：12/12，137.56 秒，包括普通、历史与 selected merge。

本节只记录上述定向验证，不代表完整 workspace、HTTP、物理介质或吞吐验收。物理介质与吞吐验收未执行。
