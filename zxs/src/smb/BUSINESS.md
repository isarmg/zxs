# ZXS 内置 SMB 3.1.1：普通业务边界

这是同一 `zxs` ELF 内唯一维护的 ZXS 私有 SMB 的**源码台账**，包含普通目录 FileStandard 和下述跨文件系统移动。不提供通用/独立 SMB 服务、旧格式兼容或迁移。客户端与应用验收见[验收用例](../../TASKS.md)。

| 业务 | Rust 候选源码状态 | 尚未证明或未接 |
| --- | --- | --- |
| 登录、连接 `ZXS`、退出 | 有严格 NTLMv2、适用 MIC、SHA-512 预认证和原流 AES-CMAC；使用真实 Core 当前账户与盘权限；ECHO、TREE_DISCONNECT、LOGOFF 有原命令分派。 | 未从源码推断 Windows 冷连接、Finder、Linux 挂载或任一真实客户端登录；已有离线构建与开发检查；仍未部署或原生客户端验收。 |
| 浏览和目录 | 有真实虚拟根、普通 F 卷根/子目录和 R 相关目录的独立 OPEN、过滤枚举、继续分页和 CLOSE；普通子目录有带类型的 CREATE/OPEN_IF 和已授 DELETE 的同父不覆盖改名。 | 只有有限的关联形状；某些嵌套通配缺点目录支持。没有目录跨父/跨卷、整组回收和恢复；不是通用目录服务。 |
| 普通 FILE 和写稿 | 有显式已授权 FILE_OPEN、窄 FILE_CREATE/OPEN_IF、偏移 READ/WRITE、已授权 SET EOF、FLUSH、显式正常 CLOSE；原对象单写、兼容读者读确认视图，工作稿保存不发正式版本；有窄范围的独立非阻塞字节锁。 | 不接覆盖式 CREATE、写属性/EA、阻塞或多元素字节锁。断开未确认写稿不等于删除已确认的新名字。 |
| 查询 | 支持普通读/写 FILE 的 Standard、稳定 Internal 和原 OPEN 的 Access，原普通 F 标签／嵌套目录与原读／写 FILE 句柄的 NormalizedName（仅返回当前已核准的共享内原拼写，写入状态未知时拒绝），普通文件/目录与虚拟根的 AttributeTag；已有普通 F 卷根/子目录和实际仍在 R 的 FILE 也支持独立 Standard。根的容量/卷属性及少量探测关联已有单独限界。 | Access 只报告原句柄已经取得的权限，不额外授权或表明写入成功。不对虚拟根或 R 查询 NormalizedName，不返回主机路径。FileAll、真时标及其它未证类不能用宿主或零值补齐；R 根/删除目录、虚拟根和关联形状不进入这些 Standard 路径，已还原旧 R 读柄不再报告当前 R 元数据。 |
| 改名、移动、回收 | 普通非写者 FILE 已有原柄同父改名、限同一实际登记 F 的独立跨父移动（kind14）；普通 FILE 的最后正常 CLOSE 可按原删除标记入 R，实际保留 FILE 可在同树空位恢复（kind12）。同一 FileId、工作修订、历史不重发。 | 普通 FILE 跨不同实际文件系统 F→F 已接入；隔离 Linux 客户端未验。R 不开放普通写或覆盖；现有回收/恢复同步回合不是长请求/取消支持。 |
| 截断、退场和取消 | 已有的 SET EOF 是同一已开 Writer 的工作操作；干净退场、未提交写稿及已确认但丢答分别处理。独立强签 CANCEL 无命中时不应答不授额。 | 跨卷普通 FILE 改名有原请求的在途 CANCEL 和 STATUS_PENDING；不适用于通用复合，也不承诺取消已进内核的 I/O。会话失效不触发 FLUSH/CLOSE/删除。 |

范围外：其它 SMB 方言、guest、AD、多账户/任意共享、单独发布或部署的 SMB 程序、通用 DFS/IPC/RPC、透明缓存/耐久句柄、服务端 COPYCHUNK、宿主元数据透传以及 Web/Helper 的运维权限。正式验收须产品入口真实启动并完成所需业务，再用真实客户端和应用验证；孤立的客户端源码夹具不是产品已通过。

## 普通保存不是一个开关

