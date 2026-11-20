# ZXS 内部 Rust SMB 服务

**当前只有私有受托连接上的协议与业务前置，未接生产凭据、监听或业务树，不是可挂载的服务。** `wire/` 能分开 Direct TCP 帧、校验整条请求链的 SMB2 头和边界、读取客户端的 3.1.1 协商提议。`backend/` 能为已选对象有界预留读、写、删共享许可，兼容判定完成不等于已取得 Core 写权。此段没有生产存储句柄的签发者，也未实现目录首开特例、范围锁或删除待定。客户端说“支持”不等于服务器同意。受托入口能从同一已接受 TCP 流读取并完整写回 3.1.1 协商消息；无生产服务实例提供者，不监听 445，不签发生产认证、会话或共享。现行 Samba 运行入口不调用这里；没有编译、运行测试或进行真实客户端验收。

## ZXS 产品范围（目标，不是已实现清单）

| 保留 | 明确不做 | 仅在目标实机证明必要后补 |
| --- | --- | --- |
| Direct TCP/445，只协商 SMB **3.1.1**（`0x0311`）。`SMB2` 头部和命令名是 3.1.1 沿用的线格式，不是支持 SMB 2.x 方言。一个专用 SMB 账户，实际 NTLMv2 证明、适用的 MIC、SHA-512 预认证、双向强签；保留 credits、复合消息、按命令的失败处理、原请求取消和会话／树／打开对象的生命周期。 | SMB1、SMB 2.0.2/2.1、SMB 3.0/3.0.2、旧方言回退；域加入／AD／域控、打印、多账户、访客或匿名。 | 额外的合法原生探测只补实际所需分支，不加旧方言、不放宽身份验证或强签。 |
| **一个业务共享 `ZXS`**；数据盘与回收站是它的子目录。普通文件／目录能列出、创建、读写、改长、重命名与删除入回收。回收子树只读，只允许 Core 核实原删除记录与可写目标后的同树恢复；不对其授普通写、覆盖或永久删除。 | 通用多共享管理、共享发现、DFS、命名管道平台或宿主 RPC；不做客户端缓存租约／oplock 授权、耐久／持久句柄、连续可用或多通道。 | 仅在已知共享的基本文件操作被阻断、且规范允许时，补窄的 AAPL、流／扩展属性、额外信息类、变更通知、具体 IOCTL 或已认证的最小 IPC$ 探测处理；不等于实现全套能力，不开匿名 IPC$ 或新数据共享。 |
| 业务上每个 FILE 单写多读，SMB 的 ShareAccess／范围锁另按协议核对；盘离线仅隔离受影响的对象及依赖该盘的操作。正式手动版本仍由 Web 显式提交，SMB 的保存／FLUSH／CLOSE 不是提交。 | 不在 SMB 重做 Web、Helper、容器、版本或物理存储；不展示 SYSTEM、宿主文件树或私有布局。 | 仅在实际工作流需要时补最小的协议表现，文件、锁、模式、回收和版本的权威仍在现有 Core。 |

收到合法探测不等于授予能力：CREATE 先核成员范围，再按各 context 的规范处理。[未提供租约](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/fc4f8879-f295-4995-b71e-21f309d8d7c8)和[不授耐久的打开](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/33e6800a-adf5-4221-af27-7e089b9e81d1)仍可完成适用的普通 CREATE；重连请求不能降格为按名字新开。命令应按各自条文忽略、明确失败或断连，不能一律丢弃探测，也不伪造成功。例如不支持目录通知时，合法的 [CHANGE_NOTIFY](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/05869c32-39f0-4726-afc9-671b76ae5ca7) 按命令规定失败。若目标部署要求 SMB 加密，须先真正实现并审过 AES-128-GCM，否则不支持；协商 offer 本身不是强制策略，不能以明文回退或关闭客户端安全策略求通。

