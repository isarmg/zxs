# 最后元数据成员退出：服务管理态接线

状态：源码、回归场景和格式整理已完成；本补丁未运行构建、测试或 CI，必须与真实 [terminal authority](../architecture/terminal-management-authority.md) 核心实现联合验证。它不包含部署或运行池清理。

## 准入与所有权

`ExitPreview.authority` 明确区分活动成员代际与 `terminal_management`。最终 voter 的目标活动成员数可以为零，但不构造零成员 Config，不改变原认证 N/Q。普通 Exit 保持原协议，最后 voter 经过 `begin_disk_exit`、`prepare_terminal_disk_exit` 与 `TerminalDiskExit::complete`。

服务先停止发放普通 backend 和后台观测的准入，持引擎锁验证准确预览并保存原 operation/actor 的意图，然后建立核心 fence。`Arc::try_unwrap` 原子取得唯一 backend，而不是用引用计数快照冒充关闭证明。仍有服务、SMB、HTTP、维护 owner 时保存 Draining 状态并返回原操作待续；核心 complete 还必须完成真实 I/O gate/descriptor closure。失败保留原 Engine 与同一个 TerminalDiskExit handle，后台恢复不能把它重新发布为 Ready。成功才释放旧 Engine，并将 dispatcher 切换为持有系统认证材料的 TerminalManagement。

系统会话在入口、冻结/归档阶段和最终 complete 之前重新验证。关闭页面不取消，未知结果保留原编号；只有与准确归档、目标、动作和 Published terminal receipt 相符的结果才显示完成。HTTP 回执尚未落盘时，可以从已认证 terminal archive 重建该回执绑定。

## 冷启动和可读管理面

恢复先读取受保护系统 locator 的 terminal 选择，再打开 TerminalManagement；不先调用普通 Engine 或初次 activation 的盘探测。坏或缺失 terminal 证据进入恢复错误，不当作首次安装。恢复期间的旧 worker 不可覆盖 Draining、ChangingAuthority 或 Terminal 的 dispatcher。

`/api/management/status` 返回 `terminal_management`，不是 Ready、Unconfigured 或可自动重试的普通故障。管理存储状态显示原固定 N/Q、当前保留登记数、空登记或仍有登记的准确状态。data-only 目录仍列出；Disabled 保留登记，Detached 从当前列表消失。磁盘卡片、系统备注、登记 topology、历史事件和永久 operation outcome 可读；正文与普通业务读写不从这个对象开放。磁盘信息只用系统历史投影，不探测停止目录，也不伪报实时容量或健康。

原始磁盘内容、完整控制历史和旧永久回执不删除。不写目标恢复文件或备份；对象仍是登记目录，同文件系统多个目录不构成禁止条件。设备 SMART 的 UI 来源统一为“设备 SMART”，不把未在 wire 中携带的具体查询实现伪报为 smartctl。

新元数据组必须走专门、已认证原历史的 resume 协议。此补丁不发布尚未接通真实新组激活的按钮，也不把终态接到普通 Add、初次 Genesis 或离线 restore。对应恢复接线另行提交。

## 源码回归与成本

新增服务场景覆盖：真实最后 voter 退出、保留 data-only 登记、重复原请求、持有 backend 导致排空等待、撤销旧 session 后拒绝续作、认证后续原操作、HTTP 归档绑定未落盘，以及停止根替换成普通文件后的系统-only 冷打开。Web 场景拒绝把 N=0 普通预览或缺 terminal authority 标记的回执解释为完成。

服务只复制本次受限控制记录；不复制全部 State。终态历史读取借用已认证 State，事件页用二分游标和既有 1000 条上限；新查询不增加停止根 I/O 或 fsync。核心归档与实际停止/发布的同步预算保持原核心约束。

## 新元数据组接续（后续服务提交）

后续接线复用原 Add API：`POST /api/admin/disks/preview`、`POST /api/admin/disks`、原 addition status/resume。`AdditionPreview.authority` 是必填闭合变体：活动成员添加或绑定具体 terminal authority/原退出 operation 的元数据组接续。客户端 hash 只是预览一致性字段，不是授权 token；服务必须持有核心认证的 `TerminalManagement`。

