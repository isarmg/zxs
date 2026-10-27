# 业务源码与测试分离

业务与基础实现保留在 `src/`；构建和治理工具保留在 `tools/`。可独立维护的测试实现、固定夹具和验证脚本统一放在仓库根 `test/`，不建立第二个 Cargo 工作区。

| 位置 | 职责 |
| --- | --- |
| `test/<组件>/unit/` | 原组件内部的单元测试，仍由原模块的 `cfg(test)` 入口编译 |
| `test/<组件>/integration/` | 业务集成测试，通过原 Cargo 包的显式测试目标运行 |
| `test/tools/<工具>/` | 工具单元、集成测试及其夹具 |
| `test/tools/browser-smoke/runner/` | 实际浏览器验收场景和运行入口，原工具 manifest 通过显式 binary 目标接线 |
| `test/ci/` | CI 脚本回归和独立消费者夹具 |
| `test/foundation/<包>/` | 对应上游身份的当前维护测试、语料与单元片段 |
| `test/evidence/` | 历史检查脚本与受保护的参考实现；不是当前浏览器验收的替代品 |

## 模块与执行边界

- 单元测试通过 `#[path]` 或编译期 `include!` 接回原模块，保留私有访问、模块名称、特性条件和测试名称；不为迁移新增公开生产 API。
- 集成测试仍属于原包，保留 `required-features`、harness 和目标名称。`CARGO_MANIFEST_DIR` 继续表示原包目录。
- `cfg(test)` 模块接线与业务函数中的测试 hook 分别判断。测试实现、差分断言和临时计数器放在根 `test/`；必要 I/O 故障注入显式隔离并使用类型化注入点，不污染正常业务契约。既有 hook 整改须先保留对应测试能力，不能以目录整理为由直接删除覆盖；完整规则见[编码规范](../../禁止防御性编程.md)。
- Rustdoc 示例仍在文档中。生产资源、生产生成宏和已有导出的下游代码生成 API 不因名字中含 `test` 而移动或删除。
- 未具备开发依赖、原本未启用的上游测试不因目录变化自动成为活动测试目标，也不能记为通过。

### Server 的 core 特性边界

`storage-server` 的 HTTP / Backend 契约库支持不启用 core 的构建；这些库代码及其可用单元测试不因集成测试依赖核心而被排除。四个实际核心集成目标 `http_engine`、`login_admission`、`telemetry_status`、`service_startup` 分别调用 EngineBackend 或启动需要 core 的守护进程，统一在 Cargo 的 `required-features = ["core"]` 声明准入。默认 core 模式继续运行这些目标；不用测试文件级 cfg 把不满足条件的目标伪装成零测试通过。

仅 core 测试调用的 BackendAction 全变体清单、树请求夹具和 SMB 记录同步故障注入按同一 core 条件接线；不把无 core 仍可运行的命名空间记录、HTTP parser 或授权 mock 测试一起屏蔽，也不用 allow 隐藏空目标中的未使用项。

workspace 门禁同时检查无默认特性的 all-targets 图，然后执行默认原生测试。前者验证可用目标的类型与接线，不代表核心集成测试或无 core 单元测试已经执行；运行结果仍需单列。

### 运行时夹具路径回归

`test/ci/test_manifest_runtime_paths.py` 按当前来源目录和根工作区的实际 manifest 所有权检查显式 `[[test]]`、`[[bench]]` 入口，并与 `test/ci/fixtures/manifest-runtime-paths.json` 中已审阅的有限清单比对。辅助检查只识别基于 `CARGO_MANIFEST_DIR` 的静态 `concat!` 字符串和 `Path::new`／`PathBuf::from` 后接字面量 `.join()` 两种形式；不分析嵌套模块、隐式目标或动态 Rust 表达式，也不代替编译与运行验收。

模式验证组件另检查真实包及隔离镜像均能找到 canonical-suite 的 31 份 JSON、1509 个用例，并保留 Rust 测试内的相同计数断言。目录和计数检查通过不表示这些用例已执行；实际结果仍由 `check-schema-validation.py` 报告。

## 来源与完整性

`compliance/external-tests/` 将外置材料关联回原来源目录，并记录精确文件集合和当前哈希。包内源码与包外测试共同构成当前来源覆盖；移动测试不是减少源码或降低运行内存。

初次迁移使用单独固定的可逆证明，将当前源码和外置片段重建为迁移前哈希对应的视图。ring 和平台历史验证也通过认证映射继续核对原始证据，既有历史收据、上游原文和许可证不被改写。以后的已审阅代码修改仍通过正常 refresh 更新当前哈希；这与不可变的初始迁移证明分开。

根 `test/` 同样受路径、符号链接、意外 Cargo manifest、文档和许可证集中管理规则检查。新增测试必须接入实际测试入口，不能依靠目录名暗示它会执行。

## 维护检查

调整测试布局时需要比较前后 Cargo 目标身份、测试名称或定义数、Python/Node 发现范围、夹具字节及来源覆盖，并运行对应测试。构建成功或零测试套件通过不能替代这些比较。真实浏览器、Samba SDK 和环境受限用例的通过状态分别记录。
