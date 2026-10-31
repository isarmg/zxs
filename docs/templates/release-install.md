当前运行目标是 **Debian 13 amd64：单个 `zxs` ELF（内含 Web）＋系统 apt Samba＋配套独立 `zxs.so`**。通用运行库由 Debian 提供；不需要部署机器安装 Rust/编译器，不需要旁置 `public/` 或 `--web-dir`。Samba 是系统服务，不是 ZXS 内嵌进程。

下载同一候选/版本的七个发布文件至空目录：`zxs-版本-debian13-amd64`、`zxs-vfs-版本-debian13-amd64.so`、`zxs-materials-版本.tar.gz`、`VFS-BUILD.json`、`BUILD.txt`、`INSTALL.md`、`SHA256SUMS`。先从核实的仓库和工作流确认标签、提交与验收结论。下面仅校验并在当前目录准备文件，不安装服务、不创建账户、不操作磁盘：

```sh
(
set -eu
VERSION="$(python3 - <<'PYVERIFY'
import json
from pathlib import Path
import re
import stat

def regular(name):
    path = Path(name)
    info = path.lstat()
    if not stat.S_ISREG(info.st_mode) or not info.st_size:
        raise SystemExit('发行文件须为非空普通文件：' + name)
    return path

def metadata(name):
    with regular(name).open('rb') as source:
        data = source.read(65537)
    if len(data) > 65536:
        raise SystemExit('发行元数据过大：' + name)
    return data.decode('utf-8')

fields = {}
for line in metadata('BUILD.txt').splitlines():
    key, separator, value = line.partition('=')
    if not separator or not key or key in fields:
        raise SystemExit('BUILD.txt 字段无效或重复')
    fields[key] = value
version = fields.get('version', '')
if not re.fullmatch(r'[0-9]+\.[0-9]+\.[0-9]+([-+][A-Za-z0-9.+-]+)?', version):
    raise SystemExit('发行版本不符合文件名约束')
expected = {'INSTALL.md', 'BUILD.txt', 'VFS-BUILD.json',
            f'zxs-{version}-debian13-amd64', f'zxs-vfs-{version}-debian13-amd64.so',
            f'zxs-materials-{version}.tar.gz'}
names = set()
for line in metadata('SHA256SUMS').splitlines():
    match = re.fullmatch(r'[0-9a-f]{64}  ([A-Za-z0-9.+_-]+)', line)
    if not match or match[1] in names:
        raise SystemExit('摘要清单条目无效或重复')
    names.add(match[1])
if names != expected:
    raise SystemExit('摘要清单必须覆盖完整且唯一的发行文件集合')
for name in expected:
    regular(name)
vfs = json.loads(metadata('VFS-BUILD.json'))
if (fields.get('native-baseline') != 'debian13-amd64'
        or fields.get('native-web-embedded') != 'true'
        or fields.get('native-web-commit') != fields.get('commit')
        or vfs.get('commit') != fields.get('commit')
        or vfs.get('tree') != fields.get('native-web-tree')
        or vfs.get('release') != '13' or vfs.get('architecture') != 'amd64'):
    raise SystemExit('ZXS、Web 与 VFS 的来源/目标不匹配')
print(version)
PYVERIFY
)"
sha256sum --check SHA256SUMS
mkdir "server-$VERSION"
cp "zxs-$VERSION-debian13-amd64" "server-$VERSION/zxs"
cp "zxs-vfs-$VERSION-debian13-amd64.so" "server-$VERSION/zxs.so"
cp VFS-BUILD.json "server-$VERSION/"
chmod 755 "server-$VERSION/zxs"
)
```

`SHA256SUMS` 是完整性清单，**不是独立的发布者签名**。`BUILD.txt` 绑定构建源码、内嵌 Web 与输出字节，不承诺位级可重现。

`zxs-materials-版本.tar.gz` 仅为非运行必需的许可证、来源资料、文档、示例配置和可选离线救援工具 `zxs-rescue`；不需要解压它才能运行 `zxs`，它也不是“单个二进制”的别名。保留、分发对应许可证/版权与来源资料。

## 系统 Samba 与模块

由管理员在 Debian 安装受支持的系统 `samba` 与必要运行库。读取 `VFS-BUILD.json` 的 `package_version`，并用 `dpkg-query -W -f='${Version}\n' samba` 核对实际版本；只有该精确版本的真实加载/客户端验收通过才构成支持。不要将 4.23 的旧模块或其他 Debian 版本模块装入当前宿主。

按照附带 SMB 文档/配置示例，将配套模块以 `zxs.so` 放在实际 `smbd -b` 输出的 `MODULESDIR/vfs/`（受信任、认证用户不可写），配置 `vfs objects = zxs` 及 ZXS Unix socket；当前宿主不接受绝对模块路径。SMB 仍使用一个既有系统账户及独立显式凭据，禁止 guest、force-user、静默复用 Web 密码；首次运行并不会自动创建系统账户、修改权限或启动 smbd。

系统 Samba 安全升级后重新构建并验证匹配的 VFS；版本不匹配模块应拒绝加载，不应为旧插件长期锁定有漏洞的软件包。真实设备、账户/网络部署属于人工安装验收，不由候选构建代替。

## 实测通用运行库

`BUILD.txt` 的 `native-runtime-dependencies` 是 Debian 13 构建环境对实际 `zxs`／`zxs-rescue` ELF 的 `readelf NEEDED` 与 `ldd` 解析结果；包含传递依赖及 loader，每项记录实际路径、解析后路径、文件摘要与 `dpkg-query` 的包名/精确版本。`native-runtime-dependencies-sha256` 绑定该 JSON 字段原文。构建工具不把这些通用库复制进 ZXS。

安装前从该字段核对 `binaries[].libraries[]` 的包名与版本，使用 Debian 软件源准备对应运行库；不要将构建容器的全部开发依赖当作运行依赖。该清单是实际验证版本的证据，不意味着只能永久锁定旧版本；系统安全升级后应重新验收。VFS 所需系统 Samba 与库以 `VFS-BUILD.json` 和对应 SDK 运行验收记录为准。缺库或版本不匹配须先处理，不能宣称“零系统运行依赖”。
