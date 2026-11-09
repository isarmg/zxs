# 开发期短 CI 与完整检查恢复

[CI 入口](../ci.md) · [固定策略与校验器](../../.github/scripts/development-ci.py) · [完整工作流恢复快照](../../.github/ci/full-workflows/ci.yml)

2026-11-09 用户最新要求每次提交运行 GitHub 短 CI。`ci.yml` 对每次 push 自动触发，并保留 `workflow_dispatch`；不含 schedule 或 pull_request 入口。`release.yml` 仅保留人工入口，标签发布、长检查及可复用 SDK job 仍固定暂停。本轮不在本地运行程序、编译或测试；提交后的实际 push run 才是检查证据，不重复手动触发同一轮，不删除 workflow、历史运行或 artifacts，不改仓库权限／保护设置。

push 与人工入口实际执行相同的短配方：`static` 和七套短矩阵 `quality`、`security`、`web`、`contracts`、`build-tools`、`smb-protocol`、`foundation-platform`。这些套件仍运行原入口的全部命令，不按提交有无变化空跑，不把缓存命中、零测试或部分结果当成功。

每次提交后只读核对 push run 的链接、实际 head SHA 与各 job 结论，不用旧 run 的 rerun 冒充当前提交。确需按授权人工发起时选择当前 `main`，只 dispatch 一次；触发结果不确定时先查新 run 而非重复点击。仓内没有计时器，外部提醒不属于工作流配置。并发组为 workflow／SHA／run ID，`cancel-in-progress: false`；每轮独立，后续 push 或人工检查不会替换同组排队轮次或取消正在运行的检查。

`static` job 的执行上限为 5 分钟；成功后启动的每个矩阵 job 上限为 10 分钟，彼此无依赖，也不人为设置串行矩阵槽。唯一活动依赖路径是 5 + 10 = 15 分钟。预算含 checkout、安装、缓存及 job 内所有步骤，不含平台排队和 runner 回收。超时必须失败；冷启动或后续增长不能在 10 分钟内完成时不保证绿灯。七套并行 job 的累计计算时长可能超过 15 分钟，这不是单条依赖路径的执行墙钟预算。

## 暂停范围与证据边界

- 矩阵暂停：`rust-sources` 的完整回归、`workspace`、`foundation-crypto`、`foundation-data`、`foundation-language`、`foundation-async`。
- CI job 暂停：`web-production`、`browser-smoke`、`smb-sdk`、`phase-one-core`。
- Release 在获准人工触发时只保留 5 分钟的 `static`；版本、完整验证、Linux/Web 产物、SDK、组装和发布全部暂停，标签与手动演练也不能绕过。
- 可复用的 SDK 工作流自身也固定暂停，不能由另一个调用入口启用。

暂停是未执行，不是成功，也不是复用历史证明。短 CI 通过不证明上述长检查、真实 Chrome、SDK、生产 WASM、完整来源回归或发行产物验收通过；开发期不会生成新的安装候选或 Release。`static` 仍完整执行原静态检查，包括所有 `test/ci` 回归；它不执行 Rust 产品测试。

`.github/ci/full-workflows/` 保存切换前的三个工作流原文，位于 Actions 执行目录之外。活动文件保留原 job 体、依赖关系、固定 Action SHA、环境初始化、缓存边界、同 commit SDK、失败材料和产物验收，施加固定暂停、短预算及 push/人工入口、独立并发组和不取消策略。校验器核对原快照 SHA-256，并只允许明确的固定差异；新增工作流、恢复 schedule 或增加其他触发器、重复 push、删除暂停、input/env 启用、超时放宽或吞错均使静态检查失败。

旧 `select-ci-suites.py` 的 13 套完整选择与精确历史复用证明保留供恢复审阅，当前短 CI 不调用它。`development-ci.py select` 对 push、dispatch、缺少历史证明及其他非准入事件测试环境都输出同样七套，连同 `automatic_triggers: true`、暂停清单和空 `reused` 列表；不存在完整矩阵 fallback 或手动全跑开关。保留的回归通过 push、dispatch 及伪造 schedule 等环境执行实际 selector，验证环境不能恢复长检查，同时拒绝新增 schedule、空矩阵、任意 job 条件及伪成功命令。

完整 foundation 覆盖映射和旧工作流回归现在明确检查保存的完整 recipe。`retained_recipe_scripts()` 证明的是恢复配方的静态接线，不是当前活动开发覆盖；其中 `executed` 分类表示原完整 recipe 的执行设计，不是本次运行回执。当前活动矩阵由新的正负回归单独核对，不将暂停路由伪装成可运行路由。

## 恢复条件与操作

本次授权是每次 push 运行短 CI，并保留人工入口，不是恢复完整检查。新增 schedule、PR、标签发布或长检查仍是独立决定，需用户另行批准。保留 dispatch 入口不等于某轮已经开始或通过，只有准确提交的实际运行能作为证据。任何后续触发调整仍须在独立提交中同步活动 workflow、`development_workflow()` 固定投影和本页；不能只更改 YAML 导致策略校验失配。

所有当前任务完成后，必须另行明确批准恢复长检查。不得通过 dispatch 参数、环境变量、修改保护规则或临时取消暂停提前恢复。

恢复完整 CI 时，基于保存的完整配方逐项准备独立恢复提交，同时保留届时仍有效的 push 与人工入口要求；不能把触发方式调整说成长检查恢复，也不能盲目覆盖后续改动。原文快照用于审查每个 job 的命令、120/60/30 分钟原矩阵预算和发布依赖；不要只复制工作流而留下开发策略校验器。

恢复前核对：完整 13 套 selector、前置 static、WASM 与同 commit SDK 到 Chrome 的依赖、全部 Release 验证到组装/发布的依赖、产物缺失失败和原有负向回归全部恢复。新增配置验证与实际长套件执行另行排程；恢复定义不等于其检查已经通过。分支保护和 required checks 均不由本次变更修改。
