# 本地开发入门

[文档目录](README.md) · [工程结构](20-engineering.md)

以下命令在仓库根目录执行，用目录模拟数据盘来验证开发流程。生产部署见[初始化与维护](17-lifecycle-and-maintenance.md)及[配置规范](21-configuration.md)。


需要 Linux amd64 构建宿主和 `rust-toolchain.toml` 指定的 Rust 1.99.0，当前验收目标为 `x86_64-unknown-linux-gnu`。Web 的 `wasm32-unknown-unknown` 目标不扩展原生平台支持范围。先构建守护进程与离线救援入口，再通过受保护 Web 引导初始化；没有默认密码，也不接受明文密码命令行参数。

```sh
cargo build --locked --offline --target-dir target/native -p storage-server -p storage-rescue
target/native/debug/zxs --root "$PWD/runtime/pool" \
  --management-root "$PWD/runtime/management" --listen 0.0.0.0:7400 \
  --web-dir /absolute/path/to/verified-web-output \
  --setup-disk "P01=$PWD/runtime/disks/P01" --allow-development-setup
```

最低配置为系统盘加一个存储盘，二级、三级、四级及后续备份均可不配置。运行前准备一个现有、私有的测试存储目录和管理目录，并替换 Web 输出路径。操作员在管理目录安全预置 `bootstrap.key`：服务 UID 所有、普通单链接文件、0600、恰好64个十六进制 ASCII 字节且无换行，mtime起30分钟有效。不要将密钥写进命令行、日志或版本库。打开 Web 的首次初始化页，输入引导密钥，选择至少一个批准的存储盘 ID，并设置首次管理员；引导授权不保存在浏览器持久存储中。缺少有效授权时拒绝初始化，不自动生成管理员或数据池。

### 安全预置开发引导密钥

下面是供操作员审查后执行的 Linux 示例，不是启动服务时自动生成凭据的步骤。先准备服务 UID 所有、无符号链接的私有管理目录；以同一 UID 执行，路径与上面的 `--management-root` 一致。示例拒绝覆盖已有密钥，不输出密钥，也不使用 shell 变量、参数或管道传递密钥；不要在录屏、调试跟踪或日志采集会话中读取密钥。

```sh
python3 - <<'PYBOOTSTRAP'
import os
import secrets
import stat
from pathlib import Path

root = Path.cwd() / "runtime/management"
metadata = root.lstat()
if (root.resolve() != root or not stat.S_ISDIR(metadata.st_mode)
        or metadata.st_uid != os.geteuid()
        or stat.S_IMODE(metadata.st_mode) != 0o700):
    raise SystemExit("管理目录须为当前服务 UID 所有、无符号链接的 0700 目录")
fd = os.open(root / "bootstrap.key",
             os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
with os.fdopen(fd, "wb") as output:
    os.fchmod(output.fileno(), 0o600)
    output.write(secrets.token_hex(32).encode("ascii"))
    output.flush()
    os.fsync(output.fileno())
directory = os.open(root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
try:
    os.fsync(directory)
finally:
    os.close(directory)
PYBOOTSTRAP
```

随后由操作员通过可信的本机私密查看方式将密钥填入首次初始化页；不要粘贴到聊天、URL、工单或共享终端。30 分钟有效期按文件修改时间计算；已有密钥或未完成初始化时先查[初始化恢复](management/setup-recovery.md)，不删除旧授权或恢复证据来重跑示例。此示例只说明密钥格式与文件权限，不初始化任何盘，也不替代生产部署的 UID、目录和设备审核。

单盘池只保存一份数据，没有磁盘冗余；该盘损坏可能导致数据永久丢失。新建仓库默认目标份数为 1（包含主盘），不要把“目标已满足”理解为有备份。可选多盘向导按选择顺序建立备份链，高级预置可以指定自定义多盘布局；已有多成员池不会因丢盘自动缩减成员或降低发布证明要求。