- **同一原件上写。** 窄受支持顺序是原有 `FILE_OPEN`，或新件的 `FILE_CREATE`/`FILE_OPEN_IF`，原柄 `WRITE`、需截短时 `SET_INFO(FileEndOfFileInformation)`，再 `FLUSH` 或显式正常脏 `CLOSE`，另开回读。当前只有足够且真实已授访问位的 OPEN 可走这条路；泛型/属性/EA 访问位不能削掉后假装原请求已满足。应用用普通保存还是另存、是否轮询元数据、是否发锁，都须记录确切构建和请求顺序。
- **打开即截断。** [Linux v6.12 `cifs_get_disposition`](https://github.com/torvalds/linux/blob/v6.12/fs/smb/client/file.c#L414-L430) 的 `O_TRUNC`/`O_CREAT|O_TRUNC` 对应 `FILE_OVERWRITE`/`FILE_OVERWRITE_IF`；[Samba 4.22 `put`](https://github.com/samba-team/samba/blob/samba-4.22.0/source3/client/client.c#L1776-L1790) 实际请求 create+truncate。Rust 的字段枚举识别这些值，但实际业务拒绝；这不是普通 `FILE_OPEN_IF`，也不能至首条 WRITE 才悄悄截短。[实际覆盖的 FSA 规则](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-fsa/41f3734a-5bba-4c3b-9d04-7baafc9b7bfe) 还约束访问、属性及 EA。若以后明确做，已有分支须以真实同一 FILE/单 Writer、合法属性和原授权为前提，先正确确认长度动作再给 `FILE_OVERWRITTEN`；空位分支才是真首次出生。`SUPERSEDE` 不等价于保持身份的截断；目前都不开。
- **应用临时文件替换。** 写临时文件后 `SET_INFO(FileRenameInformation, ReplaceIfExists=1)` 替换已有目标，与截断原 FILE、无覆盖改名都不同；可能涉及两个 FileId、两份历史/权限和占用。不能把它模拟成覆盖成功、删旧重建或悄悄从源/目标各取一部分历史。无覆盖操作遇已有名字不修改；[TASKS.md 的 WPS 普通保存验收](../../TASKS.md) 不接受用同名新 FileId 或另存代证。当前不支持覆盖式替换；先拿实际应用证据和独立业务规则再设计。
- `FLUSH` 或正常 CLOSE 确认工作修订，不替 Web 手动提交。真正异常断开不隐式确认；结果无法判断时只核原单，不重发保存。若应用确实依赖范围锁、普通 `SET_INFO` 属性或某个数据类，按该已证行为补最小真实权限与存储实现，不把一个目录元数据答复当作保存或挂载证明。

## 普通 FILE 非阻塞字节锁

- 当前限同一 `zxs` 运行内，对已通过原签名、信用和两半 FileId 验证且有正文访问位的普通 FILE OPEN，执行独立、单元素、非空区间的共享锁、独占锁（两者须要求立即返回）或准确的原柄解锁。限制每个原柄 16 把、每个 FILE 64 把，关闭或断开只释放该柄实际持有的锁。不会因为同账户、同路径或同一个数值 ID 把锁转给另一柄。
- 兼容共享锁可重合；共享锁禁止重合写入，包括同一柄的写入；独占锁禁止其他柄读写同一区间。实时读、已持有的同 FileId 保留视图、写入和原 EOF 追加都查同一个状态。ZXS 同时约束显式改长所增减的区间；普通 FLUSH 仅确认原已接收的工作，不另作一次 WRITE，仍不发行正式历史。
- 不支持阻塞等待及其取消、零长锁、多元素或复合、重放、持久柄，或给目录、虚拟根和 R 取得锁。此为[协议定义](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/a2f8b1cc-ebe0-4378-9da9-4e25de5c628f)的受限源码范围，真实 Linux 客户端用例尚未运行。

## 普通 FILE 跨数据卷移动

普通连接已接入这条移动路径。Linux 客户端验收：未运行。不同数据卷用同树路径表达，是 ZXS 扩展；[MS-SMB2 §3.3.5.21.1](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/43cf22fe-84cb-49cf-b3e7-e3d7d766a7b7) 对改名中的分隔符有更窄限制。

- **原请求。** 同一 SMB 树内向另一已登记普通数据卷的空目标发独立、签名、无覆盖的 FILE 改名。用原会话、树和两半 FileId 寻到实际已授 DELETE 的 OPEN，只在它是唯一 OPEN 且没有待删、并行改名时取得交割回合。两端须是同一当代主体的 RW 普通共享；目录、R、不同树和同文件系统的双登记不走此路径。
- **存储。** 原工作端独立打开同一 S，在两个已登记、持有的实际 F 根核原 FILE、父目录、容量和全部保留材料。同步并核对目标后，只由原 kind-12 SQL 事务选择新址和记录旧源清退义务。FileId、历史和工作稿不变；目标不可用时不回退到旧址。
- **原连接。** 一个原改名最多一个 worker，保留已付原帧与 OPEN。挂起响应一次发布 AsyncId 和额度；取消可在最终选址前胜出。工作端以原目标、原回执进行交割复核；同一 OPEN 持旧确认读视图，新开目的见当前稿。终局用原 MID/SID/AsyncId 强签、零额外额度；未决或断流只关闭原连接，不重发 MOVE。
- **验收。** 在同一隔离 Linux 上用[固定版本的 `smbclient rename`](https://github.com/samba-team/samba/blob/samba-4.22.0/source3/libsmb/cli_smb2_fnum.c#L3049-L3149) 覆盖独立签名的 OPEN、无覆盖改名和 CLOSE，查原 FileId、历史、工作稿及源名；另用有类型的客户端在原 OPEN 上验证 READ/CLOSE 及取消、丢答、目标离线。客户端不得代以复制再删；证据按[验收台账](../../TASKS.md)分别记录。