新组仅允许 1 至 32 个全新元数据目录，使用新 disk/slot、generation 1。目标 Config 保留全部历史条目并取消其旧 metadata 角色，新的固定 N/Q 来自真实新组；不会删除旧历史或生成 Genesis。HTTP 意图额外保存原 Config，读回时验证它与完整目标和预览的关系。精确结果必须是同 actor/op、同 transition digest、同 terminal authority 的 `terminal-metadata-resume` Published receipt，不能把 `synchronize` 的新 Q 或缓存 HTTP Completed 当作新服务准入。

服务独占 `TerminalMetadataResume`，同步或 HTTP 保存失败保留同一 live handle。`control_archive()` 与实际 status 可重建崩溃窗口中遗漏的 HTTP 绑定。消费式激活失败才进入显式 ResumeRecovery；SYSTEM-only `metadata_resume_recovery` 校验真实保存的目标、actor/op 和权威，决定重供原目录 FD 或完成已经选择的 generation，绝不从 HTTP phase 猜测。冷启动不自动用新的 actor 发起接续。新 Engine 只在核心真实恢复当前 namespace 和永久发布后安装，并复用普通启动的当前系统认证复制及准入。已完成历史 receipt 可在后继 terminal 管理态再次查询，不代表当前池仍可写。

本批核心尚未实现 terminal-resume Abort，服务对 Cancel 返回 UnsupportedOperation，Web 预览与确认明确告知“本阶段暂不支持取消”，提交后保留原操作继续恢复，不显示不可执行的取消按钮。此限制只描述当前实现阶段，不是永久产品规则；不能调用普通 Add 的旧固定 Q Abort 代替它。

### 新目录路径边界

生产发现、检查、提交和断电重开都从既有受管 discovery scope 进入；保留 no-follow、held-parent、实际 inode/device/mount 的重验。目录导航和生命周期变更共用协调锁，避免检查时处于 terminal、打开时却落回普通探测入口。尚未打开现有池权威时不允许把它当首次安装目录浏览。

停止历史路径在任何子项 stat/open 之前经规范路径和 mountinfo 的文件系统内路径映射排除；当前活动目录仍使用既有真实 FD 重叠检查。活动 Engine 的 Disabled 或已 Detached 历史也走停止路径分支，因此新组恢复后的下一次普通 Add 不会为排除旧根而打开它。相同文件系统的不同合法目录仍可选择。

该预排除覆盖可见的静态 bind 别名，但并非与外部特权 mount 操作原子化。mountinfo 读取后至实际 open 之间仍存在外部挂载命名空间变更窗口；前后 inventory 重验和 held-parent 身份验证可拒绝已观察到的变化，不能据此声称任意恶意动态别名下从未发生过一次旧根 open。最终物理根准入仍由核心真实持有句柄、bootstrap identity 与 generation proof 决定。未为规避这项边界禁止整个文件系统。

### 源码验证与同步预算

补充真实服务源码场景：管理态到新 Engine、同请求重放、历史 receipt/保留登记、冷重供目录且旧根替换成普通文件、丢失 HTTP archive 绑定、撤销会话后拒绝续作、重新认证后续原操作、不支持取消时不修改原 continuation、发布后的 HTTP 缓存丢失与真实冷启动。目录测试覆盖停止根早拒绝、静态 bind 映射、同文件系统合法兄弟目录、准备改变 ctime 后同 FD 身份重开与父目录替换拒绝。Web 约束要求精确 terminal receipt，并隐藏该阶段不可执行的取消。

服务不复制 State 或归档，只持有/传递核心 owner 和已限定的 HTTP 控制记录（4 MiB，新增目录最多 32 个）。目录列表仍最多扫描 4096 项、返回 200 项；mountinfo 有 4 MiB/4096 条上限，停止路径映射在请求边界以路径数 × mount 条目构建一次，每个候选只扫描 mount 表和已映射停止路径；有停止路径时在访问前后重读 inventory，不为每个列表子项重新读取整个 mountinfo。受保护阶段重新验证当前会话，核心准备/投票/发布调用与管理认证变更串行；该期间登录或撤销可能等待调用结束。没有新设弱化核心 fsync/法定多数的缓存。

本批仅执行官方 rustfmt 和 `git diff --check`；没有运行 Rust/Web 构建、测试、模型或 CI。须由统一集成验证配合核心 resume 与 completed-selector 只读修正、terminal head 后继轮转共同确认。
