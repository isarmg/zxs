# 辅助工具：已有池的准确空目录 Add 批次

[辅助工具](../components/helper.md) · [磁盘职责](helper-disk-management.md) · [任务清单](../../TASKS.md)

本片实现源码，按当前开发指令没有编译、测试、审查或 CI；没有对真实主机运行工具、添加磁盘、改权限或启动服务。它是四类磁盘职责中的一个有限纵切，不表示整项迁移完成。

## 实际范围

独立 `zxs-helper` Web 新增三个同源 POST：

- `/api/disks/add-preview`：按服务账户、客户端生成的原 operation ID、整批准确根/slot/level/upstream/metadata-member 生成预览；
- `/api/disks/add-original`：以保留的原 SHA-256 请求摘要读取计划，再从原生控制记录与永久核心回执核对状态。只有 root 意图已落盘而核心意图尚未创建时，必须重新核对**完全相同**的空目录预览；不会自动发布；
- `/api/disks/add-commit`：明确确认预览，添加并初始化新空目录，或继续同一原操作。没有改编号重试、替换目标或自动回滚。

只支持当前已初始化 production 池和固定 `/zxs/data/{state,management,disks}`。现有所有配置根与本批新根都须为 `disks` 的规范化直接子目录，已有准确服务 UID/GID 和 0700，整集合须等于实际子目录集合。客户端不能把未知外来目录塞入 `registered_roots` 当作已登记盘。每批 1–32 个新目录，真正空且通过共用候选选择的目录、挂载与身份复核。

支持的初始化是 **Add 协议内初始化新空目录**，不是分区、格式化、挂载、改权限，也不是以擦除旧材料获得空盘。首个全新池继续使用既有首批初始化流程。

高级关系由完整 target Config 和核心纯 Add 门禁校验，所需低级祖先必须是当前可准入的原成员，或同一准确批次中的新根。不允许虚拟低级占位，不接纳 Backup 凭据为普通 Add/Candidate。不支持把外来非空高级盘先装入后再补链。

## 共享核心与专用编排

`src/server/src/management/storage_addition/active.rs` 的类型化 `ActiveAddition` 同时由现有服务 Add 协调器与辅助工具使用。共享的是 target 规划、控制记录格式、原主体/operation/preview/target/归档绑定、pending 互斥检查和原生发布，不复制另一套成员协议。

每一新操作先耐久保存原控制记录，再保存 latest 指针并沿既有 fence → 控制归档 → enroll → joint decision → activation 运行。每个阶段仍沿当前主凭据重新准入，只有准确永久结果可以成为 Completed；本片不改变固定 quorum、历史哈希或原归档验证。原完成结果查询还核对核心回执，保存的 phase 不是独立成功证明。若原意图已选择 Cancel，重发 Add commit 不会把它切回 Add；本片 Web 不提供新的取消命令。

现有服务 HTTP 仍先验证真实管理会话。公共本机控制 API 必须绑定活着的主 `AuthStore`，使用其只读 `AuthReader`；不回落到灾备快照授权，不伪造 session，不把无认证 Web 访问者记为某个已登录人。记录主体是本机当前管理员的存储协议身份，**不是网络操作者的身份认证**。

辅助专用 root 计划、HTTP、页面、worker、固定安装布局和进程所有权在 `helper/`。首批初始化和本片共用 helper 内的 `managed_layout.rs`、`service_worker.rs`，不复制 UID/GID 降权或另一套进程排空代码。

## 生命周期与失败

root 在 `/zxs/.zxs-helper-addition-摘要/plan.json` 保留有界、无密码的原计划。helper 工作槽一次只接纳一个操作；原生控制再持有管理目录里的专用 lock。root 编排释放其 host lease 后，由同一二进制的服务 UID/GID 子进程取得、核对并全程持有 host lease、既有 AuthStore、Engine 与目录身份。没有主服务 HTTP 请求，也不启动主服务恢复线程、后台定时器或 listener。

进程只能是 `/proc/self/exe internal-addition-worker`，清空环境、无 shell、无继承附加组，双向 IPC 有 64 KiB 上限。root 始终 drain/join 已启动的子进程；浏览器断连或关闭页面不把执行中的写者遗弃为后台任务。介质阻塞不能被称为已排空，不承诺固定秒数完成。

父子之间及子进程结束后重新取 host lease 存在明确交接窗口。若当前 ZXS 在窗口赢得 lease，本操作拒绝或保留结果不确定，不绕锁并行。所有存储工作发生在子进程持有 lease 期间。旧二进制、Samba 和其他外部管理者不参加这个协议，须由用户明确停止，确认不等于 OS 强制排除。

预览和状态也会打开既有 Engine，可能执行当前核心的已有恢复与同步；它们**不是任意磁盘的字节级只读检查**。只读独立盘检查继续使用 authority 流程。凭据、账号或根身份改变会拒绝原计划，不能通过改请求摘要重绑旧操作。中断或超时可能已部分/全部发布，保存原 ID 和摘要，先核对原状态；不得清空目录、删记录或换编号掩盖不确定性。若损坏导致 Engine 无法打开，本入口只报告失败，不能保证所有故障可自动续作。

## 明确未完成

- 注销/移除迁入 helper、终态 metadata resume 和 stopped-root readmission；原有安全服务入口未因本片被删除；
- 外来历史/来源映射/完整低级链批接纳、Backup 最终 generation 与活动 writer；
- 新独立逐盘权威替代当前原生 Add 协议；
- 所有对象/历史/标签/三范围数据库的最终干净运行证明，完整首装和 systemd/SMB 激活。

回执明确 `foreign_history_imported:false`、`complete_installation_ready:false`、`devices_formatted:false`、`service_started:false`。不提供计划下线动作，实际失联、损坏、Disabled 和未决记录仍然保留。

## 成本和验证边界

增加的预览/根集合核对为 O(当前配置根数 + 本批新根数)；本批上限 32，IPC/保留计划上限 64 KiB。共享原有 4 MiB 控制记录预算与阶段原子替换/fsync，没有删同步或缩减历史。原生 fence、全控制归档和成员发布的历史/成员成本保持现有协议，不宣称性能提升。

添加了 helper 确认/严格 DTO/未知根拒绝/响应边界、HTTP Origin/未支持入口及共享原生 coordinator/会话隔离的测试源码，均未运行。实机服务身份、Samba 排除、掉电/介质故障和端到端浏览器验收尚缺。
