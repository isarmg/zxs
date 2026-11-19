# 专用 SMB：安装原件预检和起停交接

**这不是已经可以登录的 SMB 服务。** 当前可交付的宿主入口是一项可选的预检：核对安装者指定的两个程序、已有的专用实例和两个实际的只读文件投射。它不启动程序、不打开 445、不尝试登录，不读 `secrets.tdb`、SQLite、口令或业务正文。只有管理员明确安装、启动一次检查单元才读取现场；helper 包只在 `/usr/share/local-ops-helper/smb-host/` 附两份材料，不向 systemd 安装或启用单元。这不是管理 HTTPS 或磁盘 Web 的一部分。

## 哪些材料必须另有来源

- 完成[同一池的专用首设](samba-domain.md)，保留 PoolId、InstanceId、DOMAIN_OP、已核 128 字节 `domain.bound` 的独立 SHA-256 和登记的非 root 存储 UID/GID。域正本、账号回执与批准各有用途，不能取当前查到的字节为自己背书。存储根仅支持以 `/srv/` 或 `/mnt/` 开头的受保护规范 ASCII 原路径；不要复制池以通过检查。
- 由独立签名发行和部署记录取得两个**不同的** Debian 13 amd64 专用 ELF 的确切 SHA-256。该宿主约定把修补过的前台 smbd 安装在 `/usr/lib/zxs-smb-runtime/debian13-amd64/sbin/smbd`，配套 ZXS 存储端在 `/usr/lib/zxs-smb-runtime/debian13-amd64/bin/zxs-smb-storage`。不找 PATH、不用系统 `/usr/sbin/smbd` 或离线创建域的 Python 胶囊替代。单个 ELF 文件摘要**不检查整个动态装载链、NSS、模块或发行签名**；这些必须另在受控独立构建/安装中核准。没有可验证的配套制品不应启用。
- 本片只接受此前的 root:存储组 0750 实例根、存储 UID/GID 的 0700 `private` 和 `state`、root:root 0444 的正式配置及非秘密 BOUND。受保护的原父和进程凭据是站点前提；不要从共享主机的陌生同名文件中生成配置。初装正式配置为已经审定的 1277 字节、仅回环端口、无数据共享版本，不是域初始化配置。既有 ACL、非本地权限、特权方并发换挂载/文件或未核的软件更新不在此片可证明范围。

## 准备一个可检查的本机单元

在普通用户自己拥有、其他人不可写的工作目录，用已核发行里的脚本。参数均非秘密，不能传口令、SID、任选命令或任意程序路径。

```sh
/usr/bin/python3 -I -S -B /usr/share/local-ops-helper/smb-host/check.py render \
  "$SYSTEM" "$POOL_ID" "$INSTANCE_ID" "$DOMAIN_OP" "$STORAGE_UID" "$STORAGE_GID" \
  "$BOUND_SHA256" "$DEDICATED_SMBD_SHA256" "$SMB_STORAGE_SHA256"
```

只排他新建 `./zxs-smb-preflight.service`；已有名不覆盖。所有数字和摘要由该站点确认，不通过运行未核的服务探测。管理员另行按独立原件复核候选的完整字节，确认 unit 名、别名和 drop-in 均未被使用，并以 root:root 0644 放进受保护的系统 unit 目录。不从可写工作区直接链接，不替换既有宿主 Samba。`daemon-reload` 与手工启动 **zxs-smb-preflight.service** 是管理员另行决定的部署动作，不是生成器行为。该 unit 无 `[Install]`、`Restart=no`，没有启动常驻服务的依赖，也不用网络。

现场检查以真实存储 UID/GID、空能力集、NoNewPrivileges 运行。systemd 在该次服务的私有 tmpfs /run 中只读 bind 两个公开原文件；脚本核确切 `ST_RDONLY`、不同挂载且与原名仍为同一活 inode。全文件逐字节校验协议和摘要；对原 SYSTEM 与原 `private/state` 只看目录元数据，对两个固定程序 ELF 先核类型/保护再限 128 MiB 完整读到 EOF。输出 0 只说明此次观察符合；脚本已进入检查而遇到缺物、替换、同字节不同投射原物或查询失败，返回 2。若必需源不存在、挂载不可建立，systemd 可以先报命名空间准备失败，此时脚本没有运行。不会输出 SID、凭据、文件路径或原始报错。这个临时检查进程结束即释放全部原件和投射，**不是之后启动的授权、跨重启身份或长租约**。

## 谁实际提供 fd 3/4/5，何时可以停

这部分是给后续**唯一宿主启动主管**的实现/验收契约；上述一次性检查不充当主管。不能拼写两条 systemctl ExecStart 就声称交付了这些内核对象，也不要现在手工运行两端。必须选实际消费 fd 5 的配套存储发行；过去仅接 fd 3/4 的版本即使有可执行文件也不能用，更不能拿那版声称已核 smbd 原进程。精确源码、独立验签的匹配制品或合格主管缺任一项，保持停用。

