# ZXS Web 与 SMB 发布

当前业务入口为 Web 和 SMB。服务端使用 `zxs` 提供业务 API、Web 静态资源和本地 Samba 桥接；Samba VFS 模块单独按固定 SDK 构建。

每次发行前须逐项核对[发行验收清单](validation/release-checklist.md)，明确区分自动门禁、目标环境实测和仍未关闭的发行阻断项。

## 触发与验证

`.github/workflows/release.yml` 在推送 `v*` 标签时运行；标签须为 `v` 加合法 SemVer，并与根 workspace 中第一方产品包版本一致。先提交版本与锁文件更新，并同步 `test/fixtures/command-line-cli.json` 中产品 `--version` 的固定回归快照，验证实际命令输出后再创建标签；独立构建工具的版本不随产品版本机械替换。`workflow_dispatch` 只演练构建，保留 `release-complete` artifact，不创建标签或公开 Release。

发布流程执行源码、格式、严格 Clippy 与安全检查、业务测试、契约检查、正式 WASM 构建、真实浏览器冒烟和同提交的 Samba 4.23.0 SDK/FFI 检查。CI 与 Release 调用同一 SDK 工作流，发布组装等待其成功。质量门禁直接属于发布的 `verify` 依赖，失败会阻止公开版本。全部成功后严格核对两个归档与 INSTALL.md、BUILD.txt、SHA256SUMS 组成的五文件集合；摘要清单必须完整且无重复，BUILD.txt 的版本和提交必须匹配发行目标。输入通过持有目录及不跟随链接的非阻塞文件句柄读取，生成已校验的私有上传快照，再创建草稿、上传、下载并复核同一集合、源码身份与 SHA-256，最后公开。失败保留草稿；重试仅续传同一源码的本流程草稿，拒绝覆盖已公开版本。发布前后均核对标签提交。只有发布任务持有 `contents: write`。

普通 CI 的 `browser-smoke` 也在归档验收与模板浏览器检查成功后组装安装材料，上传保留 7 天的 `installation-candidate-linux-web` artifact，包含两个归档以及 `INSTALL.md`、`BUILD.txt`、`SHA256SUMS`。这是安装演练候选，不创建标签或公开 Release；该 artifact 的存在不代表其他并行 CI 门禁通过，也不等同于上面的完整 Release `workflow_dispatch` 演练。候选使用同次 CI 的检出提交和产物，部署前仍须核对 `BUILD.txt`、校验摘要并审阅整次 CI 结果。

预发布标签标为 prerelease 且不更新 Latest；正式版由 GitHub 默认规则决定 Latest。

## 发布文件

- `zxs-VERSION-x86_64-unknown-linux-gnu.tar.gz`：Linux x86_64 的直接守护进程 `zxs` 和最小离线救援工具 `zxs-rescue`。发布前须通过 Web 初始化和日常管理替代验收。
- `web-interface-VERSION.tar.gz`：`public/` 中的正式 Web 静态目录，包括 WASM、绑定文件、所需法律文件及 `build-manifest.json`；完整资料置于旁边，不进入静态资源目录。
- `INSTALL.md`、`BUILD.txt`、`SHA256SUMS`：安装与重建说明、源码提交及构建工具记录、全部其他资产的摘要。

两个归档均携带完整 `docs/`、根说明和中央 `compliance/`，包括操作手册及 SMB 集成说明；另附 `deploy/systemd/zxs.service` 与 `deploy/samba/smb.conf.example` 两个部署模板。归档属主和组统一记录为数值 0，避免 root 解压时继承构建机账户 UID；归档权限统一移除组/其他用户写权限，并保证目录和可执行文件可遍历/执行，避免打包者的私有 umask 使服务账户无法读取安装资料。模板仅供审阅，不会自动安装、启用或启动服务。独立 `INSTALL.md` 从[发布安装模板](templates/release-install.md)生成，不依赖从源码文档目录复制后的相对链接。源码与构建脚本引用须在 `BUILD.txt` 指定的仓库提交中查阅。

此流程不发布 Samba VFS 二进制。VFS 必须使用项目固定的 Samba 4.23.0 SDK 构建并核对 ABI，步骤见 [SMB 集成](components/smb.md)。真实 Samba 操作、物理盘故障和恢复仍需专项验收，构建通过不能代替这些验证。

## 下载与安装

下载、完整校验及解压步骤以[发布安装模板](templates/release-install.md)为唯一规范；同一内容随每个版本作为独立 `INSTALL.md` 发布。校验失败或目标目录已存在时必须停止，不要跳过缺失文件或覆盖已有安装。

按运维文档配置服务，将 `--web-dir` 指向 Web 解压目录中的 `public/`，而不是带有完整 `docs/` 与 `compliance/` 的解压根目录。

## 离线重建 Rust 源码

检出 `BUILD.txt` 记录的精确提交，准备 `rust-toolchain.toml` 指定的 Rust 工具链及 WASM 标准库。第三方 Rust 源码由仓库提供，首次正式业务构建也使用 `--locked --offline`；CI 和 Release 使用空的独立 Cargo home，不缓存 registry/git 源码。

```sh
rustup show active-toolchain
rustup target add wasm32-unknown-unknown
python3 tools/rust-sources/manage.py verify --metadata
cargo run --locked --offline -p release-tools --
bash .github/scripts/check.sh web-production
python3 .github/scripts/check-rust-diagnostics.py -- \
  cargo build --locked --offline --release --package storage-server --package storage-rescue --message-format=json
```

打包校验的边界：`release-package.sh` 会核对 Web 输入/输出摘要，但对原生程序仅检查普通文件、非空及可执行权限，不证明其对应当前源码。官方 Release 在干净检出中紧接上述原生构建后打包，并用解压后的二进制验收。直接本地调用打包或组装脚本前也必须重新构建两个原生程序；修改源码后不能沿用旧 `target/native/release/` 文件。`BUILD.txt` 的提交号是组装时检出的源码身份，不是原生构建收据。

正式 Web 绑定工具来自 [绑定生成工具](tools/web-bindings.md)：从 wasm-bindgen-cli 0.2.129 提取 Web 命令，删除测试运行器、其他目标命令及网络/TLS 依赖。其版本字符串保持 `wasm-bindgen 0.2.129`，与 Web schema 匹配；原始来源、许可证及修改记录纳入[统一源码清单](THIRD_PARTY_RUST_SOURCES.md)。

Python 3 校验源码清单；安全扫描独立管理 cargo-audit，并在线读取漏洞公告与 yanked 状态，所有发现及扫描不完整均阻止通过。Rust 工具链、SQLite、Samba、Linux 系统库及必要 SDK 继续按既有方式准备。托管 runner 和原生依赖可能更新，因此以上流程保证可追溯重建，不承诺位级一致。

## 发布故障回归

`bash test/ci/release-publish.sh` 在临时仓库使用假的 `gh` 离线检查新建、草稿续传、503/403 拒绝创建、已公开版本拒绝覆盖、远端额外文件、校验后本地输入变化以及下载损坏拒绝公开。它纳入 `build-tools` suite，不会写入 GitHub。
