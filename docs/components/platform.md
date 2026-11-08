<a id="platform-io"></a>

# Linux 平台基础（platform-io）

ZXS 独立的 Linux 服务端基础库，在本项目维护和构建。

crate 入口在编译期拒绝非 Linux、非 x86_64 或非 64 位目标；内部 SQLite
快照模块及导出直接服从这一准入，不再重复操作系统 cfg。可选连接池仍由
`sqlite-pool` 特性控制，不改变默认连接策略或公开快照 API。

| 模块 | 本项目职责 |
| --- | --- |
| `fs_safety` | 持有目录句柄、拒绝链接和路径逃逸、单链接文件、文件及目录同步、无覆盖发布 |
| `payload` | 有长度上限的完整流式写入、BLAKE3 校验、新 inode、不可覆盖发布 |
| `sqlite` | SQLx 0.9 索引连接、WAL/FULL/外键、defensive、原生连接限额、完整性及快照检查 |
| `schema_identity` | 索引表结构身份和确定性指纹 |
| `service_lease` | 稳定目录 inode 的非阻塞 OS 排他租约；当前二进制与 helper Engine 命令的生命周期前置，不是初始化/业务权威 |
| `secret` | 默认脱敏、不可序列化、释放时清零的秘密类型 |
| `disk_smart` | 已持有块设备的同步只读 NVMe／SAT 健康命令、固定缓冲区与准确完成错误 |

`sqlite::block_on_sqlite_connection` 支持同步核心执行直接 `SqliteConnection` 操作；不支持连接池、网络、Tokio 定时器或递归调用。同步 owner 可以跨调用持有直接连接；换文件或释放其保护锁前必须显式 `Connection::close().await`，不能在 future 中遗失连接所有权。`open_index_connection` 固定索引连接策略并启用 defensive；`create_if_missing=false` 不允许隐式新建数据库。 直接连接、连接池和快照连接的错误映射仅解包已识别的 `ConnectionLimitError`；其他 SQLx 错误原样保留，包括 I/O 的原始 errno 和非限额来源，不按 `ErrorKind` 重建。

直接索引打开先取得连接，再执行读取 schema 的 WAL/FULL PRAGMA、defensive 及可选限额初始化；初始化失败显式等待关闭，关闭也失败时返回 `InitializationAndClose`，同时保留两项原因，不能仅凭原损坏错误移动数据库及 sidecar。只读候选入口 `open_index_read_connection` 不创建文件，使用 20 ms busy timeout 与固定原生限额；普通写连接仍使用 5 秒预算。这不是无阻塞或读取成功保证。

`disable_checkpoint_on_close` 是显式、连接局部的能力，不由普通打开入口自动启用；引擎长期索引 owner 在准入前调用它，防止关闭时隐式 checkpoint 或删除可能已换绑的 sidecar。FULL 提交和显式／自动 checkpoint 策略不变；关闭后 WAL/SHM 仍可能存在，不能把关闭理解为仅主文件即可搬移。

`ValidationSnapshot` 属于独立诊断进程的临时验证材料，不是备份或引擎发布谱系。它在原生读锁下捕获一致的 main/WAL/journal，SHM 只检查与锁定、不复制；不得在已持有源 SQLx 连接的同一进程中打开在线源。`into_pool` 消耗同一已捕获代，先前适配器必须已关闭连接，不重新捕获源；`ValidationSnapshotPool::close` 等待池关闭后才释放 wrapper 的快照所有权，池克隆仍通过内部共享所有权保留私有目录。恢复写入仅发生于私有副本，不赋予源数据库健康或事务权威。

平台库不拥有数据盘路由、产品账户状态、产品 DDL 或跨盘事务真相。SQLite 仅为可重建索引。`payload::Receipt` 证明单文件落盘，不能被当作 ACCEPT、FINALIZE 或 PUBLICATION。

