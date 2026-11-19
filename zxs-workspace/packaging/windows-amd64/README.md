# Windows 11 x64 本机便携候选

这个配方把现有 ZIP Workspace Qt 窗口做成 **本机单目录候选**，不依赖其他项目。成品入口是 `ZipWorkspace.exe`，运行时应携带同目录的 `_internal`，不是只有一个 EXE，也不是安装器。不添加 PATH、文件关联、服务、开机启动、管理员权限或驱动。

包内只有窗口入口：已核快照浏览、同库历史/版本对照、适用本机 NTFS 目录的只读比较和普通 Store ZIP 预览。窗口原有平台准入不变。**不创建可编辑副本、不签出或提交、不启动或打包 WinFsp**；内部附件服务、虚拟盘和单文件写入资格源码不属于这个包。原 Windows 写入合同仍待对正式路径来源、权限、原件和实际恢复验收。

## 准备经核对的离线环境

这是供获权构建者**以后**运行的流程，不是现在要安装的程序。使用干净的 Windows 11 x64 及官方常规（有 GIL）[CPython 3.14.10 Windows installer (64-bit)](https://www.python.org/downloads/release/python-31410/)；不是 32 位、ARM64、自由线程或 embeddable 包。官方安装器 SHA-256：`92480239264744fe0386a1bd8275786fefa126e3129e56a68e22b52c73329965`。由构建者单独核官方公示散列和发布证明，先准备可信工具。后续安全更新应升级并再核配方，不为匹配旧配置回退安全补丁。

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

- 交接 `dist\ZipWorkspace` 完整目录和 `candidate.json`。后者在打包返回后才写入，记录项目版本、构建依赖与整个包里每个普通文件的大小、SHA-256（文件路径相对于包目录）；同时要求 PE 为 AMD64 GUI、真实 Python DLL 与 Qt 平台插件存在。不能把构建中间目录、EXE 单件或不完整记录当候选；工具不删除既有结果。不复制 `work`、`temp` 或缓存。
- 记录确切源码提交和工具来源；按记录重算每件的哈希并检查没有额外文件，查看 EXE 的 `asInvoker` / `uiAccess=false` 清单和 DLL 实际依赖。须在已许可的干净 Windows 11 x64 普通账户，用不装 Python / Qt / WinFsp 的环境启动窗口并核浏览、确切两版本对照、NTFS 比较及退出；确定是否依赖现有 MSVC runtime。先跑独立副本，不截用用户档案。源码与哈希清单不是运行验证、签名、系统拦截放行或可复现构建证明。
- 先落实本产品的分发权，逐项核真正带入的 Python、Qt/Shiboken、PyInstaller 启动器与可能的系统运行库许可；[随包告知](COPYRIGHT-NOTICE.txt)不是许可合规证明。若之后由获权所有者在单独阶段签名，核真实签名及独立信任的发布身份，为签过的文件重新计算摘要，不能用候选摘要代替。**未构建、未运行、未签名、未发布 Windows 安装包或便携包。**
