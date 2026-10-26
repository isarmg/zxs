# ZXS SMB 桥接

本目录包含 `smb-protocol` 本机协议与 `smb-vfs` Rust 模块，目标 ABI 固定为官方 Samba 4.23.0 SDK。Samba/smbd 提供网络服务，ZXS 通过受保护的本机 RPC 执行授权和存储操作。编译、单元测试与真实客户端互操作须分别验证。

## 目录与职责

| 路径 | 职责 |
| --- | --- |
| `src/smb/src/protocol.rs` | 有上限的严格 JSON 帧、SCM_RIGHTS 文件描述符转交 |
| `src/smb/src/client.rs` | 同步本机 RPC 客户端；同一连接上的请求串行化 |
| `src/smb-vfs/src/module.rs` | Rust 编写的 Samba VFS 回调，不包含自写 C 桥接 |
| `src/smb-vfs/src/asynchronous.rs` | 两个 Rust 工作线程、有界队列及 eventfd/tevent 完成与取消处理 |
| `src/smb-vfs/build.rs` | 校验固定 SDK 版本，使用 bindgen 生成 C ABI 绑定 |
| `src/server/src/smb_rpc.rs` | 唯一身份绑定、句柄、私有工作文件、核心提交和读租约 |

客户端的同一连接在全部 clone 之间串行使用。请求发送、响应接收或帧解析失败后，立即关闭并移除共享连接；后续调用明确返回 `NotConnected`，不会把残留响应当作另一请求的结果。完整远端业务错误保留连接。VFS 从句柄表移除文件后立即建立 FD 所有权守卫，锁失败或保存失败也会关闭本地描述符；本机关闭策略与协议回归共用 `client::close_descriptor`，不依赖手写 Samba ABI 测试替身。