认证算法的唯一实现和对应回归测试在 [authentication](auth.md)。该独立 crate 不依赖 Linux 或 SQLite，服务端和辅助工具直接依赖它；平台层不保留兼容导出，避免同一密码/令牌策略维护两份实现。

本项目定制包括：

- 所有索引池连接和直接索引连接在处理业务语句前启用 SQLite defensive。
- 完整 payload 使用固定 64 KiB 缓冲区，不使用硬链接、reflink、稀疏复制或去重；摘要或长度不匹配时，仅在名称仍绑定持有文件时清理暂存文件。
- 存储核心和运维 CLI 使用此 crate 的 Linux amd64 本机能力；远程管理客户端 storage-client 和浏览器 Web 不引入 Linux 盘操作及 SQLite 本地 FFI。原生产品和构建宿主的支持范围仍仅为 Linux amd64。


平台库的 Tokio 直接生产依赖仅启用 `rt`；SQLite 依赖自行声明所需的 `sync`/`time`。测试单独启用宏和多线程运行时，以覆盖两种执行环境。这不会移除服务端实际需要的多线程运行时，也不改变 SQLite 持久化策略。

<a id="admitted-directory-handoff"></a>

## 已准入目录句柄的移交

`OpenAt2Root::from_private_directory` 消耗 `PrivateDirectory`，把已持有的 `File`
直接移交共享私有构造步骤，不沿路径重开、复制 FD 或重复目录类型及 owner/mode 准入。
转换前路径被重命名或替换时，根身份与后续相对读取仍绑定原目录；这不是命名路径
仍指向原目录的证明，也不是新的设备身份缓存。转换仍用 fstat 读取持有 FD 的身份，
并执行必需的 openat2 探测，错误保留原来源；接受未知 `File` 的 `from_directory`
仍在该边界检查目录类型。`SameMount` 仍添加
`NO_XDEV`；相对文件打开仍拒绝符号链接、非普通文件和多链接。

`PrivateDirectory::verify_identity` 的显式命名核验逐分量使用
`DIRECTORY | NOFOLLOW` 打开，再检查 device/inode 与私有权限；祖先符号链接即使
指回同一目录也拒绝，原始 ENOTDIR/ELOOP 等错误保留。它不锁住命名空间，核验后的
读写仍须锚定原 FD。深度 d 为 O(d) 次目录打开、O(1) 额外同时存活句柄，不增加
同步。此原语修正不表示 SYSTEM 的路径消费者或 SQLite 已完成迁移，剩余边界见
[SYSTEM 所有权候选设计](../adr/0014-system-directory-ownership.md)。

## 私有 payload 打开模式

`PrivateDirectory::open_payload` 显式接收 `PayloadOpenMode`：`CreateNew` 使用
`CREATE | EXCL` 排他创建，目标已存在即失败；`Existing` 要求目标已存在。
两种模式都保持可读写、不截断、非阻塞、拒绝符号链接，并在打开后检查普通文件、
单链接、当前服务所有者和精确 `0600` 权限；系统调用错误保留来源。
操作仍锚定持有目录 FD，目录路径被换绑时不转向新目录；提交前另由
`verify_payload` 检查目录和文件身份。HTTP 首次及新代上传使用 `CreateNew`，
同代重试使用 `Existing` 并核对全部物理前缀后续写；SMB 写副本始终排他创建。
模式不提供隐式补建、截断或耐久提交，不改变既有同步、预算及取消所有权。

`verify_payload` 复用本次 `Existing` 打开检查已经取得的设备／inode 身份，
不再对刚打开的同一 FD 重复查询 metadata。身份仅在本次调用内传递；仍重新打开
当前叶名称并检查类型、链接数、所有者和权限，仍检查命名目录及调用方持有文件的
身份。因此先前打开之后的换绑、硬链接或权限变化仍会拒绝，不缓存这些可变条件。
验证成功路径静态减少一次 metadata 查询；打开、内容校验及同步预算不变，非性能实测。

