# ZXS Web 与 SMB 发布

当前业务入口为 Web 和 SMB。Debian 13 amd64 服务端使用单个 `zxs` ELF 提供业务 API、内嵌 Web 和本地 Samba 桥接；系统 apt Samba 使用精确匹配的独立 `zxs.so`，不内嵌或自动管理 smbd。

每次发行前须逐项核对[发行验收清单](validation/release-checklist.md)，明确区分自动门禁、目标环境实测和仍未关闭的发行阻断项。

## 触发与验证

`.github/workflows/release.yml` 在推送 `v*` 标签时运行；标签须为 `v` 加合法 SemVer，并与根 workspace 中第一方产品包版本一致。先提交版本与锁文件更新，并同步 `test/fixtures/command-line-cli.json` 中产品 `--version` 的固定回归快照，验证实际命令输出后再创建标签；独立构建工具的版本不随产品版本机械替换。`workflow_dispatch` 只演练构建，保留 `release-complete` artifact，不创建标签或公开 Release。

发布流程执行源码、格式、严格 Clippy 与安全检查、业务测试、契约检查、正式 WASM 构建、真实浏览器冒烟和同提交的 Debian 13 系统 Samba 匹配 SDK/FFI 和真实 SMB 检查。CI 与 Release 调用同一 SDK 工作流，发布组装等待其成功。质量门禁直接属于发布的 `verify` 依赖，失败会阻止公开版本。全部成功后严格核对单 ELF、独立 VFS、资料归档与 VFS-BUILD.json、INSTALL.md、BUILD.txt、SHA256SUMS 组成的七文件集合；摘要清单必须完整且无重复，BUILD.txt 的版本和提交必须匹配发行目标。输入通过持有目录及不跟随链接的非阻塞文件句柄读取，生成已校验的私有上传快照，再创建草稿、上传、下载并复核同一集合、源码身份与 SHA-256，最后公开。失败保留草稿；重试仅续传同一源码的本流程草稿，拒绝覆盖已公开版本。发布前后均核对标签提交。只有发布任务持有 `contents: write`。

普通 CI 的 `browser-smoke` 也在归档验收与模板浏览器检查成功后组装安装材料，上传保留 7 天的 `installation-candidate-linux-web` artifact，包含下述七文件集合。这是安装演练候选，不创建标签或公开 Release；该 artifact 的存在不代表其他并行 CI 门禁通过，也不等同于上面的完整 Release `workflow_dispatch` 演练。候选使用同次 CI 的检出提交和产物，部署前仍须核对 `BUILD.txt`、校验摘要并审阅整次 CI 结果。

发行 UI 门禁使用宿主正常沙箱的 Google Chrome 作为远端客户端，被测单 ELF 则运行在 Debian 13 容器。测试 launcher 只映射该 ELF 和私有夹具目录、非 root、无新增 capability，服务强制只监听 `127.0.0.1`；容器句柄负责退出和清理。另有无编译器 Debian 13 最小运行环境逐资源 SHA 门禁。此组合不能宣称“容器内 Chromium 已通过”：该尝试的 namespace `EPERM` 诊断保留，未通过关闭 sandbox 或更改宿主安全策略绕过。

若后续验收失败，`unaccepted-native-提交` 仅保留带原始源码收据的诊断字节；它不是最终发行资产，不能重新标记成其他提交或当作 UI 验收通过。

预发布标签标为 prerelease 且不更新 Latest；正式版由 GitHub 默认规则决定 Latest。

## 发布文件

- `zxs-VERSION-debian13-amd64`：Debian 13 amd64 的单个原生 ELF，内嵌正式 Web；运行无需旁置静态目录。
- `zxs-vfs-VERSION-debian13-amd64.so`：与 `VFS-BUILD.json` 所记 Debian Samba 包精确匹配的独立模块。Samba 服务由系统提供，不打包进 ZXS。
- `zxs-materials-VERSION.tar.gz`：完整 `docs/`、根说明、中央 `compliance/`、部署模板及可选离线救援工具 `zxs-rescue`。资料归档不是主程序，也不是启动时必须解压的运行依赖。
- `VFS-BUILD.json`：Debian 源码及补丁、系统包版本、ABI、源码身份和模块摘要收据。
- `INSTALL.md`、`BUILD.txt`、`SHA256SUMS`：安装与重建说明、原生程序及内嵌 Web 的源码/构建身份、全部其他资产摘要。

资料归档的属主和组统一记录为数值 0，权限移除组/其他用户写入，保留目录与可执行文件的遍历/执行权限。部署模板仅供审阅，不自动安装、启用或启动服务。独立 `INSTALL.md` 从[发布安装模板](templates/release-install.md)生成，不依赖源码文档相对链接。通用运行库由 Debian 安装；这不是静态链接、任意 Linux 通用程序，也不代表 ZXS 自动配置系统 Samba 或账户。

