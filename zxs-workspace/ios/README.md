# iOS：预览一份归档的目录

打开应用，点「选择 .zxs 或 Store ZIP」，在系统文件选择器里选一份文件。点目录看下一级，文件下方显示 ZIP 声明的字节数。每页最多 100 项，可查看空目录、翻页和返回。取消或改选不继续显示过时的读取结果；要重新观察文件，需要再选一次。

**不是快照核验。** 画面是本次读取的一份目录记录，不是实时文件监视。它检查完整中央目录、层级、条目类型和必要的物理范围；不打开成员，不核正文、CRC、实际本地头或归档注释中的快照身份。正文损坏仍可能列出原来的名称与大小。末尾定位窗口可能读到最后成员的一部分，不能据此证明正文完整。

- **位置和授权：** 只通过系统单文件选择器原地打开，不创建副本。成功取得临时安全作用域后，在后台的文件协调访问器内只打开一次系统交付的 URL；要求普通可定位文件，检查实际句柄与名字在读取前后的身份和文件状态。完成即关闭句柄、释放作用域，不保留文档 URL、书签或目录清单文件。
- **网络与取消：** 应用没有网络客户端、上传、成员提取、缓存或归档写入功能。iCloud Drive 或其他文件提供方可能因系统选择或协调而自行下载、物化或缓存文件，不能保证选档完全离线。取消会撤回当前界面请求，通知协调器并在读取间检查；不保证系统已开始的下载或单次阻塞读取立即停止。
- **支持的声明：** 只读单卷、未加密、Store 的 ZIP 和固定标准结束记录的 ZIP64。名称须为 ASCII 或明确标为 UTF-8 的规范可移植名称；重名/别名、特殊文件、必然重叠的成员范围、未知格式或超限均拒绝整份目录，不显示部分结果。中央目录最多 2 MiB、8,192 条显式记录、12,288 个含隐式目录的节点、累计名称 512 KiB、48 层；单文件声明 128 GiB、合计 256 GiB。

## 工程与发行

独立打开 `ZipPreview.xcodeproj`，应用及 XCTest 源目标均包含在 `ZipPreview` 方案内。最低系统为 iOS / iPadOS 16；源码使用 UIKit、Foundation、Uniform Type Identifiers 和 Darwin，没有外部产品依赖。工程不预设开发团队、证书、云容器或发布账户。

**本次只编写/静审：** 没有用 Xcode 构建、运行测试、安装到设备或生成签名包。可发行前须在获准的 macOS/Xcode 环境独立构建并签名，在真实 iPhone/iPad 上验证本机和外部文件提供方、无授权/失联/等待下载、取消/换选/场景关闭、目录导航和超过 4 GiB 的 ZIP64；另核正常使用的辅助功能、图标、隐私披露与发行配置。

设计依据：[Apple 文档选择器](https://developer.apple.com/documentation/uikit/uidocumentpickerviewcontroller)、[安全作用域](https://developer.apple.com/documentation/foundation/url/startaccessingsecurityscopedresource())、[文件协调和取消](https://developer.apple.com/documentation/foundation/nsfilecoordinator/cancel())、[PKWARE ZIP 规范](https://pkware.cachefly.net/webdocs/casestudies/APPNOTE.TXT)。
