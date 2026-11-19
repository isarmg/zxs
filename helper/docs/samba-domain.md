# 专用 Samba 实例的首次域供给（宿主契约 1）

适用 Debian 13 amd64。helper 只在独立获准的**全新专用实例**中调用 Samba 自己的实现产生本机 SAM 域，不能给定/导入 SID。ZXS 将来在同一运行的 standalone smbd 中重核域和实例，并验证实际用户、凭据代次和认证会话。两者都不以域记录证明有人能登录。SQLite 仍唯一保存 Web 管理员和 SMB 用户；这里不放密码、等价验证值、账号清单或对用户的授权。

## 实例和原始凭据

- 先由本机管理员核原 PoolId 和 SYSTEM 原目录、取得与在线业务相斥的 EX，并另批此次实例供给。宿主维护者独占其明确选定的安装目录，不能使用全机 Samba 配置或搜索 `secrets.tdb`。新发行一个非零随机 InstanceId 和一个固定 ProvisionOp。生产配置文件、专用 private/state 原目录都必须在可信宿主配置中有唯一、可核对的位置；整个运行期间持原物、重新核名字与原挂载，未明来源不写。不能把挂载编号当跨重启凭证。
- 宿主将已核同一实例的原 private/state 投射为 **/instance/private、/instance/state**；安装时只有此次被批准的空 private 可写。确切引导配置 [domain-bootstrap.conf](../resources/samba-domain/domain-bootstrap.conf) 没有 include、共享、访客或备用 passdb；其中 private dir 指向同一原目录，非工作组名（ZXSWORK）的本机 NetBIOS 名固定 ZXSLOCAL。引导配置用于生成本地域，**不代替**届时上线的有效 smb.conf；上线映射 `/instance/smb.conf`、`/instance/domain.bound` 两个确切原文件都必须为真实只读挂载。正式配置须非空、≤1 MiB、无内嵌 NUL；明确空的 `usershare path =` 和 `usershare max shares = 0`、无外接脚本。两份配置必须指向同一实际原 private/state、固定本机名和服务器角色，上线的唯一用户后端是 `zxs_sqlite`，数据共享才用 `zxs_file`。引导或域供给不加载这些后端、不建立用户库。 首版正式配置也不允许 include、config file 重定向、registry 配置或动态用户共享；原正式配置变化须另获宿主维护授权、更新该实例绑定，不能按眼前文件自行重算放行。共享启停及只读/读写模式属于 SQLite，不随每次更改重写 smb.conf。
- [私有 guest 源码](../resources/samba-domain/fresh_guest.py) 的三次官方调用依次是固定 `set_smb_config`、`set_secrets_dir`、`get_global_sam_sid`。这是**有写的首次供给**，不是只读发现。只能用于全新 exec 的受限非 root 进程，不能复用解释器、服务进程或有旧全局状态的 fork。运行环境须由受信宿主提供：唯一 apt 认证的软件及 Python 模块闭包、没有用户包/脚本、网络/IPC、外部描述符、宿主 /etc 或 /var；只显露已选定的原件。受关注的目标是同一次原供给而非任意路径、参数、布尔值或子进程的 stdout。宿主固定启动及有界监督已有内部源码；受信发行装配/本机审批尚不能构造启动许可。
- 原 private/state 必须是各自确切的空目录。遇 `MACHINE.SID`、空/既有 `secrets.tdb`、其他文件、同一目录别名或起因不明即止，不读取、删除或迁移。Samba 会在指定 private 下创建并管理自己的 0600 `secrets.tdb`；预计只含此实例的域所需状态，不对它遍历或导出。进程内的第二次 getter 会命中缓存，`get_domain_sid` 又查工作组名，都不能冒充持久读回。任何错误/崩溃留下已产生的原状态；无已确认的绑定不自动重跑、不重新随机造域、不接管。

### 初次正式配置，不是引导配置

