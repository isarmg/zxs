# 专用 Samba：启动守卫和接入边界

目标是 **Debian 13 amd64 的专用 standalone 实例**，不是把文件系统目录直接共享给 Windows。helper 装配实例并在单独获准的首次供给中初始化其 SAM 域；ZXS 在线消费该实例的认证结果并实现 FILE 业务。唯一用户权威仍是 SYSTEM 的 SQLite：

- Web 管理员不能登录 SMB；SMB 用户不能登录管理页。禁止匿名、guest 和错密码后转 guest，包括匿名 IPC$ 登录和共享枚举。
- 每个共享只取当前 SQL 设定（停用/只读/读写）。一个已成功的 Samba 身份不等于任意共享可用，更不等于可直接打开宿主路径。

## 源码已接到哪里

该私有源码覆盖由[Debian 原文件补丁](debian-trixie/0001-managed-standalone.patch)和[两个新源文件](debian-trixie/new/source3/param/)组成。组装时将 `new/` 下这两个文件按相对路径放入同一已验证的专用 Debian 源树，再应用原文件补丁；Waf 已列出实际源及 tini/GnuTLS 依赖。缺件不构建，没有回落系统 smbd 的第二条路。没有新增网络服务或自写 NTLM：

| 实际切点 | 这段代码能判的事 |
| --- | --- |
| smbd 首次加载、配置热重载及认证路径的直接重载 | 用 Samba **实际装入的有效配置**核 `ZXSLOCAL`、指定 private/state、standalone/user、NTLMv2-only、拒外部 Kerberos keytab（standalone 的原 GENSEC 因此禁用 Kerberos）、强制签名、禁 guest 回退、`restrict anonymous=2`、禁别名/用户共享/打印/多通道等。逐活动 share 核无 guest、用户替代、wide links 或用户/root 连接/关闭脚本；生成的 IPC$ 只读且不加 VFS 模块，拒只有默认 VFS 或另带模块的业务共享。Samba 固有磁盘 VFS 仍位于模块栈底；真正的 `zxs_file` 必须明确接管各业务操作，不支持的不得透传到底层（尚未接线）。唯一守卫在私有 `lp_load_with_shares` 本身；`reload_services` 及认证的 `auth_generic.c` / `auth_ntlmssp.c` 直接调用同样必经。失败终止当前进程，不依赖调用者检查布尔返回。 |
| 原生 SAM 域 | 只读、无创建地打开该实例的专用 Samba 状态；只取 Samba 原生接口给出的现有本地域 SID。没有域/类型不符则启动失败，不查旧 `MACHINE.SID`、不生成域、不修复。启动、fork 后业务子进程正式接手前、实际重载及真实登录的终态门从同一专用 TDB 重新取原域，与受管绑定的 24 字节逐字段比较；同形另一域或同名替换的 TDB 都不通过。这仍不是对 SQLite、该次用户认证或 SID 唯一硬件身份的证明。 |
| SMB2 初次登录 | NTLM 只选现有的 `sam` 路径，不选匿名、Unix 或忽略域的后备。原 GENSEC 成功后再拒绝非普通用户或不属当前本地域 RID 1000 的主 SID；拒绝旧会话替换、多通道绑定及给已认证会话重认证，未完成的同一次多轮 NTLM 握手不当成重认证。仅为拒绝门，不能生成业务 `SmbSession`。 |

这不是可用的 Windows 登录程序。尚未提交这套专用实例的 `zxs_sqlite` passdb、`zxs_file` VFS、准确认证尝试到成功 SMB2 session 的凭据代次传递、逐次 SQL 共享授权和现场验收。不会把操作系统用户名、裸 SID、Samba 日志或来源不明的 socket 当作成功认证。当前库级 FILE 夹具代入已认证事件，不替代上述任一项。

### 受管原件怎样交接

helper 供给的 `/instance/domain.bound` 正好 128 字节：`ZXS-SAM/BOUND-1\0`、三个 16B 标识（Pool/Instance/供给原单）、32B **上线**原配置的 SHA-256、8B `ZXSLOCAL`、24B 本地域。这不是秘密。两个普通原文件 `/instance/domain.bound`、`/instance/smb.conf` 必须真正只读投射；`/instance/private` 与 `/instance/state` 是宿主选定的两个不同原目录。本机宿主先确保来自该次已批准的安装正本并在隔离配置中核池/实例/原单；只读标志、文件属主、这些字节本身都不构成宿主授权。ZXS 不自行挂载或供给。

