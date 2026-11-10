# 错误定义组件族

error-definition 与 error-definition-macros 分别维护错误运行时和派生实现，原始来源为 thiserror 与 thiserror-impl 2.0.21。原始版本、许可证、版权及历史搬迁收据保留在 compliance/provenance 与 docs/upstream，不改写上游身份。

## 当前接口与版本约束

活动 Rust 导入使用 error_definition，生成代码使用稳定的隐藏支持模块 __derive_support，不再把发布版本拼接进标识。错误文本、Display 格式、source 链和 backtrace 语义保持不变。

运行时精确固定派生实现的完整版本。foundation 门禁拒绝运行时与宏版本不一致、非精确依赖、旧别名和非本地实现路径。该私有接口不能独立升级；Cargo 的必要版本元数据与上游来源版本仍然保留。

## 测试与来源

维护门禁执行原有派生回归、原生及 WASM no_std、文档示例，并显式覆盖 generic-member/backtrace 场景；实际验证补丁、次版本和主版本偏移都会被拒绝。默认产品构建不因此启用不稳定编译器能力。

外置测试保留原始 source 标签，当前目标目录统一为 test/foundation/error-definition。现有合并来源的主包测试归属、历史改名/搬迁/退役记录保持不变。命名改动与来源验证由[统一命名规范](../NAMING.md)和当前来源清单管理。
