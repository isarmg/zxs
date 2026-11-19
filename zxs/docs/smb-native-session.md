# 两个固定账户：Web 看 SQLite，SMB 看专用 Samba

这是新池的唯一目标，不是线上可用说明。Web 密码只由本池 SQLite 的 Argon2id 验证；SMB 密码只由固定 Debian Samba 发行的本机 **tdbsam** 验证。SQLite 保留两种身份的非秘密绑定、操作代和每个共享的权限，**不含 SMB 密码、NT 验证值或其备份**。上游用于本次 SMB 签名的会话材料留在 Samba 内。没有密码校验或取密 IPC。现有 v6 源码仍有 `smb_password.nt_owf`、配置仍选 `zxs_sqlite`、正向回调仍拒绝；它们不是新格式的兼容路径。只准两端共同切换新格式，不导入已有凭据。

## 什么有权产生一次 SMB 身份

1. 宿主从原安装记录选定同一池、实例、配置和实际私有目录。唯一允许的有效配置值是 **`tdbsam:/instance/accountdb/passdb.tdb`**。这里的原生 location 固定在专用发行中，不接受默认路径、任意参数、别名、动态模块或多后端。原宿主的 `accountdb/` 与 `private/` 同级：受控上级不可被替换，前者为 root:root 0700、原 TDB 0600，无额外 ACL、别名或重定向给 Web、存储或 NSS 身份读写/换名。只投射给获准的原生发行者和专用 smbd；不能把“未投射”当成宿主 DAC 隔离。原先由存储 UID 参与的机器域私目录不放用户库，不自动改旧目录权限。现有 `secrets.tdb` 保存本地域事实，不保存 ZXS 用户密码，也不是备用用户库。独立核原 BOUND、加载配置的实际字节和 SAM 域；不能沿名字搜索其他 Samba 安装。修改上线配置需新的宿主批准与相匹配的 BOUND，旧配置摘要不能复用。专用 Samba 不打开业务 SQLite；非 root 存储、Web、浏览器都不打开 passdb。
2. 帐户只能是本池受管的非特权 SMB 用户：原实例、域 `ZXSLOCAL`、域 SID 加 RID 1000、规范登录名和活动凭据代必须准确匹配。[首设/恢复](first-account-v1.md)规定唯一原记录。没有 SAMR/远程改密、临时创建、系统 Unix 用户、Web、guest、匿名、空密码、trusted-domain、Kerberos/Unix 后备。协议错误不映射来宾；即使没有凭据也不能枚举 IPC$。Samba 启动所需的内部 SYSTEM/guest token 不产生网络登录或 ZXS 权力。专用 NSS 身份 `zxs-smb-client` 仅用于执行：固定真实非特权 UID/GID、名字及获准组；不以客户端名字查宿主任意 NSS、不借 root/nobody 或认证缓存。
3. 仍是原服务器派给该客户进程的 TCP 连接、这一次真正的 SMB2 新会话及其唯一原 GENSEC。现有私端只负责有界的非秘密挑战、原实例与 SQL 代次切面。原生本地域 SAM 在 smbd 内拿该账号的第一份真实 SAMU，确认原名/真实 SID、当前代标记和正常账户策略，用它走该次严格的标准 NTLMv2 核验及 GENSEC 原生后验；再次取账户只能复核同一绑定，不能给早先核过的密码补签新代。不得恢复上游通用域/LM 兼容成功、禁用或受限账户、调试转储密钥或证据的路径。新域/用户名/标记的查询本身不是成功。Samba 处理原生协商完整性、签名和必要的会话密钥，不用一个布尔结果或 SID 冒充。
4. 只在这次 GENSEC 最终成功、收到同一成功对象，原生普通非 guest token 的业务 SID/登录名与固定低权 Unix 映射均符合，且原 SQL 活动映射依旧同代时，才可让此请求继续。仅在真实新会话 `smbXsrv_session_claim` 后那次 `smbXsrv_session_update` 成功，才将非秘密身份来源附于本进程**这一个**会话；中途的 update 不算。另一个进程、会话号、SID、绑定通道、旧 durable 重连及未实现的重认证均不补发。后续 TREE/FILE 操作仍要按原会话、当下 SQL 代及共享 RO/RW 重验；定义一个共享本身不授读权。业务层单写仍按池与逻辑 FileId，只排其他写者，不额外禁读。
5. 标准 Samba 的 `smbXsrv_session_global.tdb` 含必要的短期协议密钥，不是账户权威或可重新领的 ZXS 登录证明。保留既有受管进程、受保护易失运行目录、禁止密钥/报文诊断以及宿主转储/换页边界；停机排空、重开新运行层，不把运行状态持久备份为另一个凭据库。授权算法在实际宿主不允许就关闭，不松开策略代替。

## 原生接口与上线门

固定源是 [Debian 13 Samba 2:4.22.11+dfsg-0+deb13u1](https://packages.debian.org/source/trixie/samba)。`source3/passdb/pdb_tdb.c::pdb_init_tdbsam` 选路径；其 `tdbsam_open` 默认会创建/转换数据库。**在线打开必须无 CREATE、无转换/备份并核原受保护目录和准确原 TDB/可信主/次版本两项**确实存在且均准确**为 4/0**；仅靠预检存在再调用会自动创建的普通入口不够。只允许该实例这一业务用户；原生账号读取、必要的受限登录计数和账户策略不能演化成可远程创建、修改或重置凭据的通道。默认管理方法、命名管道和旧 `zxs_sqlite` 并不因此获准。实际 passdb 仍要在重新打开/工作进程继承时核原物，服务不能靠缓存顶替当前原物。

原 `source3/auth/check_samsec.c` 对同一登录会先后取两次账户；`auth/ntlmssp/ntlmssp_server.c` 负责随后 NTLMSSP 后验。`source3/auth/server_info_sam.c::make_server_info_sam` 默认用 SAM 用户名查 NSS：若业务名与固定 Unix 名不同，不能原样当成已支持；只在原生验密的这一成功对象上使用固定执行映射，最终仍验证普通 token、权限和实际 Unix 名/ID。任何一个门未完成，维持拒绝。正确密码也不能提前放行。

**交付与未运行验收。** 双项目先共审精确的新 DDL、SQL 操作向量、原生 tdbsam 标记、宿主专用目录、授权和配置；在同一次快进切换相互依赖的版本/模板，拒旧族，不双写。之后把已审核的受管 Samba 原生校验接入真实回调，再接已确认的 RO 共享列出/读取。验收分别覆盖：唯一正确身份；未知/错误、清空凭据的匿名 IPC$、Web 身份；第二条普通 SAM 账号、同名异域/旧标；维护前已有会话、锁和在途写排尽且不能复活；仅改 Web 密码与仅改 SMB 密码彼此不牵连；登录成功但共享停用或正向 VFS 未接时仍不能读 FILE。以上是源码/设计边界，未执行或部署。