`--allow-development-setup` 仍只标记开发模拟；普通生产存储目录不需该开关，也不要求 Btrfs、独立挂载点或物理设备独立性。用户自行保证底层故障域，程序保留目录 device/inode、seal、权限和完整性检查。高级初始配置由本机 `--setup-config ID=/absolute/private-config.json` 预置，再在 Web 选择；浏览器不能传任意路径或配置 JSON。

服务运行时通过 Web 管理、SMB 访问文件。直接启动 `zxs`，不使用 `serve` 子命令。离线救援见[救援说明](components/rescue.md)，需要打开池的救援操作先停服。默认监听 `0.0.0.0:7400`，不禁止非回环地址；浏览器以服务器实际 IP 或主机名访问。网络可达范围和传输保护由部署者保证，HTTP 会明文传输凭据与数据；需要 HTTPS 时配置反向代理和正确的 `--external-origin`。仅供本机开发时可显式指定 `--listen 127.0.0.1:7400`。系统盘管理凭据与数据成员分离，缺失或损坏的数据池不阻止管理 HTTP 启动。

Web UI 不提供文件上传、下载或正文预览；文件正文由 SMB 或受控 HTTP API 读写。HTTP API 仍是可测试的协议边界，不因 UI 缺少按钮而删除，也不等同于额外的原生用户 App。页面范围以[Web 组件说明](components/web.md)为准，协议以[API 规范](15-api-and-errors.md)为准。

Web 构建、资源服务及真实浏览器验证见 [Web README](components/web.md)。仅提供浏览器和 SMB 用户入口，不安装原生 App。Samba 的目标 SDK、单一认证账户绑定和构建边界见 [Samba README](components/smb.md)。

```sh
cargo test --locked --offline --target-dir target/native
cargo run --locked --offline -p contract-check --target-dir target/native
cargo run --locked --offline -p unicode-build -- --check
rustup target add wasm32-unknown-unknown
bash .github/scripts/check.sh web-production
```

Rust 依赖源码已经纳入仓库，首次业务构建也使用根锁文件和 `--locked --offline`。须预先准备 Rust 工具链、WASM target 和所需系统 SDK；Web 构建器调用本地维护的绑定工具。工具链或 SDK 的安装不属于 Rust 依赖下载。本机 Unix 套接字和所有权隔离测试需要相应系统能力，不能把隔离环境拒绝执行当作业务测试通过。

[持续集成说明](ci.md)列出同一根工作区中按目标选择的服务端、SMB、正式 WASM 构建与浏览器烟测，以及 SDK 和真机验证边界。 标签自动发布、下载校验与安装方式见[发布说明](release.md)。

## 一次 API 变更的贯通检查

这是[工程规范的文档与契约同步要求](20-engineering.md)的执行导航，不新增第二套接口规则。以增加或修改一个业务请求字段为例：

1. 先在[对应领域规范](README.md)明确语义、权限、失败与结果未知的处理；通过[术语表](02-concepts-and-invariants.md#s02-04)对应到现有对象，不凭 UI 文案创建重复概念。
2. 核对 `src/contract` 的 Rust 类型，以及[契约附件](contracts/README.md)中的 schema、OpenAPI 和固定向量；修改适用项，注明不适用项的原因。
3. 跟踪服务入口到核心的校验与授权，再检查 `src/client`、Web 或 SMB 的真实调用方。只涉及受控 HTTP 正文能力时，不因此增加 Web 上传/下载界面。
4. 在[测试导航](validation.md)找到契约、核心及真实 HTTP 消费者用例，覆盖正常值、未知/缺失字段、边界值、无权限、冲突及结果未知；涉及界面时增加实际 WASM 浏览器验证，涉及 SMB 时保留 SDK/互操作验收。
5. 按[CI 入口](ci.md)运行受影响测试和契约检查，记录实际命令及结果；同步规范与操作说明，不新增改进流水或单次验收报告，也不以文档链接通过冒充端到端业务验收。
