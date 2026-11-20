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

## 单实例生命周期（只供单独授权的合成资格）

`lifecycle.c` 使用真实 SDK 创建一份磁盘型卷，在启动 dispatcher **以前**把所有经它分派的请求一律拒绝，包括读目录和正文；没有盘符、目录挂载点、网络卷或“启用读取”开关。没有盘符并不代表卷在 Windows 不可见。[驱动可能在创建时即做内核挂载](https://github.com/winfsp/winfsp/blob/v2.1/src/sys/volume.c#L109-L121)。该源码不在默认构建、CLI 或 Qt 中。

将来若单独获准做资格验证：只限自行准备的普通用户测试环境、经核实的配套 x64 WinFsp 2.1 SxS SDK/DLL、已安装的厂商驱动和已知合成包。必须另行同意“创建一个系统原生卷，且 SDK 可能尝试启动该已装驱动服务”。[厂商 Create 和 Preflight 都可能尝试启动服务](https://github.com/winfsp/winfsp/blob/v2.1/src/dll/fsctl.c#L70-L96)；本代码不把 Preflight 当查询，也不接受可能另找其他安装的非 SxS 模式，不安装、提权、改服务配置或在结束时停止系统服务。

- 选择时及交付 SDK 时核对同一正常、中完整性、未模拟用户与同一在手归档。每笔只能接受一次；取消先于受理则未调用 Create，受理后只能请求收尾，可能已经有服务/卷方面的影响。关闭选择不会抢关正在用的归档句柄。
- 回执通过原控制 HANDLE 向驱动索取设备 GUID 名，创建后、dispatcher 启动后和释放前逐次核对，不按名称重开/清理，不以相同卷标冒充原件。最多观察 30 秒，不保证系统创建、I/O 或回调排空能在这段时间完成。等待/停止在独立拥有线程上进行，释放 GIL，先等完调度回调，再解绑定、删除本方原实例并关闭归档。
- 回执区分“创建已返回”“已观察本实例”“SDK 报告主动/异常停止”“本方已释放”。对主动收尾，还要原卷复核成功、最后回调对应本次停止且原调度结果为成功或[厂商对应的正常队列取消](https://github.com/winfsp/winfsp/blob/v2.1/src/dll/ntstatus.i)；原设备/I/O 等错误即使与主动停止重合也只记为未确认，保留具体状态码。它不证明有可打开的盘，也不证明系统其他引用已消失或服务停止。前台可用盘符仍需真正的访问者授权、附件来源、驱动及卸载资格。

`tests/test_winfsp_sealed.py` 的原生用例在普通环境跳过，要求已知合成包和独立的服务启动同意；资格扩展由 `lifecycle.c` 与 `provider.c` 对应的厂商 SDK/CPython 单独构建。未编译或运行。
