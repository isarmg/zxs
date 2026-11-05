# 单仓库结构与来源维护

[文档目录](README.md) · [工程约定](20-engineering.md) · [统一许可证与溯源](../compliance/README.md)

ZXS 采用一个 Git 仓库、一个根 Cargo 工作区和一个活动锁文件。产品用户入口为 Web 与 SMB；`zxs` 直接启动守护进程，初始化与日常管理由受保护 Web/API 提供，独立 `zxs-rescue` 保留离线救援与配置草稿操作。自有应用和工具属于根工作区；基础实现通过本地路径接入同一依赖图，共同使用根锁文件、依赖解析和验证流程。

## 目录职责

| 位置 | 职责 |
| --- | --- |
| `src/{auth,contract,naming,platform,engine,server,client}` | 认证、业务契约、命名、平台能力、存储引擎、HTTP 服务与客户端 |
| `rescue` | 独立离线恢复与本机救援工具 `zxs-rescue` |
| `src/web` | 浏览器 Rust/WASM 产品界面及静态样式、字体资源 |
| `src/interface` | 主题、语言、生命周期、共享组件及直接合并的 CSR 基础原语 |
| `src/macros` | 与编译器进程边界一致的界面过程宏 |
| `src/{smb,smb-vfs}` | 独立 Samba → 本机桥接路径及 VFS 接口 |
| `src/foundation` | 在仓库内直接维护的 Rust 基础实现；活动包按职责命名；真实上游名称、版本、来源摘要及本地变更由中央溯源清单追踪 |
| `tools` | 构建、绑定生成、契约、安全、来源管理和验收工具；Rust 工具属于根工作区，脚本承担构建与验收编排 |
| `test` | 按原组件组织的单元测试、集成测试、工具回归和保留的基础库测试材料；不建立第二套产品工作区 |
| `docs` | 唯一文档目录：规范、组件说明、开发运维、验证方法、上游原始说明 |
| `compliance` | 唯一许可证和溯源目录：原始条款、版权通知、来源、迁移映射和校验收据 |
| `build/web` | 生成的正式 Web 资源；编译缓存位于 `target` |

测试实现与业务实现分离。`test/<组件>/unit` 仍通过原组件的 `cfg(test)` 模块进入同一个 Rust 模块作用域；集成测试由原 `Cargo.toml` 的显式目标注册，名称、特性条件和测试类型保持不变。只服务测试的结构字段或故障注入 hook 可继续在业务声明旁以 `cfg(test)` 保留，不为搬迁扩大生产可见性。文档示例保留在文档中，非测试的生产资源不因文件名包含 fixture 而搬迁。

Rust `Cargo.toml` 定义编译组件和目标特有依赖，不是独立仓库。源码目录没有独立 `[workspace]`、活动子锁文件或来源缓存镜像；迁移前的相关文件仅在 `compliance/provenance/` 保留为原始证据。Web、SMB 与服务端按目标选择包构建；浏览器代码不参与 Linux 原生产品默认构建。

同名多版本基础 crate 通过本地依赖接入，不在根工作区成员中重复登记；这些依赖仍由根依赖图、根锁文件和统一入口解析与构建。源代码中的每个必要编译包都受同一来源、路径和结构门禁约束。

基础源码按 Linux amd64 原生产品、WASM Web、SMB、构建工具与验证工具的五个登记闭包保留。原生产品和构建宿主仅支持 Linux amd64，当前登记和验收目标为 `x86_64-unknown-linux-gnu`；其他 Linux amd64 ABI 仅保留必要共享源码，尚未验收。浏览器产物保留 `wasm32-unknown-unknown`。移除其他平台或未使用可选实现时，同时裁剪依赖表、特性转发和实现分支；所有活动 manifest 的特性边都接受静态核对，活动源码树必须至少被一个登记闭包使用。已移除来源的最后源码摘要、原因和原始身份记入中央 `retired` 清单，不继续保留一份不参与构建的小仓库。维护步骤见 [Rust 源码治理工具](tools/rust-sources.md)。

原生服务使用根 release profile；Web 构建器在同一工作区内显式选择体积优化、fat LTO 和符号剥离，使用同一个根锁文件。生成目录携带中央许可与来源收据的分发副本，源码中的权威材料仍统一维护。

## 主要组件边界

Web 共享基础原语位于 `src/interface`，由 `web-interface` 编译。生命周期、主题、共享组件和 CSR 原语使用同一组件内部的模块；过程宏因 Rust 编译器要求保留过程宏目标，仍由根工作区统一管理。

Unicode 规范化实现位于 `src/naming`，与项目的固定映射、命名规则共同维护。原始来源身份、许可条款、原目录逐文件哈希和合并映射在 `compliance` 保留。基础实现按实际编译依赖保留组件，不通过改名或删除声明掩盖来源。

较大的服务适配模块按职责拆分：`server/engine_backend` 中的内容交付、线协议转换和授权回归各有独立模块，HTTP 边界测试与生产入口分离；`smb-vfs/module` 中的路径转换、描述符操作和测试独立维护。拆分保留业务及 ABI 边界，异步请求继续使用已有线程、所有权和清理规则。

界面过程宏通过仅用于开发验证的 `web-interface` 依赖，在实际消费者上下文中编译 Rustdoc 示例。常规 Web 测试入口同时运行单元、集成和文档测试，不再用只运行 `--tests` 的方式跳过文档。编译诊断由统一的 [质量门禁](quality.md) 核对，覆盖本地基础实现。

<a id="按职责命名"></a>

## 按职责命名

自有源码目录和内部 Cargo 包不使用项目名前缀，按实际职责命名：

| 目录 | 当前 Cargo 包 |
| --- | --- |
| `src/auth` | `authentication` |
| `src/contract` | `storage-contract` |
| `src/naming` | `name-policy` |
| `src/platform` | `platform-io` |
| `src/engine` | `storage-engine` |
| `src/server` | `storage-server` |
| `src/client` | `storage-client` |
| `rescue` | `storage-rescue`，离线救援命令为 `zxs-rescue`；正常守护进程 `zxs` 由 `src/server` 生成 |
| `src/web` | `web-app` |
| `src/interface` | `web-interface` |
| `src/macros` | `view-macros` |
| `src/smb` | `smb-protocol` |
| `src/smb-vfs` | `smb-vfs` |
| `tools/web-bindings` | `web-bindings` |

其他工具使用 `web-build`、`browser-smoke`、`contract-check`、`unicode-build`、`release-tools`、`perf-check` 等职责名称。第三方基础组件与原始归档仍保留真实上游身份；项目产品名、命令名、公开协议及持久化标记按各自用途维护。

## 文档与许可维护规则

第一方组件入口说明放于 `docs/components/`，工具说明放于 `docs/tools/`，验收说明放于 `docs/validation/`。源码中的 `include_str!` 文档引用指向这些中央材料，构建输入收据覆盖中央目录。

上游原始说明保存在 `docs/upstream/<原始包名>-<版本>/`，原始许可证放在 `compliance/licenses/`。修改当前使用文档时不覆盖上游归档字节；需要更新第三方实现时同步记录来源版本、上游提交／归档地址、校验和、本地修改和实际源码位置。维护工具比较当前文件与中央来源收据，合并前的快照用于核对改造历史。

当前统一结构不改变业务规则：Web 经 HTTPS 业务 API 接入；SMB 经 Samba → 本机桥接接入，两条路径由服务端协调同一存储核心。完整业务架构见[总体架构](03-architecture.md)。构建通过、静态来源核验、真实浏览器、实际 SMB 客户端、部署和发布应分别记录证据。
