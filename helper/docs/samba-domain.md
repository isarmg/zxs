# 专用 Samba 实例的首次域供给（宿主契约 1）

适用 Debian 13 amd64。helper 只在独立获准的**全新专用实例**中调用 Samba 自己的实现产生本机 SAM 域，不能给定/导入 SID。ZXS 将来在同一运行的 standalone smbd 中重核域和实例，并验证实际用户、凭据代次和认证会话。两者都不以域记录证明有人能登录。SQLite v7 保存两个身份、Web 校验值、业务许可与凭据代次。SMB 密码等价物只留专用 Samba 的原生账号库，**不进 SQLite**；BOUND 不载凭据、不授予登录。旧 v6 不升级、不兼容。

## 本机首设入口

先完成 [install-pool](sqlite-first-install.md) 并保存 PoolId 和 INIT 原单。在可信宿主的同一前台控制终端进入唯一的 `setup-pair` 命令。参数顺序固定，以下是源码中的语法；本轮未打包、部署或采集口令。

```text
helper setup-pair provision --create-layout --instance ID --approval ID
helper setup-pair provision --system DIR --pool ID --init ID --instance ID --op DOMAIN_OP --approval ID
helper setup-pair enroll --system DIR --pool ID --init ID --instance ID --domain-op DOMAIN_OP --op SETUP_OP --approval ID --web-login NAME --smb-login NAME [--receipt-only|--resume]
helper setup-pair reset-smb --system DIR --pool ID --init ID --instance ID --domain-op DOMAIN_OP --setup-op SETUP_OP --op RESET_OP --approval ID --smb-account ID --smb-login NAME --previous-op PREVIOUS_OP --previous-generation N [--receipt-only|--resume]
helper setup-pair reset-web --system DIR --pool ID --init ID --setup-op SETUP_OP --op RESET_OP --approval ID --web-account ID --previous-op PREVIOUS_OP --previous-generation N [--receipt-only]
```

1. **准备本机目录。** 管理员可用上面的 `--create-layout` 明确批准新建实例；这不是预演，也不开始生成域。只收独立的实例 ID 和本次审批参考，没有任意路径、属主或口令输入。从固定的 `/var/lib` 沿已核实的原目录创建尚缺的 `local-ops-helper/samba-instances/<InstanceId>`；只有需新建的两个容器目录才设 root:root 0711。现有容器仅核对，不改其权限。对已选原父只读有界目录项以确认固定分量的准确拼写；不进入、记录或显示无关项。新实例为 root:专用存储组 0750，只含 root:root 0444 单链接的 `smb.conf`（与发行模板完全相同）和各归备案存储 UID/GID、0700 的空 `private`、`state`，另有 root:root 0700、无额外 ACL 的空 `accountdb`。已有实例即使为空也拒；已有相应暂存名不接管。

   创建前须有独立确认的整套发行软件、受保护的安装主体和非 root 存储 UID/GID；本动作不建立身份、策略或信任记录。原前台显示实际固定父、dev/inode/本次挂载、ID、审批、属主、配置及发行摘要；新随机答复只批准这一次。仅支持可查普通 Unix 模式、无额外 POSIX 访问 ACL 的受保护本地原链；新建父还不得有默认 ACL。既有祖先须允许降权后的存储身份搜索。从宿主 PID 1 的实际根逐级持有原父。正常独立挂载的 `/var` 或 `/var/lib` 均可用（两处可以都独立）；只在这两处按实际挂载根、真实父挂载与本地文件系统证据接受转场。普通文件系统要求完整根，Btrfs 可为已挂载的子卷根，后者不把内核两种设备号混同。普通子树 bind 或 site 自身、包目录、实例处的额外挂载仍拒绝；不改现有挂载。这不区分管理员有意的整根 bind，不遍历其它命名空间，也不证明没有外部使用者；维护者须确认所示原父位置专用。若直接创建父为 setgid、本进程 umask 掩去 owner 位或实际权限不足，则在暂存前拒绝；不改原父或主机设置。现阶段复用宿主已支持的 ext2/3/4、XFS、Btrfs、F2FS 普通目录识别，不使用其专有功能，不规定 ZXS 池的格式；远程、叠加或无法归属的宿主路径不试写。宿主额外强制访问策略仍由部署者核准。

   布局先在原父中新建 `.layout-<首个缺失分量>.new`，持新实例目录的 EX，核全部名字、属主/模式、确切配置和原件，逐件和自下而上同步；仅用原父内的 `RENAME_NOREPLACE` 发表，父同步并按最终名独立读回后才报 `layout=prepared`。不锁全机 `/var/lib`，不改服务、网络、宿主用户、SYSTEM 数据库或账户。失电/取消可能保留暂存名，或最终名已出现但父同步/读回未确认；未确认不自动删除、接管、重做或生成域。依赖受保护的宿主父和遵约管理者，不能抵抗恶意 root 的同时替换。也可由独立宿主部署事先提供同等合格的目录和模板，而后直接选下一步。InstanceId、原操作和审批参考为非零 32 位小写十六进制，不是身份或登录权。
