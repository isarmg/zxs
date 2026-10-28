# ZXS 命名策略

`PortableName` 与 `PortablePath` 在服务器、WASM 和 CLI 使用同一实现。Unicode 数据固定为 16.0.0；不得随宿主机更新。`src/naming/src/unicode16.rs`（允许集、拒绝集、大小写折叠及 `UNICODE_VERSION`）与 `src/naming/src/unicode/tables.rs`（NFC 所需的组合类、规范分解、组合与快速检查）都是生成表，`src/naming/data/` 保存对应原始输入，[`sources.tsv`](../../src/naming/data/sources.tsv) 登记版本、输入／测试夹具／输出的 SHA-256 与官方来源 URL。

两张表都由本项目的 [Rust 生成器](../tools/unicode-build.md)重现；默认检查输入 SHA-256、输出逐字节相等及输出摘要，`--replay` 在空目录中只用登记输入重放，修改必须显式执行 `--write`。`name_policy::UNICODE_VERSION` 取自生成表，与仓库记录和历史包导入门禁使用的值相同。

Unicode 数据来自 Unicode Consortium 的 16.0.0 UCD（https://www.unicode.org/Public/16.0.0/ucd/），许可证全文见 [Unicode License v3](../../compliance/licenses/upstream/unicode-ucd-16.0.0/LICENSE)。NFC 实现从 `unicode-normalization` 0.1.24 直接合入 `src/naming/src/unicode/` 私有模块，与命名策略共同编译；只保留命名键与校验实际执行的规范分解、规范组合与 NFC 快速检查，兼容分解、流安全、CJK 兼容变体、组合标记及其它规范化形式的表和 API 已按调用链删除（见[生成器说明](../tools/unicode-build.md#trim-list)）。原始包身份、许可证和合并映射保留在[统一来源清单](../THIRD_PARTY_RUST_SOURCES.md)，不再保留独立规范化包。

内部 `unicode::nfc_casefold` 将 NFC、固定映射大小写折叠、再次 NFC 合为迭代器流水线，不创建完整中间字符串，持久化键字节形式由固定命名规则决定。纯 ASCII 键直接执行固定的 ASCII 小写映射；其他键预留最终输出容量，并对 ASCII 字符跳过 Unicode 表二分查找。组合字符序列仍按 Unicode 规范缓冲，名称和路径预算仍由本 crate 校验。`test/naming/unit/key_tests.rs` 对所有 Unicode 标量、全部折叠表项及相邻组合字符验证与原实现一致；`golden_tests.rs` 以裁剪前实现取得的行为摘要锁定全部标量及 `NormalizationTest-16.0.0` 输入的查找键、校验结果、NFC 快速检查与 NFC 输出，并用官方向量独立检查 NFC 一致性；可用 `cargo run -p name-policy --example key_profile --release --locked --offline` 重复测量运行时间。

`PortablePath::parse` 在创建或磁盘首次读入边界保留显示路径及固定 Unicode 查找键；`LookupPath::parse` 允许等价的 NFD 查询，但不用于新名称创建。字段私有，只提供只读访问；命名错误使用本 crate 的具体枚举，上层转换为既有 API 错误码。`storage-contract` 依赖这些路径类型，命名层不再依赖存储错误。命名空间提交不对未改变条目重复规范化，边界与同夹具测量见 [验证记录](../validation/parsed-namespace-paths.md)。

源文件、测试夹具与生成输出的当前 SHA-256 在 [`sources.tsv`](../../src/naming/data/sources.tsv) 维护，生成器与 CI 以它为准。官方测试夹具另有[不可变导入收据](../../compliance/fixtures/unicode-ucd-16.0.0-normalization.json)保存原始来源快照，不随生成器更新，也不冒充本项目创作。