`wire/path_fields` 从单条 CREATE / SET_INFO 普通 rename 消息的声明范围提取名字，`backend/path` 是唯一业务名字解析：严格 UTF-16、以同一共享为根，逐部件调用现有显示名规则，保留大小写和 Unicode 原拼写，不从名字判断数据盘／回收站，不解释宿主路径。CREATE 空名和未处理的 contexts 一起保留，不把重连请求误作根权限。rename 源是本次协议句柄，目标的替换标志不是覆盖授权。关于多部件目标，[微软 SET_INFO 条文](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/43cf22fe-84cb-49cf-b3e7-e3d7d766a7b7) 的分隔符限制与 full pathname 表述并存；[Linux 客户端](https://github.com/torvalds/linux/blob/v6.12/fs/smb/client/smb2inode.c#L1076-L1112) 会编码目标路径。此处为 ZXS 同树移位保留这个目标形状，不表示任何原生客户端已完成还原。具体成员字节额度、当前盘和删除原单仍由 Core 选择和复核。

`wire/create_fields` 进一步按 [CREATE 固定字段](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/e8fb45c1-a03d-44ca-b7ae-47385cfd7997) 和 [CREATE contexts](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/75364667-3a93-4e2c-b771-592d8d5e876d) 验原成员。保留原 DesiredAccess、分享掩码、顺序与请求选项；仅去规定忽略的位，不产生实际 Grant。最多 16 个 context，Next、名称与数据各自限在原 context 中，零数据不看 DataOffset；精确标准保留 context 只忽略内容。空名偏移也必须在原成员 Buffer 对齐范围内，只表示申请打开当前共享的已有根，不执行新建／删除，也不能给耐久重连、旧版本、回放或不明扩展换成当前路径。合规 lease/durable 新申请不授缓存或持久资格；只识别 [Apple 定版的 24 字节 AAPL Server Query](https://github.com/apple-oss-distributions/SMBClient/blob/99da2d9a4bbe17cde3cee382ad3edc16e25f5535/kernel/netsmb/smb_2.h#L94-L107) 为不改目标的提示，不把 Resolve ID 当普通打开。原请求 `MxAc`（无时间与零时间不同）和 `QFid` 仍是必查的原对象应答义务，不得以假权限或伪 ID 填空。仅解析，无真实树／OPEN、Core 入口或 CREATE 成功。

目录和元数据查询不从协议数据构造业务对象；以下根目录正文暂不接真实发送。按 [QUERY_DIRECTORY](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/29dfcc9b-3aec-406b-abb5-0b4fe96712e2) 与 [QUERY_INFO](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/abb48417-27e5-4a16-82e0-3e5981db97e8) 保留原请求长度供 credits 核验，不把客户端 FileIndex 或 SMB2_FILEID 变成 Core 游标。查询目录的模式是单个分量，不可当路径；没有新模式不能让续页丢失原筛选条件。[Apple 原生实现](https://github.com/apple-oss-distributions/SMBClient/blob/99da2d9a4bbe17cde3cee382ad3edc16e25f5535/kernel/netsmb/smb_smb_2.c#L8315-L8369) 会给空模式留 offset 96 和两个零字节，当前仅和标准的零 offset 一并识别为空模式。[Linux 客户端](https://github.com/torvalds/linux/blob/v6.12/fs/smb/client/smb2pdu.c#L5358-L5394) 与 [Apple 客户端](https://github.com/apple-oss-distributions/SMBClient/blob/99da2d9a4bbe17cde3cee382ad3edc16e25f5535/kernel/smbfs/smbfs_smb_2.c#L8176-L8185) 的源码分别给出目录信息类选择；这是格式依据，不是原生挂载通过证据。

一页只为有权、匹配的逻辑顺序前缀预算，独立限制条数、客户长度和复合应答剩余字节。只在记录之间算 8 字节对齐；首条装不全、下一条装不下、固定部分都装不下不是同一结果。真实适配须按原 OPEN 重核列出权，再按来源的真实排序与集合结束标记推进，不消费未回的匹配条目；不能让游标先推进再丢弃未返回的尾部，也不把过滤致空当结束。[MS-FSA 目录查询](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-fsa/fa8194e0-53ec-413b-8315-e8fa85396fd8) 的模式延续、匹配、点记录、溢出数据和结束状态仍须接线。元数据按已持对象区分虚拟根、目录和 FILE；访问属性和具体卷必须来自原始授权对象。虚拟根不挑任意实体盘冒充总容量，不对下线盘填假数据，不返回宿主路径或私有标识。

`wire/query_reply` 只接收原授权根 OPEN 已封存的完整非空记录，按 [MS-SMB2 §2.2.34](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/4f75351b-048c-4a0c-9ea3-addd55a71956) 编写固定 8 字节正文（StructureSize=9、输出 offset=72 及实际长度）；不把之后复合成员之间的对齐算作数据。记录和原待提交游标仍不可分。空目录、缺权限、首条溢出或来源错误都不会制造非空成功；此层不发 SMB 头、签名、提交游标或网络发送。

## 已接受连接的交付与协商

未来已启用的服务所有者只能交付一条**尚未读取、独占**的 TCP/445 连接以及同次运行的私有实例。这里不建监听、不接收外部传来的已登录连接、不克隆或替换流；OS 提供的原对端 IP 只作连接元数据。[接入状态](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/91e2ec0b-530b-4558-aee0-2ca4d586f66a)与认证身份是两回事。每个未协商的入口在首次阻塞读之前与已协商连接共享最多 128 席；失败释放，成功将同一席转为原连接登记，不在注册锁内做 I/O。

按 [Direct TCP 帧格式](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/1dfacde4-b5c7-4494-8a14-a09d3ab4cc83)先完整读四字节、检查本地首次 NEGOTIATE 68 KiB 上限后才分配，不误吞后续帧。接入首包有固定五秒预算，不把它当成已登录客户端的空闲时间。输出只加四字节前缀，原议定 SMB 全文完整写入同一 TCP 后才发布初始额度与 preauth 链；EOF、超限、零写或半包失败终止这条流，无手工“已发送”生产入口。成功写完不等于对端已经收到，更不代表有账户或文件权。三类 SESSION_SETUP 回复和后续原帧读取见下；业务持有者与通用派发仍未启用。源码夹具在 `src/test/smb/accepted.rs`，未运行或开 socket。

### SESSION_SETUP 不能手工确认发送

机制选择 MORE、实际 Type 2 挑战 MORE 和最后经严格证明签出的成功回复，只能把原额度账绑定的完整 SMB 字节加上前缀，写入原连接独占的同一流。每次短写前及写完后检查原登记、阶段、期限与所需凭据代，不在网络写入时持注册或凭据锁。选择 MORE 并不证明账号，实际挑战要在原凭据代下才转为可用；最终成功仍在同一注册和原代原子发布后才是有效会话。部分写、完整最后一字节时已撤代、失败或流丢失均不补信用、不转阶段，终止原连接。仅测试保留人工拨动完成态的原单元夹具；`src/test/smb/setup_send.rs` 另以实际完整写口覆盖三种过程与失败。这不是生产账号来源、全部入站命令或可挂载服务。

### 原流入站到真实握手与 ECHO

协商后的原 `Peer` 自行从同一套接字读下一条，不接受外部提供的字节、Connection 或另一个输出端。空闲轮询（一秒未收到首字节）保留连接，已开始但超时／EOF 的半包必须关闭。先按 [MaxTransactSize+256](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/ef8e06ba-c921-46c9-9780-a2e9843e1d30) 核长度，再分配；轮询会清理期限已过的原握手，首次登录等待在六十秒边界检查。该预算不是已登录会话空闲超时，真正的服务调度者仍须轮询或释放连接。

在单条新建／续接认证时，原 MID 先消费，选择 MORE、真实挑战 MORE 或最终签名成功依原阶段返回。已认证普通 ECHO 仍须原整帧强签。依[会话查找](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/e545352b-9f2b-4c5e-9350-db46e4f6755e)、[GSS 失败](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/5ed93f06-a1d2-4837-8954-fa8b833c2654)及[出错应答](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/41295064-be3f-41dc-9aa5-f68545f945f0)，仅对上述无签、无既有认证权的独立尝试，实际返回原 SID/MID、原额度及明确失败：找不到原登记、严格证明不通过、无可用凭据及坏 token 等分别处理；错误报文既不发权也不收回同连接另一有效会话。部分或去向不明的写绝不再补第二份回复。带签重认证、binding、旧 SID 接管、含 SETUP 的复合以及未接业务都不退回这个无签错误路径。普通业务只交原短借的受检对象，不给可重新喂包的连接；遗弃欠账要关原流，不会假装已响应。

`src/test/smb/accepted_turn.rs` 只含公开合成验证材料：三种握手走同一双工流、以实际协商盐和往返算终签、再核一个真实强签 ECHO；另区分首字节前空闲与半包超时、错连接 SID、错误证明和可继续的新尝试。无生产发行者、监听、文件权或实机结论，未运行。

## 每条直连 TCP/445 的 MessageId 和 credits（未接监听）

`connection` 从原始 3.1.1 NEGOTIATE 请求建立一次性起点：这时只允许 MessageId 0。保留真实客户端 GUID、安全模式、capabilities 及**完整原序的 dialect 数组**，而非仅记录是否见过 3.1.1。根据[协商处理](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/b39f253e-4963-40df-8dff-2f9040ebbeb1)，TCP/445 上 3.1.1 必须支持多额度，服务器成功回复须含 LARGE_MTU；客户端初始请求即使要 0 也至少回 1。仅已序列化且匹配本次请求、SHA-512 transcript、应发额度、实际读／写／事务大小的回复才能绑定；原发送者提交了这份字节后，才产生仅限原连接的可用窗口。已协商的连接不能再次走 NEGOTIATE。现在由同一个 Opening 生成下节所述的实际成功消息，由受托入口原流完整写出后才登记；没有启动传输服务。

- [服务端窗口](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/dec8e905-9477-4c3f-bc64-b18d97c9f905)和[序号验证](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/0326f784-0baf-45fd-9687-626859ef5a9b)：额度对应已实际发出的、单调新发的编号。可乱序接收，不能重用、跨空洞，或在 u64 末端回绕。每成员从自身 MessageId 消耗至少一格，正多额度需整个连续区间仍可用；关联成员不继承 MessageId。整批只能在同一**事前已发**窗口上扣减；任何范围无效即关本连接，不逐项执行后再发现越界。本地最多 64 格跨度，且可用额度加未完成普通请求不超过 64；不能靠不断填高编号扩张内存。这是额度记录，不替代原连接的 RequestList、会话签名或业务权。同步票已把不可换包的原字节与该项账务合在一起。将来的 RequestList 仍须按 §3.3.5.2 给每个非 CANCEL 成员登记原请求，并与后续验签、派发和在途取消绑定；不能拿账务结果另配 SESSION_SETUP 或业务帧。 本端只公布可兑现的 MaxWriteSize，不超过约束整个入站 SMB 消息的 MaxTransactSize；这是本地入站边界的约定，不要求三种协议上限相等。MaxReadSize 的数据方向相反。
- [载荷校验](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/fba3123b-f566-4d8f-9715-0f529e856d25)独立于序号：零计费不表示零消费，最多覆盖 64 KiB；正计费按每格 65,536 字节。READ、WRITE、QUERY_DIRECTORY、CHANGE_NOTIFY、QUERY_INFO、SET_INFO、IOCTL 用原始声明值。查询信息按入／出最大值，IOCTL 两边分别在 u64 相加后再比较。错误或截断不能变成少计费的成功。已花合法编号但载荷不足是成员参数错误，不归还旧编号、不伪称已操作文件；前序认证、树／原 OPEN 失败仍按对应命令的规范顺序处理。
- [CANCEL](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/57bae3d3-5dd7-4a5f-92cb-fc52e2087dad)只豁免这一步的序号消费：同步／异步形式不占新格、不产生本命令应答、不赠额度。查找同连接的原在途请求、核原签名／身份、是否取消及原操作结果留给唯一 RequestList；不能仅凭重用的 MID/AsyncId 取消工作。
- [授额](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/2e366edb-b006-47e7-aa94-ef6f71043ced)与[同步复合回复](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/dde8b59f-52ca-4cf8-8253-211bc43639cf)按成员对账。候选额度尚不可花；服务端先对最终不可变回复整链核同一次批次的原 MID／command／CreditResponse，待原传输确认成功提交才一次公布。跨连接的批次、重复公布、丢失普通应答、传输结果不明、编号耗尽都不能恢复旧额度。当没有剩余额度时，服务器须补至少一格或关闭耗尽的连接，不能留下无法继续的伪活动连接。此出口不允许异步 interim/final 冒充同步回复；未来异步只允许在 interim 发一次额度。该模块只核信用，不把正确签名、可用状态或授权视作已成立。

`src/test/smb/credits.rs` 保留初包、乱序、多额度、关联／非关联、CANCEL、原载荷边界、同步回包对账和整数末端的源级例子。未运行客户端或测试，也未对生产端口发包。

## 原生 NEGOTIATE 成功应答（不含认证成功）

对一条合法的全新 Direct TCP 请求，`connection/negotiate` 从原 Opening、应授额度和同一次服务运行的 GUID 出发，使用已有的 Linux `getrandom(0)` 封装单独取 32 字节公开盐，并按 [FILETIME](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-dtyp/2c57429b-fdd4-488f-b5fc-9e4cf020fcdf) 记录服务端当前 UTC 时间；不给空标识、随机失败或越界时钟造替代值。`wire/negotiate_reply` 写[同步成功头](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/d39a64be-b8f8-424d-b8e5-a2cf5257459e)与[完整协商正文](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/63abf97c-0d09-47e2-88d6-6bfa552949a5)：只选 0x0311／SHA‑512，安全模式是启用且必须签名；只公布 LARGE_MTU，读／写／事务三项目前均为 64 KiB。上下文相对 SMB2 头 8 字节对齐；不宣告加密、租约、多通道、持久句柄、压缩、RDMA 或通知。客户端的 cipher 清单只是 offer，不是“强制加密”开关；要求数据加密的客户端／共享不得作为已加密使用，更不能静默退回传明文。

没有 Signing Capabilities 时仍强制 AES‑128‑CMAC，但不加签名协商 context；有共同 CMAC 时只回该项。合法非空清单与本端没有共同算法时，[服务器规则](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/b39f253e-4963-40df-8dff-2f9040ebbeb1)仍要求回选 CMAC，而[客户端规则](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/3b29f3af-86f9-4962-8cf3-43471cb59363)要求拒绝未提议的算法。原回复可形成，提交后本端也不给这一路可用连接或 SESSION_SETUP 起点。没有 3.1.1、没有共同预认证哈希、畸形 Signing 清单分别拒绝；普通错误回复和真正的 RequestList 仍未实现。成功包只由原流所有者提交；credits 和 SHA‑512 复核的就是交给传输者的同一份不可变字节。

[服务器初始化](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/04e0383e-e2b4-49c2-803f-1313d9c0b2ff)要求新生成 ServerGuid；[Windows 的启动行为 <26>](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/a64e55aa-1152-48e4-8206-edd96444e7f7)也是每次启动换一个。它对同一运行实例的所有连接稳定，按 [MS-DTYP 的 GUID 字节序](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-dtyp/001eec5a-7f8b-4293-9e21-ca349392db40)写入，不是永久安装 ID、池 ID、Samba instance_id 或客户端身份。本切片为将来的真实初始化所有者预留不透明 `RunningServerGuid`，没有生产构造器、磁盘变更或 per-connection 自动生成。绝不能用测试 GUID 启动服务或复用旧权限。

选择标准允许的**空 GSS 安全缓冲区**（起点 128，长度零），由客户端发起认证；不编造 Kerberos、NTLM 或 GSS 服务器广告，不从空缓冲区放行匿名。原生源码方面，[Linux v6.12](https://github.com/torvalds/linux/blob/v6.12/fs/smb/client/smb2pdu.c#L1237-L1268)在空缓冲区启用 raw NTLMSSP 选择，[Apple SMBClient](https://github.com/apple-oss-distributions/SMBClient/blob/99da2d9a4bbe17cde3cee382ad3edc16e25f5535/kernel/netsmb/smb_smb_2.c#L6891-L6951)可继续解析协商 context；均不能代替后续严格 MIC 或三端挂载验收。 `src/test/smb/negotiate_reply.rs` 保留默认／显式两套完整应答及下一条原请求哈希固定值、无共同签名、未知能力、时钟边界和本地失败源码例；没有运行构建、测试或客户端。

## SHA-512 原字节记录（已用于严格认证，未接生产监听）

`wire/preauth` 从已解析的单条 3.1.1 NEGOTIATE 出发，连接只保留请求和实际序列化回复的链式哈希。每个新的 SESSION_SETUP 尝试从该连接值单独起步；接收完整原请求，并且仅在同一 MessageId、同一已分配 SessionId 的 MORE_PROCESSING_REQUIRED 回复时继续。固定的 Rust SHA-512 实现见[上游来源及许可](../../vendor/rust/README.md)。TCP 长度头不入哈希，原 SMB2 消息头、正文、间隙/填充均入哈希。最后成功的 SESSION_SETUP 回复将使用由最后一个请求哈希派生的密钥签名，因此它不会再次进入这份 KDF 上下文。该值不是口令证明、签名或有权访问文件的会话。预认证层本身不运行 GSS/KDF，不编码应答，不查询凭据或启动生产监听。依据：[协商处理](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/b39f253e-4963-40df-8dff-2f9040ebbeb1)、[新会话](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/ea10b7ae-b053-4e4c-ab31-a48f7d0a79af)、[GSS 处理](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/5ed93f06-a1d2-4837-8954-fa8b833c2654)。

## 原连接的 SESSION_SETUP 第一阶段（不签发身份）

`Connection::receive` 先审完本帧全部成员的额度，将**这同一个已解析的不可变帧、其额度账单、原连接的 SHA‑512 基准和终止标记**一起交付。同步处理借用原接收缓冲，不复制每个大报文，也不让业务方拿另一帧搭配已花的 MessageId、签名或 transcript。连接对象释放会使尚在手中的票失效；放弃该原连接尚欠的普通回复也只终止它自己。未来真正异步保留完整独占或共享只读缓冲和有界范围，不能将同步借用延长到失效之后。

`Received::start_setup` 只能消费一次：没有 SETUP 的帧原样保留给别的处理器；含 SETUP 的复合帧在本配置整批拒绝，不顺带执行后面的文件命令。单条且头内 SessionId 为零的合法开局，从票内原 transcript 和同一 security field 得到“先选择 SPNEGO 机制”或“已有 Type 1，待真实挑战”；令牌不合法时仍保留该帧的已消费额度供今后准确失败回复，不能混成匿名或倒回 raw。非零 SID 只表示待真实注册表判定，不是重连或身份。通用额度回复入口明确不接受任何 SETUP；第一阶段分类自身不签发 SID，不创建挑战或发送成功回复。下节的专用机制选择所有者才可接着准备自己的 MORE。

### 正在协商的 SID 和第一个 request-mic MORE

每次服务初始化对应唯一不透明运行对象，包含 GUID 和所有连接共用的 in-progress 索引；知道相同的 GUID 字节不能重建这个对象。根据[新会话及双表规则](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/ea10b7ae-b053-4e4c-ab31-a48f7d0a79af)，在真正的 `Select` 编码 MORE 时，全服务登记从未使用的非零、非全 1 SID，原连接登记自己的阶段；连接之间不重复发号，号码不是认证凭证。全运行最多 64 个、每连接最多 8 个会话槽（进行中或已确认）；初始握手有效期 30 秒（单调时钟），并不作为已确认会话的有效期；过期、撤销和错误不能使编号复用。全局表只弱引用阶段注册；连接私有表才持有令牌状态。狭小的索引锁不跨 GSS、哈希或发送。

首包需要选机制时，发出的[SESSION_SETUP 正文](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/0324190f-a31b-4666-9fa9-5c624273a694)是真正的 STATUS_MORE_PROCESSING_REQUIRED，加[SPNEGO request-mic](https://www.rfc-editor.org/rfc/rfc4178#section-4.2.2)和唯一选中的 NTLM OID，不捏造 Type 2、用户名、guest 或成功。保留原 MID、charge 和阶段专属应回额度。用同份最终字节同时核原 transcript／GSS 和额度。只有原发送者确认这份不可变包已成功提交，才把原连接该槽从“待发送”改为“等 Type 1”并公布信用；未提交、结果不明或临界时过期则关闭本连接，不留下可续阶段。

下一条非零 SID 请求须已在**同一原连接**消费自己的新 MID，再从双方索引找到同一未超时对象、恰处于等 Type 1。仅一次移出该阶段，按同一 transcript 校验原 token；坏 token 只移除这个槽，无权触碰别的会话。跨连接借来的同号码不能接管或取消。不在表的 SID 或过期不会创建新会话。单独的机制选择只推进到有效 Type 1；下节的 Type 2 仅由持有原连接和受保护凭据代的代码签发，仍无成功会话或原生监听。生产错误回复、主动定时唤醒和其他 [GSS 完成／清理](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/5ed93f06-a1d2-4837-8954-fa8b833c2654)仍属后续；每次查找与增发都在原处检查期限，不能用被动清理延长权限。

### 本次机制挑战的生成（不签发会话）

严格机制只从原 Type 1 选已经支持的标志。按 [MS-NLMP 服务器处理](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-nlmp/81d15e3e-3ac3-41f0-920d-846149f3a814)、[Type 2 格式](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-nlmp/801a4681-8809-4be9-ab0d-61dcfe762786)与[协商标志](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-nlmp/99d90ff4-957f-4c8a-80e4-5bfe5a9a9832)，独立服务器返回本机的 NetBIOS TargetName、受信的名称 AV、UTC FILETIME 和内核新取的 8 字节一次性挑战；不反射客户端名字、不冒充域控制器、不把 NTLM SEAL 当作 SMB 加密。原 standalone Type 2、第一次 SPNEGO 的 request-mic/NTLM OID/Type 2、已有选择后的仅 accept-incomplete/Type 2 分开编码。MORE 不同于认证成功。基础库仅接受归属本服务的一份受保护凭据代，与已签发的确切挑战、原 GSS 包和原绝对截止绑定；凭据替换后相同数字、名字或 SID 不能代替同一原对象。没有生产凭据构造、收集器、导入用户密码或认证回退。源码夹具有独立固定的完整 Type 2、短/长 DER 边界、错封装、拒绝和撤代例，无实跑。

连接现在能把这份机制挑战变成真实 SMB2 MORE，但没有启用任何服务。直接 Type 1/首个乐观 token 在本连接预留新 SID；经上一次 request-mic 得到的 Type 1 只消费当时那同一项，不延期、不另发 SID。只从本次运行持有的受保护提供者取凭据代和 Type 2，不接收单独传入的密钥、提供者或 SID。原接收票决定 MID、额度和不可换包的回复；只有提交原字节、同一代未撤销且原截止没到，状态才从 SendingChallenge 进入 WaitingAuth。未配提供者没有挑战，放弃或不确定发送终止原连接。挑战发送阶段本身不消费 Type 3、不回成功、不授 Tree；全局注册只限制有效弱项，部署时仍需终结静默到期连接。

原连接收到末次 Type 3 时，先把原 WaitingAuth 从只有一次所有权的槽中取走，再检查本包、原凭据代、完整 NTLMv2/MIC 和原截止；计算期间不持凭据或全局 SID 锁。失效／校验失败只清退该槽。成功只得到绑定原已付请求与原 Claim 的私有证明，不能从数字或用户名重建，也还不是能发包的成功会话。重复 token 即使另用合法 MID 也不能重复消费同一挑战。

## SESSION_SETUP 的机制封装（止于未验证的 Type 3）

`session/envelope` 只处理一次新的连接型 NTLMv2 尝试。它取原请求或已序列化 MORE 回复的同一个 security buffer 和预认证状态，不收另传的 token。第一次选定 RFC 4178 的 GSS-SPNEGO 或直接 NTLMSSP 后不得换层；不扫描尾部、不从坏 DER 回退、不把客户端的账户名、目标、MIC 标志或非零 SessionId 当证明。[Linux v6.12 的原生 SMB 客户端](https://github.com/torvalds/linux/blob/v6.12/fs/smb/client/smb2pdu.c#L1708-L1846) 确有直接传两段 NTLMSSP 的代码，两种封装最终必须满足同一账户、挑战及签名要求。本段不接收 SMB1、NTLMv1 或匿名。

[初始 GSS 封装](https://www.rfc-editor.org/rfc/rfc2743#section-3.1) 只在客户端首条使用；[RFC 4178 §§4–6](https://www.rfc-editor.org/rfc/rfc4178#section-4.2) 的后续 NegTokenResp 不重复包装。保存客户端机制顺序及原 DER SEQUENCE（不含显式 [0]）给独立的 mechListMIC 校验。若 NTLM 不在首位，不使用其他机制的乐观 token 或早期 MIC，必先用实际 MORE 选择 NTLM 并请求名单 MIC；此后收到新的 Type 1 才接收 Type 2。优先 NTLM 无乐观 token 时也要先走机制选择。后续客户端不能再选机制，未知 DER 扩展有界跳过但不授予能力；即使客户端首条带有 Init2 的 NegHints，也只忽略其有界 SEQUENCE，不从提示名推出目标或身份。首答已 request-mic 而末条没有相应 MIC，按 §5(c)(V) 终止；即使形状和长度都正确也须实际核验，不能由“有字段”推出认证。

[MS-NLMP 三种报文与字段](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-nlmp/033d32cc-88f9-4483-9bf2-b273055038ce) 的有界视图保留原始 Type 1/2/3、目标信息及客户端声明；启用字段不超出本条、不别名固定头或其它有效字段，不解释按标志应忽略的描述符。[MS-SPNG 微软产品说明 <8>](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-spng/211417c4-11ef-46c0-a8fb-f178a51c2088) 区分最优先 NTLM 且带 NTLM MIC 的保护与原生直接 token 行为；本版对已选 SPNEGO 的这类输入也要求名单 MIC，不能切到直接封装来逃避。未宣告 Negotiate Late Fallback，不在半途重选。没有 GSS 分片重组、Kerberos、原生客户端验收或跨连接恢复；若后续改用服务端先行 GSS，需单审机制广告／NegHints 与出站封装；当前协商明确选择客户发起，不靠假广告。

该封装层产物只是 `Unverified`，不会生成挑战、校验口令、导出密钥、创建 `VerifiedExchange`、授予 Tree 或发送成功。真正的接受端还须拥有安全随机的一次性限时挑战与原连接/凭据代，校验原协商标志、目标/时间、NTLMv2 证明、所需 NTLM MIC 和双向 mechListMIC，并复核当前业务身份，再把该次最后请求的 transcript 和导出键一起交给签名器。不能只相信客户端的 MIC 指示位。该封装层的形状夹具不是有效口令证明。

## 严格 NTLMv2 机制证明（开发资料，未启用）

`session/strict_verifier` 消费同一次握手的原 Type 1、服务器实际序列化并逐字核对的 Type 2、最后一个 SMB 请求与其 Type 3。受保护的材料是针对**一个确切账户、域和代次**的预派生 ResponseKeyNT，等同该身份的秘密，不是 Web 哈希，不从网络接收；没有生产账户提供者、密码输入／读取、账户库或网络发包函数；仅在受保护的实际凭据代存在时，内部挑战器才取新的系统熵。候选挑战只能消耗一次，包括出错；核对原 SID／MORE 的原 MID 和有限期，把最终请求的原 SMB transcript 随证明保留。连接 owner 已把不可预测挑战、限额／防重复 MessageId、独占取出和原凭据代／实例终验连在同一请求上；缺真实凭据来源或尚未完成最终提交都不产生可用身份。

| 层 | 规范与本开发配置的决定 |
| --- | --- |
| NTLMv2 响应 | 使用[原服务器挑战及客户端原始 v2 blob](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-nlmp/5e550938-91d4-459f-b67d-75d70009e3f3)校验 NTProofStr，匹配受保护账户／域、服务器目标信息、时间及配置的 CIFS SPN。拒绝 LM/NTLMv1、匿名、替换账户或凑出的备用域。仅接受 Unicode、签名、ESS、VERSION 和 128 位等本配置的协商结果；标志不一致即拒。没选 KEY_EXCH 时用原 SessionBaseKey；选了时仅从本次 16 字节加密字段恢复导出键。这里只接普通无 TLS 通道绑定的 profile，不假造 CBT 或 SMB 数据加密。 |
| NTLM 内层 MIC | [MS-NLMP 客户端规则](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-nlmp/c0250a97-2940-40c7-82fb-20d208c71e96)：带服务器时间戳时客户端 SHOULD 发送；[AV_FLAGS 的 0x2](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-nlmp/83f5e789-660d-4781-8491-5f8c6641f75e)是客户端表示已经包含 MIC，不是身份或服务器强制开关。[服务端规则](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-nlmp/f9e6fbc4-a953-4f24-b229-ccdcc213b9ec)要求核已标注 MIC；[安全说明](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-nlmp/1e846608-4c5f-41f4-8454-1b91af8a755b)区分未标注的通用允许范围与篡改风险。**本版直接、SPNEGO 两路均硬性要求并实际核验；未标、缺失或算错全拒**，不以三平台兼容为由降级。原 Type 1＋2＋3 只清零 Type 3 的 MIC 本身；三报文均已确定带 VERSION，不猜 16 字节缺口。 |
| SPNEGO mechListMIC | [RFC 4178 §5(c)](https://www.rfc-editor.org/rfc/rfc4178#section-5) 与[微软产品说明 <8>](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-spng/211417c4-11ef-46c0-a8fb-f178a51c2088)：保护的对象是首个客户端原 MechTypeList DER SEQUENCE，不是 Type 1＋2＋3。当本严格配置选择 SPNEGO 时还必须独立校验客户端方向，并生成服务端方向的最终名单 MIC；方向、序号、签名键均不能混用。NTLM [ESS 签名](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-nlmp/a92716d5-d164-4960-9e15-300f4eef44a8)的可选 RC4 仅按协商保护短校验和、封装会话键，不用于文件、密码存储或新功能。最初就是直接 NTLM 时没有名单可签；已选择 SPNEGO 后不允许回退。 |
| SMB3.1.1 | SHA-512 preauth 绑定原 SMB 报文；它不是 NTLM MIC 或名单 MIC。只有持原 `MechanismProof` 的连接 owner 在同代复核后才可准备签名成功应答；提交原消息才发布本连接身份。普通命令另需强制 AES-CMAC、原活会话与业务授权。本实现无生产凭据来源、业务树、监听或线上口令。 |

Windows、macOS 及 Linux 均没有通过原生验收。[Linux v6.12 的 raw NTLMSSP 构造](https://github.com/torvalds/linux/blob/v6.12/fs/smb/client/sess.c#L1150-L1259)在这条普通路径构造 Type 3 而不计算本版要求的 NTLM MIC，故**该版本该路径不能通过严格配置**；能解析 raw token 不代表能挂载。未来不同 profile 须独立说明和审查，不能识别客户端名称后放宽或试错回退。

Apple 公开的[挂载认证选择](https://github.com/apple-oss-distributions/SMBClient/blob/99da2d9a4bbe17cde3cee382ad3edc16e25f5535/kernel/netsmb/smb_gss.c#L315-L355)与 [Heimdal 的 MIC 生成](https://github.com/apple-oss-distributions/Heimdal/blob/1635de38a813f6e1dd3f8fd270683187ad4f02be/lib/heimcred/ntlm.m#L306-L390)表明，生成正确的内层 MIC 并不表示两个时间字段完全一致：[NTLMv2 客户端 blob](https://github.com/apple-oss-distributions/Heimdal/blob/1635de38a813f6e1dd3f8fd270683187ad4f02be/lib/ntlm/ntlm.c#L1632-L1673)用本机当前秒写响应头的时间，另携带客户端提交的 TargetInfo。此处处理字段差异，不声称 macOS 已挂载，也不改变 Linux v6.12 缺内层 MIC 的结论。

[MS-NLMP 客户端规则](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-nlmp/c0250a97-2940-40c7-82fb-20d208c71e96)建议使用服务器发出的时间戳；[服务端核验](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-nlmp/34a9417d-7cc0-43b0-b61c-1f19740df66f)比较的是客户端 NTLMv2 响应**头**时间和服务端当前时间。原验证器现只在这个字段上采用固定的双向 120 秒偏差（本地策略，不是规范常量），每次证明验证只读一次服务端 UTC；不是拿发挑战时的 AV7 作为“现在”。原 Type 2 的 AV7 与全部原绑定的服务器 AV、SPN、原单调时间的 30 秒一次性挑战、原客户端 blob/NTProof、内层 MIC、应有的 SPNEGO 名单 MIC 和 SMB 强签均不变。无需按系统或名字选择分支，不改写原签过的时间或放开空 MIC。

按 [MS-NLMP 用户名规则](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-nlmp/f9e6fbc4-a953-4f24-b229-ccdcc213b9ec)，登录名比较只接受本地唯一已规范的 ASCII 账户，ASCII 字母大小写等价。原 Type 3 字节仍完整参与 MIC；不重新派生 ResponseKeyNT、不试另一个域或空域，同域、SPN、NTLM 内层 MIC、应有的名单 MIC 及 SMB 强签规则保持不变。这是认证账户名，不改变 ZXS 文件／目录的大小写语义。

`src/test/smb/strict_verifier.rs` 只存公开合成的完整原消息和固定答案。固定材料不对应实际账户，包含有／无 KEY_EXCH 的导出键、双方向名单 MIC，以及 v2 证明虽正确但无内层 MIC、不同但仍有效的头时间、双向边界、错 SPN、被换的原 AV7 或头/证明/MIC 等正反例；另列[微软规范公开的 SessionBaseKey 答案](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-nlmp/54973495-20d2-49e8-9925-c399a403ed4a)作交叉基准，不给生产补造凭据、连接或权限。仅静审源码，未构建、运行测试或实际认证。

### 末次回复的密钥、签名与原连接发布

只有严格证明仍对应原凭据代，才能用该证明的原末请求头和机制实际输出组装最终回复。根据 [MS-SMB2 §3.3.5.5.3 第 6 至 12 步](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/5ed93f06-a1d2-4837-8954-fa8b833c2654)，从同一次 NTLM ExportedSessionKey 和截至末请求的 SHA-512 分开派生 SMBSigningKey 与 SMBAppKey，各自的标签含终止 NUL；最终回复不再进入 KDF。单通道引用同一签名密钥，保留私有会话密钥和应用密钥，不导出给存储或 RPC。服务器按原请求的 MID、SID、charge、Reserved、优先级和应回额度，对零 guest/null/encrypt 标志、真实 GSS token（直接 NTLM 为零长，SPNEGO 有独立服务端名单 MIC）及整份原消息做 AES-CMAC 后设置 SIGNED。两条签名／分离密钥固定向量从同一原连接机制证明确认，不是实机结果。机制内封存的消息本身不授会话。只有持同一原 Claim 的连接能按下述顺序处理，文件授权不在这一步。

同一次服务运行另对最多 128 个协商成功的连接登记弱引用，按实收 ClientGuid 和实际选中的 dialect 建有界客户端表。在真实 GSS 末轮检索／插入方言；不一致不能接收成功，也不影响同 GUID 的其他会话。根据[连接丢失规则](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/eb5bfe99-47fe-4e87-8e87-08a084dcefb6)，只有该 GUID 的最后一条已协商连接消失，才在整理登记时允许清掉客户端项；GUID 不是主体。原 ProofTaken 项只归该收到的请求。最终签名包仍要核原本应回的额度，交给原发送者之前复核凭据和原期限；仅当**原发送者已完整提交同一不可变消息**，在同一注册／凭据代的短临界区内以该 Claim 转为 Valid，再结清原额度。提交不明、过期、断连或撤代不产生可用状态。密钥和主体只留在所属连接，本运行全局只留可撤销的弱标记；没有开放后台构造器、密钥 getter、生产监听或生产凭据读取。普通消息已能从这个 Valid 会话核签并短借原成员，树／打开表和实际命令派发仍未接线；不支持重新认证或绑定，不会借它刷新原有效会话。部署时真实传输必须按提交合同处理部分写／断开，并主动回收静默连接。

## 认证后的明文消息签名（已接原流 ECHO，未接通用派发与存储）

`session/` 实现[规范 KDF](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/da4e579e-02ce-4e27-bbce-3fc816a3ff92) 的一个 HMAC-SHA-256 计数块，从同一新 NTLM 会话的 16 字节导出键和最后一个请求的 64 字节 transcript 派生 128 位签名键。标签含结尾 NUL，KDF 自己的分隔零字节另算。当前没有生产账户或凭据来源；只有完成挑战、主体、MIC 和当前代检查的原尝试才能交出这一对，不能从报文、Web 身份或布尔值制造会话。密钥缓冲使用 zeroize；不承诺清除所有栈临时副本。没有使用宿主加密服务。

[协商](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/b39f253e-4963-40df-8dff-2f9040ebbeb1)没有 Signing Capabilities context 时仍强制 CMAC；显式 CMAC 或合法非空但无共同算法时应答须回且只回 CMAC。计数为零、被截断或串错请求/回复不能作“无签名”处理。最终回复的签名函数只接收与最终请求同 MessageId、同已分配 SessionId 的实际未签成功响应；拒绝 guest/null/encrypt 标记。它给该响应签名，不把成功响应再加入 KDF。普通明文请求只有签名位和全部 16 字节 [AES-CMAC 校验](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/5b570c0b-0854-4324-8fb5-d410692dde3e)均成立才通过签名检查。每段用原 SMB 消息和自己的尾填充，只有 Signature 16 字节置零；关联链继承的实际 SessionId 只用来选键，不改变验签字节。不相关链可涉及多个有效会话。入站先从已实际发出的窗口扣原整帧额度，随后整批签完才给出一次一个的会话成员短借；没有这里发出的成功应答或文件授权。普通回复头的生成/签名、CANCEL 和加密 transform 不由此入站函数处理。

`src/test/smb/signing.rs` 保留两种协商下首个 TREE_CONNECT 的独立固定值、无签/错签拒绝，以及同链/异会话 compound 的原字节向量。测试只使用公开合成值，没有执行，不是实际登录或互通。原生三端验收仍须分开“无缓存端点的首次访问”和“同一身份重连”；记录 Windows 冷访问有无 multi-protocol opener，旧服务缓存的热路径不是首连通过。不增加 SMB1 文件操作或降级。

### 原连接认证成员：先验整批，再逐条短借

`connection/sessions/ordinary` 只消费同一原连接的 `Received`。对原帧中所有受支持的普通成员，按[签名检查](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/85df1680-2ee7-4d25-a916-a982371ddc75)与[会话检查](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/b495e2da-8711-4772-b292-453be0394b49)找该连接已经真实发布的 Valid 项；同一批只认同一受保护凭据代，先核每段原始签名，末段不通过也不给前段票。无 Signing Capabilities 与明确选 CMAC 的两种情况都不准首个普通命令缺签。只完成 MORE、别的连接知道 SID、报文写了有效 TreeId 都不是授权。[TREE_CONNECT 的专门规则](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/652e0c14-5014-4470-999d-b174d7b2da87)优先于通用签名拒绝：在这个仅明文 3.1.1、非匿名/非 guest 实现上，缺签名且确属本连接已发布会话的 TREE_CONNECT 须断开本连接，不能回普通拒绝继续使用。关联成员按已核前件的会话归属，不采信其报文内另一个 SID；未知或别连接的标识不撤销他方登记。已发布的项不再强持 NTLM 密验材料，保有的是该会话自身密钥及可失效的原代弱标记。

整批通过后仅能按顺序短借同一原消息和原会话，并在发给业务前逐项重核原登记、连接和凭据代。接线方在实际使用／提交时仍须终验，不能长期保存布尔结果；不把全局或凭据锁带入网络／存储。合法消费编号但载荷计费不足的成员没有业务票，保留成员级参数错误；后续不相关成员仍独立，相关链保留前件失败而不虚构 Tree／Open 或统一错误码。[相关链执行](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/46dd4182-62d3-4e30-9fe5-e2ec124edca1)必须用紧邻命令的真实处理结果；验签使用的继承 SID 不发共享权限、Root/FILE 权限或 MaximalAccess。此通用门本身不答复、不执行文件操作或 CANCEL；下面只有真正已签 ECHO 已接同步出口。遗弃未结的应答账会关闭原连接。`src/test/smb/ordinary.rs` 的公开合成源例从同一次协商、挑战和签名末包进入此门，没有执行测试或实机。

### 第一条同步普通应答：已认证 ECHO

只给尚未消费、整批均为已认证 ECHO 的原 `CheckedBatch`。每个实际短借按[处理规则](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/fa23e613-2768-49e5-aa68-97c4bba72379)核所在连接及原会话，[请求](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/d939504d-57e2-4c0e-8ad5-1678b6fccca1)的 Reserved 不当成拒绝条件；合法返回[4 字节成功体](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/2abe9b3c-c5ab-417f-bcc3-9ab51f2fce35)，尺寸不符返回固定参数错误。没有传入任意 NTSTATUS／裸正文的成功捷径。相关链只延用上一个实际 ECHO 已核的会话，不授 Tree/Open；不相关的两项可以各自用不同真实会话。无会话、无签 ECHO 的连接级路径不在本片，不能据此判断三端可挂载。

按[成功头](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/d39a64be-b8f8-424d-b8e5-a2cf5257459e)、[同步复合](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/dde8b59f-52ca-4cf8-8253-211bc43639cf)与[出站签名](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/a3e9ea1e-53c8-4cff-94bd-d98fb20417c0)逐项保留原 MID／优先级／Reserved／charge，填真实授额、有效 SID 和零树标识，每个成员用各自通道 CMAC 覆盖整个应答段，包含中间和末段零填充。只保留一份带 Direct TCP 前缀的不可变记录：原连接私有写端限时完整写入，部分写继续、被中断重试，错误／零写／撤代／去向不明均关闭原连接；仅整条写入本地 TCP 发送端且复核原登记后结清额度，不等同对端已经收到。无外部可注入的生产 writer，也没有 `submitted=true` 替代。下述受托入口从 NEGOTIATE 前持有原收发流；没有生产 accept/监听或账号来源。`src/test/smb/echo.rs` 保存默认／显式 CMAC、错误、相关尾填充、两会话及短写失败的合成固定答案，未运行。

## 首个 TREE_CONNECT 入口（确定错误可签回，不发树）

[MS-SMB2 §2.2.9](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/832d2130-22e8-4afb-aafd-b30bb0901798) 的名称字段以本成员头为原点；范围不能借下一复合成员的字节。只接受有界、无 NUL 的 UTF-16LE 普通 UNC `\\服务器\ZXS`；ASCII 共享名不分大小写，盘和文件名规则不变。拒绝超限或确定非法的普通 ASCII DNS 标签；短 NetBIOS 和非 ASCII 拼写仍交实际可信别名／共享作用域核对，不改写名称、不进行 DNS。不重新解释集群重连、请求扩展、DFS 或相关后件；普通盘共享无重定向，客户端能接受重定向的提示不授能力。同一已支付、整批已验签的 `VerifiedMember` 保留在每个结果里，重核原会话代。字段非法、共享名不符、会话失效各有独立失败域，不拿一个假 `TreeId` 或默认状态代替。活的 3.1.1 非访客／匿名会话缺少应有的明文签名时，原连接按 [§3.3.5.7](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/652e0c14-5014-4470-999d-b174d7b2da87) 断连，不会有这个已验成员。

已付额度、验过整帧签名的独立普通 TREE_CONNECT 可由原 `Peer` 直接结清确定的失败。按 [§3.3.5.7](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/652e0c14-5014-4470-999d-b174d7b2da87) 区分格式错误 `STATUS_INVALID_PARAMETER` 与不可能匹配的其他共享（含 IPC$）`STATUS_BAD_NETWORK_NAME`；另有真正的本地分配失败。不回重定向数据／句柄或成功，也不冒充仍需单独处理的复合、扩展或重连。ECHO 与此明确失败共用封闭的同步签发出口；原 MID、会话、额度和不可变回复在同一连接核对，整条写完且复核后才公布额度。正确的 ZXS 请求仍归真实处理者，不能因无 Core 授权而签出空树。

共享授权还缺真实 Core 统一根发行口。届时 `TreeConnect.MaximalAccess` 应来自该主体在共享文件策略下的权限上限；不能拿虚拟根本身不许增删，就把整个有 RW 子盘的共享描述成只读。[原生 macOS 的挂载逻辑](https://github.com/apple-oss-distributions/SMBClient/blob/99da2d9a4bbe17cde3cee382ad3edc16e25f5535/kernel/smbfs/smbfs_vfsops.c#L1555-L1565) 也会据此及卷级只读标志决定整个挂载是否只读。但共享上限不授子盘、目录或文件访问：RO 子盘不能写，回收子树只读加经业务验权的同树恢复，真实写入仍逐操作核当前 Core 盘模式和原代；切回 RW 也不复活失效的旧写票。未发布共享上限、对象 `MxAc`、共享表或成功应答。合成源例只证明该形状拒绝把 RW、RO、回收当成另一共享或 TREE_CONNECT 的后缀；混合授权仍须真实适配后的验收。

## 文件 I/O 请求与原打开边界（未接线）

[READ](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/21e8b343-34e9-4fca-8d93-03dd2d3e961e) 和 [CLOSE](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/f2b4183a-29c2-4fc0-a5de-fe290408f68c) 从各自命令取得范围或关闭意图；普通引用携两半 SMB2_FILEID，关联引用只继承同一链紧邻前件的已证实 ID，不能以客户端填入的另一组 ID 或最近打开回退。实际入口仍须验签、验 credits 与当前连接／会话／树，再以两半与原代次匹配唯一真实 OPEN；本片没有认证发行者或第二张路由表。后端入口只接已成功绑定的 Held，不收线上的 FileId、路径或替代对象。

READ 保持原请求长度供 MaxReadSize／credits／范围锁核验，偏移与终点不得超出有符号文件范围。仅 FILE 的实际 FILE_READ_DATA 可读，不把目录的列出权、执行权或共享票当读许可；原会话、媒体／视图、范围锁及输出前后的资格由真实适配复核。只在原适配确认后区分满足最低计数且非空的短读、EOF／小于 MinimumCount、权限或 I/O 错误。TCP 不接受 RDMA 描述符，不把实际无法提供的直接读冒作成功；压缩提示本身不协商能力。回复容量不足不伪装成 EOF，也不暴露不完整校验的正文。

显式 CLOSE 消耗原打开的持有权，不等于异常断连。以后接线须先令该 OPEN 不可再路由并结束在途操作；`POSTQUERY_ATTRIB` 只返回原句柄可获且有权的真实属性。属性取不到不冒充有效；关闭与脏普通写句柄的最后确认失败单独保留，正常写关闭时由 Core 在原写权内收尾，释放底层对象后才退共享票。丢连接／撤权只清理，绝不能隐式写穿、flush 或发布历史。这部分没有真正后端接线。

成功 READ／CLOSE 应答体只能消费 Held 产生的私有结果，不能用任意正文、请求计数或属性生成成功。READ 保持真实返回长度，固定体后仅借已核正文；[MS-FSA](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-fsa/aa57e052-40d4-4d9f-837d-2b0ce7c87cf4) 的零长底层成功不等于线上空成功，[MS-SMB2 §2.2.20](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/3e3d2f2c-0e2f-41ea-ad07-fbca6ffdfd90) 规定实际返回 0 字节应答 EOF，包括合法的零长度请求。EOF 或未达 MinimumCount 用 ERROR 体，不夹带部分正文；原始 Core 错误保留给真实状态映射。CLOSE 只在请求、原 OPEN 实授 FILE_READ_ATTRIBUTES 且属性确实取得时标记有效；缺属性权的正常关闭仍执行，但不查询或回送元数据。这里只定义命令、状态和体，不构造协议头或发包；未来唯一应答出口核同一已验证请求与会话，确定长度、复合对齐及原字节后才签名。当前无监听、认证发行者或实际 Core 调用。

[WRITE](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/829f93f5-ed10-4f12-8347-42d235019459) 只借用本命令声明的正文，不含其他命令、预填字节或尾随填充；原请求长度供 MaxWriteSize 和 credits，TCP 不消费 RDMA 描述符。追加位置来自同一个仍获权的实际写者的当前稿 EOF，不看旧读句柄的长度，不把对 EOF 写或空写当作 SET_END_OF_FILE。仅追加的权限不改写或填出空洞，只能命中该 EOF；跨出原长需要明确追加权。普通写确认原写者已接收该范围，不承诺已经持久化。

逐请求 WRITE_THROUGH 不是 CREATE 的 FILE_WRITE_THROUGH 模式。按 3.1.1 组合规则，普通缓冲打开上只有逐请求写穿标志时返回参数错误；真正的直接写没有接，不退化成缓冲成功。[Apple 原生源码](https://github.com/apple-oss-distributions/SMBClient/blob/99da2d9a4bbe17cde3cee382ad3edc16e25f5535/kernel/netsmb/smb_smb_2.c#L10684-L10720) 有特定错误后的重试，不代表该平台已验收。原持有打开要求写穿时，实际写入后还必须等同一写者完成确认；[显式 FLUSH](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/026984f6-38af-4408-8200-50557eb0a286) 也只让实际有写权的原 FILE 写者确认；目录权限不变成卷同步。失败仍留在原写者与原批次的状态机，不冒充刷盘、成功关闭或正式提交。不提供伪造写者、额外正文或实际调度。

WRITE 和 FLUSH 成功各有私有 Held 收据。按 [WRITE 响应](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/7b80a339-f4d3-4575-8ce2-70a06f24f133)，Count 来自原写者核实的实际接收字节数，不从请求的 Length 生成；当前业务要求整段成功，计数矛盾不给成功，也不推断回退或自动重写。合法空 WRITE 可回实际 Count=0，这与 READ 的 EOF 不同。[FLUSH 响应](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/42f78e6a-e25f-48f5-8f08-b4f1bb4c4fa4) 只在原确认完成后能生成；原 I/O 失败与确认失败分开传给真实状态映射，不能包装成成功体。编码仍不构造协议头、不签发会话、不发包。

`wire/set_info` 只从仍有效、已签名／付额度的原成员读 [FILE SET_INFO](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/ee9614c4-be54-4a3c-98f1-769a7032a0e4)，用同连接 MaxTransactSize 和完整声明长度；不借下一成员或缓冲区尾填。普通 [FileDispositionInformation](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-fscc/12c3dd1c-14f6-4229-9d29-75fb2cb392f6) 的 mark/clear 与 CREATE 每个 OPEN 的 FILE_DELETE_ON_CLOSE 分开；后者必须明确请求 DELETE，再由真实对象授予。文件 [EndOfFile](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-fscc/75241cca-3167-472f-8058-a52d77c6bb17) 是非负绝对位置，只能交给已按 FILE_WRITE_DATA 授权的同原暂存 writer；不当作 allocation、append 或普通 WRITE 偏移。Ex、DFS 和 replay 不从本通道读取。复合的 related 成员只能用本链前件的实际 OPEN。两半 FileId、具体树／对象、当前写权、回收和落盘仍需真核，本层不改游标、文件或发成功。

[Apple 定版删除](https://github.com/apple-oss-distributions/SMBClient/blob/99da2d9a4bbe17cde3cee382ad3edc16e25f5535/kernel/smbfs/smbfs_smb_2.c#L7943-L7967) 有 related CREATE→SET_INFO→CLOSE；[Linux 内核源码](https://github.com/torvalds/linux/blob/v6.12/fs/smb/client/smb2inode.c#L1043-L1054) 与 [libsmbclient 定版源码](https://github.com/samba-team/samba/blob/2750ed68433690b5f72c54ae54226e89ee1b9d74/source3/libsmb/cli_smb2_fnum.c#L1155-L1169) 的普通 unlink 则有 CREATE delete-on-close。只保留各自真实请求形状，不宣称任一端已删到回收。ZXS 的回收业务仍只在原 OPEN 的**正常最后一次 CLOSE**且原对象和权限均成立时执行。[协议要求掉线时关闭不保留的 OPEN](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/eb5bfe99-47fe-4e87-8e87-08a084dcefb6)，但这不把异常断连、LOGOFF、停机或 Drop 变成 ZXS DELETE/FLUSH 入口；只退场和释放，不提供新授权，也不偷偷执行回收。

## 原 OPEN 的路由与退场（接线约束，未实现）

- 真实连接先预验完整复合请求的原字节签名及当前会话，再逐成员借出有当前主体与代次的票据。这个票据不从 UUID、裸 SID、SQL 布尔或单个签名器构造。私有原流已能借出经认证签名的成员，但没有生产凭据和 Core 业务树的发行口；成员、后端 Held 和应答体各自不是上线权限。由原会话和树核权限，查找及失败优先级逐命令遵循规范：READ 是 Session→Tree→OPEN，CLOSE 的标准顺序为 Session→会话内 OPEN→Tree。CLOSE 查到 OPEN 仍不能绕过核对它的原树；不能把撤权、树关闭、失效 OPEN 混成 EOF。关联成员只用本链**紧邻前项**实际已确认且仍可用的 Session／Tree／OPEN；前件没有或未取得相应范围时按 [关联请求规则](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/46dd4182-62d3-4e30-9fe5-e2ec124edca1) 失败，不能补猜独立 ID。
- 每个原会话拥有唯一 OPEN 表，条目只持有其准确原树作用域下经 `Pending::bind` 核过的 Held。先限资源、取得实物和最终原权，再发服务器自己的两半 ID 并公开；Volatile 在会话范围唯一，即使表按树分区，查出后同时核 Persistent、原树与代次；同一会话代内不得把已退休的 ID 对重新发给另一个 OPEN，旧会话不能经同名目录重找。表不取代跨会话 ShareAccess 表或 Core 单写权。Core 原树须在其借用的实际文件／目录句柄之外存活，持有本地树再建立表，销毁时逆序；不靠不安全的自引用或新的业务存储。
- 原 OPEN 可读借用，写／FLUSH 独占借用，正常 CLOSE 先摘路由阻止新请求，再取消或排空已受理操作，取得同一原件完成一次关闭。异常断连、撤权及 scope 退场是清理，不伪造显式 CLOSE 或隐式 flush；已确认和待确认由 Core 区分。底层原件保留到在途操作、相应权限和确认处理收尾，先释放它再退共享票。签出最终响应时仍核与原请求／当前会话对应，规则没有开通前不启监听。

## 目标与分工

只协商 0x0311，直接访问一个磁盘共享 `ZXS`。只有配置的 SMB 账户可用，Web 管理员不是 SMB 账户。共享根是虚拟目录：下挂被授权的数据盘和专用回收站，不呈现 SYSTEM、真实宿主目录或容器内部。共享名匹配与文件组件的精确大小写/原拼写是两层决定；不对所有路径做不分大小写归一化。服务为现有 zxs crate 的私有模块，不依赖兄弟项目或 Samba 的协议处理器。

| 边界 | 责任和不可越过的线 |
| --- | --- |
| `wire/` | 验证长度、端序、请求方向和关联/非关联链；保留原始字节。命令体、credits、签名与业务授权须分别验证。客户端名、GUID、ID 和 capability 位均不是权力。 |
| 认证/会话（后续） | 受信的实际接受端创建一次性、限时的挑战，绑定原连接、具体握手、实例、唯一账户与凭据代；完成标准机制、校验保护并重新核当前代后才授不可伪造的业务会话能力。拒绝 guest/匿名/空密码、LM/NTLMv1、错误域、Web 登录与自报布尔值。按连接、会话、树和实际 OPEN 划分生命周期及额度。 |
| 操作与存储适配（后续） | 已验证的会话/树只取得当前范围的对象能力。SMB 的 FileId 二元组仅查此会话/树的打开表，不是存储 FileId、路径或权限证明。一次跨会话协调 ShareAccess 双向关系、范围锁、删除待定；另向 Core 取得跨入口唯一的每 FILE 写权或已确认读视图，不叠加另一套业务写锁。先有界预留，再在不持全局表锁时取得物件权；两边都成功才公开句柄，失败逆序释放。 |
| 维护边界 | helper 离线开户/重置；Web/存储不取得 SMB 长期密验或会话签名密钥。现行 Samba 路径的 SQL 只记 SMB 非秘密绑定，密码验证归 tdbsam。Rust [凭据记录](credential-record.md)只做格式解析，不是可信发行者；开户、撤代和启动另审，绝不从 Web、测试构造器或伪造 Samba 私帧填空。共用 crate 不表示网络解析器能打开 SYSTEM、账户库或裸磁盘。 |

真实凭据切换、受保护的发行者/存储通道、服务监听/启动、泄露面和（若有）迁移须单独共审，不能自启动、双开同一端口或回落旧认证。对不可用的存储适配返回明确失败，不创建假 handle。写穿、脏 FLUSH、正常脏 CLOSE 只有 Core 应有材料与事务确认后才可回成功；干净关闭和异常断连都不是手动提交，断连不擅自保存未确认输入。共享模式或账户代变化必须撤销对应资格。物理回收站内不是任意写入或永久删除的后门。更多业务语义由 Core 维护，不在 wire 重写。

## 实现与审查次序

1. **本地有界入站。** 每个 Direct TCP 帧上限 2 MiB；最多 32 条命令；初次 NEGOTIATE 上限 68 KiB / 64 个 contexts。读取四字节长度即可在分配前判断，不要求 TCP read 等于一条 SMB 消息。保留完整协商字节和每段包括 padding 的字节视图。边界正确不代表已验证所有命令体。重复 PREAUTH、加密、压缩、RDMA 变换和签名 context 不是合法的独立能力；未知 context 只按声明边界跳过。加密和签名算法列表在处理该可选 context 时按它自己的长度解析。当前不处理 SMB1 初始包装、加密或压缩 envelope，不提供明文降级。解析器不回成功，也不启动服务。
2. **协商保护与真实认证。** TCP/445 的 3.1.1 要有实际的 multi-credit / LARGE_MTU：每连接维护不重复且可乱序的 MessageId 序号窗口，按请求/应答大小计费，CANCEL 关联原在途请求、不是新信用。资源上限不等于所有命令只能占一个 credit。同次服务初始化的全局 ServerGuid、每个回复的新 CSPRNG 盐、准确安全模式与实际支持的 contexts 都由上方成功应答模块分别把关，仍欠真正启动所有者。连接 preauth 从 64 个零字节开始，逐次 SHA-512(旧值 || 原始 SMB 消息)，不含 TCP 四字节。会话从已完成 NEGOTIATE 的 hash 分支，只加入该会话的 SETUP 往返以及最后的请求；用于签最终成功响应的派生快照**不含这条成功响应本身**。
3. **消息级保证。** SPNEGO/NTLMv2 需要真实的机制/主体选择、对应 flag 和 target-info、挑战时效、适用的 NTLM MIC 与 SPNEGO mechListMIC、导出的会话密钥。按规范的 SP800-108 和准确标签派生 SMB 密钥。AES-128-CMAC 为签名基线，未签或签名不符的需签请求不得触发业务。复合包逐段验含 padding 的签名，关联链不能从其他会话/树借句柄。标准密码原语优先选取可审的最小上游源码（例如 RustCrypto 对应原语）；记录精确版本、摘要、传递来源、许可证和改动。不得自制算法、手抄 Samba handler 或为 NTLM 关闭宿主密码策略。不记录密验、报文令牌与会话键。
4. **先接一条真实的目录访问。** 以可信凭据发行者和 Core 统一根为前提，先闭合 `TREE_CONNECT → 根 CREATE → QUERY_DIRECTORY → CLOSE` 的强签往返：用同一个真实树及 OPEN，按权限和分页返回原对象，再释放；不造占位根或句柄，不把这一步当三端验收。随后接标准元数据、普通文件读写、改长、FLUSH、SET_INFO、LOCK、删除待定、重命名／替换、树／会话正常退场，再验实际回收恢复。已认证 ECHO 已走原流；CANCEL 的不占新序号、不回本命令／不发额度已在账务层区分，实际原请求取消仍须接真实 RequestList，不能等同于任意异步或变更通知。
5. **安全互通另审。** 如确有加密部署目标，AES-128-GCM 的协商、每方向密钥、nonce 及 transform 验证独立审查；未实现不宣告。GMAC、其他 cipher、压缩、RDMA、QUIC 不在最小配置里。

## 原生客户端的验收边界（未执行）

- **Windows 资源管理器：**用已知 UNC。微软[产品行为附录 <130>](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/a64e55aa-1152-48e4-8206-edd96444e7f7)记录首次多协议协商与已缓存方言后的 SMB2-Only 是不同入口。分别记录全新端点首次连接和同身份重连的首包、版本和策略；本配置不接旧格式开局，不以缓存成功冒充冷连接。若策略[要求加密](https://learn.microsoft.com/en-us/windows-server/storage/file-server/configure-smb-client-require-encryption)，当前签名明文配置不满足。
- **macOS Finder：**用 [`smb://服务器/ZXS`](https://support.apple.com/en-gb/guide/mac-help/mchlp1660/mac)。Apple 定版源码的[挂载卷属性查询](https://github.com/apple-oss-distributions/SMBClient/blob/99da2d9a4bbe17cde3cee382ad3edc16e25f5535/kernel/smbfs/smbfs_smb_2.c#L2855-L2879)会在新开的 CREATE 里带 `MxAc`，查询 `FileFsAttributeInformation` 时另带 `AAPL`；没收到 AAPL 识别回应可[判作非 Apple 服务器](https://github.com/apple-oss-distributions/SMBClient/blob/99da2d9a4bbe17cde3cee382ad3edc16e25f5535/kernel/netsmb/smb_smb_2.c#L4227-L4245)，不等于整次挂载已通过。有效的 [`MxAc`](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/828a4d2d-0c9b-4c1c-8d2b-cc474172baa5) 请求要按原对象权限和真实变更时间回，不能借共享上限或伪造全权。这条源码[在 3.1.1 挂载跳过旧式协商校验](https://github.com/apple-oss-distributions/SMBClient/blob/99da2d9a4bbe17cde3cee382ad3edc16e25f5535/kernel/smbfs/smbfs_vfsops.c#L1167-L1179)；若真收到 `FSCTL_VALIDATE_NEGOTIATE_INFO`，[3.1.1 规范要求断连](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/0b7803eb-d561-48a4-8654-327803f59ec6)，不是待补成功的 IOCTL。
- **Linux 内核客户端：**用已知共享；[v6.12 源码](https://github.com/torvalds/linux/blob/v6.12/fs/smb/client/smb2pdu.c#L5358-L5394)已给出带 `*` 的目录请求和实际信息类，是普通列目录依据而非兼容证明。上述 raw NTLMSSP 的内层 MIC 缺失仍单独挡在登录前，不因普通命令支持、签名或时间字段改变而通过。
- **Linux 桌面用户态候选：** [GNOME Files](https://help.gnome.org/gnome-help/nautilus-connect.html) 使用同一已知共享；Debian 13 的 [GVfs 后端](https://packages.debian.org/trixie/gvfs-backends) 依赖 `libsmbclient`，其[定版客户端源码](https://github.com/samba-team/samba/blob/2750ed68433690b5f72c54ae54226e89ee1b9d74/auth/ntlmssp/ntlmssp_client.c#L769-L809)有 NTLM 内层 MIC，和 stock 内核路径不同。只属待验证的用户态浏览／编辑入口，不是 `mount.cifs`／POSIX 挂载验证，不给 ZXS 服务端增加 Samba 依赖或放宽认证。
- 基本验收需原生完成：正确账户可登录、错误凭据和匿名被拒；列根和多页目录，在一块盘离线时仍能读另一块在线可见盘，普通／回收两类对象按授权读取；在可写普通盘创建、偏移写、flush、重开读回，精确大小写和并发单写；当回收、改名或应用级保存声称可用时，另验实际动作和中断。直接 TCP 成功、NEGOTIATE、脚本内读写都不能代替。业务上强制 NTLM 禁用/Kerberos-only 的部署不属于本地 NTLMv2 配置，不能静默退回匿名。
- 所有 Rust SMB 测试源码放 `zxs/src/test/smb/`，从产品私有作用域用 `cfg(test)` 接入。不为测试公开认证/文件能力。区分静审、密码向量、三系统互通与真实存储故障，分别记录，不混称通过。

原始规范：[Direct TCP](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/1dfacde4-b5c7-4494-8a14-a09d3ab4cc83)、[协商](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/b39f253e-4963-40df-8dff-2f9040ebbeb1)、[复合包](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/fa6687f5-99d4-4c9b-ba2e-a770310225e0)、[GSS 会话处理](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/5ed93f06-a1d2-4837-8954-fa8b833c2654)、[MS-SPNG](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-spng/f377a379-c24f-4a0f-a3eb-0d835389e28a)、[NTLM 服务器校验](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-nlmp/34a9417d-7cc0-43b0-b61c-1f19740df66f)、[文件信息类](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-fscc/4718fc40-e539-4014-8e33-b675af74e3e1)。
