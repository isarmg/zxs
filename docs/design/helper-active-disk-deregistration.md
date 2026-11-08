# 辅助工具：保留盘字节的非终态注销

[辅助工具](../components/helper.md) · [磁盘职责](helper-disk-management.md) · [任务清单](../../TASKS.md)

本片只有源码交付，按当前开发指令未编译、测试、审查或运行 CI，也没有运行工具或改变真实盘。它迁入**活动池中注销后仍保有至少一个元数据成员**的 Deregister，不是最后一个元数据成员的终态退出，不是整个磁盘管理项目完成。

## 实际入口

独立、无登录的 root Web 提供四个同源 POST：

- `/api/disks/registrations`：从当前 Engine 的认证登记集合列出精确盘 ID、slot、绑定代数、Enabled/Disabled、角色和已记录路径。登记意图不是设备健康或在线证明，不能把失败投影为空池；不会额外探测某个 Disabled/退役路径。
- `/api/disks/deregister-preview`：只接受准确 slot 与 expected disk ID；固定为 Deregister，不接受 action、替换路径、force 或新计划下线。预览绑定源配置、sequence、topology generation、已发布 snapshot 和退出后的元数据成员数。
- `/api/disks/deregister-original`：读取 root 原计划并核对共享原生控制记录、D3 与永久回执，不自行继续。只有 root 意图存在而原生意图尚未创建时，允许重新确认**完全相同**的原发布边界。
- `/api/disks/deregister-commit`：必须明确确认 `deregister-member-preserve-bytes`，并另外承认当前归属/内容访问可能丢失。提交只延续准确原操作。

这里的移除是注销成员及其当前登记/归属视图，**不格式化、不擦盘、不删除正文或历史、不卸载设备**。不能因此承诺剩余池仍保有全部文件或历史访问：用户应先另行备份重要内容。当前命名空间与持有副本影响继续由现有核心语义决定，不新增虚假复制/迁移来掩盖影响。

只支持现有 production 池与固定服务私有布局；目标必须是 `/zxs/data/disks` 下的既有规范化直接子目录定义。最后一个元数据成员会被识别为 TerminalManagement，并在保存/执行新退出前拒绝；它需要独占消费 Engine 的专门 drain/terminal owner，尚未迁入 helper。当前原服务终态和历史操作出口未删除。

## 共用协议，不仿造成功

服务端和辅助工具共用 `management::storage_lifecycle::ActiveExit` 与原生持久阶段执行器。保留原 `storage-exits` 格式、主体、operation、preview、原 Config、archive 绑定及 latest 指针，不复制独立的退出权威或改写历史哈希。

新的原生意图只在 Engine 锁内复验源边界成功后的 progress 回调中保存，然后才 fence。过期预览拒绝不能遗留一个未准入的原生意图阻塞所有新计划。之后沿原生 fence → control archive → enroll → joint → activation/D3 继续，每个阶段维持当前主凭据准入和固定成员证明。已有结果仍须通过真实 D3/永久回执核对；仅存在 State 操作行或 `Completed` 缓存不构成成功。

共用执行器也继续供原终态/历史计划使用；新 helper 不提供 PlannedOffline，也不把原历史动作改成 Deregister。以后终态退出下查询旧活动退出回执，仍走原终态权威路径，不能强行要求重新打开活动 Engine。

helper 专用计划、页面、HTTP、固定布局、降权 IPC 与操作所有权在 `helper/`。`managed_pool.rs` 为 Add/注销共用现有 production 池 owner，持有当前主 AuthStore、Engine、目录上下文及 host lease，不创建账号、初始根或后台服务。HTTP 管理 session 规则未放宽；本机协议使用现有主凭据的唯一管理员身份，不伪造 session 或网络操作者身份。

## 不再触碰退役盘与生命周期

root 把无密码、有界计划存于 `/zxs/.zxs-helper-deregister-摘要/plan.json`。同一二进制服务 UID/GID 子进程沿既有受控 IPC 执行；请求/响应上限 64 KiB，无 shell，父进程总是 drain/join 已开始的工作。界面退出、断连或错误不能让未排空写者被报告成停止。

host lease 在父子之间及回执交接有显式窗口，所有实际存储操作只在子进程持有当前 daemon 同一 lease 时发生。若其他当前进程在窗口先取得 lease，本请求拒绝或报告原结果不确定，不绕锁。用户仍必须停止旧二进制、Samba 和其他不参加协议的写者，确认不是 OS 强制排除。

**D3 后的 helper 状态、重放与 root 回执检查不对目标执行 exists/stat/open/目录清点。** 正常 Engine 按已认证登记与 D3 排除退役 I/O，helper 只重查系统/管理/发现父目录身份。即使盘已拔出或旧名被别的材料占据，也不能把新路径当原成员；当前核心若无法核验回执，明确失败，不凭路径存在推断完成。

已有 helper Add 的全根清点也相应收紧：root 编排不再依据客户端计划直接遍历磁盘；服务 UID owner 先读真实登记。当前 Add 纵切遇任何历史停止/退役配置根会明确拒绝，**不重开这些路径**。支持带历史退役根的 Add、独立原 Add 查询及更一般的 replacement/readmission 仍未迁完，应使用对应现有安全恢复出口；不能通过删配置或清空旧盘绕过。

登记读取、预览和状态会打开已有 Engine，可能执行它的既有恢复/同步，不是任意源盘的纯只读检查。计划绑定服务账户、根身份和主凭据 revision；变化后拒绝原计划，不能自动重绑。中断可能已部分或全部发布，结果不明时不要拔盘、删证据或换 operation 重做。介质损坏、Engine 无法重开或原凭据改变不保证能自动继续。

## 明确未完成

- 最终元数据成员的终态退出/管理切换、一般移除迁移完成证明；
- 外来完整低级链的一次性正式接纳、来源身份映射、活动 Backup generation/writer 与安全解除源围栏；
- 全对象、标签、数据库最终干净运行校验与完整首装/服务激活。

回执只含真实原生 ExitStatus，并明确 `physical_bytes_erased:false`、`remaining_content_preservation_verified:false`、`service_started:false`、`complete_installation_ready:false`。外来批准备、路由材料和控制归档都不是正式接纳，继续拒绝以普通 Add 替代 H03。

## 成本与未运行验证

登记展示为 O(当前登记数 × 配置条目数) 的小规模元数据关联，本片没有把它放在线文件热路径；退出仍使用既有 4 MiB 控制记录预算和每阶段原子替换/fsync。协议、固定 quorum 与退役盘资源关闭规则未缩减，不宣称性能优化。

测试源码覆盖确认/影响承认、严格输入、拒绝新计划下线/最终成员、同源防护、真实共享 Deregister 后旧路径替换仍只读回执，以及 Add 的停止根投影。全部未运行；编译、服务身份进程、浏览器、物理拔盘/掉电和真实介质故障仍未验收。
