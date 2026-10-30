# 持续集成

[GitHub Actions 工作流](../.github/workflows/ci.yml) 在 push、pull request 及手动触发时检查项目。使用 `rust-toolchain.toml` 指定工具链与唯一根 Cargo.lock，不部署或发布。产品用户入口仅为 Web 与 SMB；没有桌面或移动 App 构建任务。

所有自有 Rust 组件和基础实现由一个根 workspace 管理，命令按包和编译目标选择。统一入口为 [check.sh](../.github/scripts/check.sh)：

| 范围 | 实际检查 |
| --- | --- |
| `static` | 不安装或调用 Rust：第一方包与依赖方向检查、工作流早期 env 上下文检查、来源静态校验、CI 与性能报告 Python 工具回归、shell 语法与发布 mock、Node 历史脚本配置和库存检查；不执行历史浏览器验收 |
| `rust-sources` | 源码、manifest、全部特性边、锁文件、来源文档和退役记录校验；空 Cargo home 重新解析五个入口／目标闭包，拒绝无闭包使用的活动源码树；完整来源工具 Python 回归 |
| `quality` | 自有 workspace 的格式与严格 Clippy；宿主机全部目标及 Web 的 WASM release 编译，所有本地依赖的 Rust warning/error 均失败；基础源码不自动格式化 |
| `security` | 固定 cargo-audit 查询当前 RustSec 与 registry/yank 状态；独立扫描器锁、产品根锁和活动／已合并实现的原始身份均扫描，告警或扫描不完整失败；退役及已升级替换的旧身份单列历史记录 |
| `workspace` | 服务端、核心、认证、平台、直接服务入口、离线救援工具、HTTP 与 socket 测试；同 job 对准确 platform-io libtest 运行原子写屏障／故障子进程，拒绝零测试通过；另行串行运行真实大历史包用例 |
| `foundation` | Linux amd64 UAPI、libc ABI、rustix 双后端 syscall、系统/socket/mio、停车锁、搜索/进程及随机数后端回归；ring 预生成/本地生成的库、集成与 CAVP 矩阵；密码哈希、真实 storage-client 消费者启用的 IDNA 全标量及数值/集合等限定 lib 测试；不是全部上游测试 |
| `web` | Web 应用、公共 runtime 与 CSR 内核/宏的单元、集成及 Rustdoc 测试；过程宏通过仅开发依赖的真实消费者验证文档 |
| `contracts` | 契约工具测试及全仓 schema、配置、文档链接检查 |
| `build-tools` | 发布校验、WASM 构建器、本地绑定工具、Unicode 重现、离线编排与性能报告校验器回归；不执行性能工作负载 |
| `smb-protocol` | 显式选择独立的 smb-protocol 协议包 |
| `smb-sdk` job | 固定哈希的官方 Samba 4.23.0 SDK，真实 VFS Clippy、测试编译和 release 构建均经过零 Rust 警告门禁，再正常执行链接测试 |
| `web-production` | 本地维护的绑定工具、wasm32 release 构建、资源清单和压缩产物 |
| `browser-smoke` | 实际 Web/WASM 对真实服务端的 Chrome 交互；响应式尺寸预览不冒充原生 App 验收 |

`browser-smoke` 成功完成真实归档解压验收及模板浏览器检查后，运行无发布副作用的 `release-assemble.sh`，生成并校验完整安装材料。随该 job 上传的 `installation-candidate-linux-web` artifact 保留 7 天，包含两个归档、`INSTALL.md`、`BUILD.txt` 和 `SHA256SUMS`，供检查或部署演练；下载后仍须完整校验摘要。

该候选只表示此 job 的打包、浏览器及组装检查通过，不代表同次 CI 的其他并行门禁已通过。它不会创建标签、GitHub Release 或公开发布；也不等同于 Release 工作流的 `workflow_dispatch` 演练，后者还通过版本工具验证并等待全部发布验证分片和固定 Samba SDK 检查，最终提供 `release-complete`。正式发布前仍须检查完整工作流结果及首发验收清单。

只需 Python 3.11+、Node.js 和 Bash 即可在仓库根先运行静态门禁，无需 Rust、registry、浏览器或联网安装依赖：

```sh
bash .github/scripts/check.sh static
```

CI 与 Release 都先运行独立的 `static` job；它通过后才安装或使用 Rust。

仅当 `main` 收到非强制的普通 push，且完整 head SHA 已在本仓库同一官方 CI 工作流的候选分支 push 中完成验证时，CI 的静态门禁才可采用那次源码验证证据。[精确 SHA 判定器](../.github/scripts/check-main-adoption.py) 使用公开只读 API，不增加 token 权限：核对工作流 ID／路径、仓库及 head 仓库、完整 SHA、成功运行尝试、全部 17 个作业及相关执行步骤，并核对七份未过期产物的来源和本次尝试的成功上传时间。PR merge SHA、不同提交／工作流、缺失或跳过的执行、仅采用旧证据的运行都不能成为来源；复用结果不再接力复用。

查询只读取最近至多三个同 SHA 的成功 push 候选，不翻页、不重试；每次响应最多 1 MiB，单次网络等待超时 10 秒。缺失、不完整、API 错误、尝试变化或任何不能确认的输入都选择原有完整路径。普通候选分支、PR、标签及手动 CI 始终走完整路径，未经候选验证的直接 main push 也完整执行。候选需先完成再将 main 快进到**相同提交**；合并、rebase 或新增提交得到的新 SHA 必须重新验证。

