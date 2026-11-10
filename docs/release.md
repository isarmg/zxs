# ZXS Web 与 SMB 发布

**发行契约：** 公开产物仅为完整源码归档、SHA256 清单、`zxs-helper` 和 `zxs` 四个文件。匹配的 ZXS/VFS 与许可材料内置 helper；原生构建、VFS 和安装收据保存在源码外层包，完整仓内源码保留真实来源、许可证与部署材料。本次是官方打包/组装/验证入口的源码迁移，尚未实际构建或发行，不能据此称为完整离线构建或安装。参见[辅助工具打包边界](design/helper-disk-management.md)。

当前业务入口为 Web 和 SMB。Debian 13 amd64 服务端使用单个 `zxs` ELF 提供业务 API、内嵌 Web 和本地 Samba 桥接；系统 apt Samba 使用精确匹配的独立 `zxs.so`，不内嵌或自动管理 smbd。

每次发行前须逐项核对[发行验收清单](validation/release-checklist.md)，明确区分自动门禁、目标环境实测和仍未关闭的发行阻断项。

## 触发与验证

`.github/workflows/release.yml` 当前保留手动静态入口，版本/构建/验收/发布长作业按 [短 CI 策略](ci/development-short-ci.md) 暂停；本片不恢复或触发它们。恢复正式发行时标签须为 `v` 加合法 SemVer，并与根 workspace 中第一方产品包版本一致。先提交版本与锁文件更新，并同步 `test/fixtures/command-line-cli.json` 中产品 `--version` 的固定回归快照，验证实际命令输出后再创建标签；独立构建工具的版本不随产品版本机械替换。`workflow_dispatch` 只演练构建，保留 `release-complete` artifact，不创建标签或公开 Release。

发布流程执行源码、格式、严格 Clippy 与安全检查、业务测试、契约检查、正式 WASM 构建、真实浏览器冒烟和同提交的 Debian 13 系统 Samba 匹配 SDK/FFI 和真实 SMB 检查。CI 与 Release 调用同一 SDK 工作流，发布组装等待其成功。质量门禁直接属于发布的 `verify` 依赖，失败会阻止公开版本。全部成功后严格核对 ZXS、helper、完整源码归档和 SHA256SUMS 的四文件集合；摘要清单只覆盖其余三项，必须完整且无重复。源码包内 BUILD.txt 的版本和提交、VFS 与完整源码 tree 必须匹配发行目标。输入通过持有目录及不跟随链接的非阻塞文件句柄读取，生成已校验的私有上传快照，再创建草稿、上传、下载并复核同一集合、源码身份与 SHA-256，最后公开。失败保留草稿；重试仅续传同一源码的本流程草稿，拒绝覆盖已公开版本。发布前后均核对标签提交。只有发布任务持有 `contents: write`。

普通 CI 的 `browser-smoke` 也在归档验收与模板浏览器检查成功后组装安装材料，上传保留 7 天的 `installation-candidate-linux-web` artifact，包含下述四文件集合。这是安装演练候选，不创建标签或公开 Release；该 artifact 的存在不代表其他并行 CI 门禁通过，也不等同于上面的完整 Release `workflow_dispatch` 演练。候选使用同次 CI 的检出提交和产物，部署前仍须核对 `BUILD.txt`、校验摘要并审阅整次 CI 结果。

发行 UI 门禁使用宿主正常沙箱的 Google Chrome 作为远端客户端，被测单 ELF 则运行在 Debian 13 容器。测试 launcher 只映射该 ELF 和私有夹具目录、非 root、无新增 capability，服务强制只监听 `127.0.0.1`；容器句柄负责退出和清理。另有无编译器 Debian 13 最小运行环境逐资源 SHA 门禁。此组合不能宣称“容器内 Chromium 已通过”：该尝试的 namespace `EPERM` 诊断保留，未通过关闭 sandbox 或更改宿主安全策略绕过。

单 ELF 运行门禁先以本次交付的同版 helper 在隔离 Debian 镜像中准备 `/zxs`，再在无网络容器中走真实私有布局、首管理员、空目录和 production 首批存储初始化。初次 helper 入口在原本不存在 `/zxs` 的隔离镜像内建立互斥根，随后原摘要准备操作须返回 existing-root 持锁回执。保留原预览、摘要、完整计划和原生完成收据；helper 存活时还须明确拒绝普通 daemon 启动，helper 停止并回收后，主程序以相同非 root 服务身份冷启动，要求管理、认证、存储均就绪、`/readyz` 成功、内嵌资源逐份匹配并正常退出。服务运行时再启动同版 helper 也必须因互斥租约被拒绝，不能把任意配置错误当作通过。Chrome 的已有测试池共用准备过 host root 的镜像。此门禁使用镜像内的既有测试账户，不覆盖 APT、Unix 账户创建、systemd 安装／激活或整套安装向导；对应检查仍须另行完成。

