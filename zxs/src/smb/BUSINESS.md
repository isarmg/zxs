# ZXS 内置 SMB 3.1.1：普通业务边界

这是同一 `zxs` ELF 内唯一维护的 ZXS 私有 SMB 的**源码台账**，包含已审普通目录 FileStandard 和下述跨文件系统准备。不提供通用/独立 SMB 服务、旧格式兼容或迁移。此私有源码增量未发布、未启动、未碰实盘；[TASKS.md](../../TASKS.md) 的历史记录不代替内置服务的客户端/应用验收。

| 业务 | Rust 候选源码状态 | 尚未证明或未接 |
| --- | --- | --- |
| 登录、连接 `ZXS`、退出 | 有严格 NTLMv2、适用 MIC、SHA-512 预认证和原流 AES-CMAC；使用真实 Core 当前账户与盘权限；ECHO、TREE_DISCONNECT、LOGOFF 有原命令分派。 | 未从源码推断 Windows 冷连接、Finder、Linux 挂载或任一真实客户端登录；已有离线构建与开发检查；仍未部署或原生客户端验收。 |
| 浏览和目录 | 有真实虚拟根、普通 F 卷根/子目录和 R 相关目录的独立 OPEN、过滤枚举、继续分页和 CLOSE；普通子目录有带类型的 CREATE/OPEN_IF 和已授 DELETE 的同父不覆盖改名。 | 只有有限的关联形状；某些嵌套通配缺点目录支持。没有目录跨父/跨卷、整组回收和恢复；不是通用目录服务。 |
| 普通 FILE 和写稿 | 有显式已授权 FILE_OPEN、窄 FILE_CREATE/OPEN_IF、偏移 READ/WRITE、已授权 SET EOF、FLUSH、显式正常 CLOSE；原对象单写、兼容读者读确认视图，工作稿保存不发正式版本。 | 不接覆盖式 CREATE、写属性/EA、字节范围锁。断开未确认写稿不等于删除已确认的新名字。 |
| 查询 | 支持普通读/写 FILE 的 Standard 和稳定 Internal，同原读／写 FILE 句柄的 NormalizedName（写入状态未知时拒绝），普通文件/目录与虚拟根的 AttributeTag；已有普通 F 卷根/子目录和实际仍在 R 的 FILE 也支持独立 Standard。根的容量/卷属性及少量探测关联已有单独限界。 | FileAll、真时标及其它未证类不能用宿主或零值补齐；R 根/删除目录、虚拟根和关联形状不进入这些 Standard 路径，已还原旧 R 读柄不再报告当前 R 元数据。 |
| 改名、移动、回收 | 普通非写者 FILE 已有原柄同父改名、限同一实际登记 F 的独立跨父移动（kind14）；普通 FILE 的最后正常 CLOSE 可按原删除标记入 R，实际保留 FILE 可在同树空位恢复（kind12）。同一 FileId、工作修订、历史不重发。 | 普通不同实际文件系统 F→F 有共享 Core 引擎及不触业务的原请求准备票，**尚无执行路由**。R 不开放普通写或覆盖；现有回收/恢复同步回合不是长请求/取消支持。 |
| 截断、退场和取消 | 已有的 SET EOF 是同一已开 Writer 的工作操作；干净退场、未提交写稿及已确认但丢答分别处理。独立强签 CANCEL 无命中时不应答不授额。 | 无真实在途 CANCEL、异步 STATUS_PENDING、通用复合或大文件移动；不承诺能取消已进内核的 I/O，也不从会话失效触发 FLUSH/CLOSE/删除。 |

范围外：其它 SMB 方言、guest、AD、多账户/任意共享、单独发布或部署的 SMB 程序、通用 DFS/IPC/RPC、透明缓存/耐久句柄、服务端 COPYCHUNK、宿主元数据透传以及 Web/Helper 的运维权限。正式验收须产品入口真实启动并完成所需业务，再用真实客户端和应用验证；孤立的客户端源码夹具不是产品已通过。

## 普通保存不是一个开关

