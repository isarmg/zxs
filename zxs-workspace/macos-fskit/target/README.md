# 可选签名版本：实际 Xcode 目标（未构建）

`ZxsRemote.xcodeproj` 包含独立的 `ZxsRemoteFS` **ExtensionKit extension** 与 `ZxsRemoteHost` **静态库**。目标为 macOS 27+ arm64、Swift 6。扩展有真正的 `@main UnaryFileSystemExtension`，在系统启动的进程内从只读空票据加载；不提供独立可执行入口、安装／授权脚本或后台服务。标准 macOS 26 ad-hoc Qt 配方不引用本项目。

正式程序集成时，须把静态库在最终 **Qt 主可执行文件**链接进去，链接 Foundation、FSKit、CryptoKit、Security 和 Swift 运行库；强制保留并从主映像导出本目录 `../host/DesktopHostABI.h` 的所有 `_zxs_fskit_*` C 符号（Mach-O 符号带一个前导下划线）。Python 使用 Darwin `RTLD_FIRST` 只找主映像。正常未改的 PyInstaller bootloader 不是这样的宿主；不尝试动态搜索或加载一个所谓“驱动”库。宿主 bundle ID 固定为 `org.zipworkspace.desktop`，其签名 Info 需写入真实 `ZxsDistributionTeam`。宿主应以 Xcode 的 Embed ExtensionKit Extensions 或等价正式签名嵌入流程把 `ZxsRemoteFS.appex` 放在 **Contents/Extensions/**，不是 PlugIns 也不是可指定的路径。

`Extension.entitlements` 与 `Host-Required.entitlements` 是申请和核验发布配置的输入，**不授予**权限。两边需要同一真实团队与专用已配置 `group.org.zipworkspace.desktop.remotefs`；单独取得宿主的 `com.apple.developer.fskit.mount`，不要用扩展的 fsmodule 权限代替。真实签名、配置、用户在系统设置启用和访问 OS 管理组的效果必须独立证明。不从终端代办安装、接受提示、改权限、拼主目录或向任意路径请求 scope。本仓库没有签名凭据或团队 ID。

元数据对照 [Apple FSKit 原始示例](https://developer.apple.com/documentation/fskit/building-a-passthrough-file-system)：单一 `com.apple.fskit.fsmodule` 扩展点、短名 `zxswork`、仅支持 path URL。本应用必须拒绝任意 URL、块设备、格式化、强制加载；普通 App Group 访问不同于示例的用户选择目录，故不要求传一个 security-scoped 目录。[Apple 共享组文档](https://developer.apple.com/documentation/xcode/accessing-app-group-containers)说明 `group.` 标识各自需要受权配置；一个路径字符串、签名文件或 0700 不能证明系统真的授予两边访问。发布前在真正 27+ 系统检查最终生成/嵌入的两份签名产品、有效 entitlement、组实际读写、ExtensionKit 发现、权限提示和正常卸载再决定是否开放功能。