2. **provision 只建立此实例的本机域。** 选择确切的已提交 INIT，使用独立的 DOMAIN_OP 并另做前台确认。只由受限的固定 Samba 生成器创建原状态，再由受限宿主阶段同步并不覆盖地发表 `domain.bound`。不输入两个账户的口令，也不启动服务。成功的本机 `domain-binding=confirmed` 回执附 `bound_sha256=`，是完成原文件和父目录同步、按原名独立完整读回后那 128 字节的摘要。只在终端留存，供另行批准的 [SMB 安装预检](smb-host.md) 选择同一原件；它不是签名、在线域或自行授权。任何已有/中断现场包括已丢回执都不能当作全新实例重试；保留原件，不换新号再做。
3. **enroll 设置两个固定身份。** 只处理同一池/INIT、专用实例/DOMAIN_OP 及两个明确不同的登录名。初始登录名为 1–64 个小写 ASCII 字母/数字或 `._-`、以字母或数字开头；SMB 不接受 guest、anonymous、nobody 或 root。请选不同的 Web 与 SMB 口令；程序不在两进程间交换明文或可用于比较的秘密。 首次创建单链接 0600 账户库之前，先在留原待办前核当前进程真实 umask 不屏蔽所有者的读写位；本命令不会修改 umask。
4. **reset-smb 或 reset-web 只改所选角色。** 保留已确认的 SETUP_OP、准确 AccountId、原凭据 Op 及代数（第一代前驱是 SETUP_OP），再选这次唯一 RESET_OP 和审批。SMB 另核真实专属原实例和原 SAM；Web 只走原 SYSTEM 与 Web 一笔 SQL，不接触 SMB 库、不改变其密码/代或 Web 之外的权限。不支持访客、另建账号、重命名、默认密码和网络设置入口。
5. **只查或中断。** `--receipt-only` 不加载密验库、不提示、不写、不补同步。退出 0 是精确原单已确认（首设返回两个原 ID，重置返回该代），1 是确无或仍待办，2 是原件、权限、冲突或最终结论未能确认。待办不等于可用，晚错不能说已经回滚。保留实际两份介质，以相同原单查询；不换 Op 重写。`--resume` 另获本次确认：只见同一目标时重新同步、不询问口令；仅 SMB reset 已证同一前驱时才可再输入同一原单所需的新口令。不从首设 PENDING + 今天缺库补造，不手删/修未知或热日志。

以上结果不是实际登录、共享开放或数据可用的证明。源码未运行、组包、安装、部署或检查真实账号。

## 实例和原始凭据

