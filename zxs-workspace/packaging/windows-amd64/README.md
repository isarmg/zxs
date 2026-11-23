# Windows 11 x64 本机便携候选

这个配方把现有 ZIP Workspace Qt 窗口做成 **本机单目录候选**，不依赖其他项目。成品入口是 `ZipWorkspace.exe`，运行时应携带同目录的 `_internal`，不是只有一个 EXE，也不是安装器。不添加 PATH、文件关联、服务、开机启动、管理员权限或驱动。

默认不带原生磁盘组件的候选只有窗口入口：已核快照浏览、同库历史/版本对照、适用本机 NTFS 目录的只读比较和普通 Store ZIP 预览。窗口原有平台准入不变。**不创建可编辑副本、不签出或提交、不启动或打包 WinFsp**；内部附件服务、虚拟盘和单文件写入资格源码不属于这个包。原 Windows 写入合同仍待对正式路径来源、权限、原件和实际恢复验收。

## 准备经核对的离线环境

使用干净的 Windows 11 x64 及官方常规（有 GIL）[CPython 3.14.10 Windows installer (64-bit)](https://www.python.org/downloads/release/python-31410/)。配方用 [IsWow64Process2](https://learn.microsoft.com/en-us/windows/win32/api/wow64apiset/nf-wow64apiset-iswow64process2) 同时核验原生 x64 主机及进程，不接收 Server、ARM64 上的仿真、32 位或自由线程解释器。官方安装器 SHA-256：`92480239264744fe0386a1bd8275786fefa126e3129e56a68e22b52c73329965`。由构建者单独核官方公示散列和发布证明，先准备可信工具。后续安全更新应升级并再核配方，不为匹配旧配置回退安全补丁。

`requirements.txt` 固定了 9 个官方 PyPI wheel 的确切版本和 SHA-256，行尾注明文件名。包括 [PyInstaller 6.22.3](https://pypi.org/project/pyinstaller/6.22.3/#files)、[hooks-contrib 2026.7](https://pypi.org/project/pyinstaller-hooks-contrib/2026.7/#files)、[PySide6-Essentials 6.11.2](https://pypi.org/project/PySide6-Essentials/6.11.2/#files) / [shiboken6 6.11.2](https://pypi.org/project/shiboken6/6.11.2/#files) 和实际 Python 依赖闭包。把这些文件先放进本地受信 wheel 目录，不用 Sdist、不用裸系统 Qt 或额外插件。

以下在从未装其他模块的独立虚拟环境运行。例中替换成实际的完整本机路径；虚拟环境和工件都应使用全新目录。PowerShell 只在这次会话忽略 pip 配置，不改系统或脚本执行策略。需要严格断网时使用真实隔离环境，不以包管理选项当沙箱。

```powershell
# 从 zxs-workspace 目录；用先前已由构建者核过的解释器和 wheel 目录
$CPython = 'C:\your-trusted-tools\Python314\python.exe'
$Wheels = 'C:\your-verified-wheels\workspace'
$Venv = Join-Path $HOME 'zip-workspace-windows-venv-01'
$Out = Join-Path $HOME 'zip-workspace-0.1.0.dev0-win-candidate-01'
if (Test-Path -LiteralPath $Venv) { throw 'Choose a new venv path' }
& $CPython -I -B -m venv $Venv
if ($LASTEXITCODE -ne 0) { throw 'venv failed' }
$Python = Join-Path $Venv 'Scripts\python.exe'
$env:PIP_CONFIG_FILE = 'nul'
& $Python -I -B -m pip --isolated --disable-pip-version-check install --no-index --no-deps --only-binary=:all: --require-hashes --find-links $Wheels -r .\packaging\windows-amd64\requirements.txt
if ($LASTEXITCODE -ne 0) { throw 'Offline wheel installation failed' }
& $Python -I -B -m pip --isolated --disable-pip-version-check check
if ($LASTEXITCODE -ne 0) { throw 'Wheel dependency closure failed' }
& $Python -I -B .\packaging\windows-amd64\build.py --output $Out
if ($LASTEXITCODE -ne 0) { throw 'Candidate not complete' }
```

使用干净可信的 Workspace 工作副本。输出父须已存在、由本人控制，构建中不被同步、移名或改写；输出不能在项目中或使用已有目录。制包步骤不会执行 pip 或自动补依赖，不启用 UPX 压缩，并把打包工具的 UPX 探测限制到新建的空目录。包不提供写入入口、不收单文件写入或挂载试验；受原平台门限制的通用业务模块仍可因窗口导入而随包。按 [PyInstaller 规格](https://pyinstaller.org/en/v6.22.3/spec-files.html)使用实际 Qt 导入和 [官方 PySide6 钩子](https://doc.qt.io/qtforpython-6/deployment/deployment-pyinstaller.html)。

## 核对真正的候选，再决定是否发行

- 交接 `dist\ZipWorkspace` 完整目录和 `candidate.json`，另从构建端记录该 JSON 的 SHA-256、源码提交和工具来源。配方核验 AMD64 GUI PE、真实 Python DLL、Qt 平台插件和嵌入的 [CreateProcess 清单](https://learn.microsoft.com/en-us/windows/win32/sbscs/application-manifests)：每个语言版本都是 `asInvoker`、`uiAccess=false`。JSON 记录包中每个文件的大小和 SHA-256。只复制成品和记录，不复制构建中间目录或缓存。
- 在另一台已获准的干净 Windows 11 x64 桌面用普通账户验候选。先准备不含系统 Python、Qt 或 WinFsp 的系统和一个新的空私有测试目录；候选目录和测试目录都用固定本机 NTFS，运行期间不移动或同步。脚本使用系统的 Windows PowerShell，无须为此在验收机安装 Python。按本机已有的脚本执行策略运行：

  ```powershell
  .\packaging\windows-amd64\qualify-launch.ps1 -CandidateRoot 'C:\candidate' -ScratchDirectory 'C:\private-test' -ExpectedReceiptSha256 '<来自构建端的64位小写十六进制摘要>' -Launch
  ```

  脚本重算完整成品，实际启动固定的窗口入口，等到 `ZIP Workspace` 主窗口后正常关闭，再重核成品。然后用独立合成归档查浏览、两版本对照和 NTFS 比较，记录是否需要预装的 MSVC runtime。保持系统安全设置不变。GitHub 的 `windows-2025` 是 Server x64，只查源码和 PE 规则；[托管的 Windows 11](https://docs.github.com/en/actions/reference/runners/github-hosted-runners) 是 Arm64，便携构建和普通账户启动要用独立的 Win11 x64 测试机。
- 先落实本产品的分发权，逐项核真正带入的 Python、Qt/Shiboken、PyInstaller 启动器与可能的系统运行库许可；[随包告知](COPYRIGHT-NOTICE.txt)不是许可合规证明。若之后由获权所有者在单独阶段签名，核真实签名及独立信任的发布身份，为签过的文件重新计算摘要，不能用候选摘要代替。**未构建、未运行、未签名、未发布 Windows 安装包或便携包。**

## 可选 R3 原生组件（不是 ZIP 写入，也不附带厂商驱动）

以上无参数候选仍不含磁盘宿主。可选构建路径先在经核对的离线 x64 Visual Studio Native Tools 环境用本目录的
`build-provider.py --sdk <已准备的 WinFsp 2.1 SDK> --import-lib-sha256 <来自独立 SDK 交付单> --output <新目录>`
链接本项目的 `draft_host.c` 和 `draft_io.c` 到 `_workspace_winfsp_draft_host.pyd`。头文件须用固定上游提交的原始字节（不转换换行），导入库按独立 SDK 交付单核验。配方核对原厂头文件、导入库和现有 Python 构建配置，不获取或启动任何厂商组件，不签名。编译环境应由构建者独占，包括 SDK、系统头文件/库及链接工具；构建输入的散列和工具链版本记入构建凭据。

由有权发布者在独立阶段复核依赖、授权和签名。仅在真实 Win11 x64 普通账户验收现有驱动、受控挂载、正常/异常停止、来源流、编辑/提交/恢复、拒绝条件后，向源码 `zipworkspace/_windows_release.py` 的单个记录加入**签名后**本项目 PYD、原厂 DLL/SYS 和独立验收报告的确切 SHA-256。当前没有这样的记录。先以可审计开发资格夹具核验同一签名 PYD 与厂商字节，再更新内嵌记录。最终签名冻结 onedir 须由独立原生环境复验整套流程；任何失败都不发布，不能以开发夹具代替最终包验收。之后使用已有构建命令附加 `--native-provider <已签并验收的同名 PYD>`，只将这个文件放入 `_internal/zipworkspace`；其他 WinFsp 文件仍拒绝。可执行入口最终由有权发布者签名，并对完整 onedir 重新出具独立发布清单。候选清单不是发布/签名证明。不得从环境变量、裸 DLL 搜索路径、当前目录或旁置 JSON 解锁。

运行侧只在该冻结布局中使用 Windows 自身的 [WinVerifyTrust](https://learn.microsoft.com/windows/win32/api/wintrust/nf-wintrust-winverifytrust)，核对完整离线签名链及固定字节；系统吊销缓存不足或仅有目录签名而无法证明原件时拒绝。普通固定 NTFS 上逐级持有原目录及不分享写入/删除的单链接原件。[原厂 MSI](https://github.com/winfsp/winfsp/blob/v2.1/build/VStudio/installer/Product.wxs) 的 HKLM 32 位视图指定原 SxS 物理目录，不走公共 bin 联接。核实精确 [WinFsp SxS 实例](https://github.com/winfsp/winfsp/blob/v2.1/src/dll/sxs.c) 及其 [既有文件系统服务](https://github.com/winfsp/winfsp/blob/v2.1/src/dll/fsctl.c)；为避免把以前已加载的未知驱动当作当下 SYS，进程首次准入要求该实例服务处于停止状态；不会替使用者停止服务。第一次使用整个内存会话都保留原件和模块，第三方服务变更或原安装升级后重新评估，绝不自动安装或降级。原生 DLL/PYD 均先按已经持有的绝对原件路径以 SYSTEM32 依赖查找加载，再初始化 CPython 模块；包无驱动、安装器、启动器或厂商 DLL，未获准的不启用挂载。此源码工作未运行构建或真实验收。

签名后逐字节完整库存、独立签署身份、真实最终 GUI 和回执/正文验证步骤见[Windows 原生最终包验收](NATIVE-ACCEPTANCE.md)。这与只启动并退出的默认无驱动烟测分开，不会自动强杀可能持有卷的程序。
