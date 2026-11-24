# iOS 本地归档候选

本目录含独立的 `ZipPreview.xcodeproj` 及共享方案 `ZipPreview`。这份配方从原生 Release 目标保留一个本地 `.xcarchive` 候选；不需要其他项目、不导出 IPA、不安装 App、不上传或注册设备。

- `Preview/Version.xcconfig` 是应用目标版本的单一来源：束 ID `org.zipworkspace.preview`、用户版本 **0.1.0**、构建号 **1**。以后签发新版时先照所用发行渠道的规则提升版本或构建号；正式束 ID 必须由发行者合法控制，不能用改 ID 冒充原 App 的升级。
- 应用资源含原创的默认 AppIcon（不透明的 1024×1024 PNG，未引入外部图形或字体）及 `PrivacyInfo.xcprivacy`。隐私清单仅说明真实的 `fstat/lstat` 用途：读取系统选择器明确授予访问的文件大小、时间戳与原件状态（[Apple 理由 3B52.1](https://developer.apple.com/documentation/bundleresources/app-privacy-configuration/nsprivacyaccessedapitypes/nsprivacyaccessedapitype)）。这些元数据不作为遥测收集或从 App 发送；系统文件提供方仍可能自行下载。此清单不替代正式发行时的隐私信息核对。
- 不注册外部“打开方式”、不接收 Files 或分享菜单直接传入的文件，不抢占系统 ZIP 处理；仍只从 App 内的系统选择器取得本次单档、原地只读的安全作用域。

## 生成待验的归档

供获准的所有者以后在干净的 macOS / 完整 Xcode 环境操作。准备带 Swift 6.1 或更新编译器的完整 Xcode，核对实际 Xcode、Swift 和 SDK 版本；此配方不选证书、团队或描述文件，不添加账号，也不要求连接 iPhone 或 iPad。若计划公开发行，还须按当时的 [Apple 上传 SDK 要求](https://developer.apple.com/app-store/submitting/) 选择工具链；这不等同于改掉 App 的 iOS 16.0 部署下限。

从 `ios/` 运行。远程功能固定使用 Xcode 共享工作区的 `Package.resolved`，其中每个版本和提交应与 `RemoteTransport/Package.resolved` 一致。不要在验收时更新锁或用未记录的依赖。输出父须已存在、由操作者独占且不被同步、搬动或改写；目录名包含版本但须是全新的。失败就停，不拿旧成果接续，不自动删除：

```sh
out="$HOME/zip-workspace-ios-0.1.0-build1-candidate-01"
(umask 077; mkdir "$out") || exit 1
/usr/bin/xcodebuild -project ZipPreview.xcodeproj -scheme ZipPreview \
  -disableAutomaticPackageResolution \
  -configuration Release -destination 'generic/platform=iOS' \
  -derivedDataPath "$out/DerivedData" \
  -archivePath "$out/ZipPreview-0.1.0-build1.xcarchive" \
  CODE_SIGNING_ALLOWED=NO archive
```

此命令没有 `-allowProvisioningUpdates`、`-allowProvisioningDeviceRegistration` 或导出参数。仅普通 App 目标参加 Release 归档，不归档 XCTest。请核实成功返回及真实产物，不把中途创建的目录当作归档成功。

## 托管模拟器检查

仓库的 [Workspace iOS simulator](../../.github/workflows/workspace-ios-simulator.yml) 仅在主线手动执行。它固定 Xcode 26.5（17F42）、iOS 26.5 iPhone 17 Pro 模拟器和 Swift 软件包提交，先在 macOS 运行传输帧与门控夹具，再运行 App 的 XCTest，并核对实际依赖提交和未变更的锁文件。运行器镜像的安装表见 [GitHub 的 macOS 26 ARM 镜像说明](https://github.com/actions/runner-images/blob/main/images/macos/macos-26-arm64-Readme.md)。环境不符就停，不更换成另外的 Xcode 或模拟器。

它检查的是编译和合成运行，不连接真实服务器、不汇入证书、不安装签名 App。实际提交验收另外核对 HTTP/2、服务器名称证书和无 ALPN 阻断、最大文本长度、首次保存、当前分支头追加、历史 main 分叉、断线原回执和明确重试，以及编辑、出入后台、取消和防晚到结果。真机还核对局域网权限、文件保护、发布签名及安装。

## 验实物后再签发

1. 由构建者记录本次源提交、Xcode 和 SDK 版本。归档应为 iOS 真机类型且恰含 `Products/Applications/ZipPreview.app`，不应是模拟器或泛用归档；记录 Release 符号。核 `ZipPreview.app/Info.plist` 的真实 ID、版本、构建号和最低系统；核 `PrivacyInfo.xcprivacy` 确实在 App 包根且可读、图标在原生资源中。可用 `plutil -lint` 检查两份 plist。Xcode 归档是目录，不当作已签文件作单一字节摘要。
2. 以上只是关闭签名的本地候选，不是 IPA、不承诺 Organizer 可直接导出，也不能安装到设备。另由获权的发行者用自己的团队/描述文件，在受控环境从同一来源制作合适渠道的已签归档并核准产物；不在项目里写入凭据。以 [Xcode 官方发行方式](https://developer.apple.com/documentation/xcode/distributing-your-app-for-beta-testing-and-releases) 为准。确认签名身份、权限、版本和实机升级；若有本地导出的已签文件，对**该件**另算 SHA-256。
3. 真机须验证 [原有阅读与取消边界](README.md#工程与发行)，另核 iPhone/iPad 主屏图标、方向及大字号、来自 Files 的正常系统选档但不直接外开。完成项目及实际依赖的分发权、正式隐私披露和签发资格核定后，才考虑对外发行。源配方不代表 App Store 审查、TestFlight 或可安装工件已经完成。

仅源码改动和静态审查；未运行 Xcode、构建 App、签名、连接设备或上传。
