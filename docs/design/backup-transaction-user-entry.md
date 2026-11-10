# 管理员原操作备份事务入口

状态：**本地组合候选，未编译或验收**。本入口以 `2769cedd` 为源码父基线，依赖同时开发的 Engine 原事务协调、真实最终活动准入和已决定冷恢复，不可将入口单独发布后声称可用。完整产品约束仍由[整体备份迁移](../adr/backup-target-replacement.md)规定。

## 范围与非等价操作

新增主服务管理员入口，不复用 `backup.configure`、文件递归备份、首次安装或独立辅助工具。当前核心仅支持固定 genesis、单一级来源、整个池应保留集合和 1–8 个新空目标；每个明确目标持有完整副本。它不是任意已活动 generation 再嵌套备份、改变已有来源边或外来盘接纳的授权。

界面分别确认创建完整材料、原池全部停写、不可逆共同登记／放置决定和最终激活。非决定性准备仍由核心后台任务执行；本切片提供明确的逐次准备／恢复入口，不声称一般“修改规则后全自动迁移”已实现。

## 唯一核心与受约束目标

HTTP 只适配 `start_backup_transaction`、`continue_backup_transaction`、`resume_backup_transaction` 和已有明确决定入口；没有第二份阶段转换、复制、SQLite、seal 或 selector 协议。`GET` 不调用任何阶段推进、恢复或激活方法。

- `StartRequest` 只有原 operation、来源槽位、原序号和全部有序目标；主体来自锁内重验的真实管理员会话，不接受客户端 principal 或 SMB 身份。
- `Context` 的 discovery 由服务既定发现根产生。提交时重新按 `CandidateSelection` 的挂载／目录身份、权限、空目录和排除集合验证，先持有全部目标目录，再按相同顺序把 `File` 交给 Engine。不直接用 HTTP 裸路径 `File::open`，不挂载、格式化、chmod 或替换目录。
- 排除 SYSTEM、管理目录、所有现成员和本批已选目标；序列化新增及决定入口与现有 `storage_addition` 生命周期锁。首次创建要求普通管理业务准入；原任务读／续作仅取得原已持有 Engine，不由此开放其它业务。
- 原请求已保留时，同 start 重试只核原操作／主体／输入并读取状态，不再把已写入材料的原目标当新空目录。续作从核心保留的原任务进入，不携带替代路径或重新采样来源。
- `current_backup_transaction_request` 读取 SYSTEM 原输入。即使第一个后台 worker 尚未建立，仍返回原操作；不存在、损坏、其它主体及原任务运行态丢失不是同一状态。

## HTTP 契约

所有入口在既有受保护路由、Origin／CSRF、当前会话重验及共享有界管理请求容量内。请求体上限 128 KiB；重复键、未知字段、错误确认值和数值型 UInt64 均按严格 wire 解析拒绝，真实错误 source 保留。

| 路径 | 行为 |
| --- | --- |
| `GET /api/admin/backup-transactions/context` | 当前序号、有效一级来源槽位和受限发现观察；不授写权 |
| `GET /api/admin/backup-transactions` | SYSTEM 原输入及可用的真实运行态；原输入存在而 worker 缺失时不返回“空任务” |
| `POST /api/admin/backup-transactions` | 严格 `StartRequest`，只启动完整材料后台任务；202 不表示完成 |
| `GET /api/admin/backup-transactions/{operation}` | 同管理员、同原操作状态；不自动重开目标、补任务或执行决定 |
| `POST /api/admin/backup-transactions/{operation}/continue` | 一个严格 typed `Command`，只进入对应核心方法；202 不表示完成 |

`Command` 精确区分：

| action | 必需 confirmation | 核心责任 |
| --- | --- | --- |
| `continue_preparation` | 无，且不允许携带额外字段 | 只推进非决定性原准备；在 fence、registry decision、activation 边界停下 |
| `resume_original_task` | `recover_only_the_original_retained_task` | 恢复同一原任务；若已明确 Activate 且持久化原最终计划，可完成该已选定切换 |
| `fence_source` | `stop_all_original_pool_writes` | 明确全池停写 |
| `decide_registry` | `select_original_irreversible_registry_decision` | 明确原登记及放置共同决定 |
| `activate` | `activate_only_the_original_published_backup` | 只完成已选定原决定的真实活动切换 |

纯 `continue_preparation` 使用严格空结构变体，不能通过 `principal`、目标或额外 confirmation 偷渡决定。start 的固定确认是 `prepare_independent_complete_copies`。

## 冷恢复和服务准入

有原 Engine 时 Resume 只调用其原任务 API。普通 `Engine::open` 因原最终 selector 部分切换而无法建立 Engine 时，只有明确 Resume 才调用 `resume_backup_transaction_at_with_workspace_limits`，传服务配置的 SYSTEM 根、原 op、当前主体和原宿主限额。该核心 API 只接受先前明确 Activate 后的未完成 bootstrap／最终切换计划；未选择计划、不同 op、替代目标或一般 open 错误不触发兼容 fallback。

