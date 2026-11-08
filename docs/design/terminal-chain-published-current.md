# 终态链首笔发布的冻结 Current 消费者

[真实首笔 Publication](terminal-chain-first-publication.md) · [完整原生输入](terminal-chain-first-native-inputs.md) · [辅助工具](../components/helper.md) · [任务清单](../../TASKS.md)

基于真实父 `3a5b3b6a`，保留已合入 Backup Current 只读消费者及其拒绝边界。本片把完整 checkpoint/registry 和实际首笔 Publication 接到 `CurrentNamespaces` 的**冻结只读消费者**，不再仅返回哈希或“预期 PUBLISHED”字符串。它不安装活动 Engine Current，不分配新 I/O epoch、不消费原 Stop、不改变启动 locator、SYSTEM/盘根活动数据库别名，也不启用 writer。终态链正式 readmission 和外来完整低级链 H03 仍未完成，缺最终权威必须拒绝。

按用户要求未运行审查、编译、测试、静态检查、浏览器验证或 CI，也没有操作实际系统、辅助程序、服务或磁盘。五项 native 和一项 HTTP 新增回归仅为未运行源码；格式化只针对明确修改的 Rust 文件。

## 原请求与真实目标证据

`TerminalManagement::open_chain_published_current` 只接受原 principal/operation/proposal 摘要和所选原目录句柄。持有 SYSTEM 管理 owner 与原目录锁/`.writer.lock`，核实际物理身份、原 seal、旧 I/O lifetime 已闭合和持久 Stop。源证据须选择当前完整终态链，目标也独立重放其完整控制归档、目标决定和相同首笔配方。

读取必须同时拥有：

- SYSTEM 原 first-input/Publication 选择与完成回执；
- 原目标 first-input 完成 marker、完整 core/Candidate/快照页、历史和 registry 对象；
- 两根相同原始 Publication 完成记录、实际 Finalize/Publication/seal 对象；
- 原目标真实三帧日志及与 seal 一致的第三帧 Receipt。

新的 `PublishedProof` 只有完整读取原始字节及实际日志后才能在进程内构造，不能反序列化、从浏览器提供或仅由源码中的 prospective result 生成。读取路径没有追加、尾修复、补回执或缺损重建；需要续写仍走原明确确认的发布入口。打开既有管理 owner 的原有持久性同步边界保留，不能把它误写为完全无文件系统同步。

## 完整命名空间与来源序列

核心从完整 checkpoint 准备精确原登记的 successor registry，保留所有 prior retirement/cancellation/permanent operation、Disabled/Detached 注册、继承 mask、隐藏路径及历史拓扑。`PreparedCurrentNamespaces::admit_terminal_chain` 必须匹配实际 Candidate registry、原业务快照/sequence、完整 checkpoint 的业务代数和实际首槽 sequence/topology。

终态管理可能有多次 sequence 增长，却没有新业务快照。新消费者不把 checkpoint sequence 错当原业务快照的 sequence，也不退回旧无链 `ResumeCore`：原业务来源边界不变，`activation_sequence` 只取已读回真实首槽。完整 State 先经 native snapshot 和精确配方字节比对，随后比较全部分支头，没有用“同一 sequence”代替完整内容证明。

`TerminalChainPublishedCurrent` 是非序列化、借用管理 owner 且持有原目标锁的只读句柄。`branch_entries` 和 `directory_entries` 通过真正 Current 投影读取，不直接暴露原始 Commit/DirectoryTree 中的隐藏条目。每次读核原目录/物理身份和来源选择；内存快照不会授予正文读取、变更状态、writer、锁转移或任意路径访问能力。丢弃句柄释放该次目标持有。

这类冻结视图与将来可服务的活动 Current 是两个不同边界。普通 PortableAuthority/Engine 安装仍拒绝此专属链 core，未删除原有终态链拒绝条件。

## 独立 Web 接线

`POST /api/disks/terminal-chain-current` 使用已有原计划与外部 writer 已停止确认，经原 host/account/layout/主凭据 revision、服务互斥和专用 worker。只在首笔证据已留存时显示检查按钮。保留 0.0.0.0、手动 root、无登录、无自启和清晰网络风险；Origin/CSRF/IP Host、单作业 owner、退出排空不变。

响应 `published_current` 来自上述真实消费者，包含精确 core/checkpoint/snapshot/Publication/seal/registry、实际 sequence、分支数与投影后可见条目统计。它不返回用户文件正文、成功接纳回执或新 epoch。明确区分：

- `frozen_namespace_admitted:true`、`target_reverified_this_call:true`：本次打开并核验原目标后得到真实冻结视图；
- `active_engine_admitted:false`、`target_writer_admitted:false`、`stop_consumed:false`；
- helper `completed_readmission:false`、`service_started:false`；
- 普通 source-only 原状态查询仍不会打开目标，其 target freshness 不会被该历史状态冒充。

目标失联、目录替换、输入/日志缺损或两根完成证据不等均拒绝。网页清除旧输出后才开始检查；错误不表示“尚未发布”，不建议清理或换编号，不自动启动服务。

## 资源和后续缺口

沿用完整 native 输入的 8,192 对象/128 MiB 集合、三倍有界读预算和 16 KiB 三帧日志限制。打开消费者需要独立两根输入/目标归档读取和完整 checkpoint 投影，成本随完整 State/历史/终态链增长；报告遍历已验证分支头计数，不扫描正文。没有性能测量或锁等待保证。

仍需专属活动 generation 的完整解析、受信冷启动选定、原 Stop 的精确消费/新 epoch、全部必要资源持有、活动 Current/serving locator/数据库切换及实际目标 writer。不能先消费 Stop 再把不可恢复状态留给下一片；本片保持原状态不变。H03 的外来多级链也不能使用这个保留原盘的冻结视图冒充一次性正式加入。
