# WASM 文本解析测试组件

`wasm-text-parser` 源于 wast 245.0.1，只为绑定变换测试提供 core WAT → 二进制输入与自定义 directive 解析。不恢复 wat 包、rstest/libtest-mimic 框架或上游 CLI；正常依赖复用本地 encoder、bumpalo、leb128fmt、memchr，诊断使用 `unicode-display-width`。

所有真实消费者均需要 core 模块能力，因此它是组件本体，不声明可选特性或无模块构建。Wast 脚本/断言 AST（`wast.rs` 与 `core/wast.rs`）、component-model 解析实现、DWARF/gimli 不纳入活动源码，原件完整保存在中央发布档案。保留最小 component 语法拒绝边界，不提供该功能的可选开关。

## 当前测试与原例处置

测试统一位于 `test/foundation/wasm-text-parser`：15 个词法/token/指令尺寸单元测试，4 个 annotations、1 个 comments、原 recursive 栈上限例保留；直接使用真实 Wat 解析，不经薄壳，也不依赖 Wast 脚本层。原 55 份 parse-fail 中，53 份 core/词法输入及独立 `.err` 原字节由固定清单普通 harness 执行；`bad-core-func-alias`、`bad-func-alias` 两对属于 component-only，移出当前测试而保留于原始发布档案，不再宣称 55 例全套执行。无 BLESS 写回，诊断路径标签保留原上游来源身份，真实查找路径固定到根 test。

新增5个 Unicode 诊断回归与 component 文本拒绝例。原 DWARF 中五个测试归档未接入。18 个 Rustdoc 示例均实际编译，不用特性条件将示例替换为空体。入口存在不代表执行，结果以当前验证证据为准。

## 本地修正

原诊断先展开 tab/移除方向控制字符，再误用原字节列切渲染结果；独立回归证明 `\t界?` 的 `?` 原报第5列而非第7列，`a\u{202e}界?` 原报第8列而非第4列。当前在原始字符边界记录渲染前缀，再用真实 Unicode 显示宽度计算列；合法行尾与公开任意 span 的数字回退有回归。时间仍为 O(n)，仅在目标边界计算一次前缀宽度。

Rust 1.99 的简单 lint 按等价写法修正，不恢复上游全局 Clippy allow。宏生成借用和固定 core 语法表等必要形状只用窄位置具名 expect，并说明原因。

## core-only 边界收口

最终复核实际构造旧 `Wat::Component(Component { span, id: None })` 后调用 encode，
证明其可达 `unreachable!`。当前 `Wat` 仅包装私有 core Module，`(component)` 在解析时
明确返回不支持错误，不再公开可构造的假 Component，也不保留单变体 enum。

`component_disabled.rs` 与 `core/binary/dwarf_disabled.rs` 从当前树移除，原文件仍在
不可变 wast 发布归档。空 `EncodeOptions`、恒为 None 的 DWARF 参数/hook、无调用方的
指令 span 跟踪配置及 142 个无当前引用的 keyword 声明一并删除；当前 span 错误定位、
branch-hint 编码与 core 指令字符串解析仍保留。对应变化位于原 `lib.rs`、`wat.rs`、
`parser.rs`、`core.rs`、`core/{binary,expr,module}.rs`，不改写初始导入计划的历史投影。