[initial-live.conf](../resources/samba-domain/initial-live.conf) 是 1277 字节的首装正式模板（SHA-256 `0154cff9b9d2f9be1f5ab7b374f36ee10530d7d1fe08f72ac2eaf19f32116406`）。仅含 global，只监听回环 127.0.0.1，不经 Avahi 发布 SMB 服务；Samba 自己产生受限的 IPC$，不导出数据目录。受信宿主若要把数据共享开放到局域网，须另行批准对应部署；不能把本模板说成客户端已经能读写文件。所选实例根内正式源文件 `smb.conf` 必须是宿主管理员的 0444、单链接、同一原挂载普通文件。本地发行步骤和日后服务投射分开：宿主侧准确模式不等于服务里已是只读挂载，线上还须独立确认有效的原配置。

在调用域生成前，宿主逐字读该实名原件至 EOF，与编入的模板以及独立批准的正式配置摘要同时核对；保留 FD，直至客体退出、私有库同步和未发布候选输出时仍复核原挂载、实体、名字及内容。不能用引导 profile、另一个安装或只有同名的配置代替。128 字节 BOUND 候选只取这次正常退出并已核原件的非秘密域；没有宿主管理员通过受限父进程的不覆盖发布时，它不是磁盘正本、开户许可或登录证据。

## 非秘密交接

客体的正常结果为 `b"ZXS-SAM/NEW-1\0" || b"ZXSLOCAL" || SAM域SID[24]`。域是 Windows 标准二进制 SID：version 1、4 个 subauthorities、authority 5；4 个 u32LE 为 21、x、y、z，最后三项不全零。拒多余/截短/用户 SID/全零域。生产者只能取上游这次实际返回的 SID 对象；解析相同字节**不产生原实例证明**。传输限定为宿主为这次子进程独建的本地 SOCK_SEQPACKET：固定结果旁仅传一个指向该新文件的 O_PATH，不读或输出库的内容。内核在交付期间持有 inode；宿主还从独立选择的原 private 固定名取得另一 O_PATH，核同时持有的实体、权限与宿主原挂载。接收方按内核报告的发送进程、UID、GID 对照独立监督者的原 guest 身份；同账户的其他进程或 bwrap monitor 的 PID 不算。多余消息（包括空消息）、可读文件 FD、变名或同字节替件都拒绝。通道 EOF 不证明子进程正常退出、原文件同步或原单已批准。固定 SMB 用户的 28 字节 SID 由同域追加 RID 1000 并将个数改为 5；它不授予共享或 Web 管理权限。

最终的宿主绑定正本（拟接，**此批不签出**）是一个宿主保护的只读普通文件，位于独立的宿主安装记录中，不在 SQLite SYSTEM、Samba 私有库或数据卷。固定编码（无宽字符串或对齐）：`b"ZXS-SAM/BOUND-1\0" || PoolId[16] || InstanceId[16] || ProvisionOp[16] || live_configuration_sha256[32] || b"ZXSLOCAL" || domain_sid[24]`。三个 ID 非零；摘要是**明确原上线配置**的完整精确字节，不是 guest 的引导配置，不能通过更换实际 private/state 来复用。原物路径由可信安装单独选择，不从这串字节反解或搜索主机。新绑定发布前须由宿主核被调的实际发布/进程和正常退出、完整唯一客体结果、再无该写者；同步实际新 `secrets.tdb` 及原 private 父，并对原配置、目录、实例重新核验。只在原批准位置不覆盖地发布且同步父目录，最后重开核准确原记录。任一项失败保留现场，仅报未确认。完整物理复制或可信宿主被替换，单靠这些字节无法证真。停机只能核这一份已保护的既有期望；上线须比较同一受管实例内真正的 SAM 名与域，无法证实则拒会话。

## 宿主内部执行边界