私有 smbd 只接受固定前台启动方式，不接受替代配置环境或额外 `--option`；对同一份已持 BOUND 及普通 live 文件，先核类型、权限、现场原物/挂载和原配置摘要，再以这**同一缓冲**送 Samba 原始 `tini_parse`（不另开路径、不重写配置解释器）。配置需 1..1 MiB、没有内嵌 NUL；拒 include、config file 重定向、注册表配置或动态注册表共享在解析时加载其他来源。原约定的两个目录、两个文件和真正具名、单链接 0600 的专用 SAM 状态持续同物。只用 dbwrap 提供的原 TDB 现场标识与当前受保护名字相比，不额外打开 TDB 来算摘要或枚举键。子进程继承原描述符、只用带偏移读取。真正的运行资格切口单独复验域；核固定私有 TDB 时才短暂用 Samba 已有的可信服务身份，及时恢复，不给客户端任何宿主路径/SQLite 权限。普通 SID 格式化可用已核不可变缓存，不签新登录。原物失效干净终止该专进程，不对既有上游调用者泄出 NULL、也不以 panic 或 core 当正常拒绝。运行中换挂载/原件、改配置、替换 BOUND 或缺域都不能用新名字刷新信任。需要变更由宿主另行授权、替换并重新启动；不干扰管理员通过 SQLite 改共享 RO/RW。

**接线顺序。** 宿主须先持有经批准、互相吻合的 PoolId、InstanceId、ProvisionOp、上线配置原件 SHA-256、`ZXSLOCAL` 和原 SAM 域；引导配置/工作组名不是上线原件。启动端必须核实际加载的专用实例、配置/依赖、private/state 和 SQLite 内该域 + RID 1000 的 SMB 行，之后才接受连接。禁不受控 include、配置重定向及任意插件。仅看到配置字段、读到域或对完整拷贝算得相同摘要不足以鉴别宿主。当前已接受管配置/原目录与 SAM 域这部分；Pool/Instance/原单的独立在线对证、同池 SMB 凭据核验和会话交接未齐，不能据此启用对外服务。实际口令验证取用的**第一份** SMB 行要在本次认证上下文中绑定 Pool/AccountId/SID、凭据原操作及代次；后续重查用户名或凭 SID 命中缓存不得替它签发。通过账户策略、真实非 guest 安全令牌并成功发布这一 SMB2 session 后才向对应 tree 打开 FILE。无相应证明的恢复/新通道不受理。

## 文件并发要求（未接 VFS）

**只限制单写，不额外禁读。** 跨进程、路径别名及不同共享，写者键是 `(PoolId, FileId)`；拥有者是真正成功打开的那一笔业务 handle，不是账号、连接、共享名、底层 RAW inode 或当前物理表示。创建/接管写句柄先验当前 RW 和 SMB 原生 ShareAccess；存在别的写者时写类打开明确拒，不降为只读。同一 FileId 的写、截短、删除、覆盖/替换目标不能绕过。可在 CREATE 返回句柄前已经截断的 OVERWRITE/SUPERSEDE 必须在第一个副作用前准确暂占，失败只放本次占位。DELETE_ON_CLOSE 的设置和最终 unlink、服务器 COPYCHUNK 的目的端也受同一限制，不支持就明确拒绝。不同 FileId 不相互上锁。

读句柄仅按其自己的权限、ShareAccess、字节范围锁及当前已确认修订处理；**不因存在写者额外拒读**，不读别人还没确认的编辑。写者自己的正常读写语义不变。SMB 共享访问模式控制读/写/删除的兼容性；字节范围锁保护具体范围；协议缓存 lease/oplock 不等于前两者，也不等于业务单写锁。不得以一把长期 SQL 写事务或整盘锁实现单写。

