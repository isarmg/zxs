# JSON 编解码

[数据序列化](data-serialization.md) · [业务契约](../contracts/README.md) · [来源与许可](../dependencies/data-storage.md)

`json-codec` 提供 JSON 文本及 `Value` 与 `data-serialization` 数据模型之间的转换。当前维护策略要求经 `deserialize_struct` 解码的具名字段结构体及外部标记枚举的具名载荷使用 JSON 对象，不接受上游可选的按字段位置排列数组。流式反序列化、owned `Value` 与 borrowed `Value` 入口保持一致；嵌套字段、`Option`、列表及映射中的记录同样适用。

普通数组、元组、元组结构体、newtype、unit 和无类型 `Value` 不受影响。自定义反序列化以及先缓冲再匹配的内部标记枚举、untagged/flatten 类型仍需按自身协议单独验证，不能把此策略视为任意数据模型的模式验证器。重复字段、整数限制、未知字段及业务约束继续由契约层和具体类型验证。

这是本项目维护的协议收紧，不是对上游行为的漏洞判断。来源身份、原始发布材料和许可保持不变；保留的上游 `test_parse_struct` 测试明确更新本地策略预期，未接入活动测试目标的上游材料不能记为已执行。

## 验证

```sh
python3 .github/scripts/check-serialization-family.py
cargo test --locked --offline -p storage-contract
cargo test --locked --offline -p storage-server --test http_engine strict_objects
```

serialization-family 使用实际消费者验证文本、字节、reader 以及两种 `Value` 入口的拒绝行为，并回归合法对象和数组表示；原生测试与 WASM 编译分别记录。契约及 HTTP 测试另外检查嵌套请求对象、错误响应及拒绝后的状态不变。