|阶段|实际归属和失败行为|
|---|---|
|宿主取得原件|重新独立核原安装、确切专用程序/加载闭包、原 SYSTEM、实例、配置及 BOUND。运行期间保护原件；不接全机 Samba、不改现有服务、网络或本机账号。原配置及 BOUND 在专用 smbd 内分别真实只读投射为 `/instance/smb.conf`、`/instance/domain.bound`，两个原目录投射为 `/instance/private`、`/instance/state`；`cache/lock/run` 是专实例私有，绝不借用全局状态。|
|启动第一阶段|宿主受限 root 主管创建（两端对创建者执行 SO_PEERCRED 时真实可见的 UID 为 0），初始 SO_PASSCRED 和 SO_PASSPIDFD 均关闭；交付**一对匿名、已连接、阻塞**的 AF_UNIX/SOCK_SEQPACKET；fd 3 各自只给一端。启动确切专用程序 `smbd --foreground`，非守护化，不可被包装器/监控进程的 PID 替代。从真实创建的本次子进程获得仍存活的 process pidfd（例如 CLONE_PIDFD；若用 fork 后 pidfd_open 则须保证 SIGCHLD 未忽略、无 NOCLDWAIT，且无人提前回收）。宿主不得按名字或 pidfile 搜进程。这里只等真实启动/exec 与原 pidfd；不能先等监听或 READY 再派存储，否则两边会互等。smbd 的主监听与所有持 fd3 的 smbd 发送进程共享获准的固定 PID 命名空间，其所属 user 命名空间就是这些进程的实际 user 命名空间；宿主和存储端可以在外层，不能拿两边可不同的 PID 数字代替 pidfd；禁止不明组合，CAP_SYS_ADMIN 必须从 smbd 的有效、许可、继承、ambient 及 bounding 集去掉而不能重新取得。|
|启动第二阶段|非 root、无额外能力的 `zxs-smb-storage` 无参数，只获 fd 3（另一端）、fd 4（宿主另核的原只读 BOUND）、fd 5（上述真实专 smbd 主进程 pidfd）。只提供 `ZXS_SYSTEM_ROOT`、`ZXS_POOL_ID`、`ZXS_SAMBA_INSTANCE_ID`、`ZXS_SAMBA_PROVISION_OP`、`ZXS_SAMBA_LIVE_CONFIG_SHA256` 的原选择，不从对端报文填充。关掉监管者和无关进程里的多余副本；不留可连接的控制路径，不给 Web、离线工具或全机 Samba。两端自己的 BOUND/READY 与一次真实接入后每条内核来源核对由 ZXS 负责，READY 不是认证、共享可用或 Windows 登录；宿主不代签。|
|正常停止或失败|专用服务域只收本次进程；请求非 root 存储端正常停止，关闭新请求、半关控制通道，受管 smbd 主/子退出，存储端待生命周期管道全部写端关闭并关库后才释放原 S。宿主需回收真正的两个原主进程并核整域无余留。半关/读 EOF、两个 pidfile、一个退出 0 或 systemd 的 inactive 均不能单独证明所有业务写入已落稳。正常停不清现场，不自动重启、切换另一服务或补办账户。超时、强杀、主机掉电只能报‘未确认正常排空’，独立查原单；后续离线维护仍须在同一 S 原目录独立取 EX。|

许可后待在隔离的 Debian 13 实机分别验：两个实际批准 ELF 与单个换件、错误的 fd 权限与 PID 命名空间、给错 smbd 的 pidfd、前后两种关停以及子进程仍持写端时的排空。此批未构建、安装、自建发行凭据、运行服务或进行 SMB 登录。技术依据：[pidfd_open(2)](https://man7.org/linux/man-pages/man2/pidfd_open.2.html)、[systemd 257 的只读投射](https://www.freedesktop.org/software/systemd/man/257/systemd.exec.html#BindReadOnlyPaths=)、[按受管进程域停止](https://www.freedesktop.org/software/systemd/man/257/systemd.kill.html#KillMode=)。

### 主管内部的原端点交接

代码已能创建一个真实 root 产生的匿名 SEQPACKET 对；在另一端交给存储进程之前，未来的可信主管必须提供它在创建本次 smbd 时取得、仍持有的原 process pidfd，此处不从进程号重开。当前内部检查只能证明进程在世且现在由本主管抚养并可等待（也可能是收养的进程），不能追溯描述符何时取得或据此签发启动许可。它核对该子已经执行专用发行的原 ELF、实际持有此端点且没有误继承存储那端。Samba 会把收到的 3 号描述符复制到 ≥5 并关闭原号，因此核验比较活内核 socket，而不把固定数字当永久身份；只看这个原子进程至多 256 个描述符链接，仅对命中端点读取内核 fdinfo；目标须是同一挂载和 inode 上可读写、阻塞的真实 socket 描述符，只持同一 inode 的 O_PATH 引用不算。其他目标不被数据打开或记录。首版保守要求它是宿主初始 user namespace 拥有的一层专用 PID namespace 的 1 号进程，未来子进程 PID namespace 与之相同、当前五套能力均无 CAP_SYS_ADMIN。通过仅是当时的逐项核验，不抵抗不遵约的特权宿主换件，也不证明所有将来子孙都受限。没有外部调用入口，不启动任一程序、不解释 BOUND 或发 READY。正常启动仍须由完整主管独立交付正确的只读 BOUND、存储身份、专用发行闭包以及进程约束，并在失败时收回自己产生的所有进程。

上线也须把 Samba 的会话状态与持久的专用 SAM 域库/池 SQLite 分开：global sessions TDB 可能含签名、加密或应用会话材料。名称叫 run/lock、权限 0600 或 dbwrap 标 VOLATILE 都不是“不会落盘”的凭据。专实例 cache/lock/run 需要独立易失、经核不可交换的内存挂载（如支持且实核的 [tmpfs noswap](https://www.kernel.org/doc/html/v6.12/filesystems/tmpfs.html)），并另外排除进程换页、休眠镜像、核心转储及跨启动复用；未核不开放实际认证。本片不更改任何宿主设置。