- 先由本机管理员核原 PoolId 和 SYSTEM 原目录、取得与在线业务相斥的 EX，并另批此次实例供给。宿主维护者独占其明确选定的安装目录，不能使用全机 Samba 配置或搜索 `secrets.tdb`。新发行一个非零随机 InstanceId 和一个固定 ProvisionOp。生产配置文件、专用 private/state 原目录都必须在可信宿主配置中有唯一、可核对的位置；整个运行期间持原物、重新核名字与原挂载，未明来源不写。不能把挂载编号当跨重启凭证。
- 宿主将已核同一实例的原 private/state 投射为 **/instance/private、/instance/state**；安装时只有此次被批准的空 private 可写。确切引导配置 [domain-bootstrap.conf](../resources/samba-domain/domain-bootstrap.conf) 没有 include、共享、访客或备用 passdb；其中 private dir 指向同一原目录，非工作组名（ZXSWORK）的本机 NetBIOS 名固定 ZXSLOCAL。引导配置用于生成本地域，**不代替**届时上线的有效 smb.conf；上线映射 `/instance/smb.conf`、`/instance/domain.bound` 两个确切原文件都必须为真实只读挂载。正式配置须非空、≤1 MiB、无内嵌 NUL；明确空的 `usershare path =` 和 `usershare max shares = 0`、无外接脚本。两份配置必须指向同一实际原 private/state、固定本机名和服务器角色，上线的唯一用户后端是 `tdbsam:/instance/accountdb/passdb.tdb`（独立原目录只读投给 smbd，Web 和非 root 存储视图不含它），数据共享才用 `zxs_file`。引导或域供给不加载这些后端、不建立用户库。 首版正式配置也不允许 include、config file 重定向、registry 配置或动态用户共享；原正式配置变化须另获宿主维护授权、更新该实例绑定，不能按眼前文件自行重算放行。共享启停及只读/读写模式属于 SQLite，不随每次更改重写 smb.conf。
- [私有 guest 源码](../resources/samba-domain/fresh_guest.py) 的三次官方调用依次是固定 `set_smb_config`、`set_secrets_dir`、`get_global_sam_sid`。这是**有写的首次供给**，不是只读发现。只能用于全新 exec 的受限非 root 进程，不能复用解释器、服务进程或有旧全局状态的 fork。运行环境须由受信宿主提供：唯一 apt 认证的软件及 Python 模块闭包、没有用户包/脚本、网络/IPC、外部描述符、宿主 /etc 或 /var；只显露已选定的原件。受关注的目标是同一次原供给而非任意路径、参数、布尔值或子进程的 stdout。宿主固定启动、有界监督和本机确认已有内部源码；已有[准确 Debian 原件的组包配方](../packaging/debian13-amd64/README.md)，已有受限降权工作器交接源码，但没有实际组出的独立验签发行物或运行结果，不构成启动许可。
- 原 private/state 必须是各自确切的空目录。遇 `MACHINE.SID`、空/既有 `secrets.tdb`、其他文件、同一目录别名或起因不明即止，不读取、删除或迁移。Samba 会在指定 private 下创建并管理自己的 0600 `secrets.tdb`；预计只含此实例的域所需状态，不对它遍历或导出。进程内的第二次 getter 会命中缓存，`get_domain_sid` 又查工作组名，都不能冒充持久读回。任何错误/崩溃留下已产生的原状态；无已确认的绑定不自动重跑、不重新随机造域、不接管。

### 初次正式配置，不是引导配置

[initial-live.conf](../resources/samba-domain/initial-live.conf) 是 1383 字节的首装正式模板（SHA-256 `89cfd6cbd9285bd066ffcdfd53c9707cb2e7ce655301fbb66fc1821eeca4763f`）。仅含 global，只监听回环 127.0.0.1，不经 Avahi 发布 SMB 服务；协议最小值和最大值都固定为 **SMB 3.1.1**，不与 SMB1、SMB2 或旧 SMB3 方言协商。Samba 自己产生受限的 IPC$，不导出数据目录；显式关闭 NT 命名管道和 DFS 转介。受信宿主若要把数据共享开放到局域网，须另行批准对应部署；不能把本模板说成客户端已经能读写文件。所选实例根内正式源文件 `smb.conf` 必须是宿主管理员的 0444、单链接、同一原挂载普通文件。本地发行步骤和日后服务投射分开：宿主侧准确模式不等于服务里已是只读挂载，线上还须独立确认有效的原配置。旧文件及其 BOUND 回执的摘要不同，不能用于这套新发行起停；安装源码不会覆盖或重签现有站点。

