# macOS 远程容器工作磁盘（FSKit）

**当前没有可发行、经原机验收的 macOS 可写盘。** 普通窗口已接入[选择、挂载、正常卸载和明确提交](../zipworkspace/desktop/container_macos_controls.py)的源码，但发布许可表为空；现有 macOS 26 ad-hoc 包只提供原有浏览等功能。本目录是 macOS 27+ arm64 的独立 Apple FSKit 后端，不引入 macFUSE 可写后端，也不改变 ZIP 或只读快照卷。

## 从原节点到 Finder

1. 在普通容器面板选择分支头或历史 main 节点，确认本次下载和挂载。窗口先核本机发行版本、开发团队、实际有效权限和[已由用户启用的原扩展](https://developer.apple.com/documentation/fskit/building-a-passthrough-file-system)。不安装组件，不打开系统设置。宿主必须实际获准 `com.apple.developer.fskit.mount`，扩展须获 FS 模块和两边专用 App Group 权限；[两个真实 Xcode 目标与链接条件](target/README.md)不等于这些授权。
2. [登记器](../zipworkspace/macos_fskit_registration.py)在同一认证连接上核实际容器修订、原分支头和节点历史。原号先保存在本机；完整原节点由 HTTPS/2 下载到专用 0700 APFS 会话，用独立只读句柄复核长度和 SHA-256。下载前估算整份容量及 256 MiB 余量，不承诺预留空间。工具不扫描或接管用户文件、同步目录或来源不明的残件。宿主只把**空票据**作为只读 [FSPathURLResource](https://developer.apple.com/documentation/fskit/fspathurlresource) 交给扩展，不交目录、正文或认证信息。
3. 宿主只调用 [FSClient 按 bundle ID 挂载](https://developer.apple.com/documentation/fskit/fsclient/mountsinglevolume(resource:bundleid:options:completionhandler:))。原系统在 `/Volumes` 返回位置；先核原 FSKit 挂载表、新 FSID、卷 UUID、原根身份和实际 `noexec,nodev,nosuid,owners`，才向 Finder 提供名字和正文。不会按短名、外部命令、内核驱动或另一个挂载点补救。
4. [FSVolume.Handler](https://developer.apple.com/documentation/fskit/fsvolume/handler) 通过实际 APFS 名字目录处理大小写及 Unicode。在可区分时保留各自拼写；发生真实冲突时先保留全部可用原名，然后只为冲突项生成“名称-容器ID[-序号].扩展名”。按真实文件身份查询；不统一转小写、归一化、挤占已存在名称或改远端名。内部登记支持 1–128 项；普通窗口每次仅编辑一个选中容器。
5. [DataCacheHandler](https://developer.apple.com/documentation/fskit/fsvolume/datacachehandler) 针对实际用户与组按项请求 `.noCache`；[读写回调](https://developer.apple.com/documentation/fskit/fsvolume/readwritehandler)使用原 APFS 句柄，按当前 EOF 返回实际读取量。单次数据请求最多 16 MiB，每件至多 128 GiB。支持读写既有文件和截断；**不支持编辑器新建临时件再重命名替换**、增删、链接或修改权限/时间。FSKit 不为读写回调提供逐应用 FD，不能把按项最后关闭说成跨进程强制单写。普通保存、关闭、同步都不会上传。

## 正常卸载与提交

用户点「正常卸载」后，先停接新打开，等原引用与在途回调排空，同步全部原工作件，关闭全部可写句柄，逐件核最终身份、字节数及整文件摘要。短写、同步错误、过迟写入或不完整检查点一律不签发干净证明。宿主只对原 `O_EVTONLY` VROOT 调用正常 [funmount(fd,0)](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/vfs/vfs_syscalls.c)。若忙，保留挂载；关闭占用者后由用户再试，不强制、不定时再卸载。

系统确认原 FSID 和位置均已消失，与提供方在真实停用/卸载路径确认干净完成，**两者缺一不可**，还须一致绑定同一个最终生成记录。终态不是仅看文件名：使用者再次同步、复读并保存原文件身份。不因外部弹出、路径消失或系统关闭而推定成功。原主机已经正常卸载时，可在保留原号的情况下释放进程句柄，未完成的提供方记录不会被补造。

[完整改动、原号和回执](../docs/invariants/macos-remote-edit.md#fskit-正常收尾到原文件提交)在本机明确交接。未改不生成新版本；已改会先展示实际目标、基准修订、完整长度、摘要及操作号，再由用户决定是否提交。无后台同步或换号重试。

## 发行和原机验证

运行时核普通 Apple 代码签名、受信团队、未开启调试、宿主签名内的唯一构建版本、独立扩展的[实际代码标识](https://developer.apple.com/documentation/security/kseccodeinfounique)、确切系统构建及能力。发行方须为任何 Qt/Python/原生/配置变化分配**新的** [CFBundleVersion](https://developer.apple.com/documentation/bundleresources/information-property-list/cfbundleversion)。在未分发的冻结候选中预留验收记录号；单独保存最终签名包的完整库存、整包指纹和原机验收，再决定是否接纳并分发那一份候选，不签后改包。运行时核的是这个签名发行版本及独立扩展，不是在应用内自证整个包的指纹。没有第二套签名机构、外置许可文件、环境开关或替代驱动。

[跨 EOF 和编辑器替换断言](../tests/qualification/macos_fskit_held_eof.c)、[实际 APFS 名称](../tests/qualification/macos_fskit_names.swift)、[原登记](../tests/qualification/macos_fskit_projection.swift)、[宿主/终态错误](../tests/qualification/macos_fskit_host.swift)与[正常后移交](../tests/qualification/macos_fskit_handoff.py)目前只有源码。仍须在批准的真实 27+ 系统核签名、用户启用、空只读票据能否提供可写卷、实际访问控制与映射、忙卷、跨进程崩溃、原来已打开的读者见到截短至零和合法短读、终态改名前后故障，以及 UI 到真实服务器的完整显式原号回执。未编译、签名、运行、挂载或验收。
