# 引擎备份策略与工作区证据错误边界

基线：`97e08850515423038ae3507f1057da649f812357`。本轮只修复真实错误被解释为合法缺失的路径，不变更持久化格式、永久 receipt、FULL 同步或发布时序。

## 备份策略

`file_placement_settings` 的仓库默认份数只能用于没有适用策略的情况。审阅指出的 `.ok()` 若遇到主命名空间缺失，会忽略目录策略；文件绑定缺失或异库 policy 也可能被 `and_then/filter` 忽略。但是不能据此声称磁盘损坏可直接导致降低份数：

`journal::resolve_admitted_snapshot → RawState.try_into → history_package/state_read::validate → project_state/HistoryManifest::validate → HistoryCoverage::finish` 已经在恢复、checkpoint 和导入边界验证这些关系。`manifest.rs` 的 current namespace/main branch 校验保证仓库主分支与 HEAD commit；backup binding 校验保证 file→policy 存在，policy binding 校验保证同一仓库，coverage 拒绝未归属记录。

因此这部分不是新加恢复错误，而是删除内部吞错：已准入关系使用带证明的 `expect("invariant: …")`，保留查询方仓库过滤（公开 `new_file_placement_for_file` 可由调用方提供另一个仓库的 file ID，State 准入不证明调用方的选择）。未知仓库仍由 `source_scope` / `file_placement_settings` 的既有查找返回业务错误。不添加新的 Result 传播链或重复准入。只有进程内变更破坏不变量才会触发这些断言，不能悄悄转成仓库默认份数。

作用域身份不再出现在当前命名空间是合法情况，仍返回 `None`；历史保留文件的显式策略绑定仍有效，不能因此删除或降份。根测试覆盖有效三份、外部缺失 branch/commit/policy 与异库 policy 在既有准入拒绝、删除当前作用域但文件绑定保留的区别。准入测试用于证明删除内部检查的前提，并非声称新增了原来缺少的准入能力；预期在基线上也通过。

## 工作区恢复

首次读取 `workspace.json`、`commit-request.json`、`wire-request.json` 是不可信持久化输入边界：

- `NotFound` 允许表示尚未写入、已缺失的冗余证据；不推断成功。
- 明确 JSON 解码损坏允许尝试另一镜像。原有单坏镜像恢复测试与保留证据语义不变；没有可用证据时返回 `RecoveryRequired` 并保留首个 JSON source。
- EIO、EACCES、资源限制和不安全路径不等于缺失或已确认可恢复的 JSON 损坏；直接传播具体错误及 source。健康镜像不能掩盖已遇到的外部 I/O 失败。
- 有一份损坏或在读取时消失的工作区镜像时，继续要求存活 manifest 的暂存正文验证，不能仅凭元数据恢复承诺。
- `open_regular` 暂存路径检查只允许 NotFound 表示既有内容不可用；其它失败传播。

历史计划的 fencing 字段仍保留现有磁盘字符串表示，在恢复使用点通过共享 canonical-decimal parser 转为 `NonZeroU64`，随后内部使用数值，不重复解析。缺失、零、符号、前导零、溢出与无法递增均明确失败，不再 `continue` 将已发布历史保存伪装成未完成工作区。合法 fence 与 manifest/publication 绑定校验保持。

## 验证记录与入口

本提交准备阶段实际通过：定向 Rust 1.99 rustfmt、`git diff --check`，测试专用 C 夹具 `cc -Wall -Wextra -Werror -fsyntax-only test/engine/integration/workspace_read_error.c`。

准备提交时未执行 Rust 编译和测试；后续在 R16 集成树实际执行，见下文。静态检查不替代运行证据。

已执行 `workspace_commit_replay` 全目标、完整引擎库（含 backup_paths 和 `historical_completion_rejects_invalid_persisted_fence_instead_of_leaving_workspace_open`）。

I/O 故障采用独立进程的测试专用 libc read interposer，不向生产业务函数添加 hook 或参数。编译夹具为临时共享库，分别以 `ZXS_TEST_WORKSPACE_ERRNO=5` 与 `13`、`LD_PRELOAD=<夹具.so>` 运行已构建的 `workspace_commit_replay` 测试二进制，过滤 `workspace_evidence_io_failure_is_not_absence_or_corrupt_mirror --ignored --exact`。marker 只在已初始化夹具的指定证据旁创建，断言原始 errno 链与证据字节未变化；这属于隔离的 EIO/EACCES 系统调用故障注入，不冒称真实坏盘或用户权限实机验收。真实 NotFound 和 JSON 截断由普通临时文件测试覆盖。

没有添加同步调用、部署变更、兼容迁移、凭据/权限变更；策略选择仍线性扫描政策集合，不宣称性能收益。

真实权限失败另有 `unreadable_workspace_request_is_not_a_missing_mirror`：仅在无 DAC 绕过能力的非 root 测试进程运行 `--ignored --exact`，对临时测试文件短暂设为 000，直接确认内核 EACCES 后检查引擎保留 PermissionDenied source 并恢复原权限。集成执行用户 uid=1000、有效 capabilities 为零，该 Rust 测试已实际通过；EACCES interposer 结果不替代这项真实权限验收。

### R16 集成发现与实际故障验证

首轮完整引擎库暴露 25 项失败，均为合法缺失被候选错误地传播：安全打开缺失叶节点产生 `Contract::Io(NotFound)`，缺失父目录产生 `Filesystem::Io(NotFound)`。边界现在精确处理这两种既有表示，不以泛化错误码或 source 链猜测缺失；新增实际缺失叶节点、缺失父目录、损坏 JSON 与健康镜像测试。修正后完整引擎库 589 项通过，包括历史 fencing 的严格拒绝，不放松其规则。

真实 EACCES 测试另发现平台 `Io` 的 transparent 派生隐藏了标准 source 链中的原始 `io::Error`。改为相同 Display 加显式 source，并补平台和引擎回归；错误码、错误消息、权限和业务判定不变。

实际通过的隔离故障入口：`workspace_commit_replay` 普通 26 项；非 root 真实 EACCES 一项；LD_PRELOAD errno 5 与 13 各一次，每次覆盖 `commit-request.json` 和 `workspace.json`，核对 errno 原因链及证据原字节保留。测试二进制来自本次 Cargo JSON artifact，非扫描旧二进制。初始失败与修正后结果分别保留，不能将这些进程级故障模拟称为物理断电或坏盘验收。