在调用域生成前，宿主逐字读该实名原件至 EOF，与编入的模板以及独立批准的正式配置摘要同时核对；保留 FD，直至客体退出、私有库同步和未发布候选输出时仍复核原挂载、实体、名字及内容。不能用引导 profile、另一个安装或只有同名的配置代替。128 字节 BOUND 候选只取这次正常退出并已核原件的非秘密域；没有宿主管理员通过受限父进程的不覆盖发布时，它不是磁盘正本、开户许可或登录证据。

## 非秘密交接

客体的正常结果为 `b"ZXS-SAM/NEW-1\0" || b"ZXSLOCAL" || SAM域SID[24]`。域是 Windows 标准二进制 SID：version 1、4 个 subauthorities、authority 5；4 个 u32LE 为 21、x、y、z，最后三项不全零。拒多余/截短/用户 SID/全零域。生产者只能取上游这次实际返回的 SID 对象；解析相同字节**不产生原实例证明**。传输限定为宿主为这次子进程独建的本地 SOCK_SEQPACKET：固定结果旁仅传一个指向该新文件的 O_PATH，不读或输出库的内容。内核在交付期间持有 inode；宿主还从独立选择的原 private 固定名取得另一 O_PATH，核同时持有的实体、权限与宿主原挂载。接收方按内核报告的发送进程、UID、GID 对照独立监督者的原 guest 身份；同账户的其他进程或 bwrap monitor 的 PID 不算。多余消息（包括空消息）、可读文件 FD、变名或同字节替件都拒绝。通道 EOF 不证明子进程正常退出、原文件同步或原单已批准。固定 SMB 用户的 28 字节 SID 由同域追加 RID 1000 并将个数改为 5；它不授予共享或 Web 管理权限。

最终的宿主绑定正本（由本机 `setup-pair provision` 在真实前台批准后发表）是一个宿主保护的只读普通文件，位于独立的宿主安装记录中，不在 SQLite SYSTEM、Samba 私有库或数据卷。固定编码（无宽字符串或对齐）：`b"ZXS-SAM/BOUND-1\0" || PoolId[16] || InstanceId[16] || ProvisionOp[16] || live_configuration_sha256[32] || b"ZXSLOCAL" || domain_sid[24]`。三个 ID 非零；摘要是**明确原上线配置**的完整精确字节，不是 guest 的引导配置，不能通过更换实际 private/state 来复用。原物路径由可信安装单独选择，不从这串字节反解或搜索主机。新绑定发布前须由宿主核被调的实际发布/进程和正常退出、完整唯一客体结果、再无该写者；同步实际新 `secrets.tdb` 及原 private 父，并对原配置、目录、实例重新核验。只在原批准位置不覆盖地发布且同步父目录，最后重开核准确原记录。任一项失败保留现场，仅报未确认。完整物理复制或可信宿主被替换，单靠这些字节无法证真。停机只能核这一份已保护的既有期望；上线须比较同一受管实例内真正的 SAM 名与域，无法证实则拒会话。

## 宿主内部执行边界

- 当前的固定调用只接受专用宿主工作进程。它继续持原 SYSTEM 的 EX，在按独立 InstanceId 选出的专用实例目录上另取 EX，核原 private/state 正好是两个不同的空目录。本片仅支持位于该安装目录同一挂载上的 private/state。固定非 root 主体、认证发布的私有 Python/Samba 用户态闭包、对应的宿主 bwrap 及其动态装载器/库、授权且被保护的宿主安装路径是前提。不能由网页、散列自报或任意目录发出这个许可。升级必须先排空，不能边替换发行物边给新域签名。
- bwrap 0.12 从已核 ELF 的 FD 执行，强制独立用户、PID、网络、IPC 及 UTS 命名空间。只映射固定私有发行树、可写原 private、只读原 state 和两份封印的引导材料；不映射宿主的 /etc、/var、/dev 或已有 Samba 实例。guest 不能分叉/脱离进程组。固定选用 `--as-pid-1`，只从 bwrap 专属状态管道的真实 child-pid 与最终正常状态，加上原 supervisor 的 wait/进程组收尾，确定此次客体。单个 PID、EOF 或 SIGKILL 都不算成功；异常保留已经产生的材料。宿主自身崩溃/被强杀不能作为全域已经退出的证据。
- 正常结束才核客体经内核传来的同一 inode，从原 private 的准确名字核对，实同步原 secrets.tdb 和原父、末端重查身份/属性及 private/state 清单。宿主不读取/转述秘密库内容。只有这一层是**未发布**的原实例证据；它不代替下述前台确认和受保护发布，更不能建立账号或证明登录。