- 同账号三次独立打开：A 持兼容读取的写句柄，B 能读已确认稿，C 请求同 FILE 第二写被拒；换共享路径、物理表示或另一个 smbd 进程结论不变。若 A 的**原生**共享参数明确拒读，则 B 也应被拒；无关 FILE 可写。
- 同一 FILE，原生读/写/删除共享各按请求核，字节锁分别验重叠、不重叠、显式解锁和关闭。临时文件＋改名替换是常规应用保存：目标句柄已关闭且没有活写者时，按稳定顺序取得所需源/目标资格，核当前 RW、基线及两端原生 share/delete 条件，准许正常替换，不要求继承已结束的句柄。若目标仍被写者占有，只允许能证明正是该目标 owner 的操作；同账号、进程或连接不是证明。失败不留下半替换。
- 正常 CLOSE 或确认断线收尾，先使旧句柄不可再改，排空或使已派发异步写/待提交 SQL 失效，再交下一写者；未实现 durable/reconnect 时明确拒，不用重连/同账号绕过。读者持确切已确认材料到交付结束，不能让后台换成 Raw 后重新按 inode 独立加第二把写锁。
- 对已绑定基底的保存，在短 SQL 提交中核前修订和原操作，过时即冲突；同单重送不重复 SAVE。客户端关闭后另开新句柄、用应用自己保留的旧内容整份写回，没有通用凭据能把它认成旧基底。要保先前编辑仍须按真实客户端行为验证。

SQLite 首份手动 SAVE 的真实库路径已分开准备写者与读者：容器 EX 只约束本次原生准备写者，Current 不参与该写锁，仍持出生头/真实已确认材料并重验当前读权。新开读在未确认时只选空的前修订，确认后新开读选修订 1；原已开空稿句柄不升级、不阻止确认。源码用例跨两个独立 SQLite 连接，在真实源文件已同步/未提交时检查此区别。只覆盖这个首稿，不是跨进程 SMB 逻辑写句柄、原生 ShareAccess/字节锁、后续修订或 Windows 实测。

## 来源与验收

- 正式输入是 [Debian trixie 的 samba 源包](https://packages.debian.org/source/trixie/samba) `2:4.22.11+dfsg-0+deb13u1`。按 [Debian .dsc](https://deb.debian.org/debian/pool/main/s/samba/samba_4.22.11+dfsg-0+deb13u1.dsc) 核对 orig 25,620,416 B 的 SHA-256 `4379aa4bc6c453bfa55697556d6195ab69b1cad71e5fdfaf2c53849d96d40f0a`，Debian 源补丁 189,836 B 的 SHA-256 `0359a3242bcbd14f74994a195084f7a8ea28bf9d1287c0021921c40b7d5d7a03`。[九个原文件摘要](debian-trixie/inputs.sha256)绑定本补丁输入；其中 `source3/param/loadparm.c` 的输入已含 Debian `usershare.patch`（默认 0→100）；ZXS 仍要求有效值为 0。其他列出的 Debian 原文件未被该补丁集更改。当前仅来源字节与补丁静审，**没有**替代 Debian 信任链验签；最终发行应验镜像/包签名及修补支持。源版本或补丁改变，必须重新独立适配，不换回任意同名 smbd。
- 此补丁只针对隔离的专用构建及其私有 Samba 库，**不得替换**宿主发行版的 smbd 或共享库；helper 首次供给使用它自己核定的正常 Samba 供给接口。补丁按原文件的 GPL-3.0-or-later 条款，见 [COPYING](debian-trixie/COPYING)。无包安装/编译/运行。
- 待验：截短/多字节/错误域/用户 SID 的 BOUND、错/可写配置，带 include 或重定向的配置，以及相同文本换新 inode、父进程/子进程重新检查时换 secrets.tdb（保留原文件），均拒。没有真实预配时须先拒，未读任何真实密钥；坏配置首次启动及热重载；空私目录不得产域、正确原域启动、同名其他实例不可替代；错误密码与**完全无凭据**的 IPC$/枚举分别拒，Web 用户跨入拒，唯一 SMB 用户从冷 Windows 连接在 RO/RW 上读/写符合 SQL；两独立打开与断线见上。每一项都要在完整受管发行及真实 handler 上取得证据，不能以配置文本、STATUS_OK 或源码审阅冒充通过。
