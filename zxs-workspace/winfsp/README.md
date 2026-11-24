# Windows 固定快照：WinFsp 只读数据面

这里是厂商 [WinFsp 2.1 C SDK](https://github.com/winfsp/winfsp/blob/v2.1/inc/winfsp/winfsp.h) 的实际回调，不是挂载命令。当前桌面不会创建盘符，普通安装不编译或加载这段 C、不安装驱动、不创建服务、不更改权限。它只表示一个选定的不可写快照，不是可编辑工作树。

- 从一个明确选择的绝对 `.zxs` 路径开始。`zipworkspace.windows_snapshot.selected_snapshot` 逐级持有已核固定本地 NTFS 目录，打开原普通单链接文件的未命名数据流，只读且不分享写/删除。首次选择完整核验 Store/ZIP64 目录、每个成员内容与树标记；若明确打开此前列出的节点，也要比对该槽原件、标记和原 ZIP 字节摘要。结束后不能凭路径恢复。它不是给以前没有持柄的 Qt 页面补发权限。
- 查询、列目录和按偏移读都基于同一个索引。名称按原拼写精确区分大小写，不造短名或别名；不能无歧义呈现的 Win32 名称直接拒绝整个视图。续页严格从标记之后开始，允许不存在的标记；非根目录有 `.`、`..`。大小是成员的逻辑大小，归档里没有登记的 Windows 时间、ACL、其他数据流和实际磁盘占用不伪造为原属性。
- 每次数据回调串行使用同一独占二进制光标，不按归档路径重开。单请求至多 16 MiB；每个涉及的 1 MiB 块在同一缓冲中校验后才取其片段。越过 EOF 只返回实际尾段；源失效或核验失败则整笔不返回正文。已在先前请求交给调用者的字节不可撤回；未来系统缓存、映射不保证每次绕回这些回调。SHA 用于一致性，不是签名或附件安全许可。

## 与宿主的边界

`provider.c` 包含正式 SDK 头文件与 CPython C API，不仿制 WinFsp 二进制结构。目标是 x64、常规带 GIL 的 CPython 3.12+ 主解释器。宿主必须保持 `selected_snapshot()` 域存活，绑定一个强引用，停掉并等完所有回调后才解除绑定和关闭原件。运行时不要求另装第三方 Python 绑定；[winfspy 0.8.4 的发布文件](https://pypi.org/project/winfspy/0.8.4/#files)有源码，但预编 wheel 最高到 CPython 3.11。

这不是一个放开挂载的库：默认数据面和下述封闭生命周期仍拒绝安全描述符查询，不返回 NULL DACL。另有不创建卷的本机登录会话资格接口：由当下非模拟、非提升的普通中完整性用户令牌取得实际 [logon SID](https://learn.microsoft.com/windows/win32/api/winnt/ns-winnt-token_groups)，为每项提供 [Windows 自相对描述符](https://learn.microsoft.com/windows/win32/api/securitybaseapi/nf-securitybaseapi-makeselfrelativesd)。文件只读，目录可遍历；不授文件执行、写入、更改 ACL 或删除。WinFsp 的 Create guard 拒绝内核绕行、缺令牌、受限或非同一登录会话；对允许的请求仍由 [SDK 的原生访问检查](https://github.com/winfsp/winfsp/blob/v2.1/src/dll/security.c)判定所请求权限。它是访问层源码，不是来源证明、全系统缓存隔离或可挂载入口；已交给授权调用者的正文无法撤回。普通安装不创建卷、发布挂载点或移除盘符。

**下一道门是宿主实测，而非再加一个按钮。** 先在受授权的独立 Windows 环境，用匹配的官方 SDK/已由操作者另装的驱动，核验普通账号的源身份、并发回调、EOF/分页以及断开后不泄漏句柄。用于真实盘符之前，仍需在真驱动上测账号/登录会话隔离、安全描述符、缓存/映射、来源标记到虚拟成员及执行策略、发布与读回盘符身份、停止并排空回调及卸载回执。不能以当前合成回调或只设 ReadOnlyVolume 代替。打包时单独核实 WinFsp [使用及发布条款](https://github.com/winfsp/winfsp/blob/v2.1/License.txt)。不把本批写成驱动、二进制或 Windows 验收。

`tests/test_windows_snapshot.py` 是不挂载的数据面用例。可选 `tests/qualification/winfsp_callbacks.c` 对厂商结构直接发起读及访问回调，不创建卷。它不是默认扩展，须在受信的普通 Windows 会话以同版本 SDK、当前 CPython 与系统 Advapi32 独立构建；仅对操作者明确指定的合成归档启用。原生夹具检查限定只读描述符、真实 AccessCheck、受限请求拒绝和内核模式门；没有运行这些用例。

Windows Server x64 托管作业使用固定的 [WinFsp 2.1 头文件](https://github.com/winfsp/winfsp/tree/ddca7bd5481857a65ba552f643b8776fd070836f/inc/winfsp)，逐件核 SHA-256 后，以该作业的 CPython 头文件和 MSVC [`/c`](https://learn.microsoft.com/en-us/cpp/build/reference/c-compile-without-linking) 独立编译数据回调、封闭生命周期和 C 夹具。作业只生成三个 x64 COFF 对象，不链接 DLL、不安装或加载驱动。接下来的实际回调、普通账号隔离与挂载验证仍需独立 Windows 11 x64 环境。

## 单实例生命周期（只供单独授权的合成资格）

`lifecycle.c` 使用真实 SDK 创建一份磁盘型卷，在启动 dispatcher **以前**把所有经它分派的请求一律拒绝，包括读目录和正文；没有盘符、目录挂载点、网络卷或“启用读取”开关。没有盘符并不代表卷在 Windows 不可见。[驱动可能在创建时即做内核挂载](https://github.com/winfsp/winfsp/blob/v2.1/src/sys/volume.c#L109-L121)。该源码不在默认构建、CLI 或 Qt 中。

将来若单独获准做资格验证：只限自行准备的普通用户测试环境、经核实的配套 x64 WinFsp 2.1 SxS SDK/DLL、已安装的厂商驱动和已知合成包。必须另行同意“创建一个系统原生卷，且 SDK 可能尝试启动该已装驱动服务”。[厂商 Create 和 Preflight 都可能尝试启动服务](https://github.com/winfsp/winfsp/blob/v2.1/src/dll/fsctl.c#L70-L96)；本代码不把 Preflight 当查询，也不接受可能另找其他安装的非 SxS 模式，不安装、提权、改服务配置或在结束时停止系统服务。

- 选择时及交付 SDK 时核对同一正常、中完整性、未模拟用户与同一在手归档。每笔只能接受一次；取消先于受理则未调用 Create，受理后只能请求收尾，可能已经有服务/卷方面的影响。关闭选择不会抢关正在用的归档句柄。
- 回执通过原控制 HANDLE 向驱动索取设备 GUID 名，创建后、dispatcher 启动后和释放前逐次核对，不按名称重开/清理，不以相同卷标冒充原件。最多观察 30 秒，不保证系统创建、I/O 或回调排空能在这段时间完成。等待/停止在独立拥有线程上进行，释放 GIL，先等完调度回调，再解绑定、删除本方原实例并关闭归档。
- 回执区分“创建已返回”“已观察本实例”“SDK 报告主动/异常停止”“本方已释放”。对主动收尾，还要原卷复核成功、最后回调对应本次停止且原调度结果为成功或[厂商对应的正常队列取消](https://github.com/winfsp/winfsp/blob/v2.1/src/dll/ntstatus.i)；原设备/I/O 等错误即使与主动停止重合也只记为未确认，保留具体状态码。它不证明有可打开的盘，也不证明系统其他引用已消失或服务停止。前台可用盘符仍需真正的访问者授权、附件来源、驱动及卸载资格。

`tests/test_winfsp_sealed.py` 的原生用例在普通环境跳过，要求已知合成包和独立的服务启动同意；资格扩展由 `lifecycle.c` 与 `provider.c` 对应的厂商 SDK/CPython 单独构建。未编译或运行。

另有独立的[远端节点本机编辑正文源码](../docs/invariants/windows-remote-edit.md)，在已核私用 NTFS 保存一个原节点，并提供按偏移读写、停接新上下文和完整正文审阅。它不接入此只读提供方或封闭生命周期，不创建 WinFsp 卷，不向服务器提交。

## 远程工作文件的 Write/Close 数据接口（不创建卷）

`draft_io.c` 是独立的 WinFsp 2.1 x64 C 回调表，不借用 ZIP 快照的读句柄或切换快照的只读位。目前只可在同进程、专用的原生资格夹具中调用；本表给真实 SDK 分派安装拒绝守卫，没有发布盘符、目录、真实根目录或 ADS。选择明确的本机别名后，打开的是已核私有 NTFS 工作文件，不改服务器名。每次 FILE_OPEN 有独立不可复用的上下文号（同时最多 1,024 个），分别判定读、写、仅追加和查属性；不授予执行、更改权限、创建、整体覆盖、改名或删除。

已实现原生 Open、GetFileInfo、Read、Write、SetFileSize（EOF）、单文件 Flush、Cleanup、Close；一次数据请求至多 1 MiB，普通写、在锁内取得 EOF 的追加、受限写的实际短计数与文件信息一起返回。分配长度请求不当作截断，卷刷新也不当作单文件刷新。[厂商 2.1 的 Write/Flush 契约](https://github.com/winfsp/winfsp/blob/v2.1/inc/winfsp/winfsp.h#L439-L487)。停止接入后既有上下文仍接受末次写；Cleanup 本身不释放，只有最终 Close 注销。即使数到零，释放此资格表仍仅属放弃，无真实内核排空证明，不得由此生成远端提交。

`tests/qualification/winfsp_draft_callbacks.c` 与 `windows_winfsp_draft.py` 提供独立、未编译/未运行的受信任合成夹具；不需要或调用创建卷、安装驱动、改系统配置。不可替代真实登录会话访问检查、虚拟文件夹及 Zone.Identifier 来源投影（否则复制会丢标记）、编辑器保存替换、缓存/映射、正常卸载身份和完整发布验收。

## 单文件可写命名空间接口（没有启动和挂载入口）

另一张专用回调表声明一个区分大小写的根目录、一项操作者指定的 Windows 别名，以及原下载策略要求时的只读 `Zone.Identifier` 数据流。ZIP 只读掩码与原归档规则保持不变。新表使用真实 SDK 的目录及数据流记录；输出缓冲不足时不会谎报完整列表。为完整来源流语法预留 22 个 UTF-16 单元（含流类型），本接口本机别名上限 233 个单元；别名不替代 R3 容器名；未支持目录内创建、编辑器临时文件替换、其他 ADS 或可执行映像。根目录属性不暴露私有工作目录的邻居、名称或修改时间。

分派守卫只接受同一普通中完整性登录会话的用户模式 FILE_OPEN，不接收备份意图、打开目标目录、重解析点或按 ID 打开。真实 SDK 继续核对同一登录 SID 的自相对安全描述符，之后再逐个上下文约束数据、目录和来源流的实际操作。保留执行拒绝不是 SmartScreen 或防病毒检查。[固定版 FSD 查命名流前](https://github.com/winfsp/winfsp/blob/ddca7bd5481857a65ba552f643b8776fd070836f/src/sys/file.c#L2746-L2761)还会发出一个带强制访问检查的只读主文件属性 (FILE_OPEN + OPEN_REPARSE_POINT) 请求；只给同一普通登录的实际现有正文通过这条无数据权路径，不放宽真正的重解析、创建、删除或根目录访问。每次投影都重新验证现有 Windows 来源政策及原 NTFS 文件上的实际 ADS；不合时拒绝，不提供去掉标记的回退。完整读取来源流及 SDK 枚举还不等于已验证 Windows 资源管理器、复制工具或附件管理器会传播来源；[固定版本示例的数据流接口](https://github.com/winfsp/winfsp/blob/v2.1/tst/memfs/memfs.cpp#L1973-L2031)。

本表不装驱动、不启动原生卷、不创建盘符或目录挂载点。原不分派的文件回调资格仍保留，新的普通登录资格用例也只直接调用 SDK 回调、没有运行。真正的私用挂载、排空、卸载与手动提交仍须另行集成和验证。

## 原生远程工作卷：新建私用目录、正常排空与卸载

独立非默认宿主 draft_host.c 与上述命名空间配套，使用 WinFsp 2.1 x64 SxS，且只由独占工作线程运行。必须分别同意创建原生系统卷（SDK 可能启动已安装的驱动服务）和在原先选定、仍持有的私有 NTFS 父目录中新建临时挂载目录。它不是盘符，不覆盖已有目录，不安装驱动、不修改已有文件权限、不使用网络/命令行挂载或猜测盘符。即使未获新目录以前，设备可能已经存在。实现和普通应用发行包/Qt 按钮仍然分离。

- 先占有原稿，再确认 DLL 并创建磁盘型驱动实例；分派器已运行但拒绝访问，直到查明控制句柄返回的卷设备 GUID、SDK 所创目录的 FileID、经原父句柄不跟随重解析点读到的同一对象、私用 DACL 及重解析目标。交给 SDK 的绝对路径是已逐层固定的卷 GUID 路径，绝不传空路径、星号或盘符；[厂商该路径的无替换创建及仅关闭自有目录句柄的语义](https://github.com/winfsp/winfsp/blob/v2.1/src/dll/mount.c#L439-L536)。磁盘容量来自原 NTFS 的真实汇总容量，不是远端节点长度、R3 配额或保留空间，单文件仍受创建时的字节上限限制。
- 用户请求正常关闭时停止新打开，仍让既有文件写入及 Close 完成；仍有句柄就等待，不按超时强制卸载。只有原稿上下文和在途原生分派均为零，才封住分派并再次检查实际目录与原卷。随后释放 SDK 所有的原链接，独立检查名称实际消失、同步原父目录、等待分派线程结束并复核同一设备，再删除原 SDK 实例。异常停止、可疑晚到的请求、核对失败或明确的强制放弃均不得提交。通过后，短时一次性凭证仅交还原稿给“审阅完整正文”；不因关闭或卸载而上载。原私有编辑文件不按名称删除。
- tests/qualification/windows_winfsp_live_draft.py 是另行明确授权才可运行的真实驱动资格源码，使用模拟 R3 字节，并通过 Windows API 打开虚拟文件、读取来源 ADS、等待真实编辑句柄关闭再审阅；另设强制放弃案例。没有编译、加载驱动、挂载或运行此用例。普通读写与 Office 式安全替换、Windows 复制及来源传播、系统映射缓存、应用发行、真实并发故障等仍不可写成已验收。若外部程序长期持有内核引用，正常关闭可能继续等待；强制放弃不承诺拿到尚在外部缓存的编辑。

### 普通 Win32 的原位编辑

R3 单文件目录挂载的数据文件允许普通 `GENERIC_READ | GENERIC_WRITE` 打开现存原件（`OPEN_EXISTING` 和只命中该原件的 `OPEN_ALWAYS`），按偏移读取/写入、SetEndOfFile 改 EOF 和 FlushFileBuffers。泛型写包含 EA/属性授权位，**不等于**支持设置 EA、任意元数据、写穿、执行、删除或改 ACL。根仍只能列出/遍历，普通 Win32 目录句柄的 BACKUP_SEMANTICS 仅允许在此根上且发起者不具有备份/还原特权；同一标志不能用于文件。来源流始终只读。缺失名字的 `OPEN_ALWAYS` 不能创建。用文件指针和 SetEndOfFile 原位截短不会重建数据文件或丢标记。

刻意不把 `CREATE_ALWAYS`/`FILE_OVERWRITE` 当作普通截短：[WinFsp 2.1 内核完成覆盖时会另外处理原命名流](https://github.com/winfsp/winfsp/blob/v2.1/src/sys/create.c#L1137-L1155)。默认不支持靠新建/重命名/覆盖的编辑器。下节为另外明确选择的一次经典替换源码。既有 ZIP 只读快照、未知名称和可执行访问不因此开放。正式 Windows 验收要从真实 Win32 API 验证泛型打开、偏移写/截短后的来源流、目录浏览、晚到写和正常回执；这里只写了源码和独立资格用例。


### 可选的同目录经典替换（单个临时文件）

单独选择时，额外只准一个未存在的普通本机临时文件。真实普通登录的 `FILE_CREATE` 经 SDK 查原根允许添加，随后严格检验 ASCII 不冲突的实际名字和权限；不接受任意另存、目录、重解析点、外来安全描述符、覆盖式创建或备用命名流。每个实际私用 NTFS 正文（包括新临时文件）保持独立真实文件身份和独占句柄，每项分配独立且不复用的虚拟 FileIndex。临时文件在写正文前先同步并核原来源流。根枚举最多两项，续页不谎报 EOF。

只处理 [WinFsp 2.1 经典 FileRenameInformation](https://github.com/winfsp/winfsp/blob/v2.1/src/sys/fileinfo.c#L1609-L1798) 的同目录正向覆盖：目的只可能是所选原别名。通常在关写后用只有改名权而无正文写权的句柄。另一条只在普通同步、不共享的精确 `CREATE_NEW` 上接纳读写及 DELETE 的组合，要求保持原创建上下文，不能从后来的打开借用其改名权或刷新证据。验证这次原请求的登录令牌和确切上下文；只在 SDK 当次目标核验中提供对原名的 DELETE 检查，不给普通原文件打开授予 DELETE。显式拦截 FileDispositionInformation／Ex、RenameEx 和 DeleteOnClose；其他改名、硬链接、ACL、EA、执行和删除照旧不支持。原文件或其他正文／来源流上下文未最终 Close、临时写没有实际 Flush 或不再可证时拒绝；任何情况下都不删除原实物。只在双原件核实及同步后一次改变所选别名指向。物理目录不进行破坏式改名。独占原创建句柄在改名后的延迟写与 EOF 变更仍落到同一实物，并使原刷新证据失效；关闭前若未从该句柄再次刷新，后来任何句柄都不能恢复审阅资格。

用户停止时（先关闭编辑器和相关资源管理器引用），宿主独立从已验证原卷重开所选名，核对真实 FileInternalInformation 与内核路径，确认虚拟临时名不存在；再走真实 Close/原卷正常撤下证明。没有重开证据或临时内容未成功替换时不能审阅、准备或上载。保留两份文件及其位置。源例 `windows_winfsp_live_draft.py --consent-live-winfsp-scratch-replace-and-retain-both-private-files` 要求独立可丢弃原生环境及专用构建；以真实 Win32 `SetFileInformationByHandle(FileRenameInfo)` 和同一原号的本机封存验证窄路径，拒绝未刷/忙句柄、其他名字/权限；不向服务器发送数据。未运行。正式验证仍需覆盖编辑器具体版本、失答/停止、来源流复制与真实签名包；另有单独同意的 Qt 6.11.2 实库源例（禁原位回退），覆盖成功及取消保留两件。均不等于已验收 QSaveFile、ReplaceFileW、MoveFileExW、WPS 或 Office；实际 API 差别见[桌面正文说明](../docs/invariants/windows-remote-edit.md#可选一次同目录临时文件替换)。