**两个进程交接，不换锁。** 启动客体前，降权 worker 在原 SYSTEM 的 EX 下，用 SQLite 只读核本池确切且已提交的 INIT 及原 S 卷标；关库后才进入 Samba。实例父原目录必须恰有准确拼写的 `smb.conf`、`private`、`state`、`accountdb`；供给只处理原 private，不把空 accountdb 投给客体。若已有 `domain.bound`、临时记录、其他项或不能完整读取，不会为了探测而让 Samba 新建秘密库。

宿主内部另用本次进程独占的匿名 socketpair，不是给用户或上线 smbd 的接口。worker 永久降权后先用这一对套接字发出本次随机挑战和范围摘要。仍持前台批准的父进程核实际子 PID/低权 UID/GID、原件与批准后才答复；子核此条答复真实来自原 root 父、挑战和范围都相符，才可查询所选 SYSTEM 或让客体运行。密封的输入、本来属于父的套接字或历史答复本身不算许可。完成的 worker 再一次交接 128 字节公开候选、与前台批准的原 INIT/发起主体/原件完全一致的 32 字节范围摘要、同步时私库及其父的 32 字节属性比较值，以及原 S 和实例两个已持 EX 的目录描述符与原库的只指路径描述符。内核持有在途的这两份原锁，不因 worker 退出而改锁或先放再取；接收方按同次子进程的实际未回收正常退出和内核 PID/UID/GID 核唯一消息，从受核当前线程的 fdinfo 确认这两个具体的 open description 各自拥有整文件独占 flock，并以另开原目录的共享锁争用佐证；不能把同 inode 上别人的独占锁算到收到的描述符名下。再对照独立预选的原目录、正式配置及原私库。没有复制或读出秘密内容。只有这份手递结果不授权写宿主安装父。

**独立发行记录先于本次批准。** 宿主在固定清单/目录查到软件不等于可以执行它。只对独立验签和托管的三摘要发行记录（[组包说明](../packaging/debian13-amd64/README.md)）验证固定 helper 原 ELF（真实 procfs 的运行者和安装原文件须同一实体及挂载；旧进程不能给新版包出具批准）、八槽宿主 launcher 发行材料，以及准确的 Samba 私有清单。先验清单摘要，才按其中的相对名字在同一原 `usr` 目录中逐个无链接打开：每层所有名字须完全匹配，只读预期的、root:root 单链接普通文件，逐一比权限、长度、全 SHA-256 和实际 EOF；陌生项/子挂载不会被打开读正文。最终再核保存的原实体、挂载、名字与时间戳。此次批准还绑定那个目录和这套发行摘要，不会以同路径的新发行物悄悄代替已选材料。包验收仅用于受控 Debian 13 amd64：不支持并行升级或任意特权进程换物，不把元数据复核当作原子快照，绝不据此证明客体真的冷启动或账号能登录。

**前台确认与固定发布。** 宿主父在原前台显示所选 SYSTEM、预期池及原 INIT（此时未查数据库；只有降权 worker 在原 S 的 EX 下确认这份 INIT 已提交，才可生成域）、实例、操作、维护主体、审批参考、存储 UID/GID 及正式配置摘要；操作者只确认此次安装，不输入密码。确认前清除旧输入，以新随机挑战匹配原前台终端的一整行；拒绝、普通中断或超时在返回前清理余行，不复用上次的答案。范围另钉 S、实例、private、state 及正式配置的本次原实体/挂载，父持原件，降权 worker 在首次写前核对；同 UID/GID 不等于同一维护主体。这些本次比较值不写进 BOUND，不当作重启后的盘身份。宿主必须从这次自己启动的受信 worker 核实正常退出、原锁和唯一材料；不接受其他 `Child`、自报 SID、任意回复地址或路径。