- **同一原件上写。** 窄受支持顺序是原有 `FILE_OPEN`，或新件的 `FILE_CREATE`/`FILE_OPEN_IF`，原柄 `WRITE`、需截短时 `SET_INFO(FileEndOfFileInformation)`，再 `FLUSH` 或显式正常脏 `CLOSE`，另开回读。当前只有足够且真实已授访问位的 OPEN 可走这条路；泛型/属性/EA 访问位不能削掉后假装原请求已满足。应用用普通保存还是另存、是否轮询元数据、是否发锁，都须记录确切构建和请求顺序。
- **打开即截断。** [Linux v6.12 `cifs_get_disposition`](https://github.com/torvalds/linux/blob/v6.12/fs/smb/client/file.c#L414-L430) 的 `O_TRUNC`/`O_CREAT|O_TRUNC` 对应 `FILE_OVERWRITE`/`FILE_OVERWRITE_IF`；[Samba 4.22 `put`](https://github.com/samba-team/samba/blob/samba-4.22.0/source3/client/client.c#L1776-L1790) 实际请求 create+truncate。Rust 的字段枚举识别这些值，但实际业务拒绝；这不是普通 `FILE_OPEN_IF`，也不能至首条 WRITE 才悄悄截短。[实际覆盖的 FSA 规则](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-fsa/41f3734a-5bba-4c3b-9d04-7baafc9b7bfe) 还约束访问、属性及 EA。若以后明确做，已有分支须以真实同一 FILE/单 Writer、合法属性和原授权为前提，先正确确认长度动作再给 `FILE_OVERWRITTEN`；空位分支才是真首次出生。`SUPERSEDE` 不等价于保持身份的截断；目前都不开。
- **应用临时文件替换。** 写临时文件后 `SET_INFO(FileRenameInformation, ReplaceIfExists=1)` 替换已有目标，与截断原 FILE、无覆盖改名都不同；可能涉及两个 FileId、两份历史/权限和占用。不能把它模拟成覆盖成功、删旧重建或悄悄从源/目标各取一部分历史。无覆盖操作遇已有名字不修改；[TASKS.md 的 WPS 普通保存验收](../../TASKS.md) 不接受用同名新 FileId 或另存代证。当前不支持覆盖式替换；先拿实际应用证据和独立业务规则再设计。
- `FLUSH` 或正常 CLOSE 确认工作修订，不替 Web 手动提交。真正异常断开不隐式确认；结果无法判断时只核原单，不重发保存。若应用确实依赖范围锁、普通 `SET_INFO` 属性或某个数据类，按该已证行为补最小真实权限与存储实现，不把一个目录元数据答复当作保存或挂载证明。

## 普通 FILE 跨不同实际数据文件系统：待实现交割

**复用 ZXS Core，不依赖旧前端。** [`file_transfer.rs`](../system/file_transfer.rs) 的 kind12 普通 `MoveOrdinary` 起点保留原 FILE 物证与该 OPEN 的旧确认视图，独立检查真实可写的两个不同文件系统、容量、名称和所有保留材料，核验并同步目标。只在 `transaction_before_commit` 的唯一选址事务切 `Place`、记同一原单和源清退义务；不创建第二 FileId、HistoryId、Node 或分支。旧实物可能仍保留，但不能再从旧可见名字打开，不能拿源替代离线目标。独立目标已验证且原退休/旧读租条件满足后才可安全清退。旧调用只是规则对照，不是实施依赖或要保留的服务。

1. **准备票已有源码，未分派。** [`qualify_ordinary_cross_filesystem_move`](connection/sessions/ordinary/outbound/ordinary_move.rs) 只从原已付强签独立请求查同一真 Session/Tree/两半 FileId，读型普通 `FileOpen` 实际已授 DELETE，再取同一个全运行共享票的独占回合（无其他 OPEN/待删/并行改名）。[`bind_ordinary_move`](../system/smb_rust_ordinary_move.rs) 在同一个 S 读切面绑定 FileId、已有原名/放置/读视图、确切目标父及叶名、同一当代 Rust 身份的双 RW 代。仅允许不覆盖、有显式不同已登记 F 标签的树相对拼写；含 R、目录、复合、跨树或别名不进入这条业务。这个只读切面不是介质证明：工作端仍须持实物并区别同文件系统/物理只读或目标离线。它是 ZXS 的窄扩展，不把 [MS-SMB2 的分隔符约束](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/43cf22fe-84cb-49cf-b3e7-e3d7d766a7b7) 当作普通客户端一定会发送或接受的跨卷协议。
2. **原 Peer 所有权已写入，收发仍不派发。** `PendingOriginalMove` 只能由同一实际流拿下的完整 Direct TCP 原字节、同借用复合对象的原码校验和该连接唯一未结 `Debits` 形成；同存原签名凭据代、MID/Session/Tree/两半 OPEN 与第 1 步准备票。不能克隆或换字节、换 peer、另借签名者，也没有解债、兑额度或提前返回成功的入口。丢原未结票即终止原流；当前同步轮若看到暂挂状态只终止，不会把它误当下一命令。将来的最多每连接一个、服务级限额、无排队的 Core 工作者只领同批准启动、独立同源 S reader、原实物/两端认证坐标和弱活性条，不领 socket、密钥或本线程 SQLite/Rc OPEN。共享引擎在初次选择、取得 FILE EX 后及写事务发预留前均拒绝相关既有预留或复用 Op，不从新请求领取旧收据、清旧候选。旧单仅同原持有/恢复规则处理。
3. **仍由原连接接收与排空。** 需要原 owner 观察网络 EOF/半关闭、坏定界、撤权、停止，以及同原签名 CANCEL；另有 worker 就绪事件，不能让其它 F 上的同步磁盘操作阻塞收包，也不借轮询线程写另一条流。原 OPEN 的冲突请求与退出要在这一个 owner 内封住并排空。若有标准 `STATUS_PENDING`，中间记录一次授额并发布唯一 AsyncId，终局用原 SID/MID/AsyncId、`CreditResponse=0`、原强签；无伪造签名/免核。挂起未完成不是完成，见[取消](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/57bae3d3-5dd7-4a5f-92cb-fc52e2087dad)与[异步额度](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/46256e72-b361-4d73-ac7d-d47c04b32e4b)。
4. **用真实最终钩子作唯一选择。** 共享 [Core](../system/file_transfer.rs) 已有 `OpenedOrdinaryMove::before_selection`；Rust 必须使其成为对应原在途票的一次非阻塞选择。早期 SQL 占位/候选不是选址。竞争撤销先胜且 Core 可证明未选址时，才可向仍活原命令回复 `STATUS_CANCELLED`；未知预留实物保留。门先胜后凭唯一确认回执收口，不因晚取消、掉线或后验失败宣布回滚，无法确认或失去原流即停止，无自动再发 MOVE。活性见证是原 socket 的零等待核查及短原身份检查，不读客户字节、不复制 endpoint 增活、不把慢 SQL/物证塞进最终回调。
5. **真正重绑原 OPEN。** 共用 [`first_work.rs`](../system/first_work.rs) 的 `MoveTargetAt` 目的核定，按同一个 Op 和 FILE 把原确认读视图保持在实际已持目标上。Rust 仍须返回真正持有的目标 Route（不是用名字重开）及原放置、源/目标认证代和耐久历史回执，匹配原共享票与 OPEN 代后方可替换其持有。发布前后须核原授权、实物和回执；慢的物证检查由同一在途单的 worker 完成，原连接只做有界核验、写原签名响应和结账。发送已不可撤回，不以晚失败假装确定无效。之后同一线上句柄仍为原 FileId/原读视图，新开目的才读当前稿。这个交割若做不到，继续明确拒绝，不能改成同步复制或删源再建。

**当前阻断点：** [`Peer::poll_with`](connection/accepted/incoming.rs) 的普通活路径仍要求每个原请求本轮结账，否则关连接；新暂挂容器只备实际所有权，未接异步 socket/worker 事件、真实取消、回执/目标复核、原柄采用和最终单次结账。没有中间或最终的响应生成器。正常移动、确认后丢答+目标离线、早/晚取消、同实际 FS 双登记、忙/撤权、保留历史和原柄退场的验证留待所需业务和统一入口完成后做一次集成验收，此处只有源码。