若后续验收失败，`unaccepted-native-提交` 仅保留带原始源码收据的诊断字节；它不是最终发行资产，不能重新标记成其他提交或当作 UI 验收通过。

预发布标签标为 prerelease 且不更新 Latest；正式版由 GitHub 默认规则决定 Latest。

## 发布文件

- `zxs-VERSION-debian13-amd64`：单个 Debian 13 amd64 ELF，内嵌正式 Web。
- `zxs-helper-VERSION-debian13-amd64`：独立维护工具，内嵌同次构建的 ZXS、准确 VFS 及法律材料；不得把缺嵌入载荷的开发 helper 当作发行工具。
- `zxs-source-VERSION.tar.gz`：外层只含 `BUILD.txt`、`VFS-BUILD.json`、`INSTALL.md` 和 `repository.tar.gz`。后者含原始 `SOURCE-COMMIT` 及 `source/` 下准确 Git tree 的全部普通源码文件，包括中央 `compliance/`、依赖来源、文档与部署模板；不受 export-ignore/export-subst 或工作区未提交文件影响。
- `SHA256SUMS`：严格覆盖以上三个文件，不自包含，不代替发布者签名。

打包先校验准确 native/Web/VFS 身份，再用原始 Git blobs 生成完整源码归档；校验器重新计算每个 blob、Git tree 与 commit 对象，不依赖归档自报文件清单。源码内层有 65,536 文件、单文件 64 MiB、合计 512 MiB 边界，两个 ELF 各有 512 MiB 边界。归档成员只能是固定作用域普通文件，拒绝路径逃逸、重复、链接、子模块、特殊文件、错误模式和超额解压；不执行或按任意成员路径提取文件。

归档属主/组和时间统一为数值 0，源码文件保留 Git 的 0644/0755 模式。安装说明直接取自经过完整 tree 校验的准确源码模板，不由 assemble job 重新读取模板或重标 BUILD。VFS 模块不作为第五个公开文件，打包仍校验实际模块及 helper 嵌入摘要；收据不能替代真实 helper 导出和系统 Samba 加载验收。

`release-package.sh` 拒绝已有的 `release-assets`，先私有整备再以独占新文件写入；写入故障保留本次部分输出供诊断，不覆盖旧发行。assemble 先完整核验私有快照，再独占创建 SHA256SUMS，最终复核当前输入。发布上传与下载复验都走同一四文件校验器，拒绝旧七文件集合而不提供双格式兼容。临时复制与多次有界读回成本随资产大小线性增长，无性能改善声明。

VFS 必须与收据中实际验证的 Debian 13 Samba 包匹配，不能用旧 4.23 SDK 模块替代系统 4.22 系列模块。系统 Samba 安全升级后，应同步重建匹配 VFS 并重验，不长期锁住有漏洞版本。仓内官方 4.23 源码保留为历史构建和测试来源，不是默认运行依赖。真实物理盘故障与恢复仍需专项验收，构建通过不能代替这些验证。

## 下载与安装

下载、完整校验及单程序安装步骤以[发布安装模板](templates/release-install.md)为唯一规范；同一内容随每个版本存入源码外层包的 `INSTALL.md`。校验失败或目标目录已存在时必须停止，不要跳过缺失文件或覆盖已有安装。

全新机器先按安装模板使用独立 helper 完成原生初始化，再启动单个 `zxs`，不传 `--web-dir`。Samba 及其独立模块按 [SMB 集成](components/smb.md)配置；显式独立凭据和系统身份不会由解压资产自动建立。

## 仓内源码与离线输入的边界

**首次完整离线构建尚未具备输入闭包。** 四文件及完整仓内源码不包含已经认证的完整 Debian deb/对应源/签名索引、固定 OCI 镜像、Rust/WASM 工具链；`smb-sdk.sh` 缓存未命中仍有 apt 路径，Dockerfile 仍联网准备系统包。本片保留这些明确阻塞，不编造固定摘要或将未核材料列为已交付。后续需生产可信完整闭包、保留许可证/对应源，再单独切换强制离线入口。

