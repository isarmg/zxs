# ZXS 内部 Rust SMB 服务

**当前是未接线的协议、严格机制校验、签名与权限前置，不是可挂载的服务。** `wire/` 能分开 Direct TCP 帧、校验整条请求链的 SMB2 头和边界、读取客户端的 3.1.1 协商提议。`backend/` 能为已选对象有界预留读、写、删共享许可，兼容判定完成不等于已取得 Core 写权。此段没有生产存储句柄的签发者，也未实现目录首开特例、范围锁或删除待定。客户端说“支持”不等于服务器同意。已能生成未发送的独立 3.1.1 协商成功消息；无生产服务实例提供者，不监听 445，不签发生产认证、会话或共享。现行 Samba 运行入口不调用这里；没有编译、运行测试或进行真实客户端验收。

`wire/path_fields` 从单条 CREATE / SET_INFO 普通 rename 消息的声明范围提取名字，`backend/path` 是唯一业务名字解析：严格 UTF-16、以同一共享为根，逐部件调用现有显示名规则，保留大小写和 Unicode 原拼写，不从名字判断数据盘／回收站，不解释宿主路径。CREATE 空名和未处理的 contexts 一起保留，不把重连请求误作根权限。rename 源是本次协议句柄，目标的替换标志不是覆盖授权。关于多部件目标，[微软 SET_INFO 条文](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/43cf22fe-84cb-49cf-b3e7-e3d7d766a7b7) 的分隔符限制与 full pathname 表述并存；[Linux 客户端](https://github.com/torvalds/linux/blob/v6.12/fs/smb/client/smb2inode.c#L1076-L1112) 会编码目标路径。此处为 ZXS 同树移位保留这个目标形状，不表示任何原生客户端已完成还原。具体成员字节额度、当前盘和删除原单仍由 Core 选择和复核。

目录和元数据查询目前只取请求与预算，不调用文件系统，也不产生成功应答。按 [QUERY_DIRECTORY](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/29dfcc9b-3aec-406b-abb5-0b4fe96712e2) 与 [QUERY_INFO](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/abb48417-27e5-4a16-82e0-3e5981db97e8) 保留原请求长度供 credits 核验，不把客户端 FileIndex 或 SMB2_FILEID 变成 Core 游标。查询目录的模式是单个分量，不可当路径；没有新模式不能让续页丢失原筛选条件。[Apple 原生实现](https://github.com/apple-oss-distributions/SMBClient/blob/99da2d9a4bbe17cde3cee382ad3edc16e25f5535/kernel/netsmb/smb_smb_2.c#L8315-L8369) 会给空模式留 offset 96 和两个零字节，当前仅和标准的零 offset 一并识别为空模式。[Linux 客户端](https://github.com/torvalds/linux/blob/v6.12/fs/smb/client/smb2pdu.c#L5358-L5394) 与 [Apple 客户端](https://github.com/apple-oss-distributions/SMBClient/blob/99da2d9a4bbe17cde3cee382ad3edc16e25f5535/kernel/smbfs/smbfs_smb_2.c#L8176-L8185) 的源码分别给出目录信息类选择；这是格式依据，不是原生挂载通过证据。

一页只为有权、匹配的逻辑顺序前缀预算，独立限制条数、客户长度和复合应答剩余字节。只在记录之间算 8 字节对齐；首条装不全、下一条装不下、固定部分都装不下不是同一结果。真实适配须按原 OPEN 重核列出权，再按来源的真实排序与集合结束标记推进，不消费未回的匹配条目；不能直接用 main v7 取页即推进的旧游标后丢弃尾部，也不把过滤致空当结束。[MS-FSA 目录查询](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-fsa/fa8194e0-53ec-413b-8315-e8fa85396fd8) 的模式延续、匹配、点记录、溢出数据和结束状态仍须接线。元数据按已持对象区分虚拟根、目录和 FILE；访问属性和具体卷必须来自原始授权对象。虚拟根不挑任意实体盘冒充总容量，不对下线盘填假数据，不返回宿主路径或私有标识。

## 每条直连 TCP/445 的 MessageId 和 credits（未接监听）

`connection` 从原始 3.1.1 NEGOTIATE 请求建立一次性起点：这时只允许 MessageId 0。保留真实客户端 GUID、安全模式、capabilities 及**完整原序的 dialect 数组**，而非仅记录是否见过 3.1.1。根据[协商处理](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/b39f253e-4963-40df-8dff-2f9040ebbeb1)，TCP/445 上 3.1.1 必须支持多额度，服务器成功回复须含 LARGE_MTU；客户端初始请求即使要 0 也至少回 1。仅已序列化且匹配本次请求、SHA-512 transcript、应发额度、实际读／写／事务大小的回复才能绑定；原发送者提交了这份字节后，才产生仅限原连接的可用窗口。已协商的连接不能再次走 NEGOTIATE。现在由同一个 Opening 生成下节所述的实际成功消息，但不写网络、不启用传输服务。

- [服务端窗口](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/dec8e905-9477-4c3f-bc64-b18d97c9f905)和[序号验证](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/0326f784-0baf-45fd-9687-626859ef5a9b)：额度对应已实际发出的、单调新发的编号。可乱序接收，不能重用、跨空洞，或在 u64 末端回绕。每成员从自身 MessageId 消耗至少一格，正多额度需整个连续区间仍可用；关联成员不继承 MessageId。整批只能在同一**事前已发**窗口上扣减；任何范围无效即关本连接，不逐项执行后再发现越界。本地最多 64 格跨度，且可用额度加未完成普通请求不超过 64；不能靠不断填高编号扩张内存。这是额度记录，不替代原连接的 RequestList、会话签名或业务权。同步票已把不可换包的原字节与该项账务合在一起。将来的 RequestList 仍须按 §3.3.5.2 给每个非 CANCEL 成员登记原请求，并与后续验签、派发和在途取消绑定；不能拿账务结果另配 SESSION_SETUP 或业务帧。 本端只公布可兑现的 MaxWriteSize，不超过约束整个入站 SMB 消息的 MaxTransactSize；这是本地入站边界的约定，不要求三种协议上限相等。MaxReadSize 的数据方向相反。
- [载荷校验](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/fba3123b-f566-4d8f-9715-0f529e856d25)独立于序号：零计费不表示零消费，最多覆盖 64 KiB；正计费按每格 65,536 字节。READ、WRITE、QUERY_DIRECTORY、CHANGE_NOTIFY、QUERY_INFO、SET_INFO、IOCTL 用原始声明值。查询信息按入／出最大值，IOCTL 两边分别在 u64 相加后再比较。错误或截断不能变成少计费的成功。已花合法编号但载荷不足是成员参数错误，不归还旧编号、不伪称已操作文件；前序认证、树／原 OPEN 失败仍按对应命令的规范顺序处理。
- [CANCEL](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/57bae3d3-5dd7-4a5f-92cb-fc52e2087dad)只豁免这一步的序号消费：同步／异步形式不占新格、不产生本命令应答、不赠额度。查找同连接的原在途请求、核原签名／身份、是否取消及原操作结果留给唯一 RequestList；不能仅凭重用的 MID/AsyncId 取消工作。
- [授额](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/2e366edb-b006-47e7-aa94-ef6f71043ced)与[同步复合回复](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/dde8b59f-52ca-4cf8-8253-211bc43639cf)按成员对账。候选额度尚不可花；服务端先对最终不可变回复整链核同一次批次的原 MID／command／CreditResponse，待原传输确认成功提交才一次公布。跨连接的批次、重复公布、丢失普通应答、传输结果不明、编号耗尽都不能恢复旧额度。当没有剩余额度时，服务器须补至少一格或关闭耗尽的连接，不能留下无法继续的伪活动连接。此出口不允许异步 interim/final 冒充同步回复；未来异步只允许在 interim 发一次额度。该模块只核信用，不把正确签名、可用状态或授权视作已成立。

`src/test/smb/credits.rs` 保留初包、乱序、多额度、关联／非关联、CANCEL、原载荷边界、同步回包对账和整数末端的源级例子。未运行客户端或测试，也未对生产端口发包。

## 原生 NEGOTIATE 成功应答（不含认证成功或网络发送）

对一条合法的全新 Direct TCP 请求，`connection/negotiate` 从原 Opening、应授额度和同一次服务运行的 GUID 出发，使用已有的 Linux `getrandom(0)` 封装单独取 32 字节公开盐，并按 [FILETIME](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-dtyp/2c57429b-fdd4-488f-b5fc-9e4cf020fcdf) 记录服务端当前 UTC 时间；不给空标识、随机失败或越界时钟造替代值。`wire/negotiate_reply` 写[同步成功头](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/d39a64be-b8f8-424d-b8e5-a2cf5257459e)与[完整协商正文](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/63abf97c-0d09-47e2-88d6-6bfa552949a5)：只选 0x0311／SHA‑512，安全模式是启用且必须签名；只公布 LARGE_MTU，读／写／事务三项目前均为 64 KiB。上下文相对 SMB2 头 8 字节对齐；不宣告加密、租约、多通道、持久句柄、压缩、RDMA 或通知。客户端的 cipher 清单只是 offer，不是“强制加密”开关；要求数据加密的客户端／共享不得作为已加密使用，更不能静默退回传明文。

没有 Signing Capabilities 时仍强制 AES‑128‑CMAC，但不加签名协商 context；有共同 CMAC 时只回该项。合法非空清单与本端没有共同算法时，[服务器规则](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/b39f253e-4963-40df-8dff-2f9040ebbeb1)仍要求回选 CMAC，而[客户端规则](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/3b29f3af-86f9-4962-8cf3-43471cb59363)要求拒绝未提议的算法。原回复可形成，提交后本端也不给这一路可用连接或 SESSION_SETUP 起点。没有 3.1.1、没有共同预认证哈希、畸形 Signing 清单分别拒绝；普通错误回复、真正的 RequestList 和 socket 仍未在此处实现。成功包只保存在私有所有者中；credits 和 SHA‑512 复核的就是交给传输者的同一份不可变字节。

[服务器初始化](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/04e0383e-e2b4-49c2-803f-1313d9c0b2ff)要求新生成 ServerGuid；[Windows 的启动行为 <26>](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/a64e55aa-1152-48e4-8206-edd96444e7f7)也是每次启动换一个。它对同一运行实例的所有连接稳定，按 [MS-DTYP 的 GUID 字节序](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-dtyp/001eec5a-7f8b-4293-9e21-ca349392db40)写入，不是永久安装 ID、池 ID、Samba instance_id 或客户端身份。本切片为将来的真实初始化所有者预留不透明 `RunningServerGuid`，没有生产构造器、磁盘变更或 per-connection 自动生成。绝不能用测试 GUID 启动服务或复用旧权限。

选择标准允许的**空 GSS 安全缓冲区**（起点 128，长度零），由客户端发起认证；不编造 Kerberos、NTLM 或 GSS 服务器广告，不从空缓冲区放行匿名。原生源码方面，[Linux v6.12](https://github.com/torvalds/linux/blob/v6.12/fs/smb/client/smb2pdu.c#L1237-L1268)在空缓冲区启用 raw NTLMSSP 选择，[Apple SMBClient](https://github.com/apple-oss-distributions/SMBClient/blob/99da2d9a4bbe17cde3cee382ad3edc16e25f5535/kernel/netsmb/smb_smb_2.c#L6891-L6951)可继续解析协商 context；均不能代替后续严格 MIC 或三端挂载验收。 `src/test/smb/negotiate_reply.rs` 保留默认／显式两套完整应答及下一条原请求哈希固定值、无共同签名、未知能力、时钟边界和本地失败源码例；没有运行构建、测试或客户端。

## SHA-512 原字节记录（尚未接到网络或身份认证）

`wire/preauth` 从已解析的单条 3.1.1 NEGOTIATE 出发，连接只保留请求和实际序列化回复的链式哈希。每个新的 SESSION_SETUP 尝试从该连接值单独起步；接收完整原请求，并且仅在同一 MessageId、同一已分配 SessionId 的 MORE_PROCESSING_REQUIRED 回复时继续。固定的 Rust SHA-512 实现见[上游来源及许可](../../vendor/rust/README.md)。TCP 长度头不入哈希，原 SMB2 消息头、正文、间隙/填充均入哈希。最后成功的 SESSION_SETUP 回复将使用由最后一个请求哈希派生的密钥签名，因此它不会再次进入这份 KDF 上下文。该值不是口令证明、签名或有权访问文件的会话。预认证层本身不运行 GSS/KDF，不编码应答，不查询凭据或启动生产监听。依据：[协商处理](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/b39f253e-4963-40df-8dff-2f9040ebbeb1)、[新会话](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/ea10b7ae-b053-4e4c-ab31-a48f7d0a79af)、[GSS 处理](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/5ed93f06-a1d2-4837-8954-fa8b833c2654)。

## 原连接的 SESSION_SETUP 第一阶段（不签发身份）

`Connection::receive` 先审完本帧全部成员的额度，将**这同一个已解析的不可变帧、其额度账单、原连接的 SHA‑512 基准和终止标记**一起交付。同步处理借用原接收缓冲，不复制每个大报文，也不让业务方拿另一帧搭配已花的 MessageId、签名或 transcript。连接对象释放会使尚在手中的票失效；放弃该原连接尚欠的普通回复也只终止它自己。未来真正异步保留完整独占或共享只读缓冲和有界范围，不能将同步借用延长到失效之后。

`Received::start_setup` 只能消费一次：没有 SETUP 的帧原样保留给别的处理器；含 SETUP 的复合帧在本配置整批拒绝，不顺带执行后面的文件命令。单条且头内 SessionId 为零的合法开局，从票内原 transcript 和同一 security field 得到“先选择 SPNEGO 机制”或“已有 Type 1，待真实挑战”；令牌不合法时仍保留该帧的已消费额度供今后准确失败回复，不能混成匿名或倒回 raw。非零 SID 只表示待真实注册表判定，不是重连或身份。通用额度回复入口明确不接受任何 SETUP；第一阶段分类自身不签发 SID，不创建挑战或发送成功回复。下节的专用机制选择所有者才可接着准备自己的 MORE。

### 正在协商的 SID 和第一个 request-mic MORE

每次服务初始化对应唯一不透明运行对象，包含 GUID 和所有连接共用的 in-progress 索引；知道相同的 GUID 字节不能重建这个对象。根据[新会话及双表规则](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/ea10b7ae-b053-4e4c-ab31-a48f7d0a79af)，在真正的 `Select` 编码 MORE 时，全服务登记从未使用的非零、非全 1 SID，原连接登记自己的阶段；连接之间不重复发号，号码不是认证凭证。全运行最多 64 个、每连接最多 8 个进行中槽，初始选择有效期 30 秒（单调时钟）；过期、撤销和错误不能使编号复用。全局表只弱引用阶段注册；连接私有表才持有令牌状态。狭小的索引锁不跨 GSS、哈希或发送。

首包需要选机制时，发出的[SESSION_SETUP 正文](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/0324190f-a31b-4666-9fa9-5c624273a694)是真正的 STATUS_MORE_PROCESSING_REQUIRED，加[SPNEGO request-mic](https://www.rfc-editor.org/rfc/rfc4178#section-4.2.2)和唯一选中的 NTLM OID，不捏造 Type 2、用户名、guest 或成功。保留原 MID、charge 和阶段专属应回额度。用同份最终字节同时核原 transcript／GSS 和额度。只有原发送者确认这份不可变包已成功提交，才把原连接该槽从“待发送”改为“等 Type 1”并公布信用；未提交、结果不明或临界时过期则关闭本连接，不留下可续阶段。

下一条非零 SID 请求须已在**同一原连接**消费自己的新 MID，再从双方索引找到同一未超时对象、恰处于等 Type 1。仅一次移出该阶段，按同一 transcript 校验原 token；坏 token 只移除这个槽，无权触碰别的会话。跨连接借来的同号码不能接管或取消。不在表的 SID 或过期不会创建新会话。单独的机制选择只推进到有效 Type 1；下节的 Type 2 仅由持有原连接和受保护凭据代的代码签发，仍无成功会话或原生监听。生产错误回复、主动定时唤醒和其他 [GSS 完成／清理](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/5ed93f06-a1d2-4837-8954-fa8b833c2654)仍属后续；每次查找与增发都在原处检查期限，不能用被动清理延长权限。

### 本次机制挑战的生成（不签发会话）

严格机制只从原 Type 1 选已经支持的标志。按 [MS-NLMP 服务器处理](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-nlmp/81d15e3e-3ac3-41f0-920d-846149f3a814)、[Type 2 格式](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-nlmp/801a4681-8809-4be9-ab0d-61dcfe762786)与[协商标志](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-nlmp/99d90ff4-957f-4c8a-80e4-5bfe5a9a9832)，独立服务器返回本机的 NetBIOS TargetName、受信的名称 AV、UTC FILETIME 和内核新取的 8 字节一次性挑战；不反射客户端名字、不冒充域控制器、不把 NTLM SEAL 当作 SMB 加密。原 standalone Type 2、第一次 SPNEGO 的 request-mic/NTLM OID/Type 2、已有选择后的仅 accept-incomplete/Type 2 分开编码。MORE 不同于认证成功。基础库仅接受归属本服务的一份受保护凭据代，与已签发的确切挑战、原 GSS 包和原绝对截止绑定；凭据替换后相同数字、名字或 SID 不能代替同一原对象。没有生产凭据构造、收集器、导入用户密码或认证回退。源码夹具有独立固定的完整 Type 2、短/长 DER 边界、错封装、拒绝和撤代例，无实跑。

连接现在能把这份机制挑战变成真实 SMB2 MORE，但没有启用任何服务。直接 Type 1/首个乐观 token 在本连接预留新 SID；经上一次 request-mic 得到的 Type 1 只消费当时那同一项，不延期、不另发 SID。只从本次运行持有的受保护提供者取凭据代和 Type 2，不接收单独传入的密钥、提供者或 SID。原接收票决定 MID、额度和不可换包的回复；只有提交原字节、同一代未撤销且原截止没到，状态才从 SendingChallenge 进入 WaitingAuth。未配提供者没有挑战，放弃或不确定发送终止原连接。本片不消费 Type 3、不回成功、不授 Tree；全局注册只限制有效弱项，部署时仍需终结静默到期连接。

## SESSION_SETUP 的机制封装（止于未验证的 Type 3）

`session/envelope` 只处理一次新的连接型 NTLMv2 尝试。它取原请求或已序列化 MORE 回复的同一个 security buffer 和预认证状态，不收另传的 token。第一次选定 RFC 4178 的 GSS-SPNEGO 或直接 NTLMSSP 后不得换层；不扫描尾部、不从坏 DER 回退、不把客户端的账户名、目标、MIC 标志或非零 SessionId 当证明。[Linux v6.12 的原生 SMB 客户端](https://github.com/torvalds/linux/blob/v6.12/fs/smb/client/smb2pdu.c#L1708-L1846) 确有直接传两段 NTLMSSP 的代码，两种封装最终必须满足同一账户、挑战及签名要求。本段不接收 SMB1、NTLMv1 或匿名。

[初始 GSS 封装](https://www.rfc-editor.org/rfc/rfc2743#section-3.1) 只在客户端首条使用；[RFC 4178 §§4–6](https://www.rfc-editor.org/rfc/rfc4178#section-4.2) 的后续 NegTokenResp 不重复包装。保存客户端机制顺序及原 DER SEQUENCE（不含显式 [0]）给独立的 mechListMIC 校验。若 NTLM 不在首位，不使用其他机制的乐观 token 或早期 MIC，必先用实际 MORE 选择 NTLM 并请求名单 MIC；此后收到新的 Type 1 才接收 Type 2。优先 NTLM 无乐观 token 时也要先走机制选择。后续客户端不能再选机制，未知 DER 扩展有界跳过但不授予能力；即使客户端首条带有 Init2 的 NegHints，也只忽略其有界 SEQUENCE，不从提示名推出目标或身份。首答已 request-mic 而末条没有相应 MIC，按 §5(c)(V) 终止；即使形状和长度都正确也须实际核验，不能由“有字段”推出认证。

[MS-NLMP 三种报文与字段](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-nlmp/033d32cc-88f9-4483-9bf2-b273055038ce) 的有界视图保留原始 Type 1/2/3、目标信息及客户端声明；启用字段不超出本条、不别名固定头或其它有效字段，不解释按标志应忽略的描述符。[MS-SPNG 微软产品说明 <8>](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-spng/211417c4-11ef-46c0-a8fb-f178a51c2088) 区分最优先 NTLM 且带 NTLM MIC 的保护与原生直接 token 行为；本版对已选 SPNEGO 的这类输入也要求名单 MIC，不能切到直接封装来逃避。未宣告 Negotiate Late Fallback，不在半途重选。没有 GSS 分片重组、Kerberos、原生客户端验收或跨连接恢复；若后续改用服务端先行 GSS，需单审机制广告／NegHints 与出站封装；当前协商明确选择客户发起，不靠假广告。

该封装层产物只是 `Unverified`，不会生成挑战、校验口令、导出密钥、创建 `VerifiedExchange`、授予 Tree 或发送成功。真正的接受端还须拥有安全随机的一次性限时挑战与原连接/凭据代，校验原协商标志、目标/时间、NTLMv2 证明、所需 NTLM MIC 和双向 mechListMIC，并复核当前业务身份，再把该次最后请求的 transcript 和导出键一起交给签名器。不能只相信客户端的 MIC 指示位。该封装层的形状夹具不是有效口令证明。

## 严格 NTLMv2 机制证明（开发资料，未启用）

`session/strict_verifier` 消费同一次握手的原 Type 1、服务器实际序列化并逐字核对的 Type 2、最后一个 SMB 请求与其 Type 3。受保护的材料是针对**一个确切账户、域和代次**的预派生 ResponseKeyNT，等同该身份的秘密，不是 Web 哈希，不从网络接收；本模块没有此材料或挑战的生产构造器、密码输入、账户库或发包函数。候选挑战只能消耗一次，包括出错；核对原 SID／MORE 的原 MID 和有限期，把最终请求的原 SMB transcript 随证明保留。连接 owner 今后还须实际生成不可预测挑战、限额／防重复 MessageId、从原连接独占取出、终验原凭据代和同一实例。未完成这些，不得宣布身份成立。

| 层 | 规范与本开发配置的决定 |
| --- | --- |
| NTLMv2 响应 | 使用[原服务器挑战及客户端原始 v2 blob](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-nlmp/5e550938-91d4-459f-b67d-75d70009e3f3)校验 NTProofStr，匹配受保护账户／域、服务器目标信息、时间及配置的 CIFS SPN。拒绝 LM/NTLMv1、匿名、替换账户或凑出的备用域。仅接受 Unicode、签名、ESS、VERSION 和 128 位等本配置的协商结果；标志不一致即拒。没选 KEY_EXCH 时用原 SessionBaseKey；选了时仅从本次 16 字节加密字段恢复导出键。这里只接普通无 TLS 通道绑定的 profile，不假造 CBT 或 SMB 数据加密。 |
| NTLM 内层 MIC | [MS-NLMP 客户端规则](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-nlmp/c0250a97-2940-40c7-82fb-20d208c71e96)：带服务器时间戳时客户端 SHOULD 发送；[AV_FLAGS 的 0x2](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-nlmp/83f5e789-660d-4781-8491-5f8c6641f75e)是客户端表示已经包含 MIC，不是身份或服务器强制开关。[服务端规则](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-nlmp/f9e6fbc4-a953-4f24-b229-ccdcc213b9ec)要求核已标注 MIC；[安全说明](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-nlmp/1e846608-4c5f-41f4-8454-1b91af8a755b)区分未标注的通用允许范围与篡改风险。**本版直接、SPNEGO 两路均硬性要求并实际核验；未标、缺失或算错全拒**，不以三平台兼容为由降级。原 Type 1＋2＋3 只清零 Type 3 的 MIC 本身；三报文均已确定带 VERSION，不猜 16 字节缺口。 |
| SPNEGO mechListMIC | [RFC 4178 §5(c)](https://www.rfc-editor.org/rfc/rfc4178#section-5) 与[微软产品说明 <8>](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-spng/211417c4-11ef-46c0-a8fb-f178a51c2088)：保护的对象是首个客户端原 MechTypeList DER SEQUENCE，不是 Type 1＋2＋3。当本严格配置选择 SPNEGO 时还必须独立校验客户端方向，并生成服务端方向的最终名单 MIC；方向、序号、签名键均不能混用。NTLM [ESS 签名](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-nlmp/a92716d5-d164-4960-9e15-300f4eef44a8)的可选 RC4 仅按协商保护短校验和、封装会话键，不用于文件、密码存储或新功能。最初就是直接 NTLM 时没有名单可签；已选择 SPNEGO 后不允许回退。 |
| SMB3.1.1 | SHA-512 preauth 绑定原 SMB 报文；它不是 NTLM MIC 或名单 MIC。未来只准持原 `MechanismProof` 的连接 owner 在同代复核后，把确切最终 GSS 输出写入成功应答、再用同次导出键及请求 transcript 为该真实应答签名；所有普通命令仍走强制 AES-CMAC。当前无钥匙读取器、`VerifiedExchange` 签发者、成功会话、业务树、监听或线上口令。 |

Windows、macOS 及 Linux 均没有通过原生验收。[Linux v6.12 的 raw NTLMSSP 构造](https://github.com/torvalds/linux/blob/v6.12/fs/smb/client/sess.c#L1075-L1171)在这条普通路径构造 Type 3 而不计算本版要求的 NTLM MIC，故**该版本该路径不能通过严格配置**；能解析 raw token 不代表能挂载。未来不同 profile 须独立说明和审查，不能识别客户端名称后放宽或试错回退。

Apple 公开的[挂载认证选择](https://github.com/apple-oss-distributions/SMBClient/blob/99da2d9a4bbe17cde3cee382ad3edc16e25f5535/kernel/netsmb/smb_gss.c#L315-L355)与 [Heimdal 的 MIC 生成](https://github.com/apple-oss-distributions/Heimdal/blob/1635de38a813f6e1dd3f8fd270683187ad4f02be/lib/heimcred/ntlm.m#L306-L390)给出另一条独立风险：其 [NTLMv2 客户端 blob](https://github.com/apple-oss-distributions/Heimdal/blob/1635de38a813f6e1dd3f8fd270683187ad4f02be/lib/ntlm/ntlm.c#L1632-L1672)用本机当前秒生成响应头时间；即使生成了正确的内层 MIC，若它和服务器 TargetInfo 中发出的时间戳不逐位相等，现有严格开发配置也会拒绝。此处是这几份公开源代码的差异，不是实机结论，也不是 Linux 那条缺 MIC 的问题。

仅供下一轮独审的时间判定，不改变当前验证器：[MS-NLMP 客户端](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-nlmp/c0250a97-2940-40c7-82fb-20d208c71e96)建议使用发出的时间戳；[服务端](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-nlmp/f9e6fbc4-a953-4f24-b229-ccdcc213b9ec)建议按服务器当前时钟与 MaxLifetime 检查差值，而不是要求客户端响应头时间等于服务器 AV。候选方案用受信任的墙钟作有界双向偏差比较，同时保留原连接一次性挑战及单调到期、原服务器绑定的 AV／目标和原始客户端 blob；不改写时间再算 HMAC，不省略任何本配置要求的 NTLM MIC、名单 MIC 或 SMB 签名。不得按客户端名字放宽，也不能复活过期挑战。

`src/test/smb/strict_verifier.rs` 只存公开合成的完整原消息和固定答案。固定材料不对应实际账户，包含有／无 KEY_EXCH 的导出键、双方向名单 MIC，以及 v2 证明虽正确但无内层 MIC、错 SPN／时间等拒绝例；另列[微软规范公开的 SessionBaseKey 答案](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-nlmp/54973495-20d2-49e8-9925-c399a403ed4a)作交叉基准，不给生产补造凭据、连接或权限。仅静审源码，未构建、运行测试或实际认证。

## 认证后的明文消息签名（未接认证、网络或存储）

`session/` 实现[规范 KDF](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/da4e579e-02ce-4e27-bbce-3fc816a3ff92) 的一个 HMAC-SHA-256 计数块，从同一新 NTLM 会话的 16 字节导出键和最后一个请求的 64 字节 transcript 派生 128 位签名键。标签含结尾 NUL，KDF 自己的分隔零字节另算。真实认证的完成凭据没有生产构造器；以后只有完成挑战、主体、MIC 和当前代检查的原尝试才能交出这一对，不能从报文、Web 身份或布尔值制造会话。密钥缓冲使用 zeroize；不承诺清除所有栈临时副本。没有使用宿主加密服务。

[协商](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/b39f253e-4963-40df-8dff-2f9040ebbeb1)没有 Signing Capabilities context 时仍强制 CMAC；显式 CMAC 或合法非空但无共同算法时应答须回且只回 CMAC。计数为零、被截断或串错请求/回复不能作“无签名”处理。最终回复的签名函数只接收与最终请求同 MessageId、同已分配 SessionId 的实际未签成功响应；拒绝 guest/null/encrypt 标记。它给该响应签名，不把成功响应再加入 KDF。普通明文请求只有签名位和全部 16 字节 [AES-CMAC 校验](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/5b570c0b-0854-4324-8fb5-d410692dde3e)均成立才通过签名检查。每段用原 SMB 消息和自己的尾填充，只有 Signature 16 字节置零；关联链继承的实际 SessionId 只用来选键，不改变验签字节。不相关链可涉及多个有效会话。整批验完才可能进入独立的 credits、当前会话/树、命令、原打开和存储终验；没有这里发出的成功应答或文件授权。普通回复头的生成/签名、CANCEL 和加密 transform 不由此入站函数处理。

`src/test/smb/signing.rs` 保留两种协商下首个 TREE_CONNECT 的独立固定值、无签/错签拒绝，以及同链/异会话 compound 的原字节向量。测试只使用公开合成值，没有执行，不是实际登录或互通。原生三端验收仍须分开“无缓存端点的首次访问”和“同一身份重连”；记录 Windows 冷访问有无 multi-protocol opener，旧服务缓存的热路径不是首连通过。不增加 SMB1 文件操作或降级。

## 文件 I/O 请求与原打开边界（未接线）

[READ](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/21e8b343-34e9-4fca-8d93-03dd2d3e961e) 和 [CLOSE](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/f2b4183a-29c2-4fc0-a5de-fe290408f68c) 从各自命令取得范围或关闭意图；普通引用携两半 SMB2_FILEID，关联引用只继承同一链紧邻前件的已证实 ID，不能以客户端填入的另一组 ID 或最近打开回退。实际入口仍须验签、验 credits 与当前连接／会话／树，再以两半与原代次匹配唯一真实 OPEN；本片没有认证发行者或第二张路由表。后端入口只接已成功绑定的 Held，不收线上的 FileId、路径或替代对象。

READ 保持原请求长度供 MaxReadSize／credits／范围锁核验，偏移与终点不得超出有符号文件范围。仅 FILE 的实际 FILE_READ_DATA 可读，不把目录的列出权、执行权或共享票当读许可；原会话、媒体／视图、范围锁及输出前后的资格由真实适配复核。只在原适配确认后区分满足最低计数且非空的短读、EOF／小于 MinimumCount、权限或 I/O 错误。TCP 不接受 RDMA 描述符，不把实际无法提供的直接读冒作成功；压缩提示本身不协商能力。回复容量不足不伪装成 EOF，也不暴露不完整校验的正文。

显式 CLOSE 消耗原打开的持有权，不等于异常断连。以后接线须先令该 OPEN 不可再路由并结束在途操作；`POSTQUERY_ATTRIB` 只返回原句柄可获且有权的真实属性。属性取不到不冒充有效；关闭与脏普通写句柄的最后确认失败单独保留，正常写关闭时由 Core 在原写权内收尾，释放底层对象后才退共享票。丢连接／撤权只清理，绝不能隐式写穿、flush 或发布历史。这部分没有真正后端接线。

成功 READ／CLOSE 应答体只能消费 Held 产生的私有结果，不能用任意正文、请求计数或属性生成成功。READ 保持真实返回长度，固定体后仅借已核正文；[MS-FSA](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-fsa/aa57e052-40d4-4d9f-837d-2b0ce7c87cf4) 的零长底层成功不等于线上空成功，[MS-SMB2 §2.2.20](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/3e3d2f2c-0e2f-41ea-ad07-fbca6ffdfd90) 规定实际返回 0 字节应答 EOF，包括合法的零长度请求。EOF 或未达 MinimumCount 用 ERROR 体，不夹带部分正文；原始 Core 错误保留给真实状态映射。CLOSE 只在请求、原 OPEN 实授 FILE_READ_ATTRIBUTES 且属性确实取得时标记有效；缺属性权的正常关闭仍执行，但不查询或回送元数据。这里只定义命令、状态和体，不构造协议头或发包；未来唯一应答出口核同一已验证请求与会话，确定长度、复合对齐及原字节后才签名。当前无监听、认证发行者或实际 Core 调用。

[WRITE](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/829f93f5-ed10-4f12-8347-42d235019459) 只借用本命令声明的正文，不含其他命令、预填字节或尾随填充；原请求长度供 MaxWriteSize 和 credits，TCP 不消费 RDMA 描述符。追加位置来自同一个仍获权的实际写者的当前稿 EOF，不看旧读句柄的长度，不把对 EOF 写或空写当作 SET_END_OF_FILE。仅追加的权限不改写或填出空洞，只能命中该 EOF；跨出原长需要明确追加权。普通写确认原写者已接收该范围，不承诺已经持久化。

逐请求 WRITE_THROUGH 不是 CREATE 的 FILE_WRITE_THROUGH 模式。按 3.1.1 组合规则，普通缓冲打开上只有逐请求写穿标志时返回参数错误；真正的直接写没有接，不退化成缓冲成功。[Apple 原生源码](https://github.com/apple-oss-distributions/SMBClient/blob/99da2d9a4bbe17cde3cee382ad3edc16e25f5535/kernel/netsmb/smb_smb_2.c#L10684-L10720) 有特定错误后的重试，不代表该平台已验收。原持有打开要求写穿时，实际写入后还必须等同一写者完成确认；[显式 FLUSH](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/026984f6-38af-4408-8200-50557eb0a286) 也只让实际有写权的原 FILE 写者确认；目录权限不变成卷同步。失败仍留在原写者与原批次的状态机，不冒充刷盘、成功关闭或正式提交。不提供伪造写者、额外正文或实际调度。

WRITE 和 FLUSH 成功各有私有 Held 收据。按 [WRITE 响应](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/7b80a339-f4d3-4575-8ce2-70a06f24f133)，Count 来自原写者核实的实际接收字节数，不从请求的 Length 生成；当前业务要求整段成功，计数矛盾不给成功，也不推断回退或自动重写。合法空 WRITE 可回实际 Count=0，这与 READ 的 EOF 不同。[FLUSH 响应](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/42f78e6a-e25f-48f5-8f08-b4f1bb4c4fa4) 只在原确认完成后能生成；原 I/O 失败与确认失败分开传给真实状态映射，不能包装成成功体。编码仍不构造协议头、不签发会话、不发包。

## 原 OPEN 的路由与退场（接线约束，未实现）

- 真实连接先预验完整复合请求的原字节签名及当前会话，再逐成员借出有当前主体与代次的票据。这个票据不从 UUID、裸 SID、SQL 布尔或单个签名器构造。当前公版还没有此发行者：签名验证、后端 Held 和应答体存在，各自不是上线权限。由原会话和树核权限，查找及失败优先级逐命令遵循规范：READ 是 Session→Tree→OPEN，CLOSE 的标准顺序为 Session→会话内 OPEN→Tree。CLOSE 查到 OPEN 仍不能绕过核对它的原树；不能把撤权、树关闭、失效 OPEN 混成 EOF。关联成员只用本链**紧邻前项**实际已确认且仍可用的 Session／Tree／OPEN；前件没有或未取得相应范围时按 [关联请求规则](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/46dd4182-62d3-4e30-9fe5-e2ec124edca1) 失败，不能补猜独立 ID。
- 每个原会话拥有唯一 OPEN 表，条目只持有其准确原树作用域下经 `Pending::bind` 核过的 Held。先限资源、取得实物和最终原权，再发服务器自己的两半 ID 并公开；Volatile 在会话范围唯一，即使表按树分区，查出后同时核 Persistent、原树与代次；同一会话代内不得把已退休的 ID 对重新发给另一个 OPEN，旧会话不能经同名目录重找。表不取代跨会话 ShareAccess 表或 Core 单写权。Core 原树须在其借用的实际文件／目录句柄之外存活，持有本地树再建立表，销毁时逆序；不靠不安全的自引用或新的业务存储。
- 原 OPEN 可读借用，写／FLUSH 独占借用，正常 CLOSE 先摘路由阻止新请求，再取消或排空已受理操作，取得同一原件完成一次关闭。异常断连、撤权及 scope 退场是清理，不伪造显式 CLOSE 或隐式 flush；已确认和待确认由 Core 区分。底层原件保留到在途操作、相应权限和确认处理收尾，先释放它再退共享票。签出最终响应时仍核与原请求／当前会话对应，规则没有开通前不启监听。

## 目标与分工

只协商 0x0311，直接访问一个磁盘共享 `ZXS`。只有配置的 SMB 账户可用，Web 管理员不是 SMB 账户。共享根是虚拟目录：下挂被授权的数据盘和专用回收站，不呈现 SYSTEM、真实宿主目录或容器内部。共享名匹配与文件组件的精确大小写/原拼写是两层决定；不对所有路径做不分大小写归一化。服务为现有 zxs crate 的私有模块，不依赖兄弟项目或 Samba 的协议处理器。

| 边界 | 责任和不可越过的线 |
| --- | --- |
| `wire/` | 验证长度、端序、请求方向和关联/非关联链；保留原始字节。命令体、credits、签名与业务授权须分别验证。客户端名、GUID、ID 和 capability 位均不是权力。 |
| 认证/会话（后续） | 受信的实际接受端创建一次性、限时的挑战，绑定原连接、具体握手、实例、唯一账户与凭据代；完成标准机制、校验保护并重新核当前代后才授不可伪造的业务会话能力。拒绝 guest/匿名/空密码、LM/NTLMv1、错误域、Web 登录与自报布尔值。按连接、会话、树和实际 OPEN 划分生命周期及额度。 |
| 操作与存储适配（后续） | 已验证的会话/树只取得当前范围的对象能力。SMB 的 FileId 二元组仅查此会话/树的打开表，不是存储 FileId、路径或权限证明。一次跨会话协调 ShareAccess 双向关系、范围锁、删除待定；另向 Core 取得跨入口唯一的每 FILE 写权或已确认读视图，不叠加另一套业务写锁。先有界预留，再在不持全局表锁时取得物件权；两边都成功才公开句柄，失败逆序释放。 |
| 维护边界 | helper 离线开户/重置；Web/存储不取得 SMB 长期密验或会话签名密钥。现行公开 v7 SQL 只记 SMB 非秘密绑定，密码验证归 tdbsam。需另审具备相同隔离、代次和修复语义的新凭据发行/撤销格式；无它绝不从 Web、旧测试构造器或伪造 Samba 私帧填空。共用 crate 不表示网络解析器能打开 SYSTEM、账户库或裸磁盘。 |

真实凭据切换、受保护的发行者/存储通道、服务监听/启动、泄露面和（若有）迁移须单独共审，不能自启动、双开同一端口或回落旧认证。对不可用的存储适配返回明确失败，不创建假 handle。写穿、脏 FLUSH、正常脏 CLOSE 只有 Core 应有材料与事务确认后才可回成功；干净关闭和异常断连都不是手动提交，断连不擅自保存未确认输入。共享模式或账户代变化必须撤销对应资格。物理回收站内不是任意写入或永久删除的后门。更多业务语义由 Core 维护，不在 wire 重写。

## 实现与审查次序

1. **本切片。** 每个 Direct TCP 帧上限 2 MiB；最多 32 条命令；初次 NEGOTIATE 上限 68 KiB / 64 个 contexts。读取四字节长度即可在分配前判断，不要求 TCP read 等于一条 SMB 消息。保留完整协商字节和每段包括 padding 的字节视图。边界正确不代表已验证所有命令体。重复 PREAUTH、加密、压缩、RDMA 变换和签名 context 不是合法的独立能力；未知 context 只按声明边界跳过。加密和签名算法列表在处理该可选 context 时按它自己的长度解析。当前不处理 SMB1 初始包装、加密或压缩 envelope，不提供明文降级。这里只解析，不回成功或架设演示服务器。
2. **协商保护与真实认证。** TCP/445 的 3.1.1 要有实际的 multi-credit / LARGE_MTU：每连接维护不重复且可乱序的 MessageId 序号窗口，按请求/应答大小计费，CANCEL 关联原在途请求、不是新信用。资源上限不等于所有命令只能占一个 credit。同次服务初始化的全局 ServerGuid、每个回复的新 CSPRNG 盐、准确安全模式与实际支持的 contexts 都由上方成功应答模块分别把关，仍欠真正启动所有者。连接 preauth 从 64 个零字节开始，逐次 SHA-512(旧值 || 原始 SMB 消息)，不含 TCP 四字节。会话从已完成 NEGOTIATE 的 hash 分支，只加入该会话的 SETUP 往返以及最后的请求；用于签最终成功响应的派生快照**不含这条成功响应本身**。
3. **消息级保证。** SPNEGO/NTLMv2 需要真实的机制/主体选择、对应 flag 和 target-info、挑战时效、适用的 NTLM MIC 与 SPNEGO mechListMIC、导出的会话密钥。按规范的 SP800-108 和准确标签派生 SMB 密钥。AES-128-CMAC 为签名基线，未签或签名不符的需签请求不得触发业务。复合包逐段验含 padding 的签名，关联链不能从其他会话/树借句柄。标准密码原语优先选取可审的最小上游源码（例如 RustCrypto 对应原语）；记录精确版本、摘要、传递来源、许可证和改动。不得自制算法、手抄 Samba handler 或为 NTLM 关闭宿主密码策略。不记录密验、报文令牌与会话键。
4. **真实文件访问。** 生命周期与标准错误（包括分页结束、短缓冲、权限、对象不存在、共享冲突、锁冲突及撤销），TREE/CREATE/QUERY_DIRECTORY/QUERY_INFO/READ/CLOSE 先接真实授权对象；随后是写、改长、FLUSH、SET_INFO、LOCK、删除待定、重命名/替换、正常退出与异常清理。根据三端实际行为补 ECHO、CHANGE_NOTIFY/CANCEL、真实元数据类和 IOCTL；命令不支持不回假成功。不得把客户端的普通保存次数当成仓库正式版本数。
5. **安全互通扩展。** AES-128-GCM 的协商、每方向密钥和 nonce 生命周期、transform 数据边界及完整性是独立阶段；未实现时不宣告支持。客户端若要求加密，绝不让请求落到明文共享；供需不匹配即拒绝/无法连接，不建议客户端关闭安全策略。GMAC、其他 cipher、压缩、租约、耐久/持久句柄、多通道、RDMA、QUIC 等不能只置位不实现。可忽略的 lease/durable *申请* 必须按各 context 规则分别忽略/不授予，真实重连必须查到原资格；不是看到探针就拒整次打开。

## 原生客户端的验收边界（未执行）

- Windows 资源管理器使用已知 UNC 共享；macOS Finder 使用 [Apple 明确支持的带共享名地址](https://support.apple.com/guide/mac-help/servers-shared-computers-connect-mac-mchlp3015/mac)；Linux 内核 SMB 客户端使用已知共享。共享发现和普通文件访问是两类实验。初次 Windows 多协议试探、macOS 对 IPC$/AAPL/流/扩展属性或 validate-negotiate 的探测及其回退要记实际版本、策略和结果，不从文档推定所有客户端都省略这些请求。匿名 IPC$ 不可用；若必要，只单独评估最小的已认证服务，不额外开放数据共享、任意命名管道或宿主 RPC。
- 基本验收需原生完成：正确/错误和匿名登录，列根和多页目录，普通/回收两类对象按授权读取；在可写普通盘创建、偏移写、flush、重开读回，精确大小写和并发单写；当回收、改名或应用级保存声称可用时，另验实际动作和中断。直接 TCP 成功、NEGOTIATE、脚本内读写都不能代替。业务上强制 NTLM 禁用/Kerberos-only 的部署不属于本地 NTLMv2 配置，不能静默退回匿名。
- 所有 Rust SMB 测试源码放 `zxs/src/test/smb/`，从产品私有作用域用 `cfg(test)` 接入。不为测试公开认证/文件能力。区分静审、密码向量、三系统互通与真实存储故障，分别记录，不混称通过。

原始规范：[Direct TCP](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/1dfacde4-b5c7-4494-8a14-a09d3ab4cc83)、[协商](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/b39f253e-4963-40df-8dff-2f9040ebbeb1)、[复合包](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/fa6687f5-99d4-4c9b-ba2e-a770310225e0)、[GSS 会话处理](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/5ed93f06-a1d2-4837-8954-fa8b833c2654)、[MS-SPNG](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-spng/f377a379-c24f-4a0f-a3eb-0d835389e28a)、[NTLM 服务器校验](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-nlmp/34a9417d-7cc0-43b0-b61c-1f19740df66f)、[文件信息类](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-fscc/4718fc40-e539-4014-8e33-b675af74e3e1)。
