# 开发期短 CI 与完整检查恢复

[CI 入口](../ci.md) · [固定策略与校验器](../../.github/scripts/development-ci.py) · [完整工作流恢复快照](../../.github/ci/full-workflows/ci.yml)

2026-11-08 按用户要求暂停全部自动 Actions：`ci.yml` 不再订阅 push 或 pull_request，`release.yml` 不再订阅标签 push。两者只保留 `workflow_dispatch`；SDK 仍仅可复用调用且其 job 固定暂停。未经新的明确指示，不手动触发、重跑或恢复自动运行。本次不删除 workflow、历史运行或 artifacts，不取消既有运行，不改仓库权限／保护设置。

人工恢复检查时保留的短配方为 `static` 和七套短矩阵：`quality`、`security`、`web`、`contracts`、`build-tools`、`smb-protocol`、`foundation-platform`。这些套件仍运行原入口的全部命令，不把缓存命中、零测试或部分结果当成功。

`static` job 的执行上限为 5 分钟；成功后启动的每个矩阵 job 上限为 10 分钟，彼此无依赖，也不人为设置串行矩阵槽。唯一活动依赖路径是 5 + 10 = 15 分钟。预算含 checkout、安装、缓存及 job 内所有步骤，不含平台排队和 runner 回收。超时必须失败；冷启动或后续增长不能在 10 分钟内完成时不保证绿灯。七套并行 job 的累计计算时长可能超过 15 分钟，这不是单条依赖路径的执行墙钟预算。

## 暂停范围与证据边界

- 矩阵暂停：`rust-sources` 的完整回归、`workspace`、`foundation-crypto`、`foundation-data`、`foundation-language`、`foundation-async`。
- CI job 暂停：`web-production`、`browser-smoke`、`smb-sdk`、`phase-one-core`。
- Release 在获准人工触发时只保留 5 分钟的 `static`；版本、完整验证、Linux/Web 产物、SDK、组装和发布全部暂停，标签与手动演练也不能绕过。
- 可复用的 SDK 工作流自身也固定暂停，不能由另一个调用入口启用。

暂停是未执行，不是成功，也不是复用历史证明。短 CI 通过不证明上述长检查、真实 Chrome、SDK、生产 WASM、完整来源回归或发行产物验收通过；开发期不会生成新的安装候选或 Release。`static` 仍完整执行原静态检查，包括所有 `test/ci` 回归；它不执行 Rust 产品测试。

`.github/ci/full-workflows/` 保存本次切换前的三个工作流原文，位于 Actions 执行目录之外。活动文件保留原 job 体、依赖关系、固定 Action SHA、环境初始化、缓存边界、同 commit SDK、失败材料和产物验收，仅施加固定暂停及短预算。校验器核对原快照 SHA-256，并只允许审阅过的固定差异；新增工作流、删除暂停、input/env 启用、超时放宽或吞错均使静态检查失败。

旧 `select-ci-suites.py` 的 13 套完整选择与精确历史复用证明保留供恢复审阅，当前保留的人工入口不调用它。当前入口 `development-ci.py select` 对 push、PR、dispatch 及缺少历史证明都输出同样七套，连同暂停清单和空 `reused` 列表；不存在完整矩阵 fallback 或手动全跑开关。

完整 foundation 覆盖映射和旧工作流回归现在明确检查保存的完整 recipe。`retained_recipe_scripts()` 证明的是恢复配方的静态接线，不是当前活动开发覆盖；其中 `executed` 分类表示原完整 recipe 的执行设计，不是本次运行回执。当前活动矩阵由新的正负回归单独核对，不将暂停路由伪装成可运行路由。

## 恢复条件与操作

恢复自动触发和恢复长检查是两个独立决定，均需用户另行明确批准。自动运行暂停不等于已通过，期间新增提交没有自动验证证据。恢复自动触发时应在独立提交中同步调整活动 workflow、`development_workflow()` 的固定投影和本页；不能只更改 YAML 导致策略校验失配。

所有当前任务完成后，必须另行明确批准恢复长检查。不得通过 dispatch 参数、环境变量、修改保护规则或临时取消暂停提前恢复。

首选撤销本次独立的 CI-only 提交，恢复所有工作流、校验入口、覆盖工具和回归测试；若之后发生相关改动，先评审差异再准备独立恢复提交，不能盲目覆盖。原文快照用于审查每个 job 的命令、120/60/30 分钟原矩阵预算和发布依赖；不要只复制工作流而留下开发策略校验器。

恢复前核对：完整 13 套 selector、前置 static、WASM 与同 commit SDK 到 Chrome 的依赖、全部 Release 验证到组装/发布的依赖、产物缺失失败和原有负向回归全部恢复。新增配置验证与实际长套件执行另行排程；恢复定义不等于其检查已经通过。分支保护和 required checks 均不由本次变更修改。
