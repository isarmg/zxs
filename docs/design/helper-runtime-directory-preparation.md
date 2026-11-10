# 辅助工具首装：运行目录与独立空 SMB 导出

本片原始候选基于 `160a642f`，发布时对齐 `ac91e1f5` 并保留其 selector/UI 及逐盘数据库任务更新；补足主服务模板已有但首装尚未准备的固定运行目录。它是 H07 的生产源码切片，不是总体安装完成。**没有审查、编译、格式化、运行测试或 CI，没有执行 helper、操作实际系统路径或启动服务。** 15 项新增测试仅为源码，其中一项完整 UID/GID 交接用例标为需显式 root 测试环境，且只使用临时目录。

## 入口与范围

独立 Web 的“首次安装：运行目录与空 SMB 导出”和下列 CLI 共用同一生产实现：

```text
zxs-helper initialization runtime-preview --service-user zxs
zxs-helper initialization runtime-prepare --service-user zxs \
  --request-digest <准确预览摘要> --confirm-empty-runtime-only \
  --external-administration-stopped
```

HTTP 为严格 JSON POST `/api/initialization/runtime-preview` 与 `/api/initialization/runtime-prepare`，沿用 64 KiB 请求预算、IP Host/同源/CSRF、单任务阻塞 owner 与退出排空。输入更改撤销预览；准备必须另作明确确认并声明已停止外部服务/主机管理写者。确认缺失在本地账户或系统路径读取之前拒绝。声明不是系统级互斥或认证。

| 固定路径 | 所有权与权限 | 首装要求 |
| --- | --- | --- |
| `/zxs/cache`、`/zxs/config`、`/zxs/home`、`/zxs/log`、`/zxs/run`、`/zxs/tmp` | 准确本机服务 UID/主 GID、0700 | 不存在或仍完全为空 |
| `/zxs/smb-export` | root:root、0755 | 不存在或仍完全为空；不是私有数据路径 |

每次都准备完整固定集合，包括 SMB 尚未启用时的无内容导出目录。它不赋予普通账户 export 写权限，不将 `/zxs/data` 或 socket/spool 暴露为共享内容。目录不代替 VFS，也不承诺任何非 root Samba peer 能穿越私有 `/zxs/run`；真实 peer 身份和 VFS ABI 必须在最终配置/激活前另核。

工具不接受任意路径、任意 UID/GID、chmod/chown 参数或清理开关。不会创建 Unix/管理员/SMB 账户、读取密码、修改 passdb、写日志/socket、清理缓存、准备数据盘、加载配置或 reload/start/enable。首次管理员和存储仍走各自现有步骤。

## 身份、固定父目录与互斥

复用现有本机账户窄类型及安全 `/etc/passwd` 读取，要求唯一的非 root UID 和非零主 GID。计划绑定账户名称/UID/GID、passwd 设备/inode/完整 SHA-256、固定目录策略和 `/zxs` 设备/inode。外部身份源仍受 4 MiB 预算。运行目录对应主 GID；后续配置安装必须继续核对 systemd Group，而不是在这里猜测另一个组。

先取得与当前 ZXS 程序共用的 `/zxs` inode 建议锁，再持有同一个目录能力。父目录必须已准备、root 所有、准确 0755，允许独立 SMB 账户遍历但不能修改它。子路径相对持有 fd 打开，不接纳符号链接或嵌套挂载；每个发布边界重核主机路径、父身份、真实账户和子目录名称身份。已存在目录仅核验及同步，不修改其 owner/mode。

该锁不证明旧版本、Samba、任意 root 或外部配置管理器已停，也不在 HTTP 响应后继续持有。无认证 root Web 监听所有接口的风险保持：只能临时在可信隔离网络使用，同源/CSRF 不等于访问认证、TLS 或公网部署保护。

## 准确意图、部分发布与完成后拒绝补造

1. 读取全部七个目标和各自准确 pending，拒绝非空、链接、异常权限、同目标与 pending 同时存在。不因前几个路径正常就先写再发现后面的冲突。
2. 在 `/zxs/.zxs-helper-runtime-intent-<摘要>/plan.json` 保存完整当前计划，沿用现有 0700 bundle、0600 普通单链接文件和目录发布屏障。已有意图必须逐字节匹配；有 pending 却无完整原计划时拒绝猜测认领。
3. 逐个在 `.zxs-helper-runtime-<摘要>-<目录名>.pending` 创建空目录。只对本请求暂存 inode 完成所有权/权限交接，先同步 inode，再以 `RENAME_NOREPLACE` 发布固定名称并同步 `/zxs`。已存在准确空目录只重新核验及同步。
4. 全部实际目录仍准确后，在 `.zxs-helper-runtime-complete-<摘要>/receipt.json` 持久化固定顺序的每个目录设备/inode。完成后再重读当前目录，与回执准确比较。

这不是七目录的原子事务。错误保留已发布目录、原意图与 pending；类型化 `Unknown`、`Published`、`PublishedDurabilityUnknown` 及底层原因随错误链返回。失败不代表未写入，也不宣称回滚。相同原计划可补齐尚未完成的空目录；不同账户/父身份产生不同计划，不能借换摘要自动清理旧材料。

原 pending 仅允许 root 所有的 0700 暂存状态或已经完成准确交接的最终属性。mkdir 后权限异常、未知内容、任意 root 并发改挂载仍拒绝保留，不猜测修复。

一旦已有完整 receipt（包括完成 bundle 的可读 pending），重试必须拥有原计划，且所有目录的身份、属性、空性完全匹配。此后仅同步，不再 mkdir/chown/chmod。已经发布的完成 bundle 缺 receipt 时直接拒绝；仅有未写完 receipt 的完成 pending，也要求全部目录已经存在，只允许补全该阶段的回执，不重新创建目录。丢失、替换、非空或权限变化一律拒绝，不能因重试“修复”已经投入使用的运行目录。完成 receipt 本身也不会被覆盖。

完成响应为 `empty_runtime_directories_durably_prepared_not_started`。`system_initialized=false`、`service_started=false`、`complete_installation_ready=false` 始终保留，`created_in_this_attempt` 与此前已完成目录分开返回。

## 预算与后续

目录数固定七个，仅查看每个目录的第一个条目以判断空性，不递归扫描或读取其内容。目录能力与阶段对象总数 O(7)，计划/回执读取各不超过 16 KiB。账户来源每次重核仍为有界 O(passwd 字节数)。意图/完成 bundle 沿用既有文件和父目录同步；每个新目录有自身 inode/父名称持久化，原目录重新同步，不以批处理为由削减必要屏障。无性能提升或零 I/O 声明。

完整首装仍缺已安装 OS 依赖与 VFS/许可载荷、实际目标 systemd 校验、有效 Samba 配置选择与账户创建、既有配置安全替换/回滚、明确服务 reload/start/enable，以及完整数据/数据库/磁盘就绪交接。本片不修改主服务模板、不启 helper 自启，也不把仅有这些目录作为正常服务准入证明。
