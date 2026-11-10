<a id="shared-form-float-formatting"></a>

# 表单浮点格式

`form-codec` 使用 `zmij` 格式化 f32/f64，与 JSON/schema 共用实现。两种类型保持各自精度，不把 f32 提升为 f64。

<a id="intentional-canonical-wire-spelling"></a>

## 规范编码

正指数包含加号：`1e+16` 编码为表单值 `1e%2B16`；f32::MAX 为 `3.4028235e%2B38`。表单字面 `+` 会解码为空格，因此百分号编码属于协议契约。负指数、有符号零与非有限值使用格式化器的规范拼写，不提供旧正指数拼写兼容层。

<a id="evidence-and-boundaries"></a>

## 验证

运行 `python3 .github/scripts/check-form-floats.py`，检查序列化、反序列化、真实原生 reqwest 表单/查询构造及 http-routing 消费，覆盖百分号编码与保持位模式的解码。表单/路由测试另编译为浏览器 WASM；此测试中的 reqwest 仅用于原生目标。依赖图要求包含 zmij、排除 ryu。

`--differential` 使用经认证的参考归档运行 `test/architecture/form-floats/differential.rs`，无需 Git 历史或网络，也不将参考实现加入产品依赖图。有限值抽样不是对全部 f64 输入的证明，WASM 编译也不是浏览器执行。

原始来源、许可证和参考实现由合规材料管理。门禁在 `foundation-language` 中登记，并由完整 `foundation` 入口调用。