目录枚举期间仍保留其打开记录，供属性查询和相对子项操作使用；`CLOSEDIR` 按宿主槽位与远端句柄身份共同匹配并消费这份记录，复用文件关闭的 Close／Detach 与本地 FD 释放流程。不能等待后续普通 `CLOSE`，因为 [Samba 4.23 的目录析构](https://github.com/samba-team/samba/blob/samba-4.23.0/source3/smbd/dir.c#L986-L1010)会使原 fsp FD 失效。首次目录页失败时仍由普通关闭负责；成功打开后，即使翻页、远端关闭或身份检查失败，本地资源也必须终结，旧游标不得释放复用宿主槽位的新句柄。

`smb-protocol` 是根工作区中的协议包，服务端直接依赖它；没有独立工作区或锁文件。VFS 位于单独的 `smb-vfs` 包，默认要求 `smb-sdk` 特性。缺 SDK 时构建模块会失败，不产生可以误认为完整插件的空实现。

## 已实现的桥接语义

- 只允许受信任的本机 Samba 进程 UID 连接，使用 Linux `SO_PEERCRED` 校验；JSON 中没有远程客户端可以自行声明的 ACL、管理员权限或任意核心命令。
- VFS 从真实 Samba 会话安全令牌读取认证身份，拒绝匿名、guest、缺少 Authenticated Users、缺失身份及 force-user 会话，不信任回调中自报的用户名。服务端只接受显式配置的单一 Samba account_name，并绑定当前唯一 ZXS 管理员；Samba 凭据与 ZXS 凭据独立。
- 分享范围固定到一个仓库的主分支。读打开取得具体版本的文件描述符，服务端保留核心读租约。后续提交不会改变旧句柄读到的版本。每次 VFS 操作（含缓存 readdir）在本地复核真实 Samba token 与同一 account_name，guest、anonymous 或 force-user 立即拒绝。ZXS 唯一身份与共享绑定在每次实际 RPC 目录页请求、读取授权和写入发布中复核；同一已返回目录页的逐项缓存访问不增加逐条 RPC，也不声称能撤回已经返回的数据。既有 FD 不允许绕过后续受支持 I/O 的身份与绑定检查。
- 写打开使用服务端私有目录中的完整独立工作文件，FD 通过 SCM_RIGHTS 转交；RPC 不返回磁盘内部路径。
- `Stat`、已有文件或目录的 `Open`、`rename` 源及 `unlink` / `rmdir` 源允许 NFC 名称的规范等价输入，例如以 NFD 查询已保存的 NFC 条目；查找仍受原始输入的字符、保留名、结构及长度预算限制。命中的打开句柄保留条目现有显示路径，可写句柄后续上传也使用此路径，不隐式改名。带创建意图的打开未命中时，先严格校验 NFC 新名称，再分配工作区或工作文件；独占创建遇到已有等价名称仍冲突。`mkdir` 和 `rename` 目标始终严格校验新名称。此入口区分不改变 Unicode 折叠、允许字符或持久化命名策略。
- 选中的未写入分支继承父版本的内容和大小。只读打开取得父版本的只读描述符，写打开从父内容建立私有工作副本；仅打开后关闭不创建版本，实际写入或显式截断并成功发布后才形成自己的分支版本，父版本不变。目录分支中各个文件分别保留继承或自己的版本。
- `pwrite` 与 `truncate` 修改前调用 `PrepareWrite` 检查已预留的文件上限与当前恢复空间，修改工作文件后调用 `Dirty`；write-through 的 `Dirty` 同步触发保存。VFS 的 `FLUSH` 和最后 `CLOSE` 经完整文件上传及核心事务提交后才返回保存成功。
- `Dirty` 先保留当前工作区和操作号的恢复记录，再重新授权；每次 Dirty 在未发布代内只写一次恢复记录。已发布句柄建立新工作区后仍再次记录新的关联。普通 `Dirty` 不同步正文 FD，不等同于 FLUSH／write-through／最终 CLOSE 的持久保存回执。
- 写锁按分支和规范化路径绑定。不同文件允许并发写入，最终由核心检查各自文件版本并串行发布；同文件跨会话写打开明确拒绝。目录改名、删除和替换目标只与路径覆盖范围内的活动写句柄冲突，读句柄仍固定原版本。
- 重试先查询原 `operation_id`，已发布结果直接返回；未决结果要求恢复，不重新生成操作或再次推进上传代际。
- `Flush` 失败保留活动句柄，可以先核对原操作再重试。`Close` 无论成功或失败都终结该句柄：保存或清理工作区失败时，将工作文件和 `*.work.json` 保留为脱离态，释放该句柄的 SMB 路径锁及句柄额度，返回原业务错误和原 `operation_id`。不因此关闭整条 RPC 连接，不影响其余句柄，不用自动 Abort 代替失败保存。
- I/O 工作线程锁中毒时，VFS 发 `Detach` 而不是尝试保存。`Detach` 只保留正文/恢复关联并释放该句柄，不提交、不放弃工作区、不删除暂存。客户端和服务端应一起升级以支持此命令。
- 断开连接不会把脏数据自动发布为历史，也不删除工作文件和恢复记录。丢失发布回执后再次改动的字节不会冒充先前发布的成功结果，须核对原操作并创建新工作区继续保存。
- 目录创建、删除、改名进入核心命名空间事务。`unlinkat` 的普通删除与 `AT_REMOVEDIR` 分别转为 `unlink` / `rmdir`，在工作区冻结树上复核目标种类；普通删除不得删除目录，目录删除不得删除文件，未知 flags 在发出 RPC 前失败。旧的不区分种类 `delete` RPC 不再接受，模块和服务端须一起升级。允许替换的改名在同一个工作区中执行删除目标及改名，避免发布中间状态；不支持的文件特性明确失败。
- 命名空间请求必须携带客户端预分配的 `operation_id`，服务端严格验证并沿用。工作区创建后、准备变更前，将原操作号、确切工作区提交请求和目标路径写入私有 `*.namespace.json`，文件及父目录同步成功后才允许提交。一旦进入提交调用，任何错误都不证明未发布，因此不自动 Abort、不删除恢复记录。只有确认核心成功，或准备阶段失败且记录同步及 Abort 均成功，才清理本次记录；记录同步失败或 Abort 失败保留已有证据并告警。不会自动用新操作号重做未知结果。
- 模块源码拦截链接、复制卸载、FSCTL 写入、权限与扩展属性等可能绕过核心的修改路径，禁止由下层 VFS 修改不可变版本文件。
- 异步读取、写入与同步在有界工作池中完成；工作线程只拥有 FD、缓冲区和 RPC 客户端，不持有宿主指针。取消请求后，不再访问已释放的事件状态。sendfile/recvfile 明确拒绝，避免绕过读取授权与保存边界。
- 文件名规范由核心负责；POSIX stat 区分实际拼写，文件名查找返回核心的规范名，以支持 SMB 不区分大小写打开与保留大小写改名。查找失败时保留调用者原指针，避免破坏宿主后续创建路径。

<a id="smb-local-rpc"></a>

## 本机 RPC 部署

socket 使用服务进程拥有的 `0700` 运行目录，例如 `/run/zxs`，socket 权限为 `0600`。SMB 完整工作副本、操作证据和预算记录独立保存在系统盘 `--management-root/smb-spool`，不从 socket 路径推导；默认管理目录为 `--root/management`。管理目录必须持久保存，不能设在 `/run`、临时文件系统或版本安装目录。受信任嵌入调用通过 `SmbConfig.spool_dir` 显式提供同样持久的路径，没有另一个 SMB spool CLI 路径开关。

两个路径分别安全校验，缺失的私有组件按 `0700` 创建，整个路径不能包含符号链接。SMB spool 及其 `empty-directory` 也要求 `0700`。祖先只接受 root 或服务身份拥有、不可被其他用户替换的目录；标准 root-owned sticky `/tmp` 可作为安全父目录，但路径校验不等于持久介质保证，不能用于生产留存材料。非 sticky 的组/全局可写目录和外部用户所有的祖先会被拒绝，既有目录不会自动 chmod。创建暂存文件及发布恢复记录使用持有的目录描述符；Flush 提交前检查目录/源文件身份，并把原可写 FD 的长度和摘要绑定到核心复制结果，校验失败不会接受工作区文件或提交。摘要和复制均使用定位读取，不改变 Samba 客户端共享的 FD 游标。

服务端启动配置需要：

1. socket 的绝对路径。
2. Samba 可信进程连接时的 UID；这个身份是进程代理的信任边界。
3. `--smb-user SambaAccount` 指定一个精确 Samba 认证 account_name；不能重复或含 `=`，不接受多账户映射。ZXS 唯一管理员须已初始化，用户名无需与 Samba 相同。

可信 Samba 进程必须在模拟远程用户前建立受保护连接。目标环境需要验证其实际回调时机与 UID；不能为了让连接成功而把 socket 或目录改成所有用户可读写。Samba 必须配置 `map to guest = Never`、`restrict anonymous = 2`、`guest ok = no`、`guest only = no`，并将 `valid users` 限于该唯一账户；不得配置 `force user`。参考[部署样例](../../deploy/samba/smb.conf.example)，部署者独立准备 Samba 凭据，不自动创建或重置密码。

socket 已存在时服务端拒绝覆盖，启动时不会删除未知对象。正常退出或任务取消时，会关闭所有活跃 RPC socket，让阻塞读取退出；只在路径仍对应自身绑定的 device/inode 时移除 socket。服务崩溃遗留的 socket 须由管理员确认后清理。

运行目录清理或系统重启不应删除系统盘上的 SMB 留存材料。旧部署若曾将 spool 放在 socket 旁，本版本不会自动扫描、迁移或删除旧目录；升级前必须另外保全并核对旧留存材料，不能假定启用新路径已恢复旧写入，也不能通过重启触发旧运行目录清理来代替核对。spool 不安全、损坏或无法盘点时只停止 SMB 桥接，管理 HTTP 保持可用。

帧格式为 `u32` 大端 JSON 长度、严格 UTF-8 JSON；最大 8 MiB，分配 payload 前检查长度。可选单个 FD 通过四字节帧头的 SCM_RIGHTS 附带；接收端立即设置 CLOEXEC，拒绝额外或截断的 FD。客户端请求禁止附带 FD。64 位字段使用规范十进制字符串。

握手双方必须显式声明 `capabilities: ["authenticated-single-account", "durable-write-reservations"]`。缺少任一能力的旧模块或服务端在创建会话、复制文件和转交 FD 前即被拒绝，不提供匿名或跳过 `PrepareWrite` 的兼容路径。能力是实现语义声明，不是认证凭证；代理 UID、受保护 socket 与真实 Samba 安全令牌仍是信任边界。

仓库在拆分、恢复后默认禁止 SMB，新连接明确返回 `SMB_REBIND_REQUIRED`；管理员须通过受权绑定接口核对历史代际后启用，原共享不自动跳到新历史。影响范围内仍有活动写工作区时，绑定变更先返回 `WORKSPACE_BUSY`，不隐式取消工作区。每次成功绑定变更推进独立的 `smb_binding_generation`，握手捕获该代际、仓库和分支。后续读取授权、新打开、目录分页、命名空间操作与写入发布在核心锁内复核，禁用或重新绑定后旧会话不会因再次启用而恢复。普通文件提交和默认目录组成选择不推进绑定代际，已打开读句柄仍固定原版本。代际失效不强行回收宿主 FD，但后续受支持 I/O 返回 `SMB_REBIND_REQUIRED`；`close` / `detach` 仍可释放句柄，失败写入保留恢复证据。

目录枚举必须先打开目录，再用 `read_directory {handle, restart}` 获取页面。打开目录时捕获不可变提交、完整规范路径和目录身份；后续页使用服务端私有游标，客户端不能提交历史快照或任意路径。每页重新授权，最多 256 项，完整响应不超过 256 KiB；直接在排序后的提交条目上定位和投影，不复制或序列化整棵树。VFS 仅缓存一页，`rewinddir` 重新捕获当前快照。打开后目录被改名或替换，原枚举仍读取已捕获快照；重新开始时若原路径身份不再匹配则返回失效错误，不跟随新目录。旧 `list` 命令已移除，模块与服务端必须一起升级。任何成功响应意外超过 8 MiB 时，服务端返回 `RESPONSE_TOO_LARGE` 业务错误并保留连接；操作可能已经完成，应按错误携带的原操作号核对，不盲目重试。

命令为 `hello`、`open`、`authorize_read`、`prepare_write`、`dirty`、`flush`、`close`、`detach`、`abort`、`stat`、`read_directory` 和 `namespace`。一个连接只能 `hello` 一次，此后固定身份与仓库。错误带 `code`、`retryable`，保存及合法命名空间请求的错误还附原 `operation_id` 供核对。命名空间请求不接受缺少操作号的旧报文；客户端遇到传输错误时关闭所有克隆共用的连接，保留原 I/O 错误种类/errno，并以 `RPC_OUTCOME_UNKNOWN` 携带请求原操作号。即使核心已成功而成功 RPC 帧丢失，调用者仍可查询原操作，不能自动换新操作号重试。命名空间错误统一 `retryable: false`，不改变活动文件 `Flush` 的原位恢复流程。这不是 ACK 协议或端到端 exactly-once；同操作号的活动请求、已有权威操作或残留记录均拒绝重新执行命名空间命令。write-through `Dirty` 若已经建立新一代工作区，错误附新一代实际提交的操作号，而非上一次成功发布的操作号。`close`/`abort` 返回业务错误后句柄也已终结，再对原 handle 发请求得到 `NOT_FOUND`；这不是原核心操作不存在的证据。要重试活动保存应使用 `flush`，不能重新提交一个结果未知的原操作。

握手和客户端响应接收有 120 秒整帧期限；已认证连接等待下一帧首字节时不设空闲期限，收到首字节后仍须在 120 秒内完成整帧，不能通过逐字节发送续期。socket 写入等待上限仍为 120 秒。服务停止会关闭已登记 socket，以取消空闲阻塞读取。断连和传输错误释放会话活动锁，但不会自动重连、重放不确定写入或复用旧 handle。失败 `close` 现在在返回响应前释放自身路径锁，不再依赖空闲超时。锁只覆盖同分支、相同或祖先/后代重叠路径的 SMB 写入和相关命名空间操作，不是 HTTP 写锁，也不影响其他分支或无重叠路径。

桥接全局最多同时接受 64 条连接，握手仍受上述期限约束；最多保留 8 条已认证 SMB 会话，所有仓库共享同一会话计数，不允许 Samba 用户名别名映射。超额握手立即返回资源限制并关闭，不能靠增加共享或永久空闲会话绕过 8 条上限。认证失败、断连、传输错误及服务取消均释放会话额度；已准入的空闲会话不因此被超时断开。每会话最多 1024 个句柄的限制保持独立。

## 私有暂存字节预算与恢复

写打开在复制原内容或交付可写 FD 之前，按已有内容大小预留额度；扩展写入由 `PrepareWrite` 在操作前增加并持久保存最高获准文件末端。这个动态额度与文件静态上限不同：静态上限取核心能力与服务端 `--max-upload-bytes` 的较小值，小文件不会预占整个静态上限。每个副本另计 64 KiB 元数据额度。`--smb-spool-bytes-per-account` 默认为 100 GiB，`--smb-spool-bytes-total` 默认为 500 GiB，`--smb-spool-recovery-reserve-bytes` 默认为 1 GiB；这是独立于 HTTP spool 的 SMB 预算，不是存储池总容量配额。原文件超过允许上限时拒绝可写打开，不先复制后检查。每次新增或增长预留时，空闲空间检查保守地额外覆盖全部已获准额度，因此可能早于文件系统实际满盘拒绝新写入；每个复制块和 VFS 写入前再次检查恢复空间。不能以本机制保证外部进程永远不占用同一文件系统。

`*.budget.json` 在复制前持久记录主体与已获准额度，后续增长在回复 `PrepareWrite` 前原子更新。`Flush` 成功不释放额度；断连和重启也不能仅按当前文件长度释放，因为 Samba 仍可能持有可写 FD，已经获准的在途写入仍可能完成。预算依赖受信任 Samba 模块在所有写入前完成预检；断连后不得绕过 RPC 直接扩展 FD，也不把此逻辑额度视为针对恶意本地进程的文件系统强制配额。只有显式成功 `Close`/`Abort` 清理工作文件、恢复记录及预算记录后，才释放额度。命名空间恢复记录也预留元数据额度。失败清理保留原证据与计费，不以删除不确定数据恢复容量。

预算增长与初始预留在全局账本锁内先计入总量和账户额度，随后释放锁持久化；未完成持久化的额度仍已占用，不同 source 可以同时落盘。同一 source 的增长由不可克隆 `Lease` 的独占可变借用串行化，不另加全局写锁。持久化成功后回锁复核共享不确定状态才回复许可；任一在途持久化失败或展开都保留最高计费并封闭后续准入，另一请求即使已完成磁盘同步也不能越过这个围栏。正常调用的锁中毒继续暴露内部错误；析构不二次 panic、不恢复中毒账本，也不删除其证据。写打开尚未交付 FD 时，本次新建私有副本的准备失败仍可按既有流程清理并放弃工作区；这不授权删除已有恢复证据或原始正文，预算结果不确定时其记录与计费仍保留。此改动不减少 budget 文件／目录同步次数，不批量预留增长额度，不按正文实际长度回收已获准高水位。

启动时重建这些预算；没有可验证所有权的旧文件或孤立文件同时计入总量和唯一主体的准入，超额时保留数据并拒绝新增预留。损坏预算记录使 SMB 暂存拒绝服务，不静默修复或删除，管理 HTTP 仍独立可用。管理员应先停止 Samba 与桥接服务、确认无残留可写 FD，再按原操作号核对权威结果，导出或明确处理保留数据及配套记录，之后重启重新盘点；不要在服务运行时直接删除预算文件来释放额度。此处没有自动过期清理或替用户决定丢弃未确认写入的接口。

预算并发回归在 `test/server/unit/smb_rpc/budget_tests.rs`：测试专用转发器仅暂停真实持久化函数的调用边界，生产仍执行 `create_new` 和临时文件同步、替换、目录同步。独立故障入口 `bash test/server/unit/smb_rpc/budget-io-eio.sh <storage-server-libtest-binary>` 将 `LD_PRELOAD` 仅传给六个子进程，分别检查初始预留／增长的真实 write、文件 fsync、目录 fsync 的 EIO 来源与封闭准入。普通文件系统夹具另覆盖 no-clobber、临时创建与替换失败；这些是进程内调度和 I/O 故障证据，不是实盘掉电、实际 Samba SDK 或客户端性能验收。普通未发布代成功覆盖／增长 Dirty 链路的静态同步调用预算为 2／4 次（不含打开、Flush、write-through），不是系统调用测量或吞吐结论。

## SDK 与模块构建

固定目标为 Samba **4.23.0**、VFS 接口 **51**。换版本需要重新生成绑定、编译并运行互操作测试；单独的协议测试不能证明 ABI 兼容。

使用官方源码树并完成该目标平台的 Samba configure，保留生成的 `bin/default/include/config.h`、NDR 头文件及所有依赖头。这里不维护手写的 Samba 结构布局。

```sh
export ZXS_SMB_SDK_SOURCE=/absolute/path/to/samba-4.23.0
export ZXS_SMB_SDK_BUILD=/absolute/path/to/samba-4.23.0/bin/default
# 按目标 Samba 配置补充 clang 参数；JSON 数组形式，避免 shell 转义歧义。
export ZXS_SMB_SDK_CLANG_ARGS_JSON='[]'
CARGO_INCREMENTAL=0 cargo build --locked --offline \
  --package smb-vfs --features smb-sdk --release --target-dir target/native
```

构建需要 libclang。`build.rs` 检查源码 `VERSION` 和 `source3/include/vfs.h` 的接口版本，然后从 `includes.h`、`smbd/smbd.h`、`smbd/globals.h` 生成实际 ABI。不同 configure 的额外 include/define 参数必须来自对应 SDK；缺失时构建失败，不能换成猜测的类型定义。

模块文件生成在 `target/native/release/libsmb_vfs.so`，按目标 SDK 的模块路径提供 `zxs.so`。SDK 测试还需要链接该配置对应的 tevent/talloc 库。

共享配置的基本形状如下。模块参数使用独立键，不能用逗号拼进 `vfs objects`：

```ini
[global]
    server role = standalone server
    security = user
    map to guest = Never
    restrict anonymous = 2

[zxs]
    valid users = zxs-owner
    guest ok = no
    guest only = no
    path = /srv/zxs-smb/empty-export
    vfs objects = zxs
    zxs:socket = /run/zxs/smb.sock
    zxs:repository = repo_<32位小写十六进制>
    read only = no
    oplocks = no
    level2 oplocks = no
    kernel oplocks = no
    durable handles = no
    aio read size = 1
    aio write size = 1
    use sendfile = no
    min receivefile size = 0
    store dos attributes = no
    ea support = no
    follow symlinks = no
```

`path` 只能是部署者预先准备的真实空目录，不包含符号链接；该目录由 root 所有，认证 Samba Unix 用户可读取和遍历但不可写，例如空目录 `0555`、root 所有的父目录 `0755`。它须独立于 ZXS 系统状态、管理、socket、spool、数据盘及工作区目录，不能放在私有的 `/var/lib/zxs` 或 `/run/zxs` 下。Samba 的 root 身份桥接握手成功后，后续共享请求仍会切换到认证 Unix 用户并进入这个宿主目录；VFS 正文通过 RPC 读写不免除目录遍历权限。不要通过放宽私有目录权限或设置 `force user` 绕过此要求。

上线前使用 `testparm` 检查配置，并验证生产用户认证、首次目录枚举和文件打开，以及 SMB 最低版本、签名、加密和可信 UID 链路；模块加载或握手成功不等于共享可用。

## 验证范围

```sh
cargo test --locked --offline -p smb-protocol
cargo test --locked --offline -p storage-server smb_rpc::tests
bash .github/scripts/smb-sdk.sh
```

协议与客户端测试检查真实 Unix socket SCM_RIGHTS、CLOEXEC、帧限制、业务错误、失败后的全克隆断开及拒绝遗留响应。服务端桥接测试检查完整保存、旧版本读取、未知发布结果、路径锁、交错发布、命名空间事务、暂存保留、取消、身份失效与共享重绑定。环境禁止 Unix socket 时，应在具备相应能力的环境运行，不能把未执行写成通过。

`test/server/unit/smb_rpc/close_tests.rs` 复用 VFS 描述符关闭策略，通过实际 Unix RPC 和开发核心检查上传拒绝、未决操作、工作区清理失败、丢回执、锁中毒及句柄终结。此链路不加载 Samba，不能替代固定 SDK ABI 或真实客户端验收。

上线前另行验证真实客户端 write-through/FLUSH、不同系统/编辑器的临时文件与多句柄保存、断网重连、故障注入、真实设备断电，以及唯一账户成功认证、匿名/guest/force-user拒绝和身份失效。活动写句柄覆盖的路径拒绝命名空间变更，关闭后改名成功不代表所有编辑器保存模式可用。文件系统时间戳未完整建模；剩余空间来自宿主文件系统，不等于保护路由可用容量。

## 未确认写入恢复

恢复时先查询 `*.work.json` 记录的 `operation_id`：若已发布，保留原发布结果；若未决或查询结果未知，先恢复/核对核心事务，不重新上传或提交原请求。`published` 仅描述最后核对的操作结果，不保证暂存 FD 当前所有字节都已发布；丢回执后它也可能尚未反映实际发布。`descriptor_changes_unconfirmed: true` 表示可写 FD 交出后，仍可能存在尚未收到 `Dirty` 通知的修改；即使 `published: true` 且 `dirty: false`，也必须保留并核对暂存文件，不能据此删除或自动提交后续字节。管理员可先导出完整暂存文件，再决定如何恢复或放弃对应工作区。当前没有自动重新挂接旧 SMB 句柄的能力。

`handle_state: "detached"`、`detach_error` 和 `retained_bytes_synced` 记录失败关闭/显式脱离的原因与当时正文同步是否成功；它们不是发布成功或允许删除的标记，也不保证其他仍持有的 FD 之后没有再改动。脱离时先尝试同步正文和原子刷新记录，再释放锁。若磁盘 I/O 使刷新失败，仍保留现有正文/旧记录并记录带 `operation_id` 和 source 的告警，不以保留活动锁或删除脏文件代替恢复。`active` 或缺少新字段的旧记录不能证明句柄仍在线，尤其在断连或进程退出之后。

断连时无法证明可写 FD 未发生修改，因此普通无写打开、以及成功 FLUSH 后的断连，也会保守保留暂存和恢复记录，不创建新历史版本；这会占用额外空间，应按恢复流程核对后处理。只有成功完成显式 CLOSE（或显式放弃）才清理该句柄的暂存文件与恢复记录；失败 CLOSE、失败放弃和 Detach 不清理。

## FFI 安全与持续验证

手写 VFS 代码启用 `unsafe_op_in_unsafe_fn` 与 `clippy::undocumented_unsafe_blocks` 的 deny 检查；仅由 bindgen 生成的 SDK 代码豁免安全注释 lint，不豁免手写模块。每个不安全边界说明有效指针、长度、所有权或宿主事件循环约束。空值检查不能证明非空裸指针有效，也不能替代匹配的 ABI。同步写入在宿主边界验证空值/长度后转为借用切片，工作线程使用自己的缓冲区；不再由安全函数接收可解引用的任意裸指针。

异步 `pread_send`、`pwrite_send` 与 `fsync_send` 将宿主准备闭包交给共同的请求边界：认证快照、FD 复制、输入复制/读缓冲申请、事件 watcher 建立及入队都在同一 unwind 隔离区。准备只在宿主事件线程执行；返回的 owned 值才交给工作线程。准备或入队失败时先注销并关闭 watcher，再以原 errno（panic 为 `EIO`）完成并 post 原请求；尚未创建的 Extra 保持空，取消/接收均由已注册的析构路径处理。已入队任务仍按原完成与取消协议运行，不能持有宿主指针。此边界不捕获 abort/OOM abort，也不证明宿主指针有效。

两线程、八槽工作池仅缓存成功初始化。初始化失败直接传播本次错误，关闭该次队列让已启动线程退出；同一请求不循环重试，后续新请求可重新尝试。初始化互斥只用于尚未成功的阶段，成功后的入队热路径不新增该锁。不会重放失败或结果未知的文件操作。`free_context` 的清理与诊断位于同一 FFI panic 边界，仍先关闭全部本地描述符再输出诊断，不借异常清理提交工作区。

CI 的独立 `smb-sdk` job 调用 `.github/scripts/smb-sdk.sh`，下载固定官方 Samba 4.23.0 tar（SHA-256 `b0cb963a63eb1515227f3779a46d3a7f790c1bbfeeb4b31fd43e405eefe7a3af`），编译真实 SDK、生成头文件与依赖库，再执行 VFS 全目标 Clippy、SDK 单元测试及 release 模块构建。缓存包含配置后的源码与生成头文件，键包含构建脚本摘要。运行前需要脚本对应的 Ubuntu 开发依赖；脚本不自动安装包、不启动或安装 Samba，也不改宿主服务配置。

```sh
bash .github/scripts/smb-sdk.sh
```

此 job 验证真实 ABI 的编译和事件/内存库单元测试，不等于运行 `smbd` 或真实客户端互操作测试；`smb-protocol` job 仍只验证本机 RPC 协议。源码 tar 校验固定摘要，不宣称验证了发行签名。

## 依赖与许可

协议和服务端桥接是本项目 Rust 代码。ABI 绑定依赖官方 Samba SDK；Samba 是 GPL-3.0-or-later 项目，本目录的 VFS 模块包使用相应许可标识。第三方依赖保留其原包名与许可，不修改其身份。

- 官方目标源码：<https://github.com/samba-team/samba/tree/samba-4.23.0>
- VFS 头：<https://github.com/samba-team/samba/blob/samba-4.23.0/source3/include/vfs.h>
- 固定目标头 SHA-256：`854fcd3d14cabc918de6aea755d1af8935ab8d2a4cf5a4d4c17a1e9f06744356`。
- tevent 接口：<https://github.com/samba-team/samba/blob/samba-4.23.0/lib/tevent/tevent.h>
- bindgen：<https://github.com/rust-lang/rust-bindgen>。

## 命名空间边界恢复记录

`*.namespace.json` 使用 `smb-protocol-namespace` 格式，包含身份、分支、操作种类、源/目标路径及 `commit_request`。`publication_outcome: "unconfirmed"` 仅表示已建立意图关联，不能证明准备变更或提交已经执行。记录可先于 Tree 检查和 Namespace staging；因此不能直接重放 `commit_request`。文件或父目录同步失败时虽保留已写证据，但不承诺掉电后的耐久性。先用原 `operation_id` 查询权威操作状态，未决或未知时走核心恢复流程，不重放该文件或另造操作号，也不把存在记录当作失败证明。确认成功后桥接会删除记录；若删除或目录同步失败，残留记录仍应按原操作查询，不重复提交。

相关回归位于 `test/server/unit/smb_rpc/namespace_tests.rs`，覆盖目标种类拒绝、旧模糊删除命令拒绝、实际发布前/后返回错误时保留原操作和工作区、不错误 Abort、成功后的记录清理、成功 RPC 帧丢弃后按客户端原操作号查询、准备/Abort 及文件/父目录同步故障，以及 write-through 新一代操作号。协议测试复用 VFS 实际调用的 flags/操作号构造函数，并用真实 Unix socketpair 验证传输错误保留原操作号和全克隆断开。socketpair 桥接测试不等于真实监听 socket 的进程/权限生命周期，也不等于固定 Samba SDK/客户端互操作；这些仍须独立验证。

## Listener 错误隔离与关闭所有权

监听端的启动、权限设置、reactor 注册及永久 accept 错误仍返回调用方；不能用
`continue` 隐藏坏 listener 或进程 fd 耗尽。只有 `Interrupted` 和
`ConnectionAborted` 可连续重试三次，每次间隔 25 ms，成功 accept 重置该预算；
第四次连续瞬时失败仍携带原 I/O 错误退出。资源耗尽、无效 fd、权限和其它未知
accept 错误立即退出，listener 本身不执行无限重建或重试。

已接受连接的 peer credential、标准流转换、阻塞模式设置及关闭用 fd 复制失败
只丢弃本连接，决策层保留 I/O 原因；错误 UID 和容量拒绝用 debug 诊断。
复制成功之后才插入关闭库存；worker 入队闭包同时拥有连接 guard 和资源 permit，
入队失败由闭包析构释放，成功则由 worker 结束释放。原有 managed worker 保留
JoinHandle、错误报告和关闭/join 所有权。会话计数锁中毒时 Drop 仅记录错误，
不修复业务计数、不清除 poison；后续业务准入仍按统一 fail-closed 策略处理。

`startup_tests` 包含无 socket 的有限 accept 分类、停止后入队资源释放测试；
实际 peer UID 拒绝需 AF_UNIX。`admission_tests` 的真实 listener 回归先断开坏
Hello，再拒绝错误用户名，最后成功认证第二类正常连接并在仍持客户端时关闭/join。
这些测试与 Samba SDK/真实客户端验收分开；受限环境不能用 socketpair 或提权绕过
AF_UNIX 禁令。测试源码存在与格式检查通过不代表这些运行时测试已执行。

SMB Hello 的唯一管理员映射从 `AuthReader` 的已提交快照读取，不取得认证写入
Mutex；它仍在可信 UID、协议、能力和配置 Samba 用户名准入之后执行，不增加匿名
访问。`Shared` 只持弱 reader，监听函数单独持有真正 `AuthStore` 的强 owner，并在
socket 资源之后释放它，保证独立调用者转移最后一个 owner 时 listener 仍可用。
reader 不延长凭据 OS 锁；owner 被销毁或权威发布被阻断后，后续读取拒绝。
Hello 得到的身份不是长期准入证明：repository/binding generation 与每次操作在
引擎锁内的当前权威验证保持不变。无 socket 快照测试覆盖认证写锁被持有时读取、
blocked/owner drop 和凭据锁释放；真实 listener 的 owner 生命周期留给 AF_UNIX CI。
