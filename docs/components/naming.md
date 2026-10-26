# ZXS 命名策略

`PortableName` 与 `PortablePath` 在服务器、WASM 和 CLI 使用同一实现。Unicode 数据固定为 16.0.0；不得随宿主机更新。`src/naming/src/unicode16.rs` 是数据表，`src/naming/data/` 保存对应原始输入。

表由本项目的 [Rust 生成器](../tools/unicode-build.md)重现；默认检查输入 SHA-256 与输出逐字节相等，修改必须显式执行 `--write`。

Unicode 数据来自 Unicode Consortium 的 16.0.0 UCD（https://www.unicode.org/Public/16.0.0/ucd/），适用 Unicode 数据许可：https://www.unicode.org/license.txt 。NFC 实现从 `unicode-normalization` 0.1.24 直接合入 `src/naming/src/unicode/` 私有模块，与命名策略共同编译；原始包身份、许可证和合并映射保留在[统一来源清单](../THIRD_PARTY_RUST_SOURCES.md)，不再保留独立规范化包。

内部 `unicode::nfc_casefold` 将 NFC、固定映射大小写折叠、再次 NFC 合为迭代器流水线，不创建完整中间字符串，持久化键字节形式由固定命名规则决定。纯 ASCII 键直接执行固定的 ASCII 小写映射；其他键预留最终输出容量，并对 ASCII 字符跳过 Unicode 表二分查找。组合字符序列仍按 Unicode 规范缓冲，名称和路径预算仍由本 crate 校验。`test/naming/unit/key_tests.rs` 对所有 Unicode 标量、全部折叠表项及相邻组合字符验证与原实现一致；可用 `cargo run -p name-policy --example key_profile --release --locked --offline` 重复测量运行时间。

`PortablePath::parse` 在创建或磁盘首次读入边界保留显示路径及固定 Unicode 查找键；`LookupPath::parse` 允许等价的 NFD 查询，但不用于新名称创建。字段私有，只提供只读访问；命名错误使用本 crate 的具体枚举，上层转换为既有 API 错误码。`storage-contract` 依赖这些路径类型，命名层不再依赖存储错误。命名空间提交不对未改变条目重复规范化，边界与同夹具测量见 [验证记录](../validation/parsed-namespace-paths.md)。

源文件 SHA-256：

- case-folding: `6f1f9c588eb4a5c718d9e8f93b782685e5c7fec872cf05e8e6878053599e09bb`
- unicode-data: `ff58e5823bd095166564a006e47d111130813dcf8bf234ef79fa51a870edb48f`
- derived-core-properties: `39d35161f2954497f69e08bdb9e701493f476a3d30222de20028feda36c1dabd`
