# 当前登记的冷恢复前缀与正文准入

本记录对应 `348ff9e89` 后续工作树对 `handoff/2026-11-05-unfinished/prefix-proof.patch` 的整合，不将原候选或历史 CI 结果视为本轮通过证据。相关入口为 `engine/recovery/workspace_receipts.rs`、`payload_admission.rs`、`membership_transition/pending/activation/exit/replay.rs` 和 `degraded_recovery.rs`。

## 真实边界与顺序

- 所有 current-registry 候选，包括只有 ACCEPT、没有 `versions` 或伪装操作名称的候选，均先验证固定成员、精确前驱、原始 snapshot 和 history 闭包，再运行纯 State/namespace 过渡证明。只有通过后，publication discovery 才能读取 data-only 本盘证明。apply 和显式 degraded recovery 保留同一准入。
- 第一次 registry activation 必须是代际规定的空正文 D3；冷前缀推进还需要完整 seal 与实际保留成员的 D3 日志回执，不用序列化 seal 代替实际 journal 屏障。新增 Preflight 证明只能构造纯验证视图，不能安装 live Engine namespace。
- 原始候选 snapshot 始终独立于降级结果覆盖。前缀仅保留最新已发布 State 和至多首个待完成候选，工作区配额仍在恢复副作用之前基于已证明发布的永久收据检查。
- 非 backup 的正文候选必须与精确新增的版本和放置增量相等；现有版本或放置不得被无操作收据冒充为新增写入。每个正文还必须由同 actor、operation、sequence 的实际可写 namespace 新提交覆盖。
- `plan.cancel`、`branch.archive`、`merge-link.create`、`file.tags.update` 保留空正文约束，但操作名称不是唯一门禁。伪造名称、删除操作标记或伪造孤立版本仍受完整 State 过渡与版本增量检查。
- backup 使用自己的当前规则、历史版本和精确 Prepared 证明；current backup 的 degraded recovery 继续明确拒绝。

## Copy 的补充证明

原候选只核对来源和路径，不能排除覆盖已有目标或复用旧文件身份。对包含正文的 Copy 候选，当前实现从永久 `transfers` 重建原始 Copy 请求并重算摘要，通过命名 HEAD 比较和 selected-tree admission 取得真实原始来源。

随后纯内存重建目标投影：目标必须原来为空，父目录必须属于该目标当前组合或同批先前新增目录；重复映射、重叠来源、重叠活动目标范围和目录复制到自身子路径均拒绝。原目标条目保持不变，全部复制条目的路径、类型、大小和来源正文必须一致，entry/file/version 使用当前前缀未知的新身份。目标索引一次建立，后续冲突检查使用有序映射，不为每条新增路径扫描全部旧目标。

这不是 workspace 完整请求重算。workspace 原始完整 CommitRequest 未持久化，本轮验证其候选、actor、operation、精确新版本增量、原 HEAD 父边和实际新提交作用域，不宣称恢复了缺失的请求字段。

## 成本与同步

新增工作发生在冷恢复 metadata-only 准入：逐 current-registry 候选解码/验证 snapshot 与 history 闭包，维护相邻 State/纯 namespace 视图并检查实际 delta；不新增 fsync、写入或持久缓存，不减固定 N/Q 或全部成员屏障。Copy 目标有序索引的空间与实际触及目标及新增条目规模相关，来源选择仍按原 batch 语义扫描对应当前组合。本记录没有吞吐或物理掉电结论。

## 验证记录

- 交接七份补丁 SHA256 均通过；`merge-links.patch` 与暂存快照字节相同，只接线一次。
- 相关 Rust 文件 rustfmt 和 `git diff --check` 已通过。
- 新增/扩展源码覆盖伪装 kind、删除标记、无业务变化夹带已有正文、孤立新版本、actor 不匹配、工作区配额、Copy 已占目标/缺父目录拒绝，以及同批先复制父目录的正常 Copy。
- `storage-engine` libtest 组合编译通过；直接执行生成的 `storage_engine-4fb75adcb35977be`，统一使用 `--test-threads=2 --nocapture`。定向结果共 35 项通过：
  - `current_plan_cancellation`：13/13，包括伪装名称、移除标记、已有正文夹带、孤立新版本及真实零 OPEN/ACCESS 冷恢复矩阵。
  - `current_payload`：3/3，包括本盘 publication-only 的冷工作区配额、actor/operation 绑定，以及现有 bootstrap payload 回归。
  - `retained_snapshot_mismatch_rejects_before_repair_or_visible_state_change`：1/1。
  - `disk_exit_current_batch_real_d3_restarts_without_reopening_or_rewriting_the_target`：1/1。
  - `disk_exit_selected_projection_batch_copies_exact_selected_versions_and_reopens`：1/1。
  - `branch_archives::tests`：14/14。
  - `current_copy_preflight`：2/2。首轮夹具未初始化 repository 的根目录 scope 而失败；补真实 `initialize_directory_scope` 后重编通过，没有放宽生产检查。
- 最终组合加入 file-tags Merge 子节点来源证明后再次编译通过，libtest SHA256 为 `848b5accdec896f9d4ac1dec2fd05d1ebd8b6532246ab426f611c06bba9272ea`。上述 35 项中的 8 项关键用例再次通过：`current_payload` 3、`current_copy_preflight` 2、snapshot 错配 1、selected-tree Copy 冷重启 1，以及全篡改冷恢复矩阵 1（160.97 秒）。这些是复验，不另计为新增测试。
- 独立源码复核未发现 Copy 空目标/新身份检查在当前 selected-tree 语义下的额外误拒或绕过，这不替代动态或完整安全验收。
- 本记录不代表完整 workspace、全部 Clippy/目标矩阵或 CI 通过。
- 未执行真实盘掉电或真实设备部署。
