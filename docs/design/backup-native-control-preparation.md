# 备份原生控制材料后台准备

本片基于 main `f5f623e9`，保留 `308e894b` 的完整目标集合，是实际落盘复制与目标本地读取的生产源码，不是最终迁移切换。按当前开发指令，**未编译、未测试、未独立审查、未运行 CI**。阶段是否能在真实运行中完成尚未验收，不能以源码存在或格式化作为通过证据。

## 入口和范围

`Engine::start_backup_native_preparation(operation_id, principal)` 为既有单目标或显式 2–8 目标后台任务追加一条明确的原生证据准备阶段。必须使用原任务的准确 operation 和 actor，整个原目标集合与顺序保持不变，不推断份数、不从候选盘挑子集、不按文件分片。

原材料任务必须已完成并可再次逐目标验存。历史复制仍由原入口发起；本入口不把一次查询变成有副作用的启动，也不偷偷为旧请求启用新协议。开始时固定完整源边界，持久保存 SYSTEM `backup-preparation/native-control.json`，然后由一个受管理线程执行。正常 Engine 冷开遇到该明确任务会自动续作，不要求再次按目标逐个操作。该阶段不修改 Config、State、Current、登记集合或原备份路由。

本片只支持**完整固定 genesis 元数据成员**及现有准备入口已支持的单一级来源整池。存在 membership generation／Current registry、降级恢复决定、原 journal 残尾或未决 slot 时明确拒绝，不截掉这些原生证据继续伪称完整。原 `start_backup_preparation`／`start_backup_batch_preparation` 能力与旧 `HistoryPrepared` 标记语义保持；没有调用新增阶段的任务不被升级或重写。

## 真正保存和读取的内容

每个选定目标新增独立 `native-control/`：

- 每个原固定元数据成员的准确原 journal 字节、身份 seal 证据和内容摘要。seal 只在归档证据域保存，**不是目标根的 `disk.json`**，不会把旧设备票据换名成新盘身份。
- 从 genesis 到冻结 head 的全部 publication 引用、Candidate、Publication、完整全成员同步 Seal 和原 FINALIZE；不靠“最高 sequence”跳过中间 slot。
- 每次出版对应的完整 Snapshot／control-auth／分页对象图，以及同一 Candidate 承诺的全部 history 图。每个快照独立走现有完整 State 准入，历史投影与它交叉核对。
- 每个 Candidate 声明的 payload 路由证明。CommitProof 的内容是封闭确定表示，只重建**原 Publication 已绑定其 hash** 的证明字节，再逐项核相同版本、route、原 disk／slot、FINALIZE；不创造目标签票，不授予新的正文写权限。
- 逐序号核对原永久 operations、principals 和 events 的完整前缀，不能以新材料丢弃或替换旧收据。最终重放的 State 摘要必须等于已经独立验存的完整材料状态。

正文继续以原历史材料中的全部应保留 FULL 为准；本阶段只增加控制材料，不复制第二套正文，不通过硬链接／reflink 冒充独立副本。完整单位仍是当前受限整池，所有父子仓库被共同包含，不引入按文件、节点或子仓库拆盘。

## 身份、资源与中断

开始前重验源 pool／Config／sequence／snapshot、完整已接纳的低级祖先链和全部目标 device/inode，锁住全部目标。Production 模式保留真实设备隔离要求；Development 的同文件系统临时目录不代表多硬盘验收。每个目标分别核剩余空间，不能借另一盘剩余容量抵扣。

原生归档每个 journal 上限 16 MiB，共同保存／读工作预算 128 MiB、32 个固定成员、2048 次完整出版、32768 帧／读取步骤；重复引用和 journal 解析消耗预算，不因存储去重免除读取成本。超限整单明确阻塞。后台状态的文件／字节仍是原已经验存的正文数量，detail 标识正在处理的原生目标；不把元数据字节加成“迁移完成”。

SYSTEM 原任务先持久化，然后各目标写 `native-preparation.json`、归档对象与 `native-control-prepared.json`。所有选定目标均重新独立读取、认证全量历史与原生前缀后，才写同一个完整集合的阶段记录。中途任一目标失败保留全部旧源、原任务及已复制证据，不缩小集合、不覆盖冲突字节、不删除旧材料。部分目标已写阶段记录也不构成部分权威决定。

冷开先核源绑定，改变时在打开目标前进入 `SourceChanged`；同一任务的原生残片可以续写且必须精确匹配原归档 hash。已有历史材料损坏或缺失则阻塞，本阶段不伪造它们已恢复。进度落盘不持诊断锁；保留既有 stop／join 与 panic 关闭语义，仍不能强制中断任意操作系统 I/O。

## 只读目标检查和真实终态

`offline_check_backup_native_preparation(paths)` 只读完整原目标集合，逐盘核原任务、物理绑定、原历史／完整 State、原 journal／逐出版证明和共同阶段记录。移走源后也不能回退到源补证据；任一 peer 缺失、坏页、少 slot、错 identity 或错 marker 都返回错误，不由另一健康盘掩盖。

新增 `NativeControlPrepared` 仍然只表示本阶段的**历史材料证据**，不是服务权威、foreign chain 接纳令牌、最终路由或 cutover 成功。该结果不提供恢复导入接口，不会覆盖既有池 State，也不绕过永久退役防复活要求。

## 下一条必须闭合的生产边界

尚缺：membership／registry 代际归档拼接、三范围 SQLite 的目标安装和独立恢复、准备期间并发变更的增量收束、按完整单元的实际多下级活动拓扑、父子旧异盘合流、helper 一次性完整低级链正式接纳，以及**新旧权威共同决定与最终切换的冷重放**。正常 ZXS 仍不得注册未完成候选或展示待接入成员。

未来切换必须消费完整目标证据并发布一项专用、可恢复、精确写集的决定；不能把此报告放进现有普通 handoff 来绕过拓扑相等检查。旧收据和源数据继续保留，清理另行授权。相关任务见根 [TASKS T03/T07](../../TASKS.md)，总体约束见 [备份换盘 ADR](../adr/backup-target-replacement.md)。
