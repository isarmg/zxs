# ZXS 内部 Rust SMB 服务

**当前只有输入层和未接线的共享打开表。** `wire/` 能分开 Direct TCP 帧、校验整条请求链的 SMB2 头和边界、读取客户端的 3.1.1 协商提议。`backend/` 能为已选对象有界预留读、写、删共享许可，兼容判定完成不等于已取得 Core 写权。此段没有生产存储句柄的签发者，也未实现目录首开特例、范围锁或删除待定。客户端说“支持”不等于服务器同意。不生成协商成功包，不监听 445，不认证、不分配会话或授予共享。现行 Samba 运行入口不调用这里；没有编译、运行测试或进行真实客户端验收。

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
