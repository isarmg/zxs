# Rust 质量与安全门禁

[文档目录](README.md) · [持续集成](ci.md) · [源码来源与许可](../compliance/README.md)

`bash .github/scripts/check.sh quality` 对根工作区中的第一方应用和工具执行格式检查及拒绝警告的 Clippy。检查按包选择，基础源码作为本地依赖参与编译，不批量格式化第三方基础实现。另对宿主机全部工作区目标（排除依赖 SDK 的 `smb-vfs`）及 WASM release 的 `web-app` 检查 Rust 编译诊断。

Clippy 和编译门禁经由 [Cargo 诊断包装器](../.github/scripts/check-rust-diagnostics.py) 读取 JSON，包括本地基础依赖的 `compiler-message`。任何 Rust warning/error、无效消息、缺少成功的 `build-finished` 或 Cargo 失败均阻止通过。包装器不改变 `RUSTFLAGS`、lint cap 或 Cargo 缓存目录；Cargo 进度和构建脚本的原有 stderr 保持可见。

产品入口为 Web 和 SMB；普通协议检查不要求 Samba SDK，独立 `smb-sdk` 任务使用固定的官方 SDK。真实 VFS Clippy、测试编译和 release 构建使用相同诊断门禁；测试先以 `--no-run` 检查纯编译 JSON，再正常执行测试，以免把测试输出当成编译消息。发布工作流的原生 release 编译和本地 Web 绑定工具也使用该门禁。

原生工作区门禁还通过 `check-native-consumer-features.py` 分别检查七个实际应用／工具消费者的生产与开发依赖图，避免统一 features 掩盖多余能力：文件系统保留 `fs`／`process`，不默认选择 `rustix/mount`；HTTP 路由保留 JSON／Query，不选择未使用的 Form 提取器。基础实现仍可在显式回归中启用这些能力，不因此删除包或宣称产物体积下降。

`bash .github/scripts/check.sh foundation` 同时纳入 CI 和 Release 验证，包含以下限定回归，并非全部上游测试：

- Linux amd64 UAPI、libc ABI、rustix 双后端 syscall，以及系统/socket/mio、停车锁、搜索/进程和随机数后端的消费者探针；另运行 `socket2`、`mio` 与 `parking_lot_core` 的 lib 测试。
- `ring` 的源码/测试材料溯源检查，以及预生成和本地生成两种模式的完整库、集成与 CAVP 测试矩阵。每种模式运行 default/std 的 debug/release 与 `std,slow_tests` release，并核对本地生成的汇编/头文件；这不是仅运行 lib 测试。
- `password-hash`、`idna_adapter` 的全 Unicode 标量测试、四个数值包，以及位集合、arena 与符号反解辅助包的限定 lib 测试。IDNA 与真实 `storage-client` 消费者一起选包，由实际依赖图启用 `compiled_data`，不向工作区之外的包直接传入特性。

不恢复已退役的上游开发依赖，也不把整个 foundation 变为工作区成员；`ring` 是显式、非默认的回归成员。探针和矩阵的实际入口以 [check.sh](../.github/scripts/check.sh) 为准，编译或本机测试不代表物理设备、其他 ABI 或 SMB 客户端验收。

`bash .github/scripts/check.sh security` 使用固定版本 `cargo-audit` 分别扫描独立扫描器锁、产品根锁和根据活动/已合并实现原始身份构造的上游身份锁。该补充审计覆盖 Cargo 常规扫描可能跳过的本地路径包，包名、版本、校验和必须与中央来源证据一致；退役及升级替换的旧身份只保留历史证据，当前后继仍接受扫描。上游身份锁不参与产品构建。

扫描器使用独立审阅的依赖锁在产品工作区之外构建，安装器核对官方源码、原始锁、精确允许的锁变化和缓存收据。扫描从临时目录使用固定隔离策略，不能继承调用者的忽略清单、严重度门槛或目标过滤。该开发工具的独立锁不增加产品锁文件；具体依赖修复、网络要求和验证边界见[安全扫描器说明](../.github/security-tools/README.md)。

已知漏洞、停止维护、被撤回版本及扫描不完整均按门禁规则失败，不维护静默忽略清单。扫描工具的版本固定，公告和 registry 状态实时读取；工具安装可追溯不代表安全结论永久不变。来源收据完整性与安全公告扫描不能代替独立安全审查或许可资料复核。

## 工程规范与架构边界

统一设计与评审要求见[第 20 章](20-engineering.md)。`python3 .github/scripts/check-architecture.py` 在 `static` 门禁核对第一方包库存、准确依赖身份及 runtime/build/dev 边界，包含目标条件和可选依赖。策略变更必须与设计、源码、测试及文档一同评审；检查通过不代表模块职责、授权、锁顺序或 unsafe 已经获得语义证明。

## 具名输入与身份边界

客户端和引擎的备份配置、放置规则与保护策略更新使用具名请求结构。仓库、路径、份数、备份前缀、预期代数、稳定范围身份、操作身份与已认证主体分别保留字段；请求类型负责表达输入，不替代鉴权、作用域检查或代际冲突处理。客户端请求不会携带服务端信任主体，服务端从认证上下文构造引擎输入。

内部文件节点发布、版本构建、历史边界插入和元数据预检同样使用具名输入。公开 Rust 方法与所有调用方同步迁移，HTTP 路径、JSON 字段、错误码及持久化格式保持原有契约。

新增包含多个身份或隔离值的方法应采用相同规则，避免连续同类型位置参数和无名元组，也不为通过参数计数检查而增加宽泛 lint 例外。具名输入不豁免正确性、所有权、内存分配、死代码或风格检查。真实 SDK 生成绑定保留必要的局部 ABI lint 处理，与手写业务 API 分开维护。

## 原生产品的可选宏边界

服务端和 SQLite 消费者显式选择追踪日志所需特性，正常产品构建不启用 `tracing-attributes` 或 `axum-macros`。追踪属性宏及 HTTP 路由宏的源码和独立能力测试仍保留；取消产品的无用依赖边，不等于退役宏包。追踪家族门禁同时核对真实原生产品依赖图和显式启用宏的独立测试。 两个宏包是非默认验证成员，不是服务端运行依赖；`axum-macros` 保留的上游 UI/文档样例需要未支持的开发依赖，因此不作为 Cargo 单元或文档测试启用，其可执行覆盖来自现有 HTTP 路由宏消费者门禁。

## 敏感依赖维护

密码学与 TLS 改动同时核对[常数时间原语](security/constant-time.md)、[证书信任](security/tls-trust.md)及[测试证书约束](security/TLS_TEST_FIXTURES.md)。编译与功能回归不构成常数时间机器码证明，也不替代证书来源或实际客户端验证。

包含断言式验收条件的维护 Python 门禁在执行前拒绝 `-O`、`-OO` 和 `PYTHONOPTIMIZE` 优化模式，不能在断言被移除后报告通过。动态回归核对当前全部这类验证入口；性能报告比较器使用显式校验，不依赖 Python 断言是否启用。

## 工作区只读视图

工作区授权和只读状态查询使用 `Engine::workspace_ref`，在现有引擎借用内检查主体和分支，避免复制完整暂存文件表。需要跨越引擎可变操作的规划仍使用拥有独立数据的 `Engine::workspace` 快照；只读视图不延长授权有效期，也不取代异步准备完成后的重新鉴权。
