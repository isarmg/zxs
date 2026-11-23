# Ubuntu 26.10 amd64：本机制作三个独立安装包

这是 **Ubuntu 26.10（stonking，非 LTS）amd64** 的离线配方，不是已发布的安装包。只需这份 Workspace 源码；无需整个仓库、其他项目、PyPI 或签名密钥。

| 想使用 | 本项目的包 | 可见入口 |
| --- | --- | --- |
| 只用命令行 | `zip-workspace` | `/usr/bin/zip-workspace`；只依赖系统 Python |
| 桌面浏览、签出等 | 基础包＋`zip-workspace-desktop` | 应用菜单 **ZIP 工作树** 或 `zip-workspace-desktop`；不依赖 FUSE |
| 仅挂一个只读快照 | 基础包＋`zip-workspace-mount-ro` | `zip-workspace-mount-ro ARCHIVE MOUNTPOINT`；不依赖 Qt |
| 在桌面明确发起挂载 | 同版本的全部三包 | 打开快照后，在窗口里另选空挂载点并确认 |

桌面未装可选挂载组件时仍能浏览，挂载按钮不可用。桌面包另提供 [freedesktop 文件类型](https://specifications.freedesktop.org/shared-mime-info/latest-single/)和「打开方式」声明，只接一份 `.zxs`。文件管理器交给应用一个路径后仍须确认；若管理器先为非本地文件准备本机副本，应用只核交到手的实际文件，不验证它与远端对象的同一性。普通 ZIP 和网址不注册。候选安装时由发行版的 MIME 与桌面数据库工具处理缓存；本项目无安装脚本，不设置用户默认应用或开机启动项。不会自动挂载、设置用户组、修改系统权限或加载驱动。使用挂载仍需操作者事先配好可用的 FUSE3 用户态和内核环境；这份配方不安装它们。

## 取得本地候选包

以下命令供获得授权的人**以后**在独占、无同步的本地工作副本运行。只用普通账户、Ubuntu 系统 `/usr/bin/python3` 和 `/usr/bin/dpkg-deb`；无需联网、root、编译器、Qt 开发工具或 FUSE SDK。输出父目录也须由操作者独占、已存在且整个制包期间不被同步、移动或改写。每次必须新建一套 0700 输出目录，不能指向旧成果或位于项目内。把示意联系信息换成获准写入包内的真实 ASCII 名字和邮箱；不会发送邮件。

```sh
/usr/bin/python3 -I -B packaging/ubuntu-26.10/package.py \
  --maintainer 'Local Packager <name@example.invalid>' \
  --output "$HOME/zip-workspace-candidate-01"
```

### 可选 HTTPS 运行库

如需一起封装 Workspace 的 HTTPS 运行库，在上述命令末尾加 `--network-wheels /绝对路径/本地轮子目录`。该目录须已有 [固定清单](../network-r3/requirements.txt) 中的六个原版 `py3-none-any` wheel。配方不下载、不调用 pip、不构建 wheel；它在创建输出目录前按 SHA-256 核对本地文件，并将 Python 模块、CA 信任数据以及各包原有的发行元数据和许可文件保留在可选的第四个包 `zip-workspace-network-r3`。它依赖同版桌面包，不设置端点、账户或挂载。安装后仍由使用者明确连接。省略参数仍制作原来的三包。

带可选包构建时，四包齐全后才写 `SHA256SUMS`；单独验证安装与移除可选包、实际导入来源、系统 TLS/ALPN 和 CA 数据。不能用任意已安装的系统库代替这份锁定闭包。

配方按 `INPUTS.sha256` 验证确切的 `pyproject.toml`、运行时 Python 源，以及安装的桌面入口、MIME 声明和版权说明。缺项或不匹配时在创建输出目录前停止；打包使用刚核验的字节，不按路径再读这些安装文件，也不从所在主机自动搜集包。归档由系统 `dpkg-deb` 制作；Qt、libfuse、Python、驱动、资格夹具和开发材料不打进去。每个应用运行文件只属于一包，均从私有安装目录加载；没有安装/卸载钩子。仅当三个包均生成才写入 `SHA256SUMS`。中断可留下 staging 或不完整的 .deb，它们不是成套结果；工具不自动删除或覆盖。摘要用于比对字节，不是签名。

此版本为 `0.1.0~dev0-1local1`，直接运行依赖固定在以下 Ubuntu 版本，不锁间接依赖或底层磁盘能力。制包工具来自 Ubuntu 的 [dpkg](https://packages.ubuntu.com/stonking/dpkg)；配方固定其调用，资格构建记录实际版本，不锁制包工具的微修订，也不声称相异构建机能产出逐字节相同的归档。未使用完整仓库快照。**不要**为了装候选包降级系统、保持旧补丁或禁用更新；如官方源版本不同，先更新配方、版本及审查记录，再行资格验证。

| 系统来源（参见各页的 Copyright File） | 本配方参考的直接版本 |
| --- | --- |
| [Python 默认解释器包](https://packages.ubuntu.com/da/stonking/amd64/python3) | 3.14.7-3 |
| [PySide6 QtCore、QtWidgets](https://packages.ubuntu.com/zh-cn/stonking/python3-pyside6.qtwidgets) | 6.11.2-1build1 |
| [Qt 平台插件](https://packages.ubuntu.com/eu/stonking/qt6-qpa-plugins) | 6.11.2+dfsg-4ubuntu2 |
| 桌面关联数据库：[shared-mime-info](https://packages.ubuntu.com/stonking/shared-mime-info)、[desktop-file-utils](https://packages.ubuntu.com/stonking/desktop-file-utils) | 2.4-5build3、0.28-1build1 |
| [pyfuse3](https://packages.ubuntu.com/stonking/python3-pyfuse3)、[Trio](https://packages.ubuntu.com/en/stonking/python3-trio)、[FUSE3 工具](https://packages.ubuntu.com/stonking/fuse3) | 3.5.0-3、0.34.0-1、3.18.3-1 |

## 哪一步才算可以发行

1. 在获准的干净参考机核对 Ubuntu 已签名软件源中的实际版本、依赖闭包及工具链。记录构建主机、配方和输入清单的版本、安装包的摘要。检查每个包的 `dpkg-deb --info` 和 `--contents`，尤其是仅一份代码、菜单/入口和没有安装钩子。
2. 分别验证：只装基础包能用 CLI；基础＋桌面可从应用菜单启动并浏览、从文件管理器只交接所选的 `.zxs`（验证空格、中文、取消以及普通 `.zip` 不被认领）、缺 FUSE 不可挂载；基础＋挂载不需要 Qt；在 `apt --no-install-recommends` 的最小组合中也有由硬依赖 `fuse3` 提供的 `/usr/bin/fusermount3`；三包在已授权的真实 FUSE 环境都能沿本次明确选择的原件工作。测试普通桌面会话、升级和移除可选包，卸载不得清除包外归档、工作树或私有状态。不同发行版、架构和 Wayland/X11 会话分别验收，不从本配方推定。

   桌面签出与提交可按[合成的 Linux Qt 验收流程](../../tests/qualification/linux-desktop-workflow.md)记录，只用基础包与桌面包。
3. 核定产品版权与对外分发许可，处理实际使用的第三方义务，再另行签名和公开发布。[来源与许可说明](COPYRIGHT-NOTICE)不是新增的产品许可。

本批只交付封装源码。没有构建、安装、签名、实机验收或公开二进制。
