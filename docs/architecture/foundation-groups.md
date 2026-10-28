# Foundation 职能与包边界

Foundation 按主要职能组织在 `src/foundation/`。目录分类不等于 Cargo 编译单元：同一分类可以包含多个包，包边界由依赖方向、平台与接口契约决定。

## 目录与边界

| 目录 | 职能 |
| --- | --- |
| `concurrency/` | 执行器、任务调度、线程同步、通道与 Future |
| `collections/` | 集合、迭代器、智能指针、借用与零拷贝结构 |
| `cryptography/` | 哈希、认证、安全熵与密码学基础 |
| `data-storage/` | 序列化、编码、压缩、Schema 与 SQLite |
| `networking/` | HTTP、路由、中间件、URI、套接字与 TLS |
| `platform/` | 系统调用、动态库、CPU、文件系统与系统信息 |
| `text-unicode/` | 文本检索、正则、Unicode 与区域数据 |
| `code-generation/` | Rust 语法、Token、通用宏与模板解析 |
| `tooling/` | C 绑定、原生构建、命令行、Shell 分词与终端样式 |
| `diagnostics/` | 错误、日志、调试信息与追踪 |
| `core-types/` | 数值、日期时间、标识符、标志位与基础值类型 |
| `web-runtime/` | DOM、响应式界面、视图与 WASM 绑定 |

每个包按提供的能力归类，不按调用它的业务模块归类。领域专用宏与运行时放在同组，通用宏工具放在 `code-generation`。`src/interface`、`src/naming`、`src/macros` 与 `tools/web-bindings` 各有独立职责，不随 Foundation 移动。外置测试位于 `test/foundation/<包>`。

当前包、消费者与特性闭包以自动生成的[消费者索引](foundation-consumers.md)为准，不在本文重复维护数量和包清单。声明依赖与工作区统一 features 的闭包是保守集合，不等于单一产物最终链接的依赖。

## 必须保留的边界

- 轻量接口不能默认引入完整后端：日志门面与过滤后端分离，Future/Stream 不默认引入执行器，序列化类型不默认引入全部格式。
- 过程宏在宿主运行，运行时在目标执行；原生 ABI、构建脚本与浏览器 WASM 分别验证。
- TLS 信任、密码学、共享类型、锁公平性、取消与安全状态不因同目录而合并。`unsafe` 保持模块级隔离，不放宽原先禁止它的模块。
- 包调整必须检查全部消费者、可选特性、宏生成路径、公共类型和原生链接；不能通过隐藏依赖绕过循环。
- Linux amd64 与浏览器 WASM 是不同验收目标；所需 `no_std`、`alloc` 和可选能力必须独立可用。
- 包数、活动源码量、编译时间、产物大小和运行性能是不同指标，不能互相代替。

<a id="source-maintenance-boundaries"></a>
<a id="来源与维护边界"></a>

## 来源与维护边界

当前路径、原始来源与协议/ABI 身份彼此分离，见[源码身份](source-naming.md)。完整性验证必须读取真实当前文件；不以旧目录副本、软链接或归档代替缺失的活动文件。

`compliance/rust-sources.json` 通过 `foundation_layout_receipt` 和 `foundation_external_path_receipt` 绑定布局及外部路径证明。路径编辑按精确、可逆映射核验；来源、许可证、历史收据及原测试 source 标签不可覆盖。`CARGO_MANIFEST_DIR` 指向编译包的清单目录，不是外置测试目录；嵌套引用也必须解析到真实目标。

常规修改使用 `manage.py refresh` 更新当前摘要，再执行 `verify --metadata`。独立、未重命名、未合并、未裁剪且不受固定证明保护的包支持显式退役与包内文件移动；每次操作保留前序收据，外置测试退役必须有精确清单和明确处置。根锁文件受当前清单约束，不是永久冻结的布局快照。

普通 `Cargo.toml` 维护保留当前依赖与特性内容，仅将路径反向映射给历史证明读取器；当前逻辑摘要随实际修改更新，不冒充原始清单。布局安装仍核对原收据的精确路径变换，原始来源和固定内容证明不因常规刷新而放宽。

`http-transport` 的已登记维护锚另有严格同所有者源码后继适配器，只接纳已有 Rust 源文件的内容补丁，外置测试完整快照不变；它不授权下列生命周期操作，也不表示组件升级或源码加测试的 backport 已闭环。收据、登记与拒绝边界见[源码维护链](../tools/rust-sources.md#http-owner-source-maintenance-chain)。

以下操作需要专门的后继适配器，工具未支持时必须拒绝：

- 涉及已登记布局/外部路径编辑的文件移动或退役。
- 分组后的重命名、整合、上游升级、源码裁剪及外置测试移动。
- 共享来源、链式来源或固定平台/密码学证明所有者的生命周期变更。

不得通过重算历史摘要、跳过验证或保留兼容目录继续执行。源码治理命令见[工具说明](../tools/rust-sources.md)，测试路径与未启用材料的边界见[测试布局](test-layout.md)，持续门禁见[Foundation CI](foundation-ci.md)。
