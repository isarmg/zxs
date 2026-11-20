# macOS 26 Apple Silicon 本机 GUI 候选

此独立配方沿用 ZIP Workspace 的 Qt 窗口，目标仅为 **macOS 26、Apple Silicon arm64**。交接的是包含原生 `ZipWorkspace.app` 的版本化 ZIP，不是安装器、背景服务或工作区数据归档。不会生成 Intel / Universal 版，不带 CLI、macFUSE、WinFsp、内核/FSKit 挂载组件，也不注册文件外开或修改系统权限。

应用保留已接入的归档浏览、分支/版本对照、本机目录对比；APFS 同卷整包另存仍须符合原来的来源、目录和权限条件。打包并不新增工作树签出/提交、磁盘映射或普通 ZIP 解包。没有 macOS 实机验证，不据此承诺云盘或 SMB。

## 预备可信的离线工具

以下供获授权的构建者将来在干净、网络隔离的参考机操作。使用原生有 GIL 的 [CPython 3.14.10 macOS 官方安装器](https://www.python.org/downloads/release/python-31410/)，核对官方发布证据与 SHA-256 `286c7018aa2730f6747ca433dccdc2b31be45f078a9ea0ffa44d5f06368fd2d7`。这是 universal2 输入，但解释器须实际以 arm64 运行，不用 Rosetta。预先备好可用的 Apple Xcode 或 Command Line Tools（正常工具链选择、可调用 lipo / codesign / install_name_tool），配方不接受协议、不激活安装提示作为备用，不装编译器或驱动。

`requirements.txt` 钉住八个 [PyInstaller 6.22.3](https://pypi.org/project/pyinstaller/6.22.3/#files)、[Qt Essentials](https://pypi.org/project/PySide6-Essentials/6.11.2/#files)、[Shiboken](https://pypi.org/project/shiboken6/6.11.2/#files) 及其构建依赖的实际 wheel；把这些官方文件放进已核的本机 wheel 目录。不要装其他 Qt 模块、使用 Sdist 或回退旧安全补丁。严格离线须切断构建机网络，而不只依赖 pip 参数。PyInstaller 的本机签名命令可能带时间戳参数；这里限定 ad-hoc 模式、不选择证书，宿主工具失败就拒绝产物，不为其打开网络或改用签名凭据。

**只有以后明确接受本机构建的 ad-hoc 签名才执行。** Apple Silicon 的冻结二进制需要本地代码签名；[PyInstaller 默认的 ad-hoc 过程](https://pyinstaller.org/en/v6.22.3/feature-notes.html#macos-binary-code-signing) 不使用 Apple Developer 账户或证书。它不是 Developer ID、公证或正式分发签名。

```sh
# 从 zxs-workspace 执行；换成你事先核过的解释器及本机 wheel 目录
set -e
base="/Library/Frameworks/Python.framework/Versions/3.14/bin/python3.14"
wheels="$HOME/verified-workspace-macos-wheels"
venv="$HOME/zip-workspace-mac-venv-01"
out="$HOME/zip-workspace-0.1.0.dev0-macos-candidate-01"
[ ! -e "$venv" ] && [ ! -L "$venv" ] || exit 1
"$base" -I -B -m venv "$venv"
PIP_CONFIG_FILE=/dev/null "$venv/bin/python" -I -B -m pip --isolated --disable-pip-version-check install \
  --no-index --no-deps --only-binary=:all: --require-hashes --find-links "$wheels" \
  -r packaging/macos-arm64/requirements.txt
PIP_CONFIG_FILE=/dev/null "$venv/bin/python" -I -B -m pip --isolated --disable-pip-version-check check
"$venv/bin/python" -I -B packaging/macos-arm64/build.py --allow-local-adhoc --output "$out"
```

工作副本须可信且无人并发改写；输出父须在本机且存在，由操作者独占，期间不移动或同步。输出只能是工程外的全新目录。构建器只调用已备好的工具；普通输出、临时文件和工具缓存隔离在新目录，UPX 不参与打包。失败不自动清理、不重新签名、不写候选回执。

## 交接和验证

- 当前版本为 `0.1.0.dev0`；bundle 标识为 `org.zipworkspace.desktop`、显示版本 `0.1.0`，构建号 `1`。只从实际生成且复核为 arm64、含 Cocoa 插件和预期 Python / Qt 组件的 `dist/ZipWorkspace.app` 打包。先验证本地 ad-hoc 签名；原生 [ditto](https://developer.apple.com/documentation/security/customizing-the-notarization-workflow) 保留包内相对链接，不把框架展开成松散文件。
- Bundle 只声明为 `.zxs` 的备选只读查看器。待有实际 app 时，可在 Finder 对一份文件点「打开方式 → ZIP Workspace」，在新窗口看路径、通过本次确认才读取；不注册普通 ZIP 或网址，不修改系统默认项，也不继承现有窗口的写入许可。按[Qt 文档打开事件](https://doc.qt.io/qtforpython-6/PySide6/QtGui/QFileOpenEvent.html)接收，不启用 PyInstaller 参数模拟。须各验未启动、已启动、重复送入及取消；当前未构建或验机。
- 成套交接的是 `zip-workspace-0.1.0.dev0-macos-arm64-candidate.zip` 和最终 `candidate.json`，以 JSON 的文件名、大小和 SHA-256 为准。不要交接未打包的中间 `dist/ZipWorkspace`、缓存或部分 ZIP。散列和本地签名不能证明证书身份、成功启动、可复现构建或系统安全放行。
- 将来在另一个全新私有目录用系统工具解开**该 ZIP**，重验实际包内链接、主程序 slice、签名和 Qt 装载。用没有额外 Python/Qt/macFUSE 的已授权 macOS 26 Apple Silicon 普通账户核验 Finder 启动、浏览和对照、取消及受限另存。若系统安全策略阻止，保留原标记并停止，本配方不清除隔离标记或要求绕过 Gatekeeper。
- 正式分发前核项目权属、[实际第三方组件和许可](COPYRIGHT-NOTICE.txt)。另由获权发行者确认适用的签名、公证及渠道；签过的重新打包文件须另验身份及散列。此步骤不读密钥、不调用公证或上传。**本批未构建、未签名、未在 macOS 执行或发布任何二进制。**