采用证据时保留既有作业名称和矩阵，只跳过重复的源码构建、测试、SDK 及浏览器执行步骤；各作业记录原始运行尝试链接。`static` 仍全部执行，`Tests (security)` 仍实时读取 RustSec 和 registry/yank 状态。原始源码证据不替代本次安全结果，也不能只凭轻量作业成功判断候选完整性。Release／标签发布工作流保持全部原有验证，共用 SDK 工作流的默认空输入仍执行真实 SDK 检查。

采用的产物留在原候选运行，保留 `BUILD.txt` 等材料中的原始 CI URL；main 只记录链接，不下载、重新上传、重命名或混装产物。需要材料时到原运行下载并核验摘要；过期或缺失材料会使判定回到完整验证。此机制仅采用精确 SHA 的完整执行证据，不按文件、目录、tree 或近似提交缓存测试成功。定向回归通过现有 `static` 的 `test/ci` 发现入口执行：`python3 -m unittest discover -s test/ci -p 'test_main_adoption.py'`；其中包含真实运行 `41031345216` 第一次尝试的 API 字段夹具，夹具只证明当时证据，不能绕过实时查询。

完整来源工具回归只在后续矩阵的 `rust-sources` 分片运行一次，不再阻塞静态预检；它与其余 Rust 测试、生产构建并行，Release 仍须等待所有验证分片通过后才组装和发布。`static` 通过不代表完整来源工具回归通过。需要单独在本地运行全部来源工具测试（无需 Rust）时，执行 `python3 -m unittest discover -s test/tools/rust-sources -p 'test_*.py'`；完整来源门禁仍使用下列 `check.sh rust-sources` 命令。

工作流上下文检查是针对本仓库 block-mapping 写法的定向回归，不是完整 YAML / GitHub Actions 语义验证器。它检查顶层及 job 的 `env`，拒绝尚未可用的 `runner`、`steps` 等上下文及 `hashFiles`；合法的 step env 和 action 输入仍可使用 runner 上下文。

安装工具链、WASM target、浏览器/驱动和所需原生库后，在仓库根执行其余门禁：

```sh
python3 tools/rust-sources/manage.py verify --metadata
bash .github/scripts/check.sh rust-sources
bash .github/scripts/check.sh workspace
bash .github/scripts/check.sh foundation
bash .github/scripts/check.sh web
bash .github/scripts/check.sh smb-protocol
bash .github/scripts/check.sh contracts
bash .github/scripts/check.sh build-tools
bash .github/scripts/check.sh quality
bash .github/scripts/check.sh security
bash .github/scripts/check.sh web-production
bash .github/scripts/check.sh browser-smoke
```

所有业务及构建辅助 Rust 源码通过根工作区的本地依赖提供；正式 Cargo 命令使用 `--locked --offline`。验证器使用临时空 Cargo home 核对登记目标的依赖闭包。工具链和非 Rust SDK 继续独立管理；[安全扫描器](../.github/security-tools/README.md)作为独立开发工具，在产品工作区之外按审阅锁联网安装依赖，并实时读取漏洞及 registry/yank 状态。它的独立锁和联网不参与正式业务构建，历史扫描通过也不替代本次扫描。

HTTP 上传集成测试使用真实文件系统剩余空间，并保留默认的 [1 GiB spool 恢复余量](21-configuration.md)。这是该测试配置对 spool 所在文件系统的可用余量要求，不是全项目统一的磁盘容量门槛；CI 应按实际编译产物、并行预留和测试数据规划空间。把全新 `CARGO_TARGET_DIR` 和测试临时目录同时放在容量较小的 `/tmp` tmpfs 中，可能使正常上传按设计返回 `429 / RESOURCE_LIMIT_EXCEEDED`，消息为 `HTTP spool must preserve filesystem recovery headroom`。先检查响应体和该目录的实际可用空间；不要减小生产配额、忽略失败或把所有 `429` 都归因于环境。空间不足时可将测试临时目录显式放到有充足空间的工作区文件系统，保持原有限额再重跑：

```sh
mkdir -p "$PWD/target/test-tmp"
df -h "$PWD/target/test-tmp"
TMPDIR="$PWD/target/test-tmp" bash .github/scripts/check.sh workspace
```

质量、SDK 和发布编译使用 [Cargo JSON 诊断包装器](../.github/scripts/check-rust-diagnostics.py)，保留命令参数与环境。包装器拒绝所有编译警告、错误及不完整或无效的构建流；其回归同时覆盖解析失败、Cargo 失败和本地依赖警告。质量任务与发布验证任务安装 WASM 标准库后检查目标闭包。

SMB SDK 编译不等于 Samba 模块已加载或跨客户端往返通过；Chrome 检查不等于所有浏览器和移动设备通过；模拟文件故障不等于物理盘掉电验收。依赖源码本地化也不等于漏洞或许可证审查完成，见 [第三方来源清单](THIRD_PARTY_RUST_SOURCES.md)。

工作流仅授予所需权限，checkout 不保留凭据，官方 Actions 固定完整提交 SHA。所有调用 Rust 的 job 在 checkout 后通过 [环境初始化脚本](../.github/scripts/initialize-cargo-home.sh) 从运行时 `RUNNER_TEMP` 创建全新空 Cargo home，再写入 `GITHUB_ENV` 供后续步骤使用；不在顶层或 job env 引用尚不可用的 `runner.temp`。不缓存 registry/git 源码或测试成功结果。新增组件必须加入根工作区、复用根锁文件、登记中央来源清单并增加对应检查；不得增加子工作区或活动子锁文件。