## 原子写屏障所有权

`AtomicFile::create` 与 `replace` 都在本次独占临时文件上完成写入和
`File::sync_all`，再修改目录项，核对发布 inode，最后同步持有的目标目录。
create-only 在发布前保留单链接检查及临时名称到持有 inode 的对应检查，
只复用私有 `rename_no_replace` 目录项步骤，不重新打开并同步同一临时文件。
公共 `NoClobberPublish::publish` 接受未知持久性的 staging，仍独立完成打开、
单链接准入、文件同步、NOREPLACE、发布身份检查和目录同步。
上述入口都只接受同一持有目录内的单个名称，不新增跨目录发布能力。

尝试 rename 之前的验证、写入及文件同步失败保留原目标，内部临时文件由
清理守卫回收，公共发布的 source 由调用方拥有。开始尝试 rename 即解除自动
临时清理：失败回复不证明命名空间没有改变，尚存在的暂存与目标作为证据保留。
rename 成功之后目录同步失败返回
`PublishedDurabilityUnknown` 并保留已发布目标，不伪称回滚。replace、权限、
现存对象重试屏障、锁文件与祖先目录持久化协议不变。

`AtomicFile::stage` 只建立同目录的独占暂存 owner；`StagedAtomicFile::prepare`
写完整字节并完成文件同步后才返回 `PreparedAtomicFile`。后者的消费式
`create` / `replace` 执行原发布与目录屏障，返回准确已发布 FD；
`AtomicCommitError::{NotPublished, Unknown, Published}` 分别表示尚未尝试发布、
已尝试但回复失败、已发布后验证或同步失败。无覆盖目标已经存在的明确检查
在调用 rename 之前返回 `DestinationExists`；rename 自身返回的任何错误都属于
`Unknown`，保留原 errno，不把 EEXIST/ENOENT 当作未发生证明。单步入口委托同一实现并保留原 `Error` API；不新增业务回调。
认证库借此在文件同步后、目录项发布前提交既有权威备份，不重排屏障。
现有单步 `AtomicFile::create` 仅在其 `Error` 适配处把竞态 EEXIST 保持为
`DestinationExists` 分类；这不是 `NotPublished` 证明，也不影响 staged API 的
`Unknown` 与原 errno。AuthStore直接消费staged结果，不经过这个投影。
阶段 owner 在尚未尝试发布时放弃或失败，只清理持有目录中的临时名；
`Unknown`/`Published` 不自动删除证据，目标不参与取消清理。
此边界适用于用户选择的文件系统，不新增本地文件系统限制；
[rename(2) 的 NFS 故障说明](https://www.man7.org/linux/man-pages/man2/rename.2.html#BUGS)
明确指出失败回复可能发生在实际重命名之后。
分阶段后 create/replace 都在提交处复核暂存名与已同步 FD 的 inode/单链接，
不把准备前的名称证明当作永久有效。目录命名路径策略仍由调用方在发布边界
调用 `verify_identity`；平台原子写本身始终绑定原目录而不追随路径替身。

`AtomicFile::replace_held` 与 `replace` 复用相同原子写、清理及全部屏障，
仅在成功后移交原来的只写文件描述符，而非关闭后按路径重新打开。
SMB 活动恢复记录用它绑定刚持久化的 inode；持有 FD 本身不证明后来的
路径身份、字节或权限仍未改变，调用方仍承担复用时的核验。
现有 `replace` 和 `create` 返回类型不变，仍立即释放该内部描述符。
平台测试的 `replace-held` 模式覆盖相同的写、文件同步、发布及目录同步故障窗口。
现有 `workspace` CI 在普通测试后通过 `check-platform-atomic-io.py` 从 Cargo
当前 JSON 产物取得唯一成功的 platform-io libtest，再调用原 `atomic-io.sh`；
不增加 job 或工作流。驱动先确认准确的 ignored 测试存在，每个隔离子进程
必须同时给出对应模式／阶段的完成标记与 `1 passed`，零测试或未运行断言不能通过。

普通 fresh create 的静态预算从两次文件同步加两次目录同步降为各一次
（不计创建祖先目录）；这不是端到端性能实测。故意仅在被删除的第二次同步
注入 EIO 时，错误表现当然不再逐调用相同；耐久顺序仍为文件先于 rename、
目录后于 rename。根测试 `test/platform/unit/fs_safety/atomic-io.sh` 接受已编译的
platform-io libtest 二进制，隔离运行 create/replace/replace-held/staged-create/staged-replace/public 的成功、写失败、
文件同步失败、真实发布冲突和目录同步失败；断言命名空间、源保全与清理。
replace 的目录冲突由 rename 前类型检查拒绝，create/public 由 NOREPLACE 拒绝；
这不冒称每种发布冲突都到达了 rename 系统调用。
其 LD_PRELOAD 日志只覆盖 `std::fs::File` 的 libc write/fsync 路径，检查文件屏障
时目标尚非该 inode、目录屏障时已存在目标，不声称拦截 rustix direct syscall、
全进程 syscall 总数、精确 rename 时间或物理断电行为。

## 目录创建失败后的重试屏障

`PrivateDirectory::create` / `create_child` 在创建或打开并准入目录之后，均执行
子目录同步、父目录同步。`EXIST` 只说明名称目前可见，不能证明先前 mkdir
后的任一屏障已经完成；`created` 只用于新 inode 的行政所有者设置，不作为
耐久证明。权限、所有者与拒绝符号链接规则不变，不修改现存目标权限。

`create_trusted` 创建模式对沿途已存在的祖先同样执行上述屏障，避免深层重试
跳过先前失败留下的祖先；只读 `open_trusted` 不增加同步。引擎
`DiskRoot::private_relative_dir` 从持有根递归准入并同步整条相对祖先链，仍使用
`BENEATH | NO_SYMLINKS | NO_XDEV`，不按命名路径重开磁盘根。

不增加全局缓存或持久格式。创建/子创建成功路径固定两次目录 fsync；深度 d 的
trusted 创建或引擎相对目录准备为 2d 次目录 fsync（相邻层的同一目录可能同步
两次），原先全部已存在路径为零。引擎逐层相对 open 的解析分量总量为 O(d²)，
递归栈 O(d)；这里优先修复耐久缺口，不宣称性能提升。叶文件发布自身屏障不变。

根测试驱动 `test/platform/unit/fs_safety/directory-retry.sh <libtest> platform|engine`
在隔离子进程注入一次真实 libc fsync EIO，覆盖 create、create_child、
create_trusted 多层路径及引擎多层对象写入，分别在新目录自身/父目录首次同步
失败后重试。断言原 EIO 来源、重试子后父屏障、原 inode、0700 权限与现存文件
保全；脚本验证准确 ignored 测试和单测试完成标记，不允许零测试通过。
这不等于完整事务掉电/恢复模型或物理断电验收；拦截只覆盖 std File 的 libc fsync。

CI入口 `.github/scripts/check-directory-retry.py` 从本次成功 Cargo JSON 构建中
取得 platform-io 与 storage-engine 的唯一 libtest，再执行上述十个故障/重试子进程。
`test/ci/test_directory_retry.py` 拒绝缺目标、重复产物、未完成/失败构建、Rust
诊断、零测试与缺完成标记；这组驱动负例不冒称执行了真实 fsync 故障。

初始化与历史导出使用的 `disk::private_dir` 复用 `create_all`，对已存在祖先
补齐屏障，不再只在 NOENT 时向上递归。它保留原准入域：已有祖先只要求目录且
无符号链接，新创建组件和最终叶检查当前owner/0700；不增加trusted ancestry或
跨挂载限制，不 chmod 或调整现存目录所有者。绝对链深度d为2d次同步；缺失
组件复用原 `create_private_at` 的两屏障，不重复同步。
`engine-path` 两场额外故障覆盖新祖先自身/父同步失败后深层导出路径重试。

认证故障驱动还显式构建 `rustix/use-libc` 隔离测试产物，使 LD_PRELOAD 能调用
真正的 renameat/renameat2 后再返回 EIO。创建/替换各覆盖调用前 EIO（保留已同步
暂存）与成功后 EIO（目标已更新但回复失败）；检查 `Unknown`、原 errno 与证据。
另在预检查后创建占位目标，再调用real renameat2获得真实EEXIST，分别验证
单步create的现有错误分类与staged-create的Unknown阶段，并保留占位和暂存。
该 libc 后端证据不冒充正常 direct-syscall 产物或真实 NFS/断电实测；正常后端
屏障矩阵仍独立执行，生产后端与依赖特性没有更改。

## 公开完整 payload 发布边界

`NoClobberPublish::publish` 要求传入已持有的 `&File`，返回
`AtomicCommitError`；不再仅凭名称重新打开文件并建立另一份身份。
`payload::write_full` 在 reader 回调前持有新建文件，发布前核对暂存名称，
rename 后再次将目标身份绑定到同一 FD 并核对普通单链接文件。成功收据的
身份与摘要属于该文件，不给替代 inode 返回原文件的摘要。
调用者仍须排除发布／清理期间并发命名空间变更，以及整个写入期间外部内容写入；
名称／身份检查不是对任意并发写入的内容不可变证明，也不是条件 unlink 原语。
reader 回调已发生的名称替换由检查拒绝，失败清理不删除观察到的替代物。

发布前失败为 `NotPublished`；开始尝试 rename 后，任何失败回复（含 EEXIST）
均为 `Unknown`，成功 rename 后的核验或目录屏障失败为 `Published`。
`payload::Error::Publication` 保留该阶段及原 I/O 来源，Unknown/Published
不自动删除暂存或目标，调用者须核验现场后决定后续处理。清理失败仍同时
保留主要失败和清理来源。此公开 API 已收紧，不提供旧签名兼容；当前仓库
生产业务未调用这两个 helper，不能将此修复描述为已发现的线上数据损坏。

完整 payload 从原先两次文件 fsync 改为对持有 FD 的一次文件 fsync，顺序仍为
文件屏障 → NOREPLACE rename → 目录屏障；固定 64 KiB 缓冲及全量摘要不变。
这是系统调用预算变化，非端到端性能测量。根 `atomic-io.sh` 的普通屏障矩阵
覆盖 payload 写、文件同步、真实占位冲突、目录同步；`rename` 矩阵须使用
`rustix/use-libc` 隔离产物，覆盖 public/payload 的 rename 前 EIO、真实 rename
后 EIO 与真实 EEXIST。断言阶段、原 errno、源／目标字节与现场保留，不冒充
生产 direct-syscall 后端的 rename 注入或真实 NFS／物理断电验收。

## SMART 设备命令的进程所有权

`disk_smart` 只接受已验证为块设备的借用 FD，提供固定 NVMe Identify／Health Log 及
SAT 电源检查、标识和 SMART 只读命令。它不打开设备路径、不创建子进程、不提供通用
透传，也不更改设备设置。调用方在每次同步 ioctl 实际返回之前保留设备及目录准入，
取消和驱动期限都不能提前释放所有权。

服务端把 ATA 固定原始回复交给官方 smartctl 离线解码，保留型号／固件数据库语义；
解码器不接收目标设备 FD 或路径。准确命令、资源上限、Linux／smartmontools 版本化
来源及待执行测试见 [SMART 查询生命周期边界](../validation/smart-query-lifetime.md)。
源码及格式检查不代表真实设备或跨进程重新准入已验收。
