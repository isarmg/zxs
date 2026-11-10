# 全量 CI 与暂停边界

[CI 入口](../ci.md) · [固定策略与校验器](../../.github/scripts/development-ci.py) · [完整工作流恢复快照](../../.github/ci/full-workflows/ci.yml)

2026-11-10 用户先要求“现在跑全量ci”，随后要求“跑自动ci”。`ci.yml` 的全部检查及可复用 `smb-sdk.yml` 已恢复；main 分支 push 自动全量执行，并保留 `workflow_dispatch`。pull_request、schedule、标签发布与 Release 仍暂停，不部署、不访问真实运行池。

## 本轮完整范围

- `static` 执行全部既有静态门禁，执行上限恢复为 10 分钟。
- 矩阵固定为全部 13 套：`rust-sources`、`quality`、`security`、`workspace`、`foundation-platform`、`foundation-crypto`、`foundation-data`、`foundation-language`、`foundation-async`、`web`、`contracts`、`build-tools`、`smb-protocol`。
- `workspace` 保留原 120 分钟预算，`foundation-crypto` 为 60 分钟，其余矩阵为 30 分钟。所有原命令完整执行，不按有无改动、缓存或历史结果跳过，不把超时、零测试或部分结果当成功。
- `web-production`、`browser-smoke`、`smb-sdk`、`phase-one-core` 恢复，分别保留原 job 体与预算。实际 Chrome 验收仍依赖 static、生产 WASM 与同 commit SDK；一期 core 独立等待 static，不等待 workspace 长测。
- `release.yml` 原样保持暂停：人工入口仅可执行原 5 分钟 static；版本、完整验证、Linux/Web、SDK、组装与发布六个 job 仍固定暂停。本次全量 CI 不等于恢复 Release 演练或发布。

执行预算包含 checkout、安装、缓存及 job 内全部步骤，不包含 GitHub 排队或 runner 回收；它不是完成时间承诺。矩阵 `fail-fast: false`，保留完整失败与未执行证据。

`development-ci.py select` 输出完整 13 套、`automatic_triggers: true`、`push_branches: [main]`、空 `paused_suites`、仅 Release 的暂停清单与空 `reused`。任何 event、input 或 env 均不能缩短矩阵或恢复发布。旧 `select-ci-suites.py` 与其证据复用回归保留，但活动 CI 不调用它；本轮明确要求全跑，不使用历史复用。

## 工作流与证据保证

`.github/ci/full-workflows/` 保留原三个快照字节与 SHA-256，不直接作为 Actions 入口。活动 CI/SDK 仅施加已审阅的 main push／人工触发、独立并发和固定全矩阵选择差异，不改原检查命令、Action SHA、环境初始化、缓存边界、同 commit SDK、失败材料或产物验收。Release 继续使用原暂停投影。

策略校验仍是 static 的第一个命令，必须同步核对活动 YAML 与固定投影。正负回归拒绝额外入口、扩大 push 分支／标签范围、缩小矩阵、隐藏 job、修改预算、吞错、去掉依赖或缺失产物门禁。恢复定义与这些静态回归通过不等于长套件实际通过。

并发组仍为 workflow／SHA／run ID，`cancel-in-progress: false`；本轮不取消或覆盖其他运行，不删除 workflow、历史运行或 artifacts，不改分支保护或 required checks。原 SQLite 两个固定维护入口不变，本次不触发。

## 执行与结果记录

本次提交进入 main 后核实对应 push run，记录 run URL、实际 `head_sha`、每个 job 的最终结论及失败原因；不另行重复 dispatch。触发结果不确定时先查新 run，不能重复触发。已经启动的短 CI 仍按旧提交和旧配方结算，不能冒充这次全量证据。

只把准确源码、实际完成的检查记为通过；明确分列失败、跳过、未执行和环境限制。CI 的安装候选 artifact 只有该 job 完成打包、浏览器和组装后才算候选，不代表其他并行 job 或整个发行验收通过，也不会创建标签或 GitHub Release。

后续 PR／schedule 触发、Release 演练或发布仍须另行明确安排；自动 main push CI 不构成发布或操作真实用户数据的授权。
