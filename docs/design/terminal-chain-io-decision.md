# 终态链原目标的真实 epoch 决定

[原后继计划](terminal-chain-activation-plan.md) · [冻结 Current](terminal-chain-published-current.md) · [辅助工具](../components/helper.md) · [任务清单](../../TASKS.md)

基于真实父 `41367d5d`，保留 Backup 普通盘根库保留式交换及其拒绝边界。已有 `chain-activation-plan.json` 仅选择 SYSTEM 中的精确未来 epoch/Stop 后继。本片新增原目标的实际不可覆盖 native `TerminalChainIoAdmission` choice，以及双端完整计划/决定证据和原操作显式冷续作。**这不是另一次规划，但也不是活动 I/O 许可：仍不更新 bootstrap record/head，不消费 Stop，不改盘 seal、首槽日志、活动 Current、writer 或数据库，不启动服务，不返回完成 readmission，不放开 H03。**

按用户要求未审查、编译、测试、静态检查、CI 或浏览器验收，未操作真实系统/磁盘。五项 native 回归、一项 HTTP 严格输入和已有路由边界扩展只是未运行源码。仅格式化明确修改的 Rust 文件。

## 决定身份与真实消费者

`TerminalManagement::decide_chain_io_admission` 先重新打开原 actor/operation/proposal/target 对应的 `TerminalChainPublishedCurrent`。完整 current checkpoint、原目标物理身份/Stop/锁、两根原生完整输入、实际首槽三帧/Publication/seal 都必须成立。之后从原保存的计划重新核对：

- `terminal-chain-io-admission` 原 epoch，不接受浏览器选择新值；
- 完整原计划对象 hash，包括原前驱 head 和精确后继 record；
- 原 Publication/seal、完整 source/checkpoint、所选物理身份；
- bootstrap 仍是精确原前驱，未来 record 未被当作当前 head。

目标独立槽位为 `metadata/membership-decisions/<planned_epoch>/`。它使用现有原子、不可覆盖、文件/目录同步和实际名称读回的 `DecisionStore`，但 choice 为独立严格的 `TerminalChainIoAdmission`。原 `TerminalChainReadmission`、旧 TerminalResume、普通 Joint 或 Backup choice 不能改标签复用。epoch 是固定槽位名，完整计划 hash 是 core，acceptance 同时绑定计划、epoch 和原实际 Publication/seal。即使改变计划的 locator 内容也不能换一个槽绕过同 epoch 的决定。

预期 receipt 只用于比较。实际成功必须来自目标真实 `decision.json` 的同步和读回；SYSTEM 中的预测字节、源-only 报告或匹配摘要均不能替代。新 API 不返回 `DiskIoBinding`、活动 writer、future locator 或可调用通用发布的 authority。

## 写入次序与中断

在目标写入前检查源和目标的已有组合、全部有界编码及新增容量。次序为：

1. SYSTEM 原操作不可覆盖保存完整 `chain-io-decision-request.json`，真实读回。
2. 原目标原 selection 目录保存完整 `io-admission-plan.json`，真实读回。
3. 再读目标实际首笔日志/全部依赖、完整来源、原计划和当前 bootstrap；原目录与 writer 锁仍持有。
4. 原目标 `DecisionStore` 选择/续作唯一 native choice，重核真实回执。
5. 目标保存 `io-admission-decision.json`；重读完整计划、实际 choice 与首笔证据后，SYSTEM 保存 `chain-io-decision.json`。
6. 再次核验原目标证据、源计划和原 bootstrap 前驱后才返回。

目标完成但 SYSTEM 尚未完成时，只能显式续作同原请求。它先核目标已有 choice 和完整证据，才补 SYSTEM 原收据；不会重新投票。source-only 查询返回 `io-decision-pending`，不隐式打开/修改目标。

SYSTEM 完成后目标计划、完成收据、真实 vote 或实际首槽日志丢失，一律拒绝，不补造缺件。slot 已出现但没有前置计划、target 已出现但没有原 SYSTEM 请求、换 choice/epoch/目标、混入别的 receipt 或原前驱改变也拒绝。未知状态保留，不能当作空槽。没有取消、换盘或自动清理接口。

SYSTEM 查询仍只读自己的已保存完整原链。完成状态为 `io-decision-preserved`，`io_decision.target_reverified_this_call:false`。只有本次实际目标决定/续作成功才在分项报告标 true。无论哪种状态，`locator_installed`、`stop_consumed`、`active_writer_admitted` 和 helper 的 `completed_readmission`/`service_started` 都为 false。

## Web 与边界

新增 `POST /api/disks/terminal-chain-io-decide`，服务端保留准确原主体、凭据 revision、操作/slot/disk/proposal 重验，只从来源已验证路径打开实际原盘。helper 专属请求/worker/Web 位于 `helper/`；复用 Engine 的通用决定槽与完整证据规则。

控制台仅在原 epoch 计划已选后显示独立确认，醒目标明不可撤销、未知结果需原请求续作、尚未接纳/不能启服务。拒绝 caller 注入 epoch、receipt、root、bootstrap、force、消费 Stop 或 writer 参数。沿用直接打开的无认证 root、0.0.0.0、无自启、Origin/CSRF/Host、停服确认、host/account owner、单任务与退出排空。成功/错误均清除旧操作表单，必须查询原状态再续作。

锁顺序不变：SYSTEM 管理 owner 与目标目录/writer owner 持续，完整 checkpoint/Publication 重核在 bootstrap store 锁外；计划重载只短暂获得 bootstrap 锁，不持其调用完整 checkpoint。决定后的原始日志没有新业务帧，原业务 State 和所有 Stop 历史不变。

新增文件和两个 native 槽文件均受既有 metadata 上限；实际新增计划/收据 bytes 的两倍加每根 1 MiB 余量，目标另留 16 KiB native 槽编码预算。完成重复不要求新文件预算。完整源链/State/实际 Publication 读取仍是有界全量验证，决定前后多次重读，不宣称低延迟或性能改善。没有掉电或并发结果。

## 后续闭包

这个真实决定为后续原计划执行提供不可改写目标证据，不是最终执行本身。仍需专属原生 generation/cold selector、目标日志的活动归属、所有必要数据库与 serving 资源、准确新 epoch/Stop 事件和活动 Current/writer 的共同安装及中断恢复。不能提前消费 Stop 再让系统没有可恢复 writer。H03 外来多级完整链接纳仍须独立完整低级链和最终权威证据，不能拿本原保留盘的决定替代。
