# 本地开发入门

[文档目录](README.md) · [工程结构](20-engineering.md)

以下命令在仓库根目录执行，用目录模拟数据盘来验证开发流程。生产部署见[初始化与维护](17-lifecycle-and-maintenance.md)及[配置规范](21-configuration.md)。


需要 Linux amd64 构建宿主和 `rust-toolchain.toml` 指定的 Rust 1.99.0，当前验收目标为 `x86_64-unknown-linux-gnu`。Web 的 `wasm32-unknown-unknown` 目标不扩展原生平台支持范围。先构建守护进程与离线救援入口，再通过受保护 Web 引导初始化；没有默认密码，也不接受明文密码命令行参数。

```sh
cargo build --locked --offline --target-dir target/native -p storage-server -p storage-rescue
target/native/debug/zxs --root "$PWD/runtime/pool" \
  --management-root "$PWD/runtime/management" --listen 0.0.0.0:7400 \
  --web-dir /absolute/path/to/verified-web-output \
  --allow-development-setup
```

首次打开 Web，先设置唯一管理员用户名和密码，再登录。首次设置无需授权密钥，也无需选盘；没有管理员时，任何能够访问此 Web 的人都可完成这一操作，因此部署者应限制初始网络可达范围并及时设好账户。已建立或损坏的管理权威都不能通过首次设置重置。

登录后在系统管理设置“存储发现目录”，默认 `/zxs/data/disks`，也可指定其他已存在目录。仅浏览其内部目录，支持合法的多层子目录，不扫描整个系统。分别核对真实挂载路径与实际空存储子目录，检查后确认首盘激活。存储子目录须为服务账户所有的 0700 空目录；服务不会挂载、格式化、创建存储目录或修改权限。systemd 示例的可写范围仍限制在 `/zxs` 下的显式目录，其他发现根必须已被部署权限允许。

只有显式灾备恢复继续使用本地 bootstrap 授权，细节见[初始化与恢复](management/setup-recovery.md)。不要删除已有凭据、权威或激活证据来重新执行首次设置。

单盘池只保存一份数据，没有磁盘冗余；该盘损坏可能导致数据永久丢失。新建仓库默认目标份数为 1（包含主盘），不要把“目标已满足”理解为有备份。首次激活的多目录选择按顺序建立逐级备份链，全部作为元数据成员；已有多成员池不会因丢盘自动缩减成员或降低发布证明要求。

`--allow-development-setup` 仍只标记开发模拟；普通生产存储目录不需该开关，也不要求 Btrfs、独立挂载点或物理设备独立性。用户自行保证底层故障域，程序保留目录 device/inode、seal、权限和完整性检查。登录后可在持久发现根内浏览并选择现有挂载文件系统中的合法空子目录，不要求逐个登记候选白名单。服务不自动挂载、格式化或改权限；当前 Web 不提供高级 Config 草稿应用入口。

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