在原实例父的同一 EX 下，仅以 `O_EXCL` 创建固定 `.domain.bound.new` (root:0600)，写满 128 字节，核 EOF，改成 0444，实同步该原文件；在同一父中以 `RENAME_NOREPLACE` 改为 `domain.bound`，同步父，再独立按原名重开比对原物、全部字节、EOF 和全组选定材料。保护父只由遵约主体写；NOREPLACE 不是来源 inode 的原子比较。不得替换既有正本或给存储 UID 安装父写权。动作推断边界：

|所见阶段|如何处理|
|---|---|
|一开始已有正本、临时项或原 private 不是空目录|不用此“全新安装”动作，不启动客体或生成新域。|
|只留下原 secrets.tdb、暂存项写到一半，或改名前失败|保留原物；不算有宿主正本，不删临时文件、不重发域。|
|已见最终名但父同步或独立读回未确认|结果未确认；不能回移、补造、覆盖或重试新单。|
|父同步且独立读回证实原物及全部字节|只证明这次本机域绑定的持久发布，不证明可开户、服务已上线或有人能登录。若最终回执丢失，需另行按原单核历史，不再走全新安装；该历史入口未开放。|

**单次启动。** 仅上述同一前台批准的持有人可从原安装 ELF 启动固定子进程。argv 不承载路径、SID、选择动作或凭据；输入是有封印且禁止执行的有界 memfd，唯一结果通道是该次匿名 socketpair。子进程在打开任何所选数据前核真实宿主、同一经登记 ELF 的当前父、实际父 PID 和内核记录的 root 对端，读取固定安装主体和软件登记后永久降到无能力的存储 UID/GID。因 Linux 改 UID/GID 会清父死信号，降权后重设并重新核原父，再读非秘密原单；六件都须先通过 O_PATH 的原实体/挂载比对。不会把请求的路径、自造的密封文件或另一个同 UID 的进程当作批准。父只在这个仍可核身份的子进程正常退出并通过原件读回后发布。超时或取消不再接受结果；强杀或宿主崩溃不证明全部进程已经收完，不能重发或视为可升级。

本机首设用上文同一个命令选择域供给或双账户登记。尚无实际签章发行物的安装/冷启动验收。