管理协调器使用已有 `recovery_worker`／`run_recovery` 所有者，入队与关闭串行，保留 writer／目录句柄，返回 `recovery_queued` 而不是完成回执。恢复所得 Engine 先保持 RecoveryRequired，再通过既有管理认证复制、capabilities 和 ready 门；在线 Activate 后也排同一服务准入路径。bootstrap 已完成后的普通冷启动仍由普通管理恢复执行，不在专用 Resume 中猜测或重新选择计划。

只有三项同时成立才返回 `completion`：

1. 核心真实 `GenerationServingActive`，其 Current／registry／writer／SYSTEM 终态已验；
2. 同 operation／当前主体的永久真实 `backup-target-replacement` 回执，状态 Published 或 Acked；
3. 同一 Engine 已通过现有管理服务准入和 ready 门。

初始 Publication 已有 Published 回执、复制字节相等、所有数据库材料或后台线程结束，均不是最终成功。阶段名只是诊断文本；UInt64 计数是十进制字符串。正常后续认证 Publication 不使已完成原回执失效，也不把当前序号必须等于首次序号复制成 HTTP 规则。

## 浏览器原请求保留和未知结果

界面一次只操作一个原任务。页面读取与后台轮询只用 GET；同一时刻最多一项本地请求。导航或登录代际改变后旧响应不覆盖新会话。三个决定有独立确认，不用一个普通“继续”隐式停写、决定或激活。

首次 POST 前，同源且按管理员键保存原 op 和完整 start 输入并读回；没有密码、令牌、权限或自动重试命令。原 start 未到服务端而页面刷新时，仍可显示原输入并经再次确认重试完全相同的请求。已有记录不能被其它 op、目标或原输入替换；损坏、未知、超长、重复字段、浏览器存储拒绝都保留错误而不是擦除记录新建任务。此记录不是核心权威，不替代服务端原输入、物理绑定和实时授权。

网络结果未知会冻结新建和改目标，保留原 op，先查询或明确恢复。冷恢复排队后继续 GET，不能自动重发 Resume 或 Activate。未返回最终 completion 时，即使计数达到总数，也显示未完成；后续恢复仍可按明确原操作发起。

## 预算与本次验证边界

共享管理并发不新增无界队列；目标最多八个，状态与原输入大小有界，后台复制及恢复预算由核心持有。HTTP 锁内重验目标增加最多八组目录／挂载检查，现有 Engine 锁在此期间持有；真实文件系统延迟和最大规模未测量。服务 recovery owner 与 HTTP job 的生命周期独立于浏览器断连，关闭不能把继续运行的后台工作当取消。

新增 contract、适配器和 Web model／恢复记录测试仅为源码，覆盖严格确认、原输入匹配、永久回执判定、worker 缺失、刷新后原 start、错误来源和旧会话阻断。按用户要求未本地编译、构建测试、运行 Rust 测试、跑真实浏览器或触发该候选 CI，未操作实际系统或磁盘。格式整理不等于类型检查，候选须与核心／数据库闭环一起整合；真实断电、目录替换、运行中授权撤销、普通后继写入及端到端成功仍需验证。

## 同批整合的服务准入

来源列表只展示实际 enabled 库存，不把来源限制为一级盘；来源须满足核心原终端边与完整低级链规则。例如 `P01 → B01 → C01` 由 `B01` 发起对旧终端边的替换。服务不复制或推定该准入。在线核心返回真实完成后，在原生命周期锁内先把同一 Engine 的管理状态设为 `RecoveryRequired`、撤销旧 admitted，再排原恢复线程重新核主认证、能力与 ready；只有后续只读 GET 可在同一 Engine 服务门通过后展示 serving。此项整合没有编译、运行测试或浏览器验证。

## 中断边界

最终在线激活在首个原 selector／seal 交换前，把原 native Publication、Current、所有原根／目标根与 writer owner 保留为同一次内存续作。失败仍保有原锁与拒写围栏，显式 Resume／Activate 只能重验并完成同一操作，不重新捕获已变化的旧 seal、不释放再抢锁，也不重发首 Publication。bootstrap 已完成但 Engine 交接尚未完成时，必须核对原已发布 locator 和完整选定实体，不能用完成标记补造丢失 selector；后继 Journal、Current 与 SYSTEM 仍须通过原终态门才释放本次 stop。

进程丢失时继续使用已选原计划的显式冷恢复；bootstrap 已完成可以由正常冷开或显式同操作恢复接管同一实际 journal，不回到旧来源快照。原 seal 的已安装／已保留 inode 对只用于判断当前交换状态；重试还须同步保旧 stage 目录，完成此前 rename 的同步尾，不能把可见旧名等同于已经持久保存。所有路径保留原数据、永久回执与宿主 workspace 限额。

本次仅恢复 `a8890879` 原候选并对齐 `82e8d6f1`，同步补上保旧目录同步尾。既有集成文件新增三项 feature-gated 回归源码，覆盖四个在线晚失败边界、已完成 bootstrap 后三种冷恢复，以及目标 selector 缺失拒绝补造；源码未编译或执行，亦非物理断电／文件系统同步失败证明。活 Engine 的首个 selector 部分交换、真实同步错误及服务端到端恢复仍需统一验证。
