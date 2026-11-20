# ZXS 内部 Rust SMB 服务

**当前是未接线的协议、签名与权限前置，不是可挂载的服务。** `wire/` 能分开 Direct TCP 帧、校验整条请求链的 SMB2 头和边界、读取客户端的 3.1.1 协商提议。`backend/` 能为已选对象有界预留读、写、删共享许可，兼容判定完成不等于已取得 Core 写权。此段没有生产存储句柄的签发者，也未实现目录首开特例、范围锁或删除待定。客户端说“支持”不等于服务器同意。不生成协商成功包，不监听 445，不认证、不分配会话或授予共享。现行 Samba 运行入口不调用这里；没有编译、运行测试或进行真实客户端验收。

`wire/path_fields` 从单条 CREATE / SET_INFO 普通 rename 消息的声明范围提取名字，`backend/path` 是唯一业务名字解析：严格 UTF-16、以同一共享为根，逐部件调用现有显示名规则，保留大小写和 Unicode 原拼写，不从名字判断数据盘／回收站，不解释宿主路径。CREATE 空名和未处理的 contexts 一起保留，不把重连请求误作根权限。rename 源是本次协议句柄，目标的替换标志不是覆盖授权。关于多部件目标，[微软 SET_INFO 条文](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/43cf22fe-84cb-49cf-b3e7-e3d7d766a7b7) 的分隔符限制与 full pathname 表述并存；[Linux 客户端](https://github.com/torvalds/linux/blob/v6.12/fs/smb/client/smb2inode.c#L1076-L1112) 会编码目标路径。此处为 ZXS 同树移位保留这个目标形状，不表示任何原生客户端已完成还原。具体成员字节额度、当前盘和删除原单仍由 Core 选择和复核。

目录和元数据查询目前只取请求与预算，不调用文件系统，也不产生成功应答。按 [QUERY_DIRECTORY](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/29dfcc9b-3aec-406b-abb5-0b4fe96712e2) 与 [QUERY_INFO](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/abb48417-27e5-4a16-82e0-3e5981db97e8) 保留原请求长度供 credits 核验，不把客户端 FileIndex 或 SMB2_FILEID 变成 Core 游标。查询目录的模式是单个分量，不可当路径；没有新模式不能让续页丢失原筛选条件。[Apple 原生实现](https://github.com/apple-oss-distributions/smb/blob/a4320c1ed655fe99ca882a43c84cd820ea10d2af/kernel/netsmb/smb_smb_2.c#L5788-L5842) 会给空模式留 offset 96 和两个零字节，当前仅和标准的零 offset 一并识别为空模式。[Linux 客户端](https://github.com/torvalds/linux/blob/v6.12/fs/smb/client/smb2pdu.c#L5358-L5394) 与 [Apple 客户端](https://github.com/apple-oss-distributions/smb/blob/a4320c1ed655fe99ca882a43c84cd820ea10d2af/kernel/smbfs/smbfs_smb_2.c#L5261-L5273) 的源码分别给出目录信息类选择；这是格式依据，不是原生挂载通过证据。

一页只为有权、匹配的逻辑顺序前缀预算，独立限制条数、客户长度和复合应答剩余字节。只在记录之间算 8 字节对齐；首条装不全、下一条装不下、固定部分都装不下不是同一结果。真实适配须按原 OPEN 重核列出权，再按来源的真实排序与集合结束标记推进，不消费未回的匹配条目；不能直接用 main v7 取页即推进的旧游标后丢弃尾部，也不把过滤致空当结束。[MS-FSA 目录查询](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-fsa/fa8194e0-53ec-413b-8315-e8fa85396fd8) 的模式延续、匹配、点记录、溢出数据和结束状态仍须接线。元数据按已持对象区分虚拟根、目录和 FILE；访问属性和具体卷必须来自原始授权对象。虚拟根不挑任意实体盘冒充总容量，不对下线盘填假数据，不返回宿主路径或私有标识。

## SHA-512 原字节记录（尚未接到网络或身份认证）

`wire/preauth` 从已解析的单条 3.1.1 NEGOTIATE 出发，连接只保留请求和实际序列化回复的链式哈希。每个新的 SESSION_SETUP 尝试从该连接值单独起步；接收完整原请求，并且仅在同一 MessageId、同一已分配 SessionId 的 MORE_PROCESSING_REQUIRED 回复时继续。固定的 Rust SHA-512 实现见[上游来源及许可](../../vendor/rust/README.md)。TCP 长度头不入哈希，原 SMB2 消息头、正文、间隙/填充均入哈希。最后成功的 SESSION_SETUP 回复将使用由最后一个请求哈希派生的密钥签名，因此它不会再次进入这份 KDF 上下文。该值不是口令证明、签名或有权访问文件的会话。预认证层本身不运行 GSS/KDF，不编码应答，不查询凭据或启动生产监听。依据：[协商处理](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/b39f253e-4963-40df-8dff-2f9040ebbeb1)、[新会话](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/ea10b7ae-b053-4e4c-ab31-a48f7d0a79af)、[GSS 处理](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/5ed93f06-a1d2-4837-8954-fa8b833c2654)。

## 认证后的明文消息签名（未接认证、网络或存储）

`session/` 实现[规范 KDF](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/da4e579e-02ce-4e27-bbce-3fc816a3ff92) 的一个 HMAC-SHA-256 计数块，从同一新 NTLM 会话的 16 字节导出键和最后一个请求的 64 字节 transcript 派生 128 位签名键。标签含结尾 NUL，KDF 自己的分隔零字节另算。真实认证的完成凭据没有生产构造器；以后只有完成挑战、主体、MIC 和当前代检查的原尝试才能交出这一对，不能从报文、Web 身份或布尔值制造会话。密钥缓冲使用 zeroize；不承诺清除所有栈临时副本。没有使用宿主加密服务。

[协商](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/b39f253e-4963-40df-8dff-2f9040ebbeb1)没有 Signing Capabilities context 时仍强制 CMAC；显式 CMAC 或合法非空但无共同算法时应答须回且只回 CMAC。计数为零、被截断或串错请求/回复不能作“无签名”处理。最终回复的签名函数只接收与最终请求同 MessageId、同已分配 SessionId 的实际未签成功响应；拒绝 guest/null/encrypt 标记。它给该响应签名，不把成功响应再加入 KDF。普通明文请求只有签名位和全部 16 字节 [AES-CMAC 校验](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/5b570c0b-0854-4324-8fb5-d410692dde3e)均成立才通过签名检查。每段用原 SMB 消息和自己的尾填充，只有 Signature 16 字节置零；关联链继承的实际 SessionId 只用来选键，不改变验签字节。不相关链可涉及多个有效会话。整批验完才可能进入独立的 credits、当前会话/树、命令、原打开和存储终验；没有这里发出的成功应答或文件授权。普通回复头的生成/签名、CANCEL 和加密 transform 不由此入站函数处理。

`src/test/smb/signing.rs` 保留两种协商下首个 TREE_CONNECT 的独立固定值、无签/错签拒绝，以及同链/异会话 compound 的原字节向量。测试只使用公开合成值，没有执行，不是实际登录或互通。原生三端验收仍须分开“无缓存端点的首次访问”和“同一身份重连”；记录 Windows 冷访问有无 multi-protocol opener，旧服务缓存的热路径不是首连通过。不增加 SMB1 文件操作或降级。

## 文件 I/O 请求与原打开边界（未接线）

[READ](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/21e8b343-34e9-4fca-8d93-03dd2d3e961e) 和 [CLOSE](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/f2b4183a-29c2-4fc0-a5de-fe290408f68c) 从各自命令取得范围或关闭意图；普通引用携两半 SMB2_FILEID，关联引用只继承同一链紧邻前件的已证实 ID，不能以客户端填入的另一组 ID 或最近打开回退。实际入口仍须验签、验 credits 与当前连接／会话／树，再以两半与原代次匹配唯一真实 OPEN；本片没有认证发行者或第二张路由表。后端入口只接已成功绑定的 Held，不收线上的 FileId、路径或替代对象。

READ 保持原请求长度供 MaxReadSize／credits／范围锁核验，偏移与终点不得超出有符号文件范围。仅 FILE 的实际 FILE_READ_DATA 可读，不把目录的列出权、执行权或共享票当读许可；原会话、媒体／视图、范围锁及输出前后的资格由真实适配复核。只在原适配确认后区分满足最低计数且非空的短读、EOF／小于 MinimumCount、权限或 I/O 错误。TCP 不接受 RDMA 描述符，不把实际无法提供的直接读冒作成功；压缩提示本身不协商能力。回复容量不足不伪装成 EOF，也不暴露不完整校验的正文。

显式 CLOSE 消耗原打开的持有权，不等于异常断连。以后接线须先令该 OPEN 不可再路由并结束在途操作；`POSTQUERY_ATTRIB` 只返回原句柄可获且有权的真实属性。属性取不到不冒充有效；关闭与脏普通写句柄的最后确认失败单独保留，正常写关闭时由 Core 在原写权内收尾，释放底层对象后才退共享票。丢连接／撤权只清理，绝不能隐式写穿、flush 或发布历史。这部分没有真正后端接线。

成功 READ／CLOSE 应答体只能消费 Held 产生的私有结果，不能用任意正文、请求计数或属性生成成功。READ 保持真实返回长度，固定体后仅借已核正文；[MS-FSA](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-fsa/aa57e052-40d4-4d9f-837d-2b0ce7c87cf4) 的零长底层成功不等于线上空成功，[MS-SMB2 §2.2.20](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/3e3d2f2c-0e2f-41ea-ad07-fbca6ffdfd90) 规定实际返回 0 字节应答 EOF，包括合法的零长度请求。EOF 或未达 MinimumCount 用 ERROR 体，不夹带部分正文；原始 Core 错误保留给真实状态映射。CLOSE 只在请求且有实际可获属性时标记有效，其他成功关闭的属性／标志为零。这里只定义命令、状态和体，不构造协议头或发包；未来唯一应答出口核同一已验证请求与会话，确定长度、复合对齐及原字节后才签名。当前无监听、认证发行者或实际 Core 调用。

[WRITE](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/829f93f5-ed10-4f12-8347-42d235019459) 只借用本命令声明的正文，不含其他命令、预填字节或尾随填充；原请求长度供 MaxWriteSize 和 credits，TCP 不消费 RDMA 描述符。追加位置来自同一个仍获权的实际写者的当前稿 EOF，不看旧读句柄的长度，不把对 EOF 写或空写当作 SET_END_OF_FILE。仅追加的权限不改写或填出空洞，只能命中该 EOF；跨出原长需要明确追加权。普通写确认原写者已接收该范围，不承诺已经持久化。

逐请求 WRITE_THROUGH 不是 CREATE 的 FILE_WRITE_THROUGH 模式。按 3.1.1 组合规则，普通缓冲打开上只有逐请求写穿标志时返回参数错误；真正的直接写没有接，不退化成缓冲成功。[Apple 原生源码](https://github.com/apple-oss-distributions/smb/blob/a4320c1ed655fe99ca882a43c84cd820ea10d2af/kernel/netsmb/smb_smb_2.c#L7338-L7367) 有特定错误后的重试，不代表该平台已验收。原持有打开要求写穿时，实际写入后还必须等同一写者完成确认；[显式 FLUSH](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/026984f6-38af-4408-8200-50557eb0a286) 也只让实际有写权的原 FILE 写者确认；目录权限不变成卷同步。失败仍留在原写者与原批次的状态机，不冒充刷盘、成功关闭或正式提交。不提供伪造写者、额外正文、响应编码或实际调度。

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
2. **协商保护与真实认证。** TCP/445 的 3.1.1 要有实际的 multi-credit / LARGE_MTU：每连接维护不重复且可乱序的 MessageId 序号窗口，按请求/应答大小计费，CANCEL 关联原在途请求、不是新信用。资源上限不等于所有命令只能占一个 credit。主机的稳定 ServerGuid、每回合新的 CSPRNG 盐、准确安全模式与实际支持的 contexts 齐备后才构造响应。连接 preauth 从 64 个零字节开始，逐次 SHA-512(旧值 || 原始 SMB 消息)，不含 TCP 四字节。会话从已完成 NEGOTIATE 的 hash 分支，只加入该会话的 SETUP 往返以及最后的请求；用于签最终成功响应的派生快照**不含这条成功响应本身**。
3. **消息级保证。** SPNEGO/NTLMv2 需要真实的机制/主体选择、对应 flag 和 target-info、挑战时效、适用的 NTLM MIC 与 SPNEGO mechListMIC、导出的会话密钥。按规范的 SP800-108 和准确标签派生 SMB 密钥。AES-128-CMAC 为签名基线，未签或签名不符的需签请求不得触发业务。复合包逐段验含 padding 的签名，关联链不能从其他会话/树借句柄。标准密码原语优先选取可审的最小上游源码（例如 RustCrypto 对应原语）；记录精确版本、摘要、传递来源、许可证和改动。不得自制算法、手抄 Samba handler 或为 NTLM 关闭宿主密码策略。不记录密验、报文令牌与会话键。
4. **真实文件访问。** 生命周期与标准错误（包括分页结束、短缓冲、权限、对象不存在、共享冲突、锁冲突及撤销），TREE/CREATE/QUERY_DIRECTORY/QUERY_INFO/READ/CLOSE 先接真实授权对象；随后是写、改长、FLUSH、SET_INFO、LOCK、删除待定、重命名/替换、正常退出与异常清理。根据三端实际行为补 ECHO、CHANGE_NOTIFY/CANCEL、真实元数据类和 IOCTL；命令不支持不回假成功。不得把客户端的普通保存次数当成仓库正式版本数。
5. **安全互通扩展。** AES-128-GCM 的协商、每方向密钥和 nonce 生命周期、transform 数据边界及完整性是独立阶段；未实现时不宣告支持。客户端若要求加密，绝不让请求落到明文共享；供需不匹配即拒绝/无法连接，不建议客户端关闭安全策略。GMAC、其他 cipher、压缩、租约、耐久/持久句柄、多通道、RDMA、QUIC 等不能只置位不实现。可忽略的 lease/durable *申请* 必须按各 context 规则分别忽略/不授予，真实重连必须查到原资格；不是看到探针就拒整次打开。

## 原生客户端的验收边界（未执行）

- Windows 资源管理器使用已知 UNC 共享；macOS Finder 使用 [Apple 明确支持的带共享名地址](https://support.apple.com/guide/mac-help/servers-shared-computers-connect-mac-mchlp3015/mac)；Linux 内核 SMB 客户端使用已知共享。共享发现和普通文件访问是两类实验。初次 Windows 多协议试探、macOS 对 IPC$/AAPL/流/扩展属性或 validate-negotiate 的探测及其回退要记实际版本、策略和结果，不从文档推定所有客户端都省略这些请求。匿名 IPC$ 不可用；若必要，只单独评估最小的已认证服务，不额外开放数据共享、任意命名管道或宿主 RPC。
- 基本验收需原生完成：正确/错误和匿名登录，列根和多页目录，普通/回收两类对象按授权读取；在可写普通盘创建、偏移写、flush、重开读回，精确大小写和并发单写；当回收、改名或应用级保存声称可用时，另验实际动作和中断。直接 TCP 成功、NEGOTIATE、脚本内读写都不能代替。业务上强制 NTLM 禁用/Kerberos-only 的部署不属于本地 NTLMv2 配置，不能静默退回匿名。
- 所有 Rust SMB 测试源码放 `zxs/src/test/smb/`，从产品私有作用域用 `cfg(test)` 接入。不为测试公开认证/文件能力。区分静审、密码向量、三系统互通与真实存储故障，分别记录，不混称通过。

原始规范：[Direct TCP](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/1dfacde4-b5c7-4494-8a14-a09d3ab4cc83)、[协商](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/b39f253e-4963-40df-8dff-2f9040ebbeb1)、[复合包](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/fa6687f5-99d4-4c9b-ba2e-a770310225e0)、[GSS 会话处理](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/5ed93f06-a1d2-4837-8954-fa8b833c2654)、[MS-SPNG](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-spng/f377a379-c24f-4a0f-a3eb-0d835389e28a)、[NTLM 服务器校验](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-nlmp/34a9417d-7cc0-43b0-b61c-1f19740df66f)、[文件信息类](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-fscc/4718fc40-e539-4014-8e33-b675af74e3e1)。
