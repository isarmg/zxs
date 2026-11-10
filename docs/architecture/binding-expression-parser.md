<a id="binding-expression-parser-consolidation"></a>

# C 绑定表达式解析器

宿主 C 绑定路径使用 `src/foundation/tooling/bindgen/src/cexpr`，`clang-sys` 保持独立 ABI 边界。该解析器不进入浏览器运行时。

<a id="reviewed-call-closure"></a>

## 调用与接口

- `clang.rs` 将 libclang token 转为 `Token { kind, raw }`；`callbacks::{Token, TokenKind}` 为 `ParseCallbacks::modify_macro` 暴露五种 token 及其拥有的字节。
- `ir/var.rs` 用 `IdentifierParser::macro_definition` 求值宏，以 `expr::expr` 处理字面 token 流，并消费 `EvalResult`、`CChar`；`ir/context.rs` 维护标识符/值映射。
- 私有切片解析支持字节标签、字符集、分支、映射、顺序、重复与识别。十进制浮点使用 `f64::from_str`；不提供通用解析器接口。

<a id="semantics-and-boundaries"></a>

## 语义与边界

整数字面先解析为 `u64`，再按位转为 `i64`；算术使用 `Wrapping<i64>`。解析保持运算符优先级、浮点/整数混合提升、位移、转义与原始字节、字符串拼接、标识符替换及函数宏声明语义。分支只对可恢复错误回溯；输入不完整是独立状态，包含所需大小和剩余输入。

数值模型为 i64/f64，右移为算术移位，忽略宽度前缀，不求值 cast、sizeof 或十六进制浮点。bindgen 的 clang 求值/回退路径负责包括 cast 与真实 ABI 宽度的独立行为。SQLite 系统头与 Samba 绑定仍使用 bindgen/clang-sys。

<a id="verification"></a>

## 验证

`python3 .github/scripts/check-binding-expressions.py` 运行不依赖 SDK 的语料与语义回归，覆盖支持运算符、回绕、提升、标识符、函数参数、转义、回调 token、非法输入、回溯及不完整传播。

`--clang` 使用真实 libclang 运行 C 头测试，位于 `foundation-data`；不依赖 SDK 的模式位于 `foundation-language`。解析器回归不能替代系统 SQLite、Samba SDK 或 ABI 验证。

第一方回归在 `test/ci/fixtures`，C 头套件在 `test/foundation/bindgen/cexpr`。cexpr 的实现来源、nom 行为来源及许可证保留于集中合规材料；保存参考文件不等于执行完整上游套件。
