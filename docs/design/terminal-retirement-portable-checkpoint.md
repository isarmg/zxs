# 终态逐笔移除的完整可携带检查点

[终态链](terminal-retirement-chain.md) · [命名空间闭包](terminal-retirement-namespace-closure.md) · [保留目录 readmission](helper-terminal-retained-readmission.md) · [任务清单](../../TASKS.md)

基于真实主线 `5801c703` 开发，并对齐真实父 `4666acb1`。本片实际接通当前已完成终态链的原生捕获、相对原冻结来源的严格解码，以及辅助工具 Web 下载。它是后续整链 metadata resume 所需的完整管理证据，不是新的 ResumeCore、候选加入意图、可激活 registry 或正式接纳。当前未审查、编译、测试、浏览器验证或运行/查询 CI，未运行任何真实辅助工具、服务或磁盘操作。

## 为什么不能只删除恢复门禁

最初 TerminalAuthority 与原完整控制归档证明最后 voter 的 handoff；后续每笔终态移除还增加独立选择、registry successor、完整 occurrence mask、Stop 和永久回执。原 ResumeCore 并不携带这条管理链。只传最终 registry、状态位或最后一笔操作会丢失早先的 Detached、不可见路径、历史身份及管理期永久结果。因此现有 `require_no_terminal_retirement` 与 `TerminalRegistry::require_resumable` 保持，不允许把本片检查点直接用于活动存储。

外来盘 H03 同样没有因此完成。一次性完整低级链还须目标真实 writer/完整最终发布和来源身份映射；现有 Backup effective Config、公共 Publication 或本检查点均不能代替目标实际准入、serving locator 与安全源围栏解除。

## 从真实已完成链捕获

`TerminalManagement::export_retirement_archive` 要求持有原 SYSTEM writer、完整已完成的每笔 native retirement，且没有未决 child/link 或 metadata resume。它按精确原 `retirement-next.json` 遍历，不扫描任意目录推断权威，也不在未决时退回已完成前缀。

单一 `terminal-retirement-chain-checkpoint` 格式保留：

- 所属 terminal authority/control-archive 的摘要和真实最终 registry、sequence、topology generation、全 State 派生摘要；不包含原业务正文或完整控制归档。
- 原来已经完成的 metadata-resume 取消证书及其准确顺序，避免在管理 State 中丢掉永久取消操作。
- 每笔原 intent/selection、确切前驱 completion/full-intent link、完整 registry/mask 图、原 Stop 材料及永久 receipt。
- Fresh Stop 还携带原 PreparedStop 的确切 predecessor locator 记录，逐字段证明它只产生原目标/操作/epoch 的 Stop；原 Disabled Stop 则必须由最初权威/前驱代际验证其真实历史停止身份。

源捕获先使用现有本机恢复路径核完整 authority、不可变材料、真实持久 locator 与进程 I/O 排除，再将原字节形成检查点。独立内容解码不打开任何数据根、不改 locator、不签发 live D3，也不把反序列化的 Stop 当成可发布类型。Stop 快照只证明被捕获记录的语义，跨进程/主机的真实排除必须另行建立。

## 相对原可信来源的严格消费

`verify_retirement_archive(bytes, expected_digest)` 要求当前 owner 已从原完整归档认证了相同冻结来源，并要求调用者另外持有可信检查点摘要。来源及检查点只存在于同一个不可信文件时不能自我认证。返回值也**不声明该检查点现在仍是最新**；最新捕获要求的完整本机链与历史检查点核验是两个不同语义。

解码先核格式/上限/域分离摘要，再从原 terminal management State 与取消历史开始，逐笔共用实时路径的来源验证、完整 registry successor、mask/namespace 投影和规定永久结果构造。每一原 operation/principal、序列及前驱必须准确；目标不能已 Detached，不能只重哈希遗漏页、改 receipt、换 Stop/epoch、重排或截断链后声称相同 checkpoint。当前 prepared namespace 只用于这一轮重放，仍没有 active namespace 转换方法。

完成后保留的历史、曾用身份、隐藏路径和不可用版本来自原冻结来源逐笔过滤；不会仅从未过滤 HEAD 或最新行重建。完整 State 只在验证器中投影，下载物仅含 State 摘要和控制证据，不输出完整 State/正文或主管理凭据。

## 辅助工具的实际入口

独立 Web 新增 `POST /api/disks/terminal-chain-export`，输入仍仅为准确服务账户和已停止外部写者的明确确认。无文件上传、无 force、无新操作或加入参数。helper root 编排与同一二进制的服务 UID `internal-terminal-chain-worker` 使用固定布局、host 租约、主凭据和原生独占 owner。下载期间不启动 ZXS；打开既有 SYSTEM 仍可能重复原有恢复与同步，因此不能把整条 HTTP 请求称为完全只读。

页面展示 checkpoint 摘要与计数后下载完整 JSON，要求另存可信摘要并同时保留原完整控制归档。无原链、任一证明缺损或操作未决会明确失败，不导出省略项的“成功”文件。所有入口保留 IP Host、Origin/CSRF、单工作槽和子进程排空。控制台仍是手动 root/0.0.0.0、无需登录的高风险维护入口，不添加 helper 自启或修改网络配置。

返回的 `metadata_resumption_supported`、`active_namespace_admitted`、`target_writer_admitted`、`live_io_exclusion_transferred`、`service_started` 全部为 false。`contains_full_source_control_archive:false` 提醒该文件不能单独重建整个系统，也不是 H03 或 readmission 的执行许可证。

## 上限与尚缺部分

整个 native 检查点受现有单元数据对象 64 MiB 上限约束，逐帧累计编码预算并在超过时拒绝，不裁剪成前缀。服务工作进程输入继续为 64 KiB；只有该下载 worker 的响应放宽到 128 MiB 加 64 KiB，以容纳一次 JSON 字符串转义，其余 worker 仍为原上限。HTTP 采用有界整对象下载，尚未做流式大链、慢连接或整体内存压力验收。超过 native 对象上限的链不具本片导出保证，原文件仍保留。

原生读取与完整 mask 验证成本随完整链及冻结历史增长；当前仅保留一份正在投影的 State/namespace 加有界 frame 材料，不给每个历史 frame 再克隆完整 State。不新增 fsync、写者或 source ledger；打开过程仍沿原协议已有同步预算。

后续仍需将完整 checkpoint 纳入新的 resume proposal/target decision、真正 activity Publication、registry/current namespace/serving locator、新 epoch 与冷恢复选择器，同时证明被注销目录不会复活。该接线尚未实现，终态移除链后的 readmission 继续拒绝；新提供的下载不是恢复成功。新增 4 项真实链/变异/冷态回归及 1 项 HTTP 边界测试源码均未执行，不能作为通过证据。

后继：[完整终态链恢复提案](terminal-chain-readmission-proposal.md)现在消费同一次检查点重放所得的私有 State/registry/namespace，生成精确原 Disabled metadata 的拟议图与 Web 下载。它尚不是 target decision/ResumeCore 或正式 readmission；以上活动准入限制保持。
