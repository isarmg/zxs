# 辅助工具：既有本地 Samba 密码重置

基于 `eafb997f`。这是独立 Web 的真实 Samba 本地改密代码路径，不是把 ZXS 管理员重置改个名字。按要求只写源码，没有编译、测试、审查、CI，未运行系统工具、修改真实密码或主机。尚无真实 Samba 或浏览器验收。

## 明确支持范围

`POST /api/credentials/samba-preview`、`samba-reset`、`samba-status` 使用现有同源/CSRF/IP Host、单作业 blocking owner 和退出排空。工具仍手动 root、无登录、默认 `0.0.0.0:9040`、明文 HTTP，无自启。网络可达者可以改密，页面和日志提示已扩大；仅可信隔离网络使用，工具不修改网络设置。

操作者先提供与已安装配置完全一致的服务账户、主组、监听和 Samba 设置，并确认已停止 Samba 与其他账户/配置管理者。沿用现有安装准入：程序必须匹配内嵌 ZXS、unit 与独立 `/etc/samba/zxs.conf` 必须全部已存在且正文准确；不安装或覆盖缺失文件，不选择 `/etc/samba/smb.conf`。Samba 账户必须是明确的既有本机非 root 用户，并与私有服务 UID 分离。

当前仅支持独立 `server role = standalone server`、`tdbsam`（或明确相同路径）、固定 `/var/lib/samba/private/passdb.tdb`。`testparm` 的实际参数必须证明 private dir 正确，Unix password sync 和 PAM password change 均关闭。固定 private 目录 root/0700，既有 passdb 非空、root/0600、普通单链接；链接、缺失、其他目录、LDAP/远端/自定义 backend 拒绝。**不创建 passdb/Unix/Samba 账户，不修复其权限或迁移旧 ZXS 格式。** 有些发行版的程序/私有目录布局不同，本片明确不支持，不猜测备选位置。

## 系统程序边界

helper 专用 `system_program` 只允许 `/usr/bin/testparm`、`/usr/bin/pdbedit`、`/usr/bin/smbpasswd`。沿受保护 `/usr/bin` 目录持有文件 fd，核 root、普通单链接、非 setid、不可被其他用户写、可执行 ELF，最大 64 MiB；记录 SHA-256/身份/长度/时间并在使用点重核。实际 exec 使用持有的 `/proc/self/fd/N`，不是可替换的 PATH 名，也不执行 shell、脚本或请求指定命令。

清空环境后仅给固定 C locale、固定系统 PATH/HOME；独立进程组。查询 stdout 最大 16 KiB，超过后排空并失败；stderr 不转发。改密 stdout/stderr 都不保留，新密码只写两行 stdin，argv/环境/计划/回执没有密码；不调用 `pdbedit -w` 或任何导出 NT/LM 散列的选项。秘密 JSON 解码沿用管理员入口，错误不展开秘密值；浏览器提交后和离页清空输入，不能保证所有浏览器/内核/第三方库副本已擦除。

系统 Samba ELF、动态库、模块和 NSS 仍是部署者信任的 OS 组件，文件摘要不是供应链签名。官方命令选项依据 [smbpasswd](https://github.com/samba-team/samba/blob/master/docs-xml/manpages/smbpasswd.8.xml)、[pdbedit](https://github.com/samba-team/samba/blob/master/docs-xml/manpages/pdbedit.8.xml)、[testparm](https://github.com/samba-team/samba/blob/master/docs-xml/manpages/testparm.1.xml) 文档；没有运行它们来验证目标系统的具体版本。

## 准备、提交与耐久结果

预览运行 `testparm` 参数检查，但不打开 passdb 读取凭据；只读取数据库元数据。预览绑定准确安装上下文、用户快照、数据库身份/大小/ctime/mtime 与三个程序摘要，再结合新的 128-bit 操作 ID 得到请求摘要。预览明确账户存在性仍待提交前检查，不声称 Samba 登录可用。

确认 `reset-existing-local-samba-password` 并手动输入两次新密码后：

1. 重新建立同一上下文并匹配摘要，持有当前 `/zxs` 生命周期租约、安装父目录锁及 Samba private 目录建议锁。最后一项**不是 Samba TDB 的 OS 事务锁**，Samba 自身事务由其工具管理。
2. 先不可覆盖地保存 `/zxs/.zxs-helper-samba-reset-<摘要>/plan.json`，不保存密码或派生密码校验器。
3. 以 `pdbedit --list --user` 查询准确单个账户，要求名称和 UID 与当前本机账户相同，不获取密码散列；意外数据库元数据变化（包括工具自身维护）会拒绝继续改密。
4. 调用 `smbpasswd -L -s -D 0 -c /etc/samba/zxs.conf <准确用户名>`，新密码只经 stdin。不带添加、删除、启用、禁用、空密码、远端或 LDAP 选项。
5. 只在子进程成功退出、原配置/路径/程序仍成立、准确账户读回成功，且同一 passdb 文件及其父目录同步后，写单独完成回执。该回执明确 `password_authentication_verified=false`、`active_smb_sessions_revoked=false`、`zxs_administrator_changed=false`、`service_started=false`、`complete_installation_ready=false`。

当前 host 租约互斥参与协议的 ZXS/helper，不自动锁住 Samba、旧二进制或外部管理员；复选框不是这些进程已停止的证据，不支持同时由其改账户、配置、程序或挂载。工具不会偷偷执行 systemctl stop，也不会把一次程序成功当作所有服务可启动。密码变更不会主动撤销已建立 SMB 会话，也不主动解除账户禁用状态。

## 中断处理

外部 Samba 命令没有本工具的持久幂等令牌。因此已有原意图绝不自动重放，含 spawn/管道失败、非零退出、响应丢失、写后同步失败或无完成回执。缺少可读原计划也不证明未发生；不能恢复整个旧 passdb 来“撤销”并覆盖别的账户。

`/samba-status` 只读私有保留记录，不执行 Samba 工具；完整回执只证明那次命令和后置步骤，明确不是当前密码或服务状态。只有意图而无回执返回结果不明、禁止重放。操作者保留原摘要和证据，核实后确需再次重置时，重新预览一个新请求并重新输入/确认密码。**本片提供中断结果查询，不提供密码命令的自动续作或自动回滚。** 子进程在正常关闭/客户端断开后仍被等待回收；故障内核/工具可能阻塞，不能用短超时假称持久动作已取消。

## 仍未完成

其他 Samba backend/布局、创建账户、真实网络登录验证、已有 SMB 会话撤销、Samba 服务的强制生命周期协议、依赖/VFS、systemd/Samba 配置激活与完整首次就绪仍缺。当前执行路径不会 reload/start/enable 服务，helper 不注册自启。测试仅包含输入/解析/来源拒绝与秘密错误隔离源码，未运行，不是 root/Samba/掉电验收。
