<a id="unified-data-serialization"></a>

# 数据序列化

`data-serialization` 提供序列化 trait、标准库/core 类型实现、派生支持及可选结构化错误路径。`serialization-derive` 是独立过程宏 crate，固定到匹配的私有支持版本。

## 数据模型与格式分工

`data_serialization::{Serialize, Deserialize}` 只定义数据模型与转换契约，不负责 JSON/TOML 文本本身。JSON 使用 `json_codec::{to_string, to_vec, from_str, from_slice}`；TOML 和 URL 表单分别由 `toml`、`form-codec` 处理，协议层共享同一套 trait。

根工作区已为 `data-serialization` 开启 `derive`。独立消费者需要自己选择该特性；不使用派生时可以手写 trait 实现。此仓库派生属性名是 `serialization`，不是上游 `serde`：

```rust
use data_serialization::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, PartialEq)]
#[serialization(rename_all = "camelCase")]
struct Item {
    item_id: String,
    #[serialization(default)]
    enabled: bool,
}

fn round_trip() -> Result<(), json_codec::Error> {
    let item: Item = json_codec::from_str(r#"{"itemId":"doc-1"}"#)?;
    assert!(!item.enabled);
    let encoded = json_codec::to_string(&item)?;
    assert_eq!(encoded, r#"{"itemId":"doc-1","enabled":false}"#);
    assert_eq!(json_codec::from_str::<Item>(&encoded)?, item);
    Ok(())
}
```

`rename` / `rename_all` 控制线上的字段名，不需要把 Rust 字段也改名。`default`、`skip`、`flatten`、`with` 等属性同样写作 `#[serialization(...)]`。当依赖通过别名或自定义 re-export 暴露时，用 `#[serialization(crate = "crate::my_serialization")]` 明确宏生成代码引用的路径；不要手写或依赖 `__derive_support` 私有 ABI。

借用反序列化可通过 `Deserialize<'de>` 保留输入切片引用，前提是输入活得足够久且所用格式支持借用。`DeserializeOwned` 要求结果不依赖输入缓冲，适用于读取后就释放输入或 HTTP 提取器；它不是 `Deserialize<'static>` 的同义词。

## ZXS 严格可空字段与空变体

契约层通过同一 `required_nullable` 反序列化函数保留“字段必须存在，但值可以为 null”的约束。各 DTO 原有的 `deserialize_with` 标记仍在原字段上；共享实现只调用目标 `Option<T>` 的解析，不把字段缺失转换为 null，也不改变目标类型错误或序列化字节。

内部标签枚举的无载荷变体共用 `empty_variant`，以拒绝未知字段的空结构解析标签外的载荷。历史集合、独立节点观察及未准入登记继续拒绝夹带额外状态；不能因枚举顶层已有 `deny_unknown_fields` 就省略此边界。有效标签和表示不变，各业务身份、预算与状态关系验证仍由原模块维护。

## 带路径的解码错误

启用 `error-path` 后，用格式自己的 deserializer 包装成结构化错误。它补充字段/数组路径，不改变格式解析器的原始错误，也不自动确保输入已全部消费：

```rust
fn decode_item(input: &str) -> Result<Item, String> {
    let mut decoder = json_codec::Deserializer::from_str(input);
    let item = data_serialization::error_path::deserialize(&mut decoder)
        .map_err(|error| format!("{}: {}", error.path(), error.inner()))?;
    decoder.end().map_err(|error| error.to_string())?;
    Ok(item)
}
```

此例与上面的 `Item` 一起使用。`decoder.end()` 拒绝合法值之后的多余内容；若只需要完整 JSON 解码而不要求路径，直接使用 `json_codec::from_str`。面向用户的错误信息是否包含输入值、内部字段或数据库细节，应由应用错误边界决定。

<a id="capabilities"></a>

## 能力

默认仅启用 `std`，不包含派生或格式实现；下面的最小集合指显式关闭默认特性。

- 无默认特性：无分配 trait 和 core 类型，不引入过程宏、JSON 或整数格式化。
- `alloc`：自持有集合、字符串及缓冲派生值。
- `std`：标准库类型，包含 `alloc`。
- `rc`：在可分配时提供 `Rc`/`Arc` 实现。
- `derive`：可选宏再导出，手写 trait 实现无需开启。
- `error-path`：仅需 alloc 的 `data_serialization::error_path`；显示路径时启用可选整数格式化器。

支持项目的 Rust 1.99 工具链、Linux amd64 和浏览器 WASM。派生输出与运行时使用同一 `__derive_support` 命名空间。JSON/TOML 协议标记、有符号/无符号整数语义、Result、借用反序列化及自定义派生 crate 路径均为维护契约。不提供旧编译器兼容探针、空操作兼容宏或不受支持的 nightly 特性。

<a id="sources-and-checks"></a>

## 检查与来源

实现位于 `src/foundation/data-storage/data-serialization`，测试位于 `test/foundation/data-serialization`。运行：

```sh
python3 .github/scripts/check-serialization-family.py
python3 .github/scripts/check-serialization-consolidation.py
```

门禁检查原生/WASM 特性配置、依赖隔离及缺失能力拒绝，并测试整数、Result、派生、Content、借用、错误路径、日志序列化、格式化和文档示例。源码完整性与行为测试分别验证，不互相替代。

来源为 [serde 1.0.229](../upstream/serde-1.0.229/README.md)、[serde_core 1.0.229](../upstream/serde_core-1.0.229/README.md) 和 [serde_path_to_error 0.1.20](../upstream/serde_path_to_error-0.1.20/README.md)；JSON 对应 [serde_json 1.0.154](../upstream/serde_json-1.0.154/README.md)。阅读原始示例时按本页替换 crate 路径与派生属性；原始身份、许可证、文档及内容寻址凭据保存在 `compliance/provenance`、`docs/upstream` 和来源清单中。
