# 完整终态链的保留元数据恢复提案

[完整链检查点](terminal-retirement-portable-checkpoint.md) · [既有保留目录恢复](helper-terminal-retained-readmission.md) · [任务清单](../../TASKS.md)

基于真实主线 `350df285` 开发，并对齐真实父 `6b8d1859`。本片将完整终态移除链的实际重放结果接到保留 metadata registration 的精确 registry successor 与继承命名空间准备，并提供辅助工具独立 Web 下载。它不是 ResumeCore，不写新目标决定，不开放活动存储；移除链后的正式 readmission 与外来完整低级链 H03 仍未完成。

本片未审查、编译、测试或运行/查询 CI，未操作实际系统、磁盘或服务。新增测试源码不代表场景通过。

## 原生来源到提案

`TerminalManagement::prepare_chain_readmission_proposal` 在已持有的原 SYSTEM owner 下捕获**当前完整且已完成**的终态移除链。未完成 child、来源缺失、图页损坏、摘要/Stop/永久回执不符或超限仍失败。它复用完整检查点验证所得的私有类型，不能用外部布尔值、最终 registry 或只含摘要的报告替代。

提案单独使用 `terminal-chain-retained-readmission-proposal` 格式/哈希域，包含：

- 全部 checkpoint 字节与准确摘要，包括每笔原操作、前驱链接、Stop、registry/mask、永久结果和以前已取消的 resume 结果。
- 本次拟用 operation/principal、仍登记为 Disabled 的**原历史 metadata** 身份，原 Stop operation/transition/seal format 和之前 admission epoch。
- 原历史配置中的精确绑定，只有被选盘变为 metadata member；不接受外来根、换盘、绑定重用或被 Detached 身份复活。
- 完整内容寻址 registry successor 图，继承全部历史定义、此前 DetachedBinding/occurrence mask 与未选 Disabled 状态，仅使所选原 metadata 登记变为 Enabled voter 的**拟议图**。

若所选盘有低级祖先，所有必要祖先必须仍在同一 registry 中有 Enabled 登记且级别连续。历史 Config 的 Disabled/Detached/缺失祖先不满足。这个检查没有打开任何祖先目录，也**不证明**真实正文齐全、物理身份、健康、完整链接纳或未来可安全启用 writer。外来盘不能走这个保留身份分支。

提案的 registry source 继续绑定原业务冻结 snapshot/sequence，只将来源 authority 绑定到准确管理 checkpoint；checkpoint 单独保留管理期后继 sequence/topology/permanent operations。验证不能把管理序列误当原业务快照边界。

原生验证器还将经过所有移除 mask 的 `PreparedCurrentNamespaces` 延续到拟议 registry，不能从未过滤 HEAD 重新生成而复活已隐藏路径、被移除副本或历史身份。该值仍然只是 prepared view，不构成活动 Current 或读写许可。

## 不能执行的边界

`TerminalChainReadmissionProposal` 只有字节和准备报告，没有 `activate`、投票、提交或 Engine 转换方法。现有 `TerminalRegistry::require_resumable`、`TerminalManagement::require_no_terminal_retirement` 和早期 `require_no_evidence` 仍在原执行路径生效。旧的无移除链 retained readmission 执行路径没有被这个提案放宽。

报告明确：`target_directory_verified:false`、`target_decision_recorded:false`、`active_namespace_admitted:false`、`metadata_resumption_supported:false`。新 operation 只是提案字段，没有被保留或写入原永久结果。用户可继续下一笔合法终态移除；因此提案只代表捕获当时的来源，不是当前最新/可执行承诺。

`verify_chain_readmission_proposal(bytes, expected_digest)` 将有界输入相对已可信认证的原完整控制归档重放，并核独立保存的可信摘要、完整图和目标。它只核这个历史提案，不断言现在仍是最新。与提案放在同一不可信文件中的摘要不能自我认证。提案不内嵌原完整控制归档、主凭据或业务正文，保留原完整归档仍是必需条件。

## 独立 Web 接线

新 `POST /api/disks/terminal-chain-readmission-proposal` 使用准确服务账户、原盘 ID/slot、拟用 operation 和已停止外部写者确认。principal 来自原主认证 store，不接受浏览器自行指定。root 编排器与服务 UID 的 `internal-chain-readmission-worker` 继续使用固定布局、host 租约、原 SYSTEM writer 和实际主认证重新核验。HTTP 保留 Origin/CSRF、单工作槽、关闭状态和严格输入；没有上传/force/自动接纳选项。

页面仅下载提案并显示明确未完成字段，不提供执行按钮。选择、账户、写者确认变化或相关操作开始时隐藏旧报告。工具依旧 root 手动运行、0.0.0.0 无登录，使用可信隔离网络/防火墙、用完退出的风险提示保持。不增加 helper 自启或修改网络。

这里不写新 native request，不持久化提案到 SYSTEM，不触碰目标目录，也不启动服务。但打开原 SYSTEM 仍沿已有协议恢复/同步先前已选操作，因此整个 HTTP 请求不能称为完全只读。

## 同目录证据归属与资源预算

resume attempt inventory 现在只把明确由 terminal retirement 模块拥有的固定文件名交回该模块，允许真实已完成链进入后续完整验证。它不使用前缀通配、不接受任意 `retirement-*.json`，也不把这些文件作为 resume 批准。terminal owner 仍验证原完整链及精确子目录，原执行早期门禁仍拒绝存在 retirement 证据。

完整 checkpoint 和提案各受单个 native 元数据对象 64 MiB 上限约束，超限不裁剪。提案 worker 响应上限是 128 MiB 加 64 KiB，输入仍 64 KiB，其他 worker 不变。原生捕获将同一次验证得到的 State/registry/prepared namespace 交给提案构造，不再通过下载 JSON 解码重放一遍；下载检查点时不保留投影 State。复杂度仍随完整链与冻结历史增长，提案验证会克隆一份 prepared namespace 用于绑定检查；未测量大链内存、延迟、网络中断或吞吐。

后续必须将这份完整来源纳入真正的 target decision/ResumeCore，独立核原物理目录与 stopped lifetime、真实 writer/新 epoch、首笔 Publication、Current/locator、永久结果和冷恢复选择器，才能放开移除链后的 readmission。H03 还需要外来来源映射及完整低级链最终 writer，不得用本片提案、Backup 隔离材料或普通 Add 替代。
