# 已发布终态链的精确新 epoch 与 Stop 后继计划

[冻结 Current](terminal-chain-published-current.md) · [真实首笔 Publication](terminal-chain-first-publication.md) · [辅助工具](../components/helper.md) · [任务清单](../../TASKS.md)

基于真实父 `b255c271`，保留 Backup generation-local 数据库别名及其拒绝边界。这一片把已发布完整终态链接到 native bootstrap 的精确后继记录构造：实际目标证据与冻结 Current 准入后，选择唯一新 I/O epoch，核对原 Stop/前次 epoch/原物理目录，并保存原请求的 SYSTEM 计划。**计划不是生效记录。本片不会更新 bootstrap record/head、消费 Stop、创建活动 I/O binding、安装 Engine Current/writer、替换活动数据库或启动服务。正式终态 readmission 与 H03 仍不放行。**

用户要求下，没有执行审查、编译、测试、静态检查、CI 或浏览器验收，没有操作实际系统/服务/磁盘。五项新增 native 回归、一项 HTTP 严格输入以及已有路由边界扩展均只是未运行源码。只格式化明确修改的 Rust 文件。

## 真实前置与 epoch 身份

`TerminalManagement::prepare_chain_activation_plan` 首先调用真正的 `open_chain_published_current`。它需要原请求 actor/operation/proposal/target，完整当前 checkpoint、准确全 State/registry/mask、两根完整原生输入/完成证据、原目标三帧日志及实际 seal。只保存 Candidate、只持源哈希或缺损目标都不能进入。

独立语义域 `terminal-chain-io-admission` 绑定：

- 原 selection/core/checkpoint/Candidate；
- 实际 Publication 与 seal；
- 原操作和主体；
- 精确原登记、物理目录、原 seal、完整 checkpoint registry；
- 原 stopped operation/transition 及前次 admission epoch；
- 仅原保留 metadata 角色。

该规范化对象的哈希是 `planned_io_epoch`，不能由浏览器自填，也不能重用前次 epoch。它不借用无终态链的 ResumeCore，不改写已发布首槽对象/哈希或原操作回执。后续真正消费 Stop 时必须继续这个原计划，不能另选一个更方便的 epoch。

## native bootstrap 后继，不是第二份活跃状态

专属 `prepare_chain_root_extension` 只构造记录。它重载当前严格 locator，确认原最终终态 Stop，沿用原生 `Record::resume_terminal_readmission` 对所选原盘逐项验证：

- pool/slot/binding、原 metadata 身份、device/inode/path；
- 原 stopped operation/transition 恰好匹配当前前次 admission epoch；
- 原目标的实际持有目录 observation 与独立来源身份一致；
- 不能引入任何根、打开其他 stopped 根或更改历史 root/Stop/admission；
- 后继仅增加一代并追加所选盘的一条 admission 事件，保留完整原历史。

随后保存 `chain-activation-plan.json`，内含精确前驱 head 和完整未来 record，但**不将未来 record 写入 bootstrap ledger**。只有 SYSTEM 这个原请求文件增加。原 Stop、当前 head/record、原盘 seal/日志、运行态 State 均保持。native store 的新 API 只有 prepare/inspect，没有 apply、返回 future locator 或发放 probe/I/O 能力的接口。通用 Add 或旧 Resume 不能凭序列化 plan 消费这个新 epoch。

计划保存前检查有界元数据和 SYSTEM 容量，原子不可覆盖保存并真实读回。同一请求可重复同步相同 bytes；不能换 actor、目标、首笔证据或前驱。已经有计划的重复读取不因为预留新文件空间而被额外拒绝。计划缺损或内容不等保留并拒绝，不重算覆盖。

## 冷态与锁

SYSTEM 保留原终态管理 owner，原目标真实目录/writer 锁贯穿准备；bootstrap store 锁覆盖准确当前前驱核对、计划保存和读回，之后才释放。完整 checkpoint 重放自己需要 bootstrap 锁，因此放在 store owner 前后，持锁内部只再核 SYSTEM writer 与原目标身份，避免递归获取同一个锁。

source-only 状态查询重新验证完整来源首槽证据、计划身份、原生历史链及未来记录的精确构造，但不打开目标。只接受仍未消费的原前驱 head；不会把“后来某个 head 看起来可用”视作执行成功或选举孤立记录。查询返回 `activation-plan-selected` 和 `target_reverified_this_call:false`。

显式原请求准备/重复核对会重新读取目标实际证据，成功返回 target fresh=true。无论哪种情况，`locator_installed`、`stop_consumed`、`active_writer_admitted`、helper `completed_readmission` 与 `service_started` 都为 false。失联目标可查询历史计划，但不能重新完成该次实盘核验。

## Web 与剩余生产闭包

新增 `POST /api/disks/terminal-chain-activation-plan` 与独立 checkbox/确认弹窗。只接受原计划、停止外部 writer 确认及固定意图字符串，不接受调用者提供 epoch/bootstrap/head/Stop/force。保留手动 root、0.0.0.0、无登录、无自启，原 CSRF/Origin/Host、凭据 revision、服务互斥、单任务及退出排空均继续生效。

界面将“计划中的未来值”和当前活动状态分开；只对原已发布请求显示准备/复核入口。结果或错误后清除旧执行表单，重新查询原计划。错误提示保留原操作并明确可能已保存，不能据此清理或启动服务。

仍需专属活动 generation 与受信冷启动 selector、全部必要资源/实际 locator/活动数据库、完整 Current/写者的原子接线和恢复。不能为了完成一个按钮先消费 Stop 再把系统留在无法安全启动或恢复的状态。本片保留严格拒绝；外来多级 H03 仍须其独立完整低级链及最终权威证明，不得用本原盘计划替代。

成本沿用完整 Current/源链验证，加一轮严格 bootstrap 历史链读取和一个有界 SYSTEM 原子文件同步。预算为元数据对象上限、实际计划 bytes 的两倍加 1 MiB 新写余量；无性能/锁等待或掉电测试结果。