VFS 必须与收据中实际验证的 Debian 13 Samba 包匹配，不能用旧 4.23 SDK 模块替代系统 4.22 系列模块。系统 Samba 安全升级后，应同步重建匹配 VFS 并重验，不长期锁住有漏洞版本。仓内官方 4.23 源码保留为历史构建和测试来源，不是默认运行依赖。真实物理盘故障与恢复仍需专项验收，构建通过不能代替这些验证。

## 下载与安装

下载、完整校验及单程序安装步骤以[发布安装模板](templates/release-install.md)为唯一规范；同一内容随每个版本作为独立 `INSTALL.md` 发布。校验失败或目标目录已存在时必须停止，不要跳过缺失文件或覆盖已有安装。

按运维文档配置服务，直接启动单个 `zxs`，不传 `--web-dir`。Samba 及其独立模块按 [SMB 集成](components/smb.md)配置；显式独立凭据和系统身份不会由解压资产自动建立。

## 离线重建 Rust 源码

检出 `BUILD.txt` 记录的精确提交，准备 `rust-toolchain.toml` 指定的 Rust 工具链及 WASM 标准库。第三方 Rust 源码由仓库提供，首次正式业务构建也使用 `--locked --offline`；CI 和 Release 使用空的独立 Cargo home，不缓存 registry/git 源码。

```sh
rustup show active-toolchain
rustup target add wasm32-unknown-unknown
python3 tools/rust-sources/manage.py verify --metadata
cargo run --locked --offline -p release-tools --
bash .github/scripts/check.sh web-production
export RELEASE_VERSION="$(python3 -c 'import tomllib; print(tomllib.load(open("Cargo.toml", "rb"))["workspace"]["package"]["version"])')"
python3 .github/scripts/release-native.py
```

原生构建显式选择既有 `x86_64-unknown-linux-gnu` 目标和 release profile，输出在 `target/native/x86_64-unknown-linux-gnu/release/`；没有提高最低 CPU ISA，也不根据内存或核心数推断 AVX 支持。构建成功后生成 `BUILD.txt`，记录构建时提交、Rust 编译器身份、两个二进制摘要、构建配置及相关编译环境的摘要（不公开环境变量原值）。打包核对暂存二进制与该收据，并绑定资料归档摘要、内嵌 Web 源身份及清单摘要；组装保留构建收据，拒绝目标、提交、版本、VFS 源身份或资产摘要不匹配。Web 输入/输出校验与 wasm32 目标独立保留。

原生收据要求构建前后均为干净 checkout；已暂存、未暂存和未被忽略的新文件都会阻止签发，HEAD 不变也不能接受源码改动。忽略的构建目录仍可使用；失败不保留旧成功收据。该检查不证明构建期间未发生后又撤销的改动，也不覆盖仓库外工具或被忽略的输入。显式 CPU／编译参数覆盖仍只记录摘要，不由此证明产物适合某台部署机器或具备完整可重现性。

收据不保证完全可重现、不证明编译器可信或部署 CPU 兼容，也不覆盖外部编译器及系统库的全部输入。官方流程在干净检出和 Debian 13 基线中构建，将单个 ELF 复制到独立目录后真实验收内嵌 Web；直接本地调用前也必须重新构建两个原生程序，修改源码后不能沿用旧产物或收据。Samba VFS 按 Debian 精确包源码与补丁单独构建，生成 VFS-BUILD.json，并在最终资产校验中与原生提交和树绑定。

正式 Web 绑定工具来自 [绑定生成工具](tools/web-bindings.md)：从 wasm-bindgen-cli 0.2.129 提取 Web 命令，删除测试运行器、其他目标命令及网络/TLS 依赖。其版本字符串保持 `wasm-bindgen 0.2.129`，与 Web schema 匹配；原始来源、许可证及修改记录纳入[统一源码清单](THIRD_PARTY_RUST_SOURCES.md)。

Python 3 校验源码清单；安全扫描独立管理 cargo-audit，并在线读取漏洞公告与 yanked 状态，所有发现及扫描不完整均阻止通过。Rust 工具链、SQLite、Samba、Linux 系统库及必要 SDK 继续按既有方式准备。托管 runner 和原生依赖可能更新，因此以上流程保证可追溯重建，不承诺位级一致。

## 发布故障回归

`bash test/ci/release-publish.sh` 在临时仓库使用假的 `gh` 离线检查新建、草稿续传、503/403 拒绝创建、已公开版本拒绝覆盖、远端额外文件、校验后本地输入变化以及下载损坏拒绝公开。它纳入 `build-tools` suite，不会写入 GitHub。
