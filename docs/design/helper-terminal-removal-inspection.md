# 辅助工具：终态剩余登记的精确移除检查

[终态退出](helper-terminal-disk-deregistration.md) · [任务清单](../../TASKS.md)

基于 `f715168d` 的安全前置源码。最后 voter 退出后，现有 TerminalRegistry 只认证那一次退出，不提供后续 registry successor、逐目标 Stop、永久结果和恢复后的防复活链。因此本片**不声称完成剩余成员注销**，也不改变登记、删除历史、调用普通 Add 或制造成功回执。

独立 Web 可选择终态内仍登记的成员，调用同源 POST `/api/disks/terminal-removal-inspect`。输入只有服务账户、准确 slot/disk ID 和外部写者停止确认，不接受 operation_id、force、替换路径或“以后补盘”。主服务 host lease、服务 UID、已有主凭据与严格 SYSTEM 终态选择沿用真实 owner。当前为活动池、身份不符、未决 metadata resume、缺失或损坏权威均失败，不推断空池。

报告绑定真实 terminal authority hash/原终态 operation、registry root/generation、当前 sequence/topology generation、完整目标绑定与记录路径，并列出登记数和直接依赖槽。后者是**历史拓扑的直接登记关系**，不是正文存在、完整历史、健康、全体后代闭包或剩余访问证明。不会打开、stat 或枚举目标盘。

`inspection_digest` 只用于比较这份完整选择，不是接纳或提交令牌，没有可接受此摘要的 mutation API。报告始终明确：

- `executable:false`；
- `blocker:terminal_registry_successor_unavailable`；
- `operation_created:false`；
- `physical_bytes_erased:false`、`target_io_probed:false`。

页面不展示提交按钮、不填写原操作摘要，也不在 SYSTEM 或 root 保存新的注销意图。用户可以下载明确命名的检查报告。旧注销的原摘要查询/续作仍独立保留，不把此报告冒充原计划。

本片的“检查”只是不发起**新**移除；打开已有 TerminalManagement 仍可能同步并完成先前已决定的原终态操作，报告 `existing_original_recovery_may_run:true`。缺少完整前置时不能靠检查结果拔盘、清材料、重初始化或解除围栏。

后续写者须先建立可认证的终态 registry 后继、真实 I/O stop/关闭证明、SYSTEM 决定及永久收据，并让冷恢复、metadata resume、当前登记和历史操作共同识别该后继。只从列表删目标或保存 helper 意图不满足这些义务。外来二级以上盘还需一次完整低级链和正式目标 writer；现有 Backup 准备/隔离 native 材料仍不能用于 H03 成功。

专用查询、诊断、HTTP 和页面在 `helper/`；服务组件只提供持有原生权威的只读 `TerminalBoundary` 元数据投影，不暴露 Engine handle 或新增 write capability。关联成本 O(登记数 × 配置数)，复用原 bounded IPC，无性能测量。

新增测试源码覆盖严格输入、无停服确认先拒绝、目标代数/活动 voter 拒绝、报告不能作为执行计划，以及真实 terminal 冷态路径替换后的边界查询。按当前指示未审查、编译、测试或执行 CI，无真实系统操作。


后继原生[决定持久化](terminal-retirement-decision-storage.md)可产生未完成选择，检查会在 `selection.source.pending_retirement` 返回该真实记录，blocker 改为 `original_terminal_retirement_pending`；检查本身仍不创建选择、不执行注销。最终后继/关闭/回执未实现前，完整移除仍拒绝。
