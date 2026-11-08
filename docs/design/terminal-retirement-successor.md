# 首笔终态管理登记后继、Stop/D3 与辅助工具确认

[任务](../../TASKS.md) · [原决定](terminal-retirement-decision-storage.md) · [辅助工具](../components/helper.md)

基于真实 `a25ff1a2` 开发，对齐真实 `c45044a0`。首笔限制已由[逐笔链与既有 Stop 消费](terminal-retirement-chain.md)继续扩展；本页记录首片的持久化基础。本片仅源码交付，未审查、编译、测试、浏览器验收或运行/查询 CI，未运行辅助工具、未改任何真实磁盘/系统。它完成受限的**首笔 no-voter 管理登记移除**，不是全体剩余成员、活动命名空间发布、metadata resume 或外来接纳的完成声明。

## 准入与保留内容

现有 SYSTEM writer、终态 selector、完整原固定成员控制归档及当前主认证仍是入口。只有当前剩余的准确 pool/disk/slot/binding、bootstrap 已记录且当前生命周期尚未 Stop，才能预览该首笔。目标路径只是展示；成功、失败和冷恢复均不打开/stat/遍历目标。旧 Disabled/已 Stop 绑定、缺失 locator 记录、epoch 不符、已用 operation、第二个不同意图、第二笔后继、metadata resume 均明确失败，不伪造新权威或空池。

原终态 authority、固定 N/Q、原永久结果、业务历史、历史盘定义、原 detach mask 及物理字节保留。新内容寻址 registry 图仅移除准确当前登记、增加 view generation 和 predecessor，原 final-voter intent 保留。该类型只在管理态消费；它没有伪造 occurrence mask，不能转为活动 namespace 或 fresh/retained resume。业务历史仍可用于未来正确恢复，不做隐式清理。

## 原生持久化顺序

1. 纯构造并有界编码 successor 与精确结果，检查 sequence/topology 溢出。
2. 复用原不可变 `retirement-intent.json` → `retirement-selected.json`。来源、主体、ID、摘要均不允许替换。
3. 不可变 `retirement-successor.json` 保存全部有界 registry 图，逐对象摘要/种类及精确前驱语义重读验证。
4. 不可变 `retirement-stop.json` 保存准确 locator 前驱和待写 Stop；在此计划同步前不写新 bootstrap Stop。Stop 只会追加限制，不改根、放大探测集合或降低固定 quorum。
5. 既有 bootstrap 原子 record/head/目录同步落地同一个 Stop。只有这个已认证选择能构造窄的 Stop 发布凭据，裸准备字节不具授权。
6. `RecoveredDiskIo` 核对整个进程实际逻辑 I/O 域已关闭、普通/控制 lease 均归零，持有排除 guard。残留旧 handle 会失败，不从 JSON 声明推断关闭。
7. 不可变 `retirement-complete.json` 精确绑定选择、successor 与永久 `disk-deregister` 结果。读回验证、重新取得 I/O 排除后，才安装管理 registry 与 State operation/principal/sequence/topology 投影。结果 `current_health=unknown`、`active_namespace_admitted=false`、`metadata_resumption_supported=false`。

D3 在这里仅表示该 no-voter 管理登记/永久结果的最终发布，**不表示活动业务视图或新 writer 已恢复**。首次完成后原 evidence 不删除，不允许新操作越过它；当前限制通过 helper 二次确认和服务 Web 状态明确展示。

## 中断与冷恢复

原 intent/selection 或 successor 存在不代表移除。Stop plan 支持准确 record/head 已写、仅部分同步等原出版窗口；重试只重建同一原屏障。Stop 已落但最终 marker 缺失时，当前登记继续显示，原结果不存在，只能同原 actor/ID/digest 完成。最终 marker 只有同时具备原 selection、完整 successor、准确原 Stop plan、当前 durable Stop 和实际 I/O 排除才返回永久结果。

缺 intent 的 successor、孤立 Stop/final、错误正文/图/摘要、错误来源或绑定失败并保留证据，不按“找不到”自动重置。缓存 Completed/helper JSON 不能作为结果。冷打开已存在的 final marker 仍重新建立原 intent/selection/successor/Stop/final 的准确同步屏障；仅看到上次可能未同步的 rename 不授予永久成功。SYSTEM 仍是原 selector，冷启动在任何数据盘探测前选终态；完成后旧 Config 也不能复活被移除登记。

本片不提供取消、自动回滚、第二笔 successor、已 Stop 生命周期重用或后继 metadata-resume 消费。对这些明确拒绝比返回伪成功安全。全链 foreign admission 仍缺最终活动 Backup writer，不借本流程绕过。

## Helper 入口与权限

专用模块 `helper/src/disk_removal/terminal_action.rs` 提供 `/api/disks/terminal-retirement-preview`、`terminal-retirement-original`、`terminal-retirement-commit`。预览是 native 私有 Plan 的 DTO 显示投影，提交由当前服务 UID owner 重建并比较全部字段。原计划先保存于 root-owned `.zxs-helper-terminal-retirement-<sha256>/plan.json`，主凭据 revision、宿主 lease、固定布局和账户身份持续绑定。原计划可下载；断连使用同一个根摘要查询/续作。

确认文本为 `retire-terminal-member-without-metadata-restart`，另需当前访问损失及本阶段无 metadata restart/第二笔的独立 acknowledgment；在系统访问前检查。沿用无认证 root / 默认 `0.0.0.0`、Origin/CSRF、单任务和停服租约，不新增登录或修改网络。没有执行真实服务、擦盘、卸载或启动。访问暴露风险仍按辅助工具首页提示处理。

## 成本与未验证范围

一次提交新增三份有界 SYSTEM envelope（successor、stop plan、completion），保留既有两份选择和 bootstrap record/head file+directory fsync；没有删减持久化屏障。registry 图和业务 State 在管理边界克隆/编码，成本随登记/历史状态增长，过大则在首个 native intent 前拒绝；不是文件热路径性能优化。冷读保留原控制归档全量认证和 locator 链校验成本。没有测量或吞吐结论。

新增测试**源码**覆盖首笔真实完成、冷态原结果/重复执行、目标路径替换仍不探测、历史定义保留、Stop 后最终 marker 缺失续作、缺 Stop 的伪完成拒绝，以及第二笔/metadata resume 拒绝。全部未运行；不代表组合、掉电、真实设备、服务身份或浏览器通过。
