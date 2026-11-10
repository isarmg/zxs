# 外来链共同决定与冷态联合 State

本地后继基于 `2769cedd`（在 `bcf065da` 上仅新增独立 Node 夹具修复），保留 `91b35` 的完整 typed 图转换；暂停的 `73ea6364` 多阶段 Web/worker 图准备没有带入。本片直接接共同决定和冷态重算，不增加按钮或另一份人工下载材料。

**尚未形成可正式接入的可用流程。** 当前实现了准确联合 State 的协议接线，但实际外来盘 pending enrollment、FULL/三范围安装和最终活动切换仍未完成。普通 Add 的空根检查没有放宽，原外来 `disk.json` 不会因此被接纳或覆盖；通用 activation、Config、I/O、registry 和后续 writer 的门保持拒绝。测试源码不构成实现已通过验证的证据。

## 一次请求的协议身份

`Engine::begin_foreign_membership_handoff` 从当前池准入基线和持有的 `ForeignImportPreview` 捕获配方，不接受调用者提交的裸 State、可编辑映射表或反序列化配方。当前基线包括完整 State、序号与准确 Config；源侧重读固定 genesis 的完整原 native 发布前缀，保存完整原 State、准确原 Config 和命名 report。请求、完整低级链及登记提案摘要均纳入原基线。

配方是 `TransitionIntent.foreign` 的私有类型。Joint proposal 原有 transition hash 绑定它的全部字节；新增 `fenced-foreign-joint-state-decision` / `fenced-foreign-device-state-decision` 用途与配方存在性必须一致。原空 Add 不能承载配方，Foreign 也不能借缺少配方的普通证书变成有效权威。此字段缺省仅表示当前语义中的非 Foreign 操作，不尝试兼容、迁移或改写历史格式。

投票生成前，从 pending 控制归档重取当前池原 State 并重算完整目标。Cold first-candidate 验证用同一实现，不接纳“报告相同”或用户提供的目标哈希：

1. 当前完整基线和 Config 必须一致；重建当前保留对象、操作、演员、plan 和目标盘的占用域。
2. 从准确原 State 重新派生全部对象名，完整 report 必须等于意图内原映射。
3. 逐字段转换所有历史图引用。原池完整 operations/principals/plans/events/control_auth 留在私有来源域归档，不成为当前认证或幂等收据。
4. 记录内 operation 引用进入确定、无碰撞的私有目标审计名字空间；作者使用已绑定 `foreignactor_` 映射。目标 State 不补造这些操作的成功回执，原字段仍可由来源归档恢复。
5. 全部历史序号以原 fence 为偏移，保持顺序；首发布为 `fence + 原 State.sequence + 1`，溢出拒绝。现池原事件/收据不改，正式首结果另由准确新操作生成。Abort 仍用其本来的 `fence + 1`，不导入外来图。
6. 新链按真实 1..N 层完整映射，所有引入 AVAILABLE 版本的候选 FULL 义务覆盖全链；slot/绑定代数/拓扑由该准确新链派生，源路径与目录 Git 工作区、index、固定 HEAD 保留。目标 SMB 绑定不自动启用。
7. 整体 State 走既有逻辑闭包验证，冷态重新编码 snapshot/全部 history roots，并逐项比较全部新增 AVAILABLE `VersionEvidence`。只认这一个准确首槽；后续普通写入仍需未来的完整接入证明。

独立 `foreign-membership-joint-state` authority schema 与 ordinary Add/Backup 分离。解析、哈希及联合 quorum 证书本身不是物理安装成功的证明，不能绕过 `ValidatedConfig::with_generation`、namespace、registry 或 I/O 活动门。没有加入真假成功回退，未改 Backup 特定拒绝或普通 Add 的准备语义。

## 当前明确支持边界

本轮配方仅允许所有保留历史属于**同一个原主盘名字空间**，且该高级源覆盖每个 AVAILABLE FULL；不支持的其他主盘、外部旁支路由、超过新链层数的原保护份数、缺失 FULL、活 D、已撤销 D/undo 历史、带保留审计的 copy/move 或退役收据保留并拒绝，绝不筛掉历史后宣布完成。原生来源仍受固定 genesis、至多 32 原盘和 2,048 发布的既有边界约束。

拒绝已撤销 D/undo 是暂时安全门：虽然当前 D 表已空，永久生命周期仍由 receipts 驱动，不能仅保存原归档而把目标查询缺少的历史称为完整；带保留审计的 copy/move 同样不偷渡。完整 D/undo 历史的目标 ledger 转换与查询接入仍是待实现需求，不是最终范围削减。

这是最小支持面的完整图转换，不是整个产品需求的豁免。多原主盘的实际目录/路径合并，以及活 D/永久退役收据的跨池权威，仍须明确实现后才能放行。

## 形成可用路径还缺什么

1. **原根接入**：用准确原 seal/设备身份和冻结来源证据扩展 pending enrollment，保存原来源布局，不能把原外来根当空根或先覆盖 `disk.json`。
2. **数据安装**：按联合 State 在每一真实低级目标及高级根安装独立 FULL、SYSTEM/盘根 current+history/FILE/目录库，逐项读回准确身份、摘要与闭包；共同决定前须拥有可恢复、不可混批的安装依据。
3. **发布与切换**：把上述真实完成证明绑定同一个 Foreign generation 首 Publication，最后安装 Config、locator、namespace、普通 writer；任何缺件维持拒绝。
4. **冷态续作**：重启从原操作、原来源归档和准确共同决定恢复同一目标，不新造映射、不改票数，不把只有文件名或完成 marker 当已安装；最终完成后才开放后续 ordinary publication。

当前 Engine 入口仍会在原外来根遇到既有 pending 空根门禁，因此没有承诺操作员可用它完成以上步骤。下一工作直接接第 1/2 项，不再添加无下游 UI 阶段。

## 预算与实际验证

配方持久字节最多 32 MiB，命名继续限 100,000 项/16 MiB；共同决定对象仍受原 64 MiB reader 限额，嵌套信封也不能跳过预算。完整池接入和 cold replay 需完整来源/基线遍历、图克隆与编码，复杂度随保留记录增长；没有热路径吞吐或峰值内存结论，没有减少 fsync。

新增六项临时原生夹具配方回归和一项 Add/Foreign schema 隔离回归，覆盖重读同一 State、当前池/映射/Config 篡改、序号溢出、生命周期拒绝、原 seal 不动和非 Foreign 证书拒绝；**只有测试源码，未执行或编译**。仅运行明确文件的 rustfmt 和补丁空白检查，不 Cargo、不 CI、不真实系统操作。

先前最后一次生产 check 未通过，所谓共享旧缓存原因仍未确认。此父基线组合和本地后继均未生产检查；独立 #927 Node 夹具修复的 Node/Python 通过不覆盖本片。