[固定版本 bubblewrap 的状态与启动实现](https://github.com/containers/bubblewrap/blob/v0.12.0/bubblewrap.c#L2909-L2925)将 child-pid 写到专属通道；不用它代替自己的进程组退出核对。

## 精确来源与尚未交付的门

[Debian 的 Samba 源包](https://packages.debian.org/source/trixie/samba)为 `2:4.22.11+dfsg-0+deb13u1`。本片静读对应 [dsc](https://deb.debian.org/debian/pool/main/s/samba/samba_4.22.11+dfsg-0+deb13u1.dsc)、原归档和 Debian 补丁：orig 25,620,416 B、SHA-256 `4379aa4bc6c453bfa55697556d6195ab69b1cad71e5fdfaf2c53849d96d40f0a`；Debian 189,836 B、`0359a3242bcbd14f74994a195084f7a8ea28bf9d1287c0021921c40b7d5d7a03`。`source3/passdb/{py_passdb.c,secrets.c,machine_sid.c}` 对应三入口、按指定目录建立原 Samba 状态及本机 SID 事务；24 项补丁中，`usershare.patch` 改默认用户共享数量，所以原配置明确设为 0。上游或其他 Debian 版本不自动放行。

已写客体、固定宿主启动、原组收尾及原库/父同步源码，**本轮没有执行、生成域或开放服务**。源码中的前台确认和不覆盖发布已接入本机入口；已经写了仅为这一域供给准备私有运行材料的组包配方；独立登记后的现场软件原件核对已接入内部批准；固定一次性 worker 已有受限启动及收尾源码；实际发行签章、冷启动与可用性验收未交付；不能凭来宾自报的版本、退出码或 SID 冒称做到。未分发/执行 Samba，没有安装包、登录、拒绝访客的实机测试或可用首设声明。

### 按原单确认两库，不交接秘密

- 实际业务组件停止并排空后，主管从受控真实宿主取原 SYSTEM、原实例两个不同的实际目录 EX（固定顺序 S→实例）。低权 SQLite 工人借本次已持那一 **OFD**，不是另一主体用同一个 OFD 变锁；它再独立从原目录取真实 S 卷标和原已提交 INIT，并用标准 Unix VFS 保留最初数据库 inode，期间不持长 SQLite 事务。不裸开 SQLite inode，不搜索其他卷。只查路径止于已提交/待办/确无原单，不打开 TDB 正文。
- 人的授权来自同一真实前台，选定一单、一个角色/明确首设两名及具体操作状态。独立签发和宿主的允许条件先于写入，不能用一个散列/口令批准号代替。固定低权 SQLite 工人从实际已核的 helper FD 执行，以密封**非秘密**原单和匿名 seqpacket 握手；使用逐条内核 PID/UID/GID，活动父的随机挑战及确切原件；只跨进程交付同一已持 S、非秘密阶段和 O_PATH，常规 EOF 不证明成功。原同一工人独立正常退出才给最终确认。
- 正向先核实际原本地域：在本实例的机器库只读取唯一规范的 `SECRETS/SID/ZXSLOCAL` 匹配 24B 域，不调用会生成新域的 getter。原生工人确认原账号库的唯一 USER/RID 及 4/0 空间。首设先从低权工人预留完整 64 MiB Web 派生区，并证实实际 SMB 提供者。暂存密钥留各自固定进程的锁定内存，终端由父维持同一次无回显，子只核当前原终端、不恢复。中断先收确切低权读者，后清终端并恢复；强杀、主机崩溃或不可恢复终端不作保证。口令不进命令、环境、日志、IPC 或临时文件。
- SQLite v7 先留 **PENDING**（首设还留 Web 的 Argon2id 校验，两组身份暂不活动；SMB reset 仅挡 SMB）。正常提交、该连接实际 `xSync` 的原主库及原父同步、独立从同一 VFS inode 复读之后，才在同一原生 TDB 事务提交密码等价值及非秘密原代号。新账号库只准本次原进程 `O_EXCL`，重置只能写准确前驱的同一文件。原 TDB 写句柄关闭后验证并同步原库及原 accountdb 父，再独立原生重开验同一原物、全部四项和目标，才准同一原单最终 SQL 确认。没有密码回读、密码备份、格式升级或失败后换模式。
- Web reset 不走 Samba、BOUND 或第二库，只在原 SYSTEM EX 下确认原精确前驱，将 Web verifier、原代和操作一笔提交、同步再独立复读，不留跨库待办。后续登录须另由实际服务重新核权。

### 独立发行与站点策略

受信站点在实际允许 NTLMv2 时才据外部验签结果，独立放置 root:root 0600、单链接的 `/etc/local-ops-helper/native-account.policy`，一行无通配符、单空格、尾 LF：

```text
SITE-NATIVE-NTLMV2-1 POOL INSTANCE DOMAIN_OP LIVE_SHA HELPER_SHA ELF_INDEX_SHA LIBGCRYPT_SHA LIBGPG_ERROR_SHA
```

包不安装站点准许，也不改变实际 FIPS 策略。原生 [Debian 构建及组包](../packaging/debian13-amd64/native-account.md)只有恰当授权后才准执行。程序核完整闭包和实际 ELF 映射，不借宿主同名依赖；内核必须支持固定限制，认证宿主须排尽旧服务，保护站点管理、终端主端、禁泄密审计、禁持久换页/转储和休眠。最低原生存储层仍有失败、断电、完整复制与恶意 root 边界；真实 Windows 登录、SMB 授权及冷启动不属于本地安装结果。

[共同非秘密契约与向量](https://github.com/isarmg/zxs/blob/2f40e4645cd4c1985c45a051fa5c1d98652371da/zxs/docs/first-account-v1.md) · [SQLite 本机同步接口](https://sqlite.org/c3ref/io_methods.html) · [Linux Landlock 6.12](https://www.kernel.org/doc/html/v6.12/userspace-api/landlock.html)。
