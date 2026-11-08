# 完整终态链的首笔原生输入

[原目标决定](terminal-chain-target-decision.md) · [整链提案](terminal-chain-readmission-proposal.md) · [辅助工具](../components/helper.md) · [任务清单](../../TASKS.md)

基于真实父 `93cebc07`。这一步将已有真实原目标决定收束成独立严格 core，并在 SYSTEM 和该原目标分别安装完整、确定的首笔原生输入。**尚未执行 ACCEPT、FINALIZE、Publication；不是活动 writer、正式 readmission 或 H03 外来完整链接纳。** 普通 `PortableAuthority` 仍不接受此 core，不能借旧的无移除链 ResumeCore 绕过门禁。

本片未审查、编译、测试、浏览器验证、静态检查或执行／查询 CI。只格式化显式改动的 Rust 文件。没有运行辅助工具、修改真实磁盘／系统／服务或重置密码。测试文件中的临时目录操作只是未运行源码。

## 从实际决定形成唯一输入

前置仍是准确原 Disabled metadata 登记、完整已完成终态链、原 Stop／物理目录／seal／旧 I/O lifetime 闭合，以及此前保存的 `TerminalChainReadmission` 实际决定。没有来源证书时拒绝，此入口不会自动生成提案、选择新目标或替代投票。

新的 `terminal-chain-readmission-generation` 严格 core 携带原选择和实际目标证书。两者绑定同一 actor、operation、原物理身份、完整 checkpoint 与旧 seal。SYSTEM 用自身认证归档重放，目标持原目录及 `.writer.lock`，只读回实际 vote、完整目标归档并独立重放来源，再分别确定相同首笔输入。不能只看同一摘要或自洽的 SQLite／JSON 就接纳。

首笔配方：

1. 从完整 checkpoint 的 State 出发，保留此前所有永久 operations、principals、events、历史、绑定和管理回执。完整 registry 与继承 namespace／detach mask 均经过原生重放。
2. 原 operation 不得已存在；仅为本次预期 readmission 添加一笔永久结果，sequence 和 topology_generation 各加一，business_generation 不变。
3. 用现有 native snapshot／分页 State、history manifests、registry graph 和 `Candidate::new_with_registry` 编码。Candidate 指向 core 摘要与准确拟议 registry，元数据成员只有所选原盘；没有新正文或版本复制义务。
4. 原终态管理链没有新的业务 Publication，因此 Candidate 的 previous_snapshot 保留原真实业务发布快照。所有后续管理步骤由完整 core 的 checkpoint 证明，不能假称它们是新业务出版。
5. core、完整 State、registry、历史页和 Candidate 构成同一有界文件集合。首笔快照里的 `Published` 是**预期结果**，没有真实 journal Publication，不可向外返回为成功操作或放入活动 State。当前 helper 只显示摘要、数量和阶段，不返回该预期 OperationResult。

## 原选择、双根安装与冷恢复

`TerminalManagement::prepare_chain_readmission_activation` 只接此前保留请求的原 actor、operation、proposal digest 与调用方明确持有的原目录。`chain-first-selected.json` 在任何首笔文件安装之前不可覆盖持久化，记录准确 core／Candidate／snapshot／checkpoint 摘要和固定预算。这一操作仍没有取消或换盘接口。

原 SYSTEM 的 terminal operation 下 `chain-first/` 和原目标 `metadata/membership-pending/<selection>/first-candidate/` 各存完整隔离集合。目录与内容不会写入活动 generation、索引、Current 或 serving 路径，也不覆盖旧 `disk.json`、Stop、正文或已有原生对象。

目标先安装并独立读回，再保存自己的 `prepared.json`；随后 SYSTEM 安装同一配方并严格解码完整 snapshot／State，实际读回全部 native 对象与 history/registry 字节。最后再次读取实际目标决定和输入后，SYSTEM 写 `chain-first-prepared.json`。源／目标标记也按精确内容读回；它们只记录**准备过的材料**，不是共同发布证书。

- `activation-selected`：来源已保留首笔配方，但未有最终 SYSTEM 准备回执。错误可能已留下部分目标或来源文件，不表示没有写入。显式原请求重试只能补相同缺失字节，异内容／目录替换保留并拒绝。
- `first-candidate-prepared`：此前两个根完成独立输入读回。显式准备成功那次，source/target material reverified 字段均为 true。
- 后续 source-only 状态读取只验证 SYSTEM 的完整来源和已完成材料，目标相关 reverified 字段为 false，不打开、探测或声明失联目标健康。
- 一旦目标准备标记存在，其对象缺失／损坏不会从 SYSTEM 自动补造；SYSTEM 最终回执存在时，目标准备标记丢失也拒绝重建。已完成 SYSTEM 材料缺损会阻止冷恢复，保留原字节供明确救援。
- 已开始首笔准备后，旧“同步目标决定”入口只能核对已有实际决定，不再借重新同步安装来修补丢失的旧选择／归档／证书。

`active_writer_admitted`、`metadata_resumption_supported`、`first_publication_recorded`、helper `completed_readmission` 和 `service_started` 全部保持 false。持有文件锁只是这次准备的互斥所有权，不是新活动 I/O epoch。

## 独立 Web 接线

新增 `POST /api/disks/terminal-chain-first-prepare`。浏览器先查询原保留状态，只有已有实际目标证书才显示首笔输入按钮。再次明确勾选／弹窗确认，仅准备不发布；严格 JSON 不接受 caller State、core、Candidate、root、force 或外来链。

沿用主凭据 revision、service UID worker、原 host/account/layout 对照、主服务停止／外部写者确认、Origin／CSRF／IP Host、单工作槽与关闭排空。首次初始化／无认证 root 风险提示不改变，不增加登录、自启、服务启动或系统配置副作用。界面输入变更或任务提交会撤销旧计划，错误显示原 operation/digest 并要求保留证据。

## 预算与尚未闭合部分

每个完整集合最多 8,192 个原生对象、128 MiB 编码字节，单对象仍为 64 MiB。每根独立容量预检包含完整集合、原子临时文件、标记及目录分配余量。严格 snapshot 解码和逐对象比较累计读取最多三倍集合／对象预算；完整原控制归档重放仍按其既有预算。未运行最坏规模或延迟测量，完整 State/graph 会在两根配方中保留，不能声称低内存常数。

新增五项真实临时根／全 State／冷续作／完成后损坏／原来源替换测试源码，扩展一项 HTTP 边界测试并新增严格输入源码，均未运行。

还需要实际首笔 ACCEPT／FINALIZE／Publication、从完整链 core 到活动代际的专用原生验证、Current／serving locator、目标新 I/O epoch、原 Stop 精确消费与冷态激活完成。整链正式 readmission 及 H03 外来盘一次完整低级链仍拒绝；没有把准备阶段或预期 PUBLISHED 状态当成真实完成。

## 真实首笔发布后继

后继[真实首笔 Publication](terminal-chain-first-publication.md)新增独立明确确认入口，以真实受限 native 三帧日志及原生 seal 固定原请求，不把预期 Published 或准备标记当提交。本页的准备入口不会自动执行发布；发布开始后禁止重装首笔输入。活动 Current/writer、新 epoch/Stop 消费和 H03 正式接纳仍未实现。
