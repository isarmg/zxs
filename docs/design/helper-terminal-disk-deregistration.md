# 辅助工具：最后元数据成员注销与终态原操作恢复

[辅助工具](../components/helper.md) · [非终态首片](helper-active-disk-deregistration.md) · [任务清单](../../TASKS.md)

本片在 `96f44f96` 开发后对齐 `25c7dcfd`，只有源码交付。按当前指示未编译、测试、审查或运行 CI，没有运行 helper、修改真实盘、服务或凭据。不是整个磁盘管理、外来接纳或完整首次安装的完成声明。

## 已接入的动作

已有 `/api/disks/deregister-preview`、`deregister-commit`、`deregister-original` 现在支持**从活动池注销最后一个元数据成员**。其他非最终成员继续使用原 ActiveExit 协议。真实预览的 `authority:terminal_management` 与 `target_metadata_members:0` 必须一致；主服务在此动作之后不再有活动存储，保留的是经过认证的 SYSTEM 管理权威，不是可写零成员池。

最后成员提交要求 `deregister-final-voter-stop-storage`，并同时勾选当前视图/内容访问丢失与终态管理的两项确认。普通 `deregister-member-preserve-bytes` 确认不能替代最后成员确认；输入、来源/CSRF 与确认均在打开系统前拒绝明显错误。预览和原查询不需要确认新提交，但用户须确认停止全部外部写者。

界面展示完整原计划、准确盘身份、来源边界、退出后的成员数和真实 ExitStatus。原请求摘要可以保存或手工恢复；不会因网络中断、未知结果或进程退出另换 operation、擦除意图或自动撤销。保留盘物理字节不保证剩余登记还能访问原文件、历史或路径，用户必须另行保留重要内容。

## 唯一 owner 与实际关闭

helper 的专用流程、账户降权、固定路径/身份、主服务 host lease、IPC、页面及确认留在 `helper/`。服务 UID worker 打开已有主 AuthStore 与 PreparedService，调用共享原生 `ExclusiveExit`，不启动管理服务线程或监听，不新建凭据/空池。当前 primary AuthReader 是持续弱能力，撤权或凭据 owner 退出后不以磁盘灾备快照继续授权；不伪造网络 session 或审计为某个已认证网络用户。

`ExclusiveExit` 在打开 Engine 前根据受保护 SYSTEM selector 选择 TerminalManagement。它只持有一个 EngineBackend，不导出 clone、流或业务 handle。控制目录仍共用原 `storage-addition-owner.lock`、严格 4 MiB 原生 Exit 记录、archive 绑定和阶段执行器。新意图仍只在 Engine 验证原来源成功后的 callback 保存，之后才 fence，过期预览不留下伪准入意图。

最终退出使用与普通服务 Arc 排空后相同的 `complete_terminal_exit`。Rust 唯一所有权不代替实际关闭证明：原核心须完成固定旧成员证据、控制归档、Stop、目标 I/O 关闭和 terminal publication 才返回 TerminalManagement。成功后丢弃原 Engine-only writer；失败保留原 handle/意图，不能把 Draining 或存下 Completed 文本当成功。

普通服务的 terminal receipt 校验与终态登记投影抽为共同函数，当前服务仍调用。没有重写核心终态哈希/发布协议，没有放宽旧 HTTP 会话要求，没有增加新的 PlannedOffline 或改动其历史记录。

## 冷态与剩余登记

成功或原终态准备已持久化后，worker 冷开直接使用 SYSTEM 内的完整终态证据，不先打开活动 Engine，也不对退役目标作 exists/stat/open。TerminalManagement::open 可以同步并完成**同一已决定原操作**，所以读取登记、状态或原计划不是逐字节只读命令。target 路径被拔除或替换不会被当作原成员重新准入。

原状态通过永久操作、actor、pool、target、绑定代数、原 archive/transition hash、结果类型、Published 与 `target_io:closed` 核对；最后 voter 还要求 `authority_kind:terminal-management`。当前原生控制投影若缺完成行，但 SYSTEM 真实终态证明完整，可以报告准确完成并通过同一 commit 恢复控制投影；仅缓存完成而无权威证据必拒绝。损坏、缺失、外国或 metadata-resume 未决材料失败保留，不回落 Engine、更不初始化空池。

`registrations` 明确返回当前 `authority`。终态中剩余非元数据登记不被隐藏或误称在线，UI 禁止发起其新注销。**本片仍没有终态内的新 registry successor 协议、metadata resume 或自动补盘/恢复存储**。其余历史/服务安全出口保留，不能通过删除 SYSTEM 或配置绕过。

## 依然缺少的外来正式接纳

外来二级及以上盘正式接纳必须同次完整纳入全部所需低级链，绑定每盘来源与 incarnation、身份映射、全部目标、材料和最终原生发布。当前备份控制归档/准备中的 native 路由材料不是活动目标 writer，也不能安全解除源围栏；没有相应最终权威 API 前，helper 继续不提供伪成功的 foreign-admit 操作。普通 Add、独立检查或导出都不能替代它，不能先放高等级盘后补低级盘。

本片也不完成最终数据/标签/数据库干净启动核验、完整首装或 systemd/SMB 激活。结果保留 `physical_bytes_erased:false`、`remaining_content_preservation_verified:false`、`service_started:false`、`complete_installation_ready:false`；`requested_terminal_management` 仅描述原计划，真正完成只看经核验的 ExitStatus/receipt。

## 成本与未运行验证

终态重开复用既有 SYSTEM 归档/证据校验和 fsync，不新增业务盘遍历或后台 worker。登记关联仍为 O(登记数 × 历史配置数)，未作性能测量；原阶段同步、固定 quorum 和真实关闭规则没有减少。helper IPC 与计划继续有 64 KiB 上限，root 原计划格式和原生记录格式未变，拒绝不能解析的材料且不迁移旧格式。

新增测试源码覆盖：最后 voter 的无 session 原生路径、真实权威/回执、全旧盘名替换后的冷查询与原重放、Stop/关闭/publication 三处中断、坏 terminal publication 不回退、主凭据 owner 消失后拒绝、过期预览不保存新意图、独立终态确认、能力列表不误报完整接纳。全部未执行。编译、浏览器、服务 UID 子进程、真实设备掉电/拔盘及组合回归均未验收。


后继增加了[终态剩余登记检查](helper-terminal-removal-inspection.md)：可读取准确来源/目标和阻断原因，但没有新的 registry successor 提交能力，不能据此把以上未完成项改为已完成。