- 当前的固定调用只接受专用宿主工作进程。它继续持原 SYSTEM 的 EX，在按独立 InstanceId 选出的专用实例目录上另取 EX，核原 private/state 正好是两个不同的空目录。本片仅支持位于该安装目录同一挂载上的 private/state。固定非 root 主体、认证发布的私有 Python/Samba 用户态闭包、对应的宿主 bwrap 及其动态装载器/库、授权且被保护的宿主安装路径是前提。不能由网页、散列自报或任意目录发出这个许可。升级必须先排空，不能边替换发行物边给新域签名。
- bwrap 0.12 从已核 ELF 的 FD 执行，强制独立用户、PID、网络、IPC 及 UTS 命名空间。只映射固定私有发行树、可写原 private、只读原 state 和两份封印的引导材料；不映射宿主的 /etc、/var、/dev 或已有 Samba 实例。guest 不能分叉/脱离进程组。固定选用 `--as-pid-1`，只从 bwrap 专属状态管道的真实 child-pid 与最终正常状态，加上原 supervisor 的 wait/进程组收尾，确定此次客体。单个 PID、EOF 或 SIGKILL 都不算成功；异常保留已经产生的材料。宿主自身崩溃/被强杀不能作为全域已经退出的证据。
- 正常结束才核客体经内核传来的同一 inode，从原 private 的准确名字核对，实同步原 secrets.tdb 和原父、末端重查身份/属性及 private/state 清单。宿主不读取/转述秘密库内容。至此仍是**未发布**的原实例证据，尚不能签出 `domain.bound`、建立账号或证明登录。

**两个进程交接，不换锁。** 启动客体前，降权 worker 在原 SYSTEM 的 EX 下，用 SQLite 只读核本池确切且已提交的 INIT 及原 S 卷标；关库后才进入 Samba。实例父原目录必须恰有准确拼写的 `smb.conf`、`private`、`state`。若已有 `domain.bound`、临时记录、其他项或不能完整读取，不会为了探测而让 Samba 新建秘密库。

宿主内部另用本次进程独占的匿名 socketpair，不是给用户或上线 smbd 的接口。完成的 worker 一次交接 128 字节公开候选、原 INIT、同步时私库及其父的属性比较值，以及原 S 和实例两个已持 EX 的目录描述符与原库的只指路径描述符。内核持有在途的这两份原锁，不因 worker 退出而改锁或先放再取；接收方按同次子进程的实际未回收正常退出和内核 PID/UID/GID 核唯一消息，从受核当前线程的 fdinfo 确认这两个具体的 open description 各自拥有整文件独占 flock，并以另开原目录的共享锁争用佐证；不能把同 inode 上别人的独占锁算到收到的描述符名下。再对照独立预选的原目录、正式配置及原私库。没有复制或读出秘密内容。只有这份手递结果不授权写宿主安装父；正式发行包、被确认的本机发布步骤和磁盘正本仍未交付。

[固定版本 bubblewrap 的状态与启动实现](https://github.com/containers/bubblewrap/blob/v0.12.0/bubblewrap.c#L2909-L2925)将 child-pid 写到专属通道；不用它代替自己的进程组退出核对。

## 精确来源与尚未交付的门

[Debian 的 Samba 源包](https://packages.debian.org/source/trixie/samba)为 `2:4.22.11+dfsg-0+deb13u1`。本片静读对应 [dsc](https://deb.debian.org/debian/pool/main/s/samba/samba_4.22.11+dfsg-0+deb13u1.dsc)、原归档和 Debian 补丁：orig 25,620,416 B、SHA-256 `4379aa4bc6c453bfa55697556d6195ab69b1cad71e5fdfaf2c53849d96d40f0a`；Debian 189,836 B、`0359a3242bcbd14f74994a195084f7a8ea28bf9d1287c0021921c40b7d5d7a03`。`source3/passdb/{py_passdb.c,secrets.c,machine_sid.c}` 对应三入口、按指定目录建立原 Samba 状态及本机 SID 事务；24 项补丁中，`usershare.patch` 改默认用户共享数量，所以原配置明确设为 0。上游或其他 Debian 版本不自动放行。

已写客体、固定宿主启动、原组收尾及原库/父同步源码，**未调用、未生成域、未开放命令**。宿主批准、完整私有 Samba 发行/验签和无覆盖绑定尚未交付；不能凭来宾自报的版本、退出码或 SID 冒称做到。未分发/执行 Samba，没有安装包、登录、拒绝访客的实机测试或可用首设声明。
