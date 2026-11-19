# macOS 固定快照虚拟盘：只读数据面

这里交付的是能接入 macFUSE 的数据回调，**不是已经能在 Finder 挂载的卷**。普通安装不加载 FUSE 动态库，不安装驱动、启用扩展、更改权限或主动联网。

## 从原文件到一次读取

`zipworkspace.macos_snapshot.selected_callbacks(selection)` 只接有在手只读文件描述符的 `Selection`。它从展示的那个打开对象取得独立句柄，不从后来同名的路径选档。取不到原件就拒绝。首次建立块索引需要在同一原件上重新完整核验 ZIP 结构、全部成员及原树标记；点开先前目录里的一条精确记录时，另比实物代次及该记录的原 ZIP SHA。使用私有位置和 `pread`，不移动展示句柄的光标。既不查询 HEAD，也不解包。

- 只接受内核在原 FD 上能确认本地卷且内容已驻留的普通文件。准入和会读取原件的回调都在当前线程按 [Apple TN3150](https://developer.apple.com/documentation/technotes/tn3150-getting-ready-for-data-less-files) 禁止自动取回占位内容，然后恢复原策略。这不约束之前的系统文件选择器或普通浏览流程，也不是对物理硬盘或绝对离线的保证。
- 名称按存档的 Unicode NFC 及大小写查找；依照[厂商 Finder 名称约定](https://github.com/macfuse/macfuse/wiki/File-Names-%28Unicode-Normalization-Forms%29)，目录列表返回 NFD，同一规范组合的查询找回同一 inode。大小写不同不命中；非法 UTF-8 不接受，不凭空补 AppleDouble 文件或资源叉。目录包含 `.`、`..` 和正续页号，接满从最后接受的一项继续。属性中的模式、属主和链接数是只读视图的合成值，不是源文件 ACL 或磁盘分配记录。
- 普通文件没有执行权限；写、截断、改名和更改属性均拒绝。当前数据回调串行，在 16 MiB 单请求限额内复核涉及的 1 MiB 块，用同一块缓冲核对 SHA 后才取片。到 EOF 按本层约定返回实际尾段或零；原件变化、读不满或核验失败，则不交付这一次请求的半段内容。早先已交付的数据不可撤回；摘要也不是发布者签名、隔离来源或 Gatekeeper 许可。
- 后续宿主必须在回调排空后结束原归档作用域。没有按路径自动卸载、启停服务或提权的入口。Linux/Windows 适配不因此获得新资格。

## 接口与放行条件

Apple 原生 [FSKit](https://developer.apple.com/documentation/fskit) 使用带 entitlement 的文件系统应用扩展；[FSPathURLResource](https://developer.apple.com/documentation/fskit/fspathurlresource) 可传递授权 URL，但不是已核文件的原 FD。当前 Qt 包未配这样的扩展。本批面向 [macFUSE 5.5.0](https://github.com/macfuse/macfuse/releases/tag/macfuse-5.5.0) 和 [mfusepy 3.1.1 的回调](https://github.com/mxmlnkn/mfusepy/blob/v3.1.1/mfusepy.py)：该绑定在 Darwin 走 **libfuse2**，不是 Linux 的 pyfuse3 或本产品原生 FSKit 扩展。文件回调按 `raw_fi=True` 使用 `fuse_file_info`；目录回调按该绑定接整数句柄。生产模块不导入 mfusepy。

`tests/test_macos_snapshot.py` 内单独选择的 ABI 夹具才加载绑定，用匹配的 C 函数签名传入读缓冲和目录 filler，**不构造 FUSE 会话、不创建卷**。环境须已由操作者独立验证官方 libfuse2、正确版本的 Python 包和解释器；路径和版本字符串不是签名证明。当前未执行夹具、构建或安装软件。

将来开放挂载入口前，需在获准的普通用户环境固定并验证 `backend=fskit`，禁止[自动退到 VFS 内核后端](https://github.com/macfuse/macfuse/wiki/FUSE-Backends)。厂商 5.5.0 在 macOS 26.4+ 增加 FSKit 只读挂载；[macOS 27 才转发调用者有效 UID/GID](https://github.com/macfuse/macfuse/releases/tag/macfuse-5.4.0)，这不是访问资格已证。还需在真实卷核对实际调用者的访问范围、隔离/不可执行、Finder 拼写与分页、空文件及末尾读（5.5.0 后端可能将空读回执改为 EIO）、缓存/映射、取消和正常卸载。绑定的 `FUSE()` 会触发厂商挂载与收尾；进程退出不能证明原卷已经卸载。缺这些证据时不开放桌面或 CLI 挂载入口。
