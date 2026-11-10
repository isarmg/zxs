# 当前登记后的显式历史节点合并

范围：`file.history.merge` 在真实 D3 去登记／下线后的普通消费者。本文记录源码及待执行回归，不代表测试、构建、物理掉电或部署已通过。

## 用户语义

- 来源仍按原文件历史图解析。`source_node_id` 和用户审阅的 `expected_node_id` 分开；来源分支即使在当前投影中已没有文件 occurrence，仍可提供可证明的旧历史节点。归档／只读别名的历史读取规则保持。
- 新结果借用同一历史域的当前 owner 中真实存在的文件 occurrence，包括准确 `entry_id`、`file_id`、路径及祖先目录。只有全局旧 `file_id`、已删除路径或同名新身份均不够；这不是恢复已消失文件的产品命令。
- 同一次 mutation 派生具名结果分支及来源 helper，然后复用既有合并节点、别名、当前文件发布器。原分支、当前默认路径、旧 Commit/Version 和永久收据不改。结果需后续显式选择，才能成为当前文件；不制造新 FULL 正文。
- 所选正文仍必须通过实际副本读取／完整性准入。输入 helper 引用的版本也不能绕过当前登记的不可用版本屏障。Disabled owner 不授予写权限；已经成功的原请求仍先按原 actor 与完整摘要永久重放。

## 来源与恢复证明

普通 `file.branches.merge` 继续只接受已有的当前分支 snapshot。仅历史消费者能构造私有、不可序列化的本次派生输入，来源来自精确 owner occurrence 校验。发布器直接消费该已证明输入，不从不存在的旧 snapshot 查新 helper，也没有全局临时允许开关。

新历史收据增加 `location`（包括显式 null），与已有 `source_inputs`、`name` 及真实 operation actor 一起重建完整原请求摘要。旧收据不回填、不重哈希。普通发布和恢复共用检查：

1. 解析真实永久收据，校验 operation、sequence、actor、请求摘要及结果字段。
2. 对同次合并只构造一次有界历史 ancestry 证明，重新核对旧来源分支及 reviewed heads。
3. 核对新 fmeta 的正文、标签、父边和归属，以及各新 helper 的名称、pending commit、最终 metadata commit、generation 与只读 alias。
4. 每个 helper 的文件 occurrence 及目录祖先必须来自同一当前可写 owner；最终正文变化不能改成其它 entry 或旧隐藏路径。缺证明或篡改时拒绝，不能安装新投影。

历史派生分支不存在于此前的目录引用中，因此该操作不扫描或重写无关冻结目录。普通当前分支合并的目录组合闭包验证不被放宽。此实现不增加 whole-branch 历史模型，不做格式迁移。

## 复杂度与同步

沿用 `MAX_ANCESTRY_NODES`、`MAX_ANCESTRY_HEADERS` 与原写预算。新增工作与本次合并来源和路径祖先规模相关；恢复按新增 merge 记录解析一次收据，再为各 helper 复用来源证明。仅失去 map 增量跟踪时走既有全量键枚举回退，不按每个 helper 重建全部历史索引。不会增加正文写入或独立 fsync 阶段；最终仍走已有事务发布闭包。

## 回归源码与状态

新增 `test/engine/unit/engine/current_history_merge_tests.rs`，从当前 file-branch 测试复用真实 D3 fixture：

- 来源分支的新 HEAD 独占于去登记目标而被当前投影隐藏，仍从其旧节点读取保留副本并产生历史合并；旧 HEAD、Commit/Version、默认路径与永久结果保持。
- 重启后相同请求重放，随后显式选择新结果并写入。
- 当前文件删除及同路径新 ID 重建后，旧文件历史合并精确拒绝。
- 所选历史正文不再可读时，不留下半个 helper 或成功收据。
- Disabled owner 拒绝新派生，同时保留去登记前真实历史收据重放。
- 恢复投影检查拒绝 actor、来源 head、location、父边、alias、commit parent、隐藏 occurrence、替换 entry 身份及缺字段篡改。

本轮仅官方 rustfmt 和静态源码复核；测试／构建／模型／CI 均未执行。统一测试由主负责人执行，并应同时跑原 branch alias/history merge 和 current file branch 回归。

## 目录选择后的普通文件合并接缝

依赖 `CurrentBatchScope` 的 selected-tree 证明。普通 `file.branches.merge` 遇到已选目录的真实 tree 与命名 HEAD 不同，直接消费同一 `batch_scope` 来源，保持 HEAD 作为分支基线、selected tree 作为 child 引用闭包来源。冻结新目录 commit 时，目录祖先取当前 owner，而非旧 commit 中可能已改名的路径。未选命名分支仍须原有精确 HEAD／entries／contexts 闭包，不给任意冻结历史树或别名借写权限。

新增 `current_selected_file_merge_tests.rs`：真实 D3 → 目录选择 → 普通文件合并 → 永久重放／重启，以及 selected occurrence、context、引用篡改拒绝。源码已格式化，未执行。包含与当前选择不等价的未选冻结父组合仍保持明确拒绝；这里不引入多套独立历史子树发布模型。