已有受信工具链和系统输入时，检出源码包 `BUILD.txt` 记录的精确提交，准备 `rust-toolchain.toml` 指定的 Rust 工具链及 WASM 标准库。第三方 Rust 源码由仓库提供，首次正式业务构建也使用 `--locked --offline`；CI 和 Release 使用空的独立 Cargo home，不缓存 registry/git 源码。

```sh
rustup show active-toolchain
rustup target add wasm32-unknown-unknown
python3 tools/rust-sources/manage.py verify --metadata
cargo run --locked --offline -p release-tools --
bash .github/scripts/check.sh web-production
export RELEASE_VERSION="$(python3 -c 'import tomllib; print(tomllib.load(open("Cargo.toml", "rb"))["workspace"]["package"]["version"])')"
python3 .github/scripts/release-native.py
```

原生构建显式选择既有 `x86_64-unknown-linux-gnu` 目标和 release profile，输出在 `target/native/x86_64-unknown-linux-gnu/release/`；没有提高最低 CPU ISA，也不根据内存或核心数推断 AVX 支持。构建成功后生成 `BUILD.txt`，记录构建时提交、Rust 编译器身份、两个二进制摘要、构建配置及相关编译环境的摘要（不公开环境变量原值）。打包核对暂存二进制与该收据，并绑定内层完整源码归档的摘要/计数/字节及内嵌 Web 源身份和清单摘要；组装保留构建收据，拒绝目标、提交、版本、VFS 源身份或资产摘要不匹配。Web 输入/输出校验与 wasm32 目标独立保留。

原生收据要求构建前后均为干净 checkout；已暂存、未暂存和未被忽略的新文件都会阻止签发，HEAD 不变也不能接受源码改动。忽略的构建目录仍可使用；失败不保留旧成功收据。该检查不证明构建期间未发生后又撤销的改动，也不覆盖仓库外工具或被忽略的输入。显式 CPU／编译参数覆盖仍只记录摘要，不由此证明产物适合某台部署机器或具备完整可重现性。

Debian 13 单 ELF 和系统 Samba 运行验收也在环境检查前撤销各自 `--output` 指定的旧 JSON，全部检查成功后才生成本次收据。复用证据目录时，前置条件或中途检查失败均不能留下上次成功结果；本次已产生的诊断日志照常保留。收据路径须与输入产物分开，脚本不清空所在目录或跟随输出符号链接删除外部目标。

收据不保证完全可重现、不证明编译器可信或部署 CPU 兼容，也不覆盖外部编译器及系统库的全部输入。官方流程在干净检出和 Debian 13 基线中构建，将单个 ELF 复制到独立目录后真实验收内嵌 Web；直接本地调用前也必须重新构建两个原生程序，修改源码后不能沿用旧产物或收据。Samba VFS 按 Debian 精确包源码与补丁单独构建，生成 VFS-BUILD.json，并在最终资产校验中与原生提交和树绑定。

正式 Web 绑定工具来自 [绑定生成工具](tools/web-bindings.md)：从 wasm-bindgen-cli 0.2.129 提取 Web 命令，删除测试运行器、其他目标命令及网络/TLS 依赖。其版本字符串保持 `wasm-bindgen 0.2.129`，与 Web schema 匹配；原始来源、许可证及修改记录纳入[统一源码清单](THIRD_PARTY_RUST_SOURCES.md)。

Python 3 校验源码清单；安全扫描独立管理 cargo-audit，并在线读取漏洞公告与 yanked 状态，所有发现及扫描不完整均阻止通过。Rust 工具链、SQLite、Samba、Linux 系统库及必要 SDK 继续按既有方式准备。托管 runner 和原生依赖可能更新，因此以上流程保证可追溯重建，不承诺位级一致。

## 发布故障回归

`bash test/ci/release-publish.sh` 在临时仓库使用假的 `gh` 离线检查新建、草稿续传、503/403 拒绝创建、已公开版本拒绝覆盖、远端额外文件、校验后本地输入变化以及下载损坏拒绝公开。它纳入 `build-tools` suite，不会写入 GitHub。

## 辅助工具内置程序首装后继

当前 native 构建源码已改为先 ZXS、后将其准确输出嵌入 helper，并校验匹配版本、来源与 SHA-256；开发 helper 明确无载荷，不运行时下载。见[程序首装及未完成范围](design/helper-program-installation.md)。四资产链已同步到源码候选，但尚未实际构建或安装；不表示 VFS/依赖安装、真实离线输入闭包或完整修复已验收。
