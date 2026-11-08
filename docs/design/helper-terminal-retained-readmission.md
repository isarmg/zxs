# 辅助工具：保留终态元数据目录的明确重新接纳

[辅助工具](../components/helper.md) · [磁盘职责](helper-disk-management.md) · [原生保留目录协议](../architecture/terminal-directory-readmission.md) · [任务清单](../../TASKS.md)

基于真实主线 `8d4c4da8` 开发，并串行对齐真实父 `801e3c8e`。本片把已经存在的受限原生 terminal retained-directory readmission 接到独立 Web、root 计划与服务 UID 工作进程。没有新增计划下线，也没有放宽终态逐笔移除之后的恢复门禁。源码未审查、编译、测试、浏览器验收或运行/查询 CI；没有运行 helper、ZXS 或操作真实主机。

## 可执行范围与明确拒绝

只有当前 production 池处于真实 terminal management、目标仍为原 registry 中的 **Disabled 元数据登记**、原 Stop/历史角色/完整终态来源仍可认证、且没有开始逐笔 terminal retirement 时，才可以预览并显式重新接纳。目标必须是原 pool/disk/slot/binding 对应的服务配置路径，不能传入替换路径或把已注销身份重新解释成 Disabled。

确认后核心验证原目录与 seal，以新的 I/O epoch、原 terminal authority 下的 metadata resume 决定、实际首笔 Publication 及永久 `disk-readmit` 结果完成准入。它不是空目录 Add，不导入外来历史，不将文件夹存在、root 保存的计划或可变 Completed 缓存当成完成。

已 Detached、非元数据、替换根、缺失或损坏证明、另一个未决操作、任何未决或已完成 terminal retirement 链继续拒绝。已有终态命名空间准备闭包不能替代整链 ResumeCore。外来二级及以上盘仍须一次性全部低级链的最终活动证明；本片不使用 Backup 归档/准备材料冒充可激活目标 writer，不解除源围栏。

## 独立 Web 与专用编排

沿原 `0.0.0.0`、root 手工启动、无登录的控制台新增：

- `POST /api/disks/readmission-preview`：明确服务账户、原 slot/disk 与新 operation，返回准确原生预览及 root 计划摘要。
- `POST /api/disks/readmission-original`：只输入原 root 摘要与同一服务账户，核对原计划、认证修订和实际原操作。没有原 native record 时，仅在原 source/preview 完全未变时报告未开始；不能自动另建操作。
- `POST /api/disks/readmission-commit`：必须重传完整原计划、摘要、字面确认 `readmit-retained-terminal-metadata` 和独立新 epoch 确认项。只执行/续作该原决定。

全部 POST 使用既有 Host/IP、Origin/CSRF、请求上限、单任务槽和退出 drain；错误不泄露凭据。它们不构成登录或 TLS，任何网络可达者仍可能发起 root 管理操作，只应临时在可信隔离网络运行，用完退出。控制台不注册自启，不修改网络设置。

界面复用真实登记读取器，选择变化使预览失效。查询已完成操作隐藏提交按钮；提交或结果不明后清除旧候选并要求重新读取登记，保留原摘要供继续。成功不启动服务，明确 `service_started:false`、`complete_installation_ready:false`、`imports_foreign_history:false`、`supports_after_terminal_retirement:false`。

## 身份、互斥和恢复

root 在固定 `/zxs` 下保存不可覆盖的 `.zxs-helper-terminal-readmission-<digest>/plan.json`，摘要绑定账户来源、持有目录身份、主凭据 revision、原生操作/目标/authority/registry/snapshot。保存计划不是 native intent 或发布证明。helper 不创建账户、修权限、挂载、格式化、擦除或切换其他目录。

只有经过既有服务身份降权的 `internal-readmission-worker` 打开 `/zxs/data/state` 和 `/zxs/data/management`。它持有 `ServiceLease`、已初始化 `PreparedService`、主 `AuthStore` 和共享 lifecycle owner 锁，阻止遵守该 host 租约的当前 daemon 并发启动。Samba、旧程序或其他写者仍必须由操作者明确停止，不能假称 host 租约已证明所有外部写者退出。

共享 `ExclusiveTerminalReadmission` 不起服务线程，不返回可借出的 Engine handle；用当前主 AuthReader 重核主体，拒绝 SMB 限定身份。原生 `storage-readmissions` 格式、archive binding、preview 比较、结果校验与原服务通路共用。新的预览先拒绝其它 Add/Exit/readmission 未决操作；原操作继续不换编号或目标。

原结果查询只用 SYSTEM 上的准确 resume selector 区分原 `ResupplyDirectories`、`CompleteGeneration` 或已经准入的 Engine。前两者只展示恢复要求，不供应目录、不激活；已准入 Engine 的打开仍可执行原有恢复/同步，不能称整个查询绝对只读。明确提交才重新持有原目录，或完成原已决定 generation，随后核实际永久结果。中断后不能依据错误或断连推断未写入；保留 root/native 记录，查询并只继续原操作。任何 root 结束重核失败也保留“可能已发布”的诊断及底层错误来源，不假装回滚。

## 未包含的完整就绪义务

本片不做管理凭据灾备副本重新同步、全盘健康/全部 SQLite 验收、完整首次安装就绪、systemd/SMB 激活或服务自动启动。首个 metadata voter 的原生恢复完成不证明这些义务已完成；正常服务仍须经过自己的现有启动准入，不接受 helper 回执作为永久启动许可证。

持久根计划/IPC 受 64 KiB 上限约束，超限在保存 root 意图前失败；共享 native record 仍沿现有元数据上限与 fsync 顺序。核心代际恢复成本取决于原归档和配置，未做容量、延迟、设备或中断恢复验收。新增请求/摘要/能力声明、Origin/CSRF/忙碌/停止、原生 owner 锁/身份/空 SYSTEM 拒绝的回归源码均未运行。真实终态恢复成功、故障注入及浏览器流程仍待后续获准验证。
