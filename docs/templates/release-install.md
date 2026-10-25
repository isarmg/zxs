下载同一版本的全部发布文件，放在同一个空目录中：两个 `.tar.gz` 归档、`INSTALL.md`、`BUILD.txt` 和 `SHA256SUMS`。先从核实的仓库及对应工作流核对所选标签与提交，再校验全部文件。以下命令需要 Python 3、`sha256sum` 和 `tar`，先检查版本及完整摘要清单，再校验文件，任一步失败都不解压：

```sh
(
set -eu
VERSION="$(python3 - <<'PYVERIFY'
from pathlib import Path
import re
import stat

def regular(name):
    path = Path(name)
    info = path.lstat()
    if not stat.S_ISREG(info.st_mode) or not info.st_size:
        raise SystemExit("发行文件须为非空普通文件：" + name)
    return path

def metadata(name):
    path = regular(name)
    with path.open("rb") as source:
        data = source.read(64 * 1024 + 1)
    if len(data) > 64 * 1024:
        raise SystemExit("发行元数据过大：" + name)
    return data.decode("utf-8")

fields = {}
for line in metadata("BUILD.txt").splitlines():
    key, separator, value = line.partition("=")
    if not separator or not key or key in fields:
        raise SystemExit("BUILD.txt 字段无效或重复")
    fields[key] = value
version = fields.get("version", "")
if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+([-+][A-Za-z0-9.+-]+)?", version):
    raise SystemExit("发行版本不符合文件名约束")
expected = {"INSTALL.md", "BUILD.txt", f"web-interface-{version}.tar.gz",
            f"zxs-{version}-x86_64-unknown-linux-gnu.tar.gz"}
names = set()
for line in metadata("SHA256SUMS").splitlines():
    match = re.fullmatch(r"[0-9a-f]{64}  ([A-Za-z0-9.+_-]+)", line)
    if not match or match[1] in names:
        raise SystemExit("摘要清单含无效或重复条目")
    names.add(match[1])
if names != expected:
    raise SystemExit("摘要清单必须恰好覆盖同版本的四个发行文件")
for name in expected:
    regular(name)
print(version)
PYVERIFY
)"
sha256sum --check SHA256SUMS
mkdir "server-$VERSION" "web-$VERSION"
tar -xzf "zxs-$VERSION-x86_64-unknown-linux-gnu.tar.gz" -C "server-$VERSION"
tar -xzf "web-interface-$VERSION.tar.gz" -C "web-$VERSION"
"./server-$VERSION/zxs" --help
"./server-$VERSION/zxs-rescue" --help
)
```

摘要核对只证明下载文件与清单一致，不是独立的发布者签名；不能仅因文件和同来源清单互相匹配就认定发布者身份已经验证。

`mkdir` 必须成功创建两个新目录；已有目录应另选新名称。服务端与 Web 分别解压到独立空目录，避免混合文件或覆盖现有业务数据。两个归档都携带完整的 `docs/`、根说明与 `compliance/`，以及 `deploy/systemd/zxs.service` 和 `deploy/samba/smb.conf.example` 两个部署模板；请从解压目录阅读 `docs/release.md`、`docs/25-operation-manual.md` 和 `docs/components/smb.md`。文档中的开发源码路径用于定位对应提交，开发与来源复核需要另外检出 `BUILD.txt` 指定的仓库提交。

服务端通过 `zxs` 提供业务 API 和 Web 静态资源；按运维文档配置数据与认证，并将 `--web-dir` 指向 Web 解压目录下 `public/` 的绝对路径（例如 `/opt/zxs/web-VERSION/public`），不要指向包含完整资料的解压根目录。Linux 二进制要求兼容 Ubuntu 24.04 的 GNU libc 及运行库。

SMB 通过独立的 Samba → 本机桥接访问核心。发布包不包含 Samba VFS 二进制；须按照 `docs/components/smb.md`，使用固定的 Samba 4.23.0 SDK 单独构建、核对 ABI 并完成实际客户端验收。


## 安装与升级边界

归档解压只是准备发行文件，不会创建服务账户、修改权限、注册服务或初始化数据。部署模板及其说明位于 `deploy/systemd/zxs.service` 和 `docs/operations/deploy/systemd.md`。必须先替换示例域名、程序/Web 路径、盘位及访问配置；不要直接原样启用模板。解压后的 `zxs` 位于服务端归档根目录，而模板的 `/opt/zxs/bin/zxs` 需要部署者另行安装或改成实际的绝对路径；Web 参数同样应指向实际 `public/`。程序及 Web 文件应由管理员所有且服务账户不可写，数据目录则按运维要求单独准备，不能把数据放进版本目录。

升级前阅读对应版本的变更与格式兼容说明，保留旧程序、匹配的 Web 文件和实际服务配置，并完成独立备份与恢复演练。停止接入写入，等待服务正常退出，确认没有旧进程持有数据后，再同时切换服务端和 Web 到同一新版本；不要原地覆盖运行中的程序或替换正在使用的静态资源。模板的正常停服没有超时强杀，若停止卡住，应先诊断；强制终止属于崩溃恢复流程。

新版本启动后检查日志、管理状态、数据引擎恢复状态，并验证读写及已部署的 SMB 接入；HTTP/Web 可用不代表数据恢复成功。若启动失败，保留现场及恢复记录，不要清空状态、暂存文件或自动重新初始化。只有明确确认数据格式和配置向后兼容、且新进程已完全停止，才能切回旧程序及对应 Web；否则应停服并按恢复流程处理，不能把“保留旧二进制”等同于可安全回滚。首发流程本身不承诺未来版本的原地升级或降级兼容性。
