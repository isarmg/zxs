# Windows / macOS 桌面 CI 验收

在 GitHub Actions 选择 **Workspace desktop qualification**，对 `main` 手动运行。只有该工作流或本验收脚本改动推送到 `main` 才自动运行。按实际 run 的提交、两个工作项和日志记录结果。

| GitHub 托管环境 | 必须执行 |
| --- | --- |
| **Windows Server 2025 x64** | 原生 Qt `windows` 窗口，固定本机 NTFS 中保留原件、准确相对路径、同名替换和默认数据流，及读取后保留原 `Zone.Identifier`。同时走无需 WinFsp SDK 的只读数据面用例。 |
| **macOS 26 arm64** | 原生 Qt `cocoa` 窗口，真实本机 APFS 原件、相对名和线程物化策略。用隔离合成目录生成并读回独立首代 `.zxs`，检查未标记原包的另存拒绝；另外用现有配方本地打包、重算 ZIP 摘要，解包后重核 ad-hoc 签名和 arm64 架构；直接启动解包的应用入口并核对其原生窗口。 |

两边用同一批核心、CLI 只读、目录/文本对照和窗口源码用例。另一个独立进程使用实际 Qt 桌面后端和真实窗口：取消首次外部打开，再确认打开合成 Store ZIP 快照，读空目录，在两个同名条目中定位精确路径，关闭并检查原句柄及归档字节。必跑的用例缺失、跳过、失败，或 Qt 不能暴露窗口都报作业失败；不退回 offscreen。

## 依赖和范围

- 使用 [GitHub 托管镜像标签](https://github.com/actions/runner-images#available-images)和 [Python 工具缓存的 CPython 3.14.10](https://github.com/actions/python-versions/blob/main/versions-manifest.json)，依照现有平台配方的 wheel SHA-256 下载；仅用 PyPI wheel，在作业独立目录离线安装这批文件。源码 Qt 流程、制包和解包后的启动分别记录。启动检查只读窗口的所属进程和大小，不录屏或更改设置；不走 Finder、文件关联或 Gatekeeper 安装验收。
- Windows 托管环境是 Server，而且 [GitHub 运行账号有管理员权限](https://docs.github.com/en/actions/reference/runners/github-hosted-runners#administrative-privileges)。Windows 11 普通账户、便携配方、系统文件关联和权限隔离另验；现有配方仍拒绝 Server。CI 不建立需要改权限的符号链接。
- macOS 只做本地 ad-hoc 签名。没有 Developer ID、公证、真实隔离来源输入或 Gatekeeper 验收。独立生成归档与托管工作树的签出、提交是不同入口；后者不在两个平台开放。
- 虚拟盘不在此作业：[Windows](../winfsp/README.md)有 WinFsp 数据回调和拒绝分派的封闭生命周期，未接成桌面盘符；[macOS](macos-snapshot-volume.md)有只读回调及无卷会话，未接公开 macFUSE/FSKit 挂载。CI 不装驱动、启动挂载、改系统安全设置、访问真实档案或共享，也不上传候选二进制。

这里列的是验收项目。实测结果以对应的 GitHub run 为准。
