# 专用 Samba 实例的首次域供给（宿主契约 1）

适用 Debian 13 amd64。helper 只在独立获准的**全新专用实例**中调用 Samba 自己的实现产生本机 SAM 域，不能给定/导入 SID。ZXS 将来在同一运行的 standalone smbd 中重核域和实例，并验证实际用户、凭据代次和认证会话。两者都不以域记录证明有人能登录。SQLite 仍唯一保存 Web 管理员和 SMB 用户；这里不放密码、等价验证值、账号清单或对用户的授权。

## 实例和原始凭据

- 先由本机管理员核原 PoolId 和 SYSTEM 原目录、取得与在线业务相斥的 EX，并另批此次实例供给。宿主维护者独占其明确选定的安装目录，不能使用全机 Samba 配置或搜索 `secrets.tdb`。新发行一个非零随机 InstanceId 和一个固定 ProvisionOp。生产配置文件、专用 private/state 原目录都必须在可信宿主配置中有唯一、可核对的位置；整个运行期间持原物、重新核名字与原挂载，未明来源不写。不能把挂载编号当跨重启凭证。
- 宿主将已核同一实例的原 private/state 投射为 **/instance/private、/instance/state**；安装时只有此次被批准的空 private 可写。确切引导配置 [domain-bootstrap.conf](../resources/samba-domain/domain-bootstrap.conf) 没有 include、共享、访客或备用 passdb；其中 private dir 指向同一原目录，非工作组名（ZXSWORK）的本机 NetBIOS 名固定 ZXSLOCAL。引导配置用于生成本地域，**不代替**届时上线的有效 smb.conf；两份配置必须指向同一实际原 private/state、固定本机名和服务器角色，上线的唯一用户后端是 `zxs_sqlite`，数据共享才用 `zxs_file`。引导或域供给不加载这些后端、不建立用户库。 首版正式配置也不允许 include、config file 重定向、registry 配置或动态用户共享；原正式配置变化须另获宿主维护授权、更新该实例绑定，不能按眼前文件自行重算放行。共享启停及只读/读写模式属于 SQLite，不随每次更改重写 smb.conf。
- [私有 guest 源码](../resources/samba-domain/fresh_guest.py) 的三次官方调用依次是固定 `set_smb_config`、`set_secrets_dir`、`get_global_sam_sid`。这是**有写的首次供给**，不是只读发现。只能用于全新 exec 的受限非 root 进程，不能复用解释器、服务进程或有旧全局状态的 fork。运行环境须由受信宿主提供：唯一 apt 认证的软件及 Python 模块闭包、没有用户包/脚本、网络/IPC、外部描述符、宿主 /etc 或 /var；只显露已选定的原件。受关注的目标是同一次原供给而非任意路径、参数、布尔值或子进程的 stdout。本片未接宿主启动者。
- 原 private/state 必须是各自确切的空目录。遇 `MACHINE.SID`、空/既有 `secrets.tdb`、其他文件、同一目录别名或起因不明即止，不读取、删除或迁移。Samba 会在指定 private 下创建并管理自己的 0600 `secrets.tdb`；预计只含此实例的域所需状态，不对它遍历或导出。进程内的第二次 getter 会命中缓存，`get_domain_sid` 又查工作组名，都不能冒充持久读回。任何错误/崩溃留下已产生的原状态；无已确认的绑定不自动重跑、不重新随机造域、不接管。

## 非秘密交接

客体的正常结果为 `b"ZXS-SAM/NEW-1\0" || b"ZXSLOCAL" || SAM域SID[24]`。域是 Windows 标准二进制 SID：version 1、4 个 subauthorities、authority 5；4 个 u32LE 为 21、x、y、z，最后三项不全零。拒多余/截短/用户 SID/全零域。生产者只能取上游这次实际返回的 SID 对象；解析相同字节**不产生原实例证明**。传输限定为宿主为这次子进程独建的本地 SOCK_SEQPACKET：固定结果旁仅传一个指向该新文件的 O_PATH，不读或输出库的内容。内核在交付期间持有 inode；宿主还从独立选择的原 private 固定名取得另一 O_PATH，核同时持有的实体、权限与宿主原挂载。接收方按内核报告的发送进程、UID、GID 对照独立监督者的原 guest 身份；同账户的其他进程或 bwrap monitor 的 PID 不算。多余消息（包括空消息）、可读文件 FD、变名或同字节替件都拒绝。通道 EOF 不证明子进程正常退出、原文件同步或原单已批准。固定 SMB 用户的 28 字节 SID 由同域追加 RID 1000 并将个数改为 5；它不授予共享或 Web 管理权限。

最终的宿主绑定正本（拟接，**此批不签出**）是一个宿主保护的只读普通文件，位于独立的宿主安装记录中，不在 SQLite SYSTEM、Samba 私有库或数据卷。固定编码（无宽字符串或对齐）：`b"ZXS-SAM/BOUND-1\0" || PoolId[16] || InstanceId[16] || ProvisionOp[16] || live_configuration_sha256[32] || b"ZXSLOCAL" || domain_sid[24]`。三个 ID 非零；摘要是**明确原上线配置**的完整精确字节，不是 guest 的引导配置，不能通过更换实际 private/state 来复用。原物路径由可信安装单独选择，不从这串字节反解或搜索主机。新绑定发布前须由宿主核被调的实际发布/进程和正常退出、完整唯一客体结果、再无该写者；同步实际新 `secrets.tdb` 及原 private 父，并对原配置、目录、实例重新核验。只在原批准位置不覆盖地发布且同步父目录，最后重开核准确原记录。任一项失败保留现场，仅报未确认。完整物理复制或可信宿主被替换，单靠这些字节无法证真。停机只能核这一份已保护的既有期望；上线须比较同一受管实例内真正的 SAM 名与域，无法证实则拒会话。

## 精确来源与尚未交付的门

[Debian 的 Samba 源包](https://packages.debian.org/source/trixie/samba)为 `2:4.22.11+dfsg-0+deb13u1`。本片静读对应 [dsc](https://deb.debian.org/debian/pool/main/s/samba/samba_4.22.11+dfsg-0+deb13u1.dsc)、原归档和 Debian 补丁：orig 25,620,416 B、SHA-256 `4379aa4bc6c453bfa55697556d6195ab69b1cad71e5fdfaf2c53849d96d40f0a`；Debian 189,836 B、`0359a3242bcbd14f74994a195084f7a8ea28bf9d1287c0021921c40b7d5d7a03`。`source3/passdb/{py_passdb.c,secrets.c,machine_sid.c}` 对应三入口、按指定目录建立原 Samba 状态及本机 SID 事务；24 项补丁中，`usershare.patch` 改默认用户共享数量，所以原配置明确设为 0。上游或其他 Debian 版本不自动放行。

本批只有客体端实际调用源码及独立编码/拒绝样例，**未调用、未生成域、未开放命令**。宿主批准和真隔离启动、发行键到二进制/依赖的验签、进程组清理、原件同步和无覆盖绑定的现场核验仍须接线；不能凭来宾自报的版本、退出码或 SID 冒称做到。未分发/执行 Samba，没有安装包、登录、拒绝访客的实机测试或可用首设声明。
