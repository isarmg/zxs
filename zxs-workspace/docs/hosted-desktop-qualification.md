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
- 本作业不验证虚拟盘。[Windows WinFsp](windows-remote-draft.md)和 [macOS FSKit](macos-remote-edit.md)的远程编辑已接入桌面源码，默认发行仍不启用；[手动 FSKit SDK 检查](../macos-fskit/target/QUALIFICATION.md)只检查源码与 SDK 接口。托管作业不安装驱动、挂载、修改系统安全设置、访问真实档案或共享，也不上传候选二进制。

这里列的是验收项目。实测结果以对应的 GitHub run 为准。

## Windows 远程原节点另存（独立资格）

Windows 托管作业先查询当前进程令牌、原暂存目录的固定本机 NTFS 与现有权限。标准托管 Windows VM 以管理员运行并禁用 UAC，预计打印 **NOT RUN**。不新建账号、不降低应用的普通用户门槛、不调整系统策略或目录 ACL。此状态不算写入验收，现有只读和窗口用例仍单独执行。若实际进程、目录和来源条件均符合，则七项原生用例都必须执行，不容许跳过后报通过。

可在 **Windows 11 非提权普通用户** 下，从 `zxs-workspace` 运行：

```powershell
python -I -B -X utf8 tests/qualification/windows_node_qualification.py 'C:\路径\现有私用测试目录'
```

明确指定本人所有、已经存在且符合原私用权限形状的固定本机 NTFS 测试目录；勿用共享盘、同步目录、EFS 或存有真实资料的目录。脚本不改变 ACL 或下载来源策略。若现有 Windows 策略不标记保留域 `workspace-node-qualification.invalid`，此次来源流验收不运行，手动入口返回非零。脚本保留专用合成案例目录供检查，不递归清理。

仅以合成 H2 响应提供正文，不登录或连接实际服务器。原生文件、文件身份、两父同步、ACL 与 Windows URL 安全区域查询均实际执行。核对精确正文与零字节、同一 FileId 和来源 ADS、晚到的大小写同名占用、非 H2/短读/错误摘要、未确认与并行关闭。不是网络 TLS、杀毒、实际桌面另存或断电耐久验收。


### macOS 原节点下载：真实来源与合成传输用例

`test_macos_node_export` 继续要求系统实际返回并核对原下载 URL。若系统仅返回隔离类型，真实下载发布资格仍失败。macOS 26 托管运行 [45293896301](https://github.com/isarmg/zxs/actions/runs/45293896301/job/138109294064) 在 CFURL 和 CFString 两种观察中都未读回 URL。

另有名称含 `synthetic_provenance` 的两个用例只模拟来源设置与读取。它们保留原 APFS 文件检查：正文摘要不符时不进入发布；最终名字刚被占用时，实际无覆盖改名保留已有文件和原暂存。单独记录这两个结果，不将其计为真实系统来源或下载发布通过，真实用例仍是必测项。
