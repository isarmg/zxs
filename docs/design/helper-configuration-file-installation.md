# 辅助工具：固定配置文件安装，不启动服务

本片在 `e2d9ac78` 的本机服务账户与私有空布局前置之上，补真实 CLI/Web 配置文件安装。**源码未编译、未测试、未审查、未运行 CI；没有执行 helper、修改实际主机或启动服务。** 这是文件安装阶段，不是 systemd/Samba 激活、一条龙安装或完整首次初始化。

## 独立入口与确认

保留 `deployment preview/prepare` 的未激活包，不改变其用途。新增：

```text
zxs-helper deployment install-preview --service-user zxs --service-group zxs \
  --listen 0.0.0.0:7400 --smb-account zxs-owner --smb-peer-uid 0
zxs-helper deployment install-files --service-user zxs --service-group zxs \
  --listen 0.0.0.0:7400 --smb-account zxs-owner --smb-peer-uid 0 \
  --request-digest <准确预览摘要> --confirm-configuration-files-only \
  --external-administration-stopped
```

Web 使用同一部署表单，分别“预览未激活配置包”和“核对账户并预览固定配置安装”；安装按钮另行确认。对应严格 JSON POST 路由 `/api/deployment/install-preview` 与 `/api/deployment/install-files` 沿用同源/CSRF/IP Host、64 KiB 请求上限、一个阻塞工作 owner 与退出排空。改变输入撤销旧计划。无密码字段、shell、任意目标路径、强制覆盖、联网下载或服务管理命令。

固定目的地为 `/etc/systemd/system/zxs.service` 与可选 `/etc/samba/zxs.conf`，均 root 所有、0644、普通单链接文件。**不修改 `/etc/samba/smb.conf`，不往其添加 include，不接管原有共享**。安装 `zxs.service` 会将文件放入 systemd 搜索目录，但不会 reload、start、enable；Samba 独立文件尚未被本工具选择为服务的运行配置。工具本身不注册 unit 或自启。

这两份配置文件不含凭据。即使安装完成，也不能据此启动缺少初始化的系统，或宣称 Samba 已使用该配置。外部服务管理者自行加载或启动不在本片操作内，操作期间不得同时进行。

## 身份与固定资源

- 仅手动 root helper；缺少确认或外部管理已停止的明确确认时，在读取系统身份/载荷或写任何文件前拒绝。
- 本机 `/etc/passwd`、`/etc/group` 必须来自 root 所有、不可由组/其他用户写的安全 `/etc`；拒绝链接、多链接和超过 4 MiB 的文件，绑定准确 inode/设备与完整 SHA-256。
- 服务 UID 与主 GID 非零，本机名称与所选 UID/GID 均不允许歧义；输入 `service_group` 必须解析为该服务账户的主组，不能借 systemd Group 改成另一个组。可选 Samba 账户必须存在、非 root 且不同于私有服务 UID。**不创建用户/组，不证明 Samba passdb 已存在，不写密码**，不调用 NSS/网络解析或读取 shadow。
- 已安装 `/usr/local/bin/zxs` 必须与正式 helper 的内置有效载荷完整匹配；开发构建无载荷、程序缺失或不同字节都拒绝。配置安装不会顺带安装/修复程序。
- 固定 `/zxs`、`/etc`、`/etc/systemd`、`/etc/systemd/system` 和可选 `/etc/samba` 必须已经存在。沿父 fd 打开子目录，拒绝路径链接，逐层保持 root 所有且无组/其他写权限，不创建或修复现用系统目录。
- 计划摘要绑定主机根、所有目标父目录、完整服务配置、程序来源与字节摘要、真实账户/组源。现有文件是否已存在不参与摘要，允许同一请求在部分发布后重试。

锁顺序为 `/zxs` 生命周期租约、程序父目录、`/etc`、`systemd`、`system`、可选 `samba`。均为非阻塞建议锁。它排斥当前 ZXS 二进制与参与同一协议的 helper，**不证明旧二进制、Samba/passdb、配置管理系统或任意 root 写者已停**。外部停止复选框只是操作者声明，不提升成安全证明；账户、程序与路径在写前/阶段边界重核，无法支持其他 root 同时改身份、挂载或加载配置。

## 永久意图、跨目录部分发布与重试

1. 读取并核验所有目标。已存在不同字节、所有者、权限、链接、类型或长度，一律拒绝，绝不覆盖。
2. 在 `/zxs/.zxs-helper-config-intent-<摘要>/` 发布完整 `plan.json`、准确 unit 与可选 Samba 正文。该 0700 目录中的文件为 0600，复用既有 pending/目录 no-clobber 和同步语义；意图持久化成功后才写 `/etc`。
3. 逐个文件原子新建：写临时文件、chmod 0644、文件 fsync、`RENAME_NOREPLACE`、持有父 fd 同步。已存在准确文件重新核验并同步，不把它说成新建。
4. 所有文件及输入仍一致后，在 `/zxs/.zxs-helper-config-installed-<摘要>/receipt.json` 持久化完成记录，最终再核目标。只有这一路返回 `configuration_files_durably_installed_not_activated`。

**这不是跨目录原子事务。** unit 可能已经发布而 Samba 文件或完成记录失败。错误保留原摘要、意图位置与具体发布错误链；rename 不确定、已发布但目录同步失败都不声称回滚。保留文件与 staging 证据，仅按准确原请求重试；准确已安装文件会重新同步，补完缺项后再落完成记录。内容冲突、双名称或异常 staging 仍拒绝保留，不清理用户或未知材料。因为此片不替换旧文件，未伪造旧配置备份/回滚能力；未来替换功能必须另建持久备份与恢复。

完成记录只证明那次准确文件集合持久化，不是当前服务状态、有效语法、数据库/磁盘准入或系统就绪凭据。任何后续重试都会重新核验真实文件。源码的 `service_reloaded=false`、`service_started=false`、`autostart_enabled=false`、`samba_default_configuration_changed=false`、`samba_passdb_changed=false`、`system_initialized=false` 必须保留。

首装的固定运行目录与独立空导出目录现有单独的[准确准备入口](helper-runtime-directory-preparation.md)。配置文件安装不会隐式调用它，也不把它的回执当作服务可用或整体 ready。

## 工作预算与未完成项

单次处理最多两份 64 KiB 配置，每份只做固定数量的安全打开、写入和文件/目录同步；意图/完成目录沿用各自目录发布屏障，不能用跨目录批量同步删掉必要持久化。完整 passwd/group 各有 4 MiB 上限，程序验证按内置 256 MiB 上限；阶段重核仍是固定次数的 O(程序字节数＋身份文件字节数)，非前台热路径，不宣称增量或零复制优化。

本配置文件阶段本身仍不完成：管理员/首批磁盘初始化、VFS 与 OS 依赖安装、Samba passdb 初始化、真实目标环境整体 `systemd-analyze`/`testparm` 校验、已有 unit/drop-in/自动启动状态治理、明确 reload/start/enable、旧配置备份替换与回滚或完整 clean-start 权威交接。运行目录和本地 passdb 重置各有独立后继，不等于完整部署闭环。**没有把文件安装降低为“服务已配置并可用”，也没有删除现有尚未迁移的安全初始化入口。** 测试源码仅新增而未运行，文件发布正向用例在非 root 测试进程只验证拒绝边界，不能算真实 root `/etc` 安装验收。
