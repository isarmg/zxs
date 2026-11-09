当前运行目标是 **Debian 13 amd64：单个 `zxs` ELF（内含 Web）＋系统 Samba＋配套 VFS**。通用运行库由 Debian 提供；部署机器不需要 Rust/编译器、旁置 `public/` 或 `--web-dir`。Samba 是系统服务，不是 ZXS 内嵌进程。独立 helper 内嵌同次构建的 ZXS、匹配 VFS 和许可材料。

## 下载与完整校验

下载同一版本的四个文件至新的空目录：

- `zxs-版本-debian13-amd64`
- `zxs-helper-版本-debian13-amd64`
- `zxs-source-版本.tar.gz`
- `SHA256SUMS`

先从可信仓库和工作流核实版本、准确提交与验收结果。将下面的 `VERSION` 改为已核实版本；操作只校验下载，不安装服务、不创建账户、不操作磁盘。

```sh
(
set -eu
export VERSION='已核实版本'
python3 - <<'PYVERIFY'
import os
from pathlib import Path
import re
import stat

version = os.environ['VERSION']
if not re.fullmatch(r'[0-9]+\.[0-9]+\.[0-9]+([-+][A-Za-z0-9.+-]+)?', version):
    raise SystemExit('发行版本不符合文件名约束')
expected = {f'zxs-{version}-debian13-amd64',
            f'zxs-helper-{version}-debian13-amd64',
            f'zxs-source-{version}.tar.gz'}
for name in expected | {'SHA256SUMS'}:
    info = Path(name).lstat()
    if not stat.S_ISREG(info.st_mode) or not info.st_size:
        raise SystemExit('发行文件须为非空普通文件：' + name)
if Path('SHA256SUMS').stat().st_size > 65536:
    raise SystemExit('摘要清单过大')
names = set()
for line in Path('SHA256SUMS').read_text(encoding='ascii').splitlines():
    match = re.fullmatch(r'[0-9a-f]{64}  ([A-Za-z0-9.+_-]+)', line)
    if not match or match[1] in names:
        raise SystemExit('摘要清单条目无效或重复')
    names.add(match[1])
if names != expected:
    raise SystemExit('摘要清单必须准确覆盖三个发行载荷')
PYVERIFY
sha256sum --check SHA256SUMS
)
```

任何校验失败必须停止。`SHA256SUMS` 是完整性清单，**不是独立发布者签名**。需要完整语义校验时，从已核实的准确源码运行 `.github/scripts/verify-release-assets.py --directory 下载目录 --version 版本 --commit 完整提交`；它同时核对两个 ELF、内置材料收据与完整源码 Git tree，不执行程序或提取任意归档路径。

源码外层包固定只含 `BUILD.txt`、`VFS-BUILD.json`、`INSTALL.md` 和 `repository.tar.gz`。内层归档保留准确提交对象及 `source/` 下的完整仓内源码，包括第三方来源、许可证、文档与部署样例。保持原归档及来源材料，不将源码压缩包当作运行时依赖。BUILD 收据绑定准确 ZXS、helper、内嵌 Web/VFS 与内层源码，不承诺位级可重现。

**完整离线依赖材料仍未齐备。** 四文件布局及 Rust 的 `--offline` 不证明已经提供固定的 Debian 安装包/签名索引/对应源、OCI 镜像或 Rust/WASM 工具链。不能把本版称为无需预备外部输入的首次离线构建或全自动离线安装。系统 Samba 及通用库仍按后文准备，未封装的输入不得临时从网络下载后冒充已验证发行材料。

**全新机器先使用独立 helper 完成离线首装，再启动普通 ZXS。** 校验后为两个下载的 ELF 设置执行权限；以原文件名运行 `zxs-helper-版本-debian13-amd64` 即可，不再从旧资料包提取 helper，也不再寻找独立发布的第五个 VFS 文件。已初始化的日常服务不需要 helper 常驻。

## 首次安装：先用独立辅助工具，再启动主服务

管理员先在工具外建立专用的非 root 本机 `zxs` 服务账户及其主组，并停止 ZXS、Samba 和其他磁盘/主机配置写者。在可信隔离网络中，手动以 root 运行已校验的 `zxs-helper-版本-debian13-amd64`，浏览器打开 `http://主机实际IP:9040`。此控制台**没有登录，且为明文 HTTP**，任何可达者均有维护权限；同源/CSRF 不是访问认证。工具不设置自身自启，使用后退出。

按控制台中独立的预览、确认和真实回执依次进行：

1. “准备安装根与运行互斥”：仅创建 root 所有、0755 的 `/zxs`。然后“安装内置 ZXS 程序”，固定安装到 `/usr/local/bin/zxs` 并保留准确载荷基线；不手工覆盖已有不同程序。
2. “首次初始化前：服务私有目录”及“首次安装：运行目录与空 SMB 导出”：使用同一服务账户，准备 `/zxs/data/{state,management,disks}` 及运行目录。它们只准备空布局，不创建管理员、数据库或磁盘。已有不匹配或非空目录须保留，不能清空来重走首装。
3. “首次初始化：只创建系统管理员”：在仍无磁盘子目录的准确空布局创建唯一 ZXS 管理员，手动输入密码并保存不含密码的原请求摘要。**不是启动普通 ZXS 后再设置管理员。**
4. 管理员在工具外准备和挂载正确的持久空盘目录，设置准确服务 UID/GID 与 0700；随后在“首次存储：整批初始化现有空目录”中一次列出 `/zxs/data/disks` 下全部真实直接子目录并确认。该阶段创建初始配置、数据库及激活证明，不格式化设备或自动挂载。
5. 核对运行库、实际 Samba/VFS 与独立 SMB 凭据，再通过配置安装入口预览并安装准确 systemd/Samba 文件。该入口不覆盖不同配置、不修改系统默认 Samba 配置、不 reload/start/enable。完整安装 ready 与自动服务激活仍未提供，不能把某一步成功当作已可上线。

