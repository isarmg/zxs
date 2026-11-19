# Android：在手机上看 ZIP 目录

从「选择本机 .zxs 或 Store ZIP」进入系统文件选择器，在**主共享存储**中选择一份文件。点击目录进入下一级；可看空目录、返回上一级，每页最多 100 项。文件后面是 ZIP 中声明的字节数。取消、新选文件或关闭窗口不会保存目录清单。旋转后已显示的目录须重新选择。

**这是目录声明，不是内容核验。** 它检查整份中央目录，而不打开或解压成员，不校验内容、CRC、归档注释中的树摘要、版本或分支头。一个正文已损坏、头部与目录不符的 ZIP 仍可能列出相同的名称和大小，不能据此用于恢复。读取文件末尾定位目录时，所取小窗口可能覆盖最后一个成员的一部分。

### 第一版接受什么

- Android 15+ 的系统 SAF 单文件临时读取权。只接受系统签名、符合 AOSP ExternalStorageProvider 约定的主共享存储普通文件，后缀为 `.zxs` 或 `.zip`；**不要从“最近”中的云或下载提供方选，直接进入主存储**。仅打开原 URI 一次，不转为磁盘路径。不接受虚拟文件、部分下载、多选、树授权、管道、不可定位或读取期间已观测到变化的文件。
- 单卷、未加密、Store 方法的 ZIP 与标准固定大小 ZIP64 结束记录。只接受 ASCII 或明确标为 UTF-8 的可移植名称；不兼容的格式、特殊成员、歧义名字、重复或父子冲突会拒绝整份预览。手机上限：中央目录 2 MiB、8,192 条显式记录、12,288 个含隐式目录的节点、累计名字 512 KiB、48 层；单成员最多 128 GiB、声明正文合计最多 256 GiB。不降级成显示部分目录。
- 不请求网络或广域存储权限，不查询别的文档、不保留 URI 授权，无缓存文件、上传、解包或写入入口。系统选择器和提供方由系统管理；主存储可被模拟，不能把 URI 或可定位句柄当作物理闪存或全系统不联网的证明。

### 构建与验收状态

本目录是独立 Gradle 根。使用 JDK 17、Gradle 8.13、Android SDK 36 和清单中固定的 Android Gradle Plugin；产品源码仅用 Android/Java 平台 API。仪器回归源码在 `src/androidTest`，只在测试构建中引用 AndroidX Runner 与 JUnit。没有生成、安装或发布 APK，也没有运行编译或测试。

发行前须在干净环境构建、核查许可并签名；在实际 Android 15+ 设备验证主存储选档/授权撤销、取消与旋转、受限提供方、空目录/多页、超过 4 GiB 的 Store/ZIP64，以及文件变化和资源上限的拒绝。没有实机证据时不承诺支持某家文件管理器、云盘、可移除盘或下载目录提供方。

设计依据：[Android 系统文件选择](https://developer.android.com/training/data-storage/shared/documents-files)、[AOSP ExternalStorageProvider](https://android.googlesource.com/platform/frameworks/base/+/master/packages/ExternalStorageProvider/)、[PKWARE ZIP 规范](https://pkware.cachefly.net/webdocs/casestudies/APPNOTE.TXT)、[Android Gradle Plugin 8.13](https://developer.android.com/build/releases/agp-8-13-0-release-notes)。