任何一步中断或响应丢失，先保留原摘要、输入、pending 和已发布材料，再走同一步的原请求查询/确认续作；不换请求重置、不删目录或凭据。初始空根和私有布局只允许准确空 staging 的权限/属主交接恢复，不修复现用数据树。

## `/zxs` 布局与 systemd

完整源码内的 `source/deploy/systemd/zxs.service` 与 helper 生成单元统一使用固定程序目的地；持久数据与运行目录仍在 `/zxs`：

```ini
WorkingDirectory=/zxs
ExecStart=/usr/local/bin/zxs --root /zxs/data/state --management-root /zxs/data/management --storage-discovery-root /zxs/data/disks --listen 0.0.0.0:7400
ReadWritePaths=/zxs/data /zxs/config /zxs/log /zxs/run /zxs/tmp /zxs/cache /zxs/home
```

`/usr/local/bin/zxs` 由 root 所有、0755、服务账户不可写；`/zxs` 为 root 所有、0755，源码外层包中的本版安装说明可保存为 `/zxs/INSTALL.md`。不要再安装另一个 `/zxs/zxs` 或用样例单元覆盖 helper 已安装的准确配置。

使用内嵌 Web，不需要 `--web-dir`。默认直接 HTTP；部署者自行控制网络暴露和传输保护。需要 HTTPS 代理时再按实际地址配置 `--external-origin`。helper 首装运行目录准备完成后，`/zxs/{config,log,run,tmp,cache,home}` 均为准确服务 UID/GID、0700；管理员另行预建该账户所有、0600 的普通日志文件 `/zxs/log/zxs.log`（不得为符号链接），不能在运行目录“空目录准备”之前预放日志。socket 位于 `/zxs/run`；凭据、完整工作文件、SMB spool、索引和恢复记录仍放 `/zxs/data`，不能当临时文件清理。日志需监测空间并在正常停服后归档，不自动删除。单元不创建 `/run`、`/var/lib`、`/var/log` 下的 ZXS 专用目录。

helper 的独立空导出目录 `/zxs/smb-export` 为 root:root、0755，认证 Samba 用户不可写；不位于任何 0700 私有目录内。系统服务注册文件、apt Samba、系统运行库和 VFS 模块仍使用发行版要求的系统目录。

完成首批存储及上述部署前提后，管理员审阅实际 unit/drop-in、执行目标环境的 systemd/Samba 配置核验，并另行决定 reload、启动或启用服务。普通 `zxs` 在任何 runtime/监听器启动前检查已有管理/SYSTEM 根、`config.json` 及初始激活证明，缺项即退出并要求 helper 原操作恢复，不提供空系统 Web 引导。启动后的 Web 使用已经离线创建的管理员登录；初始化回执不替代实际目标机启动和访问验收。

## 系统 Samba 与模块

由管理员在 Debian 安装受支持的系统 `samba` 与必要运行库。读取 `VFS-BUILD.json` 的 `package_version`，并用 `dpkg-query -W -f='${Version}\n' samba` 核对实际版本；只有该精确版本的真实加载/客户端验收通过才构成支持。不要将 4.23 的旧模块或其他 Debian 版本模块装入当前宿主。

使用 helper 已有的匹配 VFS 安装预览/确认入口；其载荷与源码包内 `VFS-BUILD.json` 绑定。实际模块以 `zxs.so` 安装到 `smbd -b` 输出的 `MODULESDIR/vfs/`（受信任、认证用户不可写）。按照完整源码中的 SMB 文档/配置示例配置 `vfs objects = zxs` 及 ZXS Unix socket；当前宿主不接受绝对模块路径。SMB 仍使用一个既有系统账户及独立显式凭据，禁止 guest、force-user、静默复用 Web 密码；首次运行并不会自动创建系统账户、修改权限或启动 smbd。

系统 Samba 安全升级后重新构建并验证匹配的 VFS；版本不匹配模块应拒绝加载，不应为旧插件长期锁定有漏洞的软件包。真实设备、账户/网络部署属于人工安装验收，不由候选构建代替。

## 实测通用运行库

`BUILD.txt` 的 `native-runtime-dependencies` 是 Debian 13 构建环境对实际 `zxs`／`zxs-helper` ELF 的 `readelf NEEDED` 与 `ldd` 解析结果；包含传递依赖及 loader，每项记录实际路径、解析后路径、文件摘要与 `dpkg-query` 的包名/精确版本。`native-runtime-dependencies-sha256` 绑定该 JSON 字段原文。构建工具不把这些通用库复制进 ZXS。

安装前从该字段核对 `binaries[].libraries[]` 的包名与版本，使用 Debian 软件源准备对应运行库；不要将构建容器的全部开发依赖当作运行依赖。该清单是实际验证版本的证据，不意味着只能永久锁定旧版本；系统安全升级后应重新验收。VFS 所需系统 Samba 与库以 `VFS-BUILD.json` 和对应 SDK 运行验收记录为准。缺库或版本不匹配须先处理，不能宣称“零系统运行依赖”。

