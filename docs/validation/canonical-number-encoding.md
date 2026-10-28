# 规范 JSON 整数写出回归与测量

后续对象键排序捷径及其 Map 特性矩阵见[编码边界回归](encoding-boundaries.md)；
本文保留原整数写出测量，不能把其数字用于后续对象排序变化。

[`baseline.rs`](../../test/contract/unit/encoding/baseline.rs) 冻结 `23dd3639` 的完整 emitter，并保留 `integer_json::validate`，仅用于测试。
[`number_emission.rs`](../../test/contract/unit/encoding/number_emission.rs) 对完整公开编码路径做差分，另有固定 UTF-16 键序、控制字符、
安全整数上下界及拒绝错误的断言。原有 `vectors.json`、字节和哈希向量不修改。

生产变化仅将已通过原检查的 `Number::to_string` 改为 `json_codec::to_writer`。
两者的整数格式化均使用本地 `itoa::Buffer`；写出版本去掉每个整数的临时 `String`
分配和释放。整数范围检查、浮点拒绝、字符串转义、对象域和公开调用方验证均保留。
UTF-16 键比较仍为原来的惰性迭代：每次比较会解码访问到的前缀，但不分配。
预存每个键的 UTF-16 会引入额外分配，本改动不推测它更快，也不改键序。
总体遍历/排序复杂度和同步预算不变；没有新生产计数器、测试 hook 或 unsafe。

## 复现

使用仓库固定工具链，独占 target，`CARGO_BUILD_JOBS=2`、
`CARGO_INCREMENTAL=0`、`CARGO_PROFILE_RELEASE_DEBUG=0`：

```sh
cargo test -p storage-contract --release --locked --offline --target-dir target/canonical
cargo clippy -p storage-contract --release --tests --locked --offline \
  --target-dir target/canonical -- -D warnings
ZXS_CANONICAL_FIXTURES=target/canonical/fixtures.json \
  cargo test -p storage-contract --release --locked --offline --target-dir target/canonical \
  measure_complete_canonical_encoder -- --ignored --nocapture --test-threads=1
```

基准包含三个发布向量、4096 整数的数组、256 个混合 Unicode 子对象；构建 fixture
不计时。比较完整 `canonical_json_value`（验证、遍历、排序、转义、分配及写出），
先预热、每个 fixture 12 个交替 AB 样本；不是排序微基准，也不是存储端到端测试。

2026-10-26，Rust 1.99.0、release thin-LTO，协调暂停本任务其他 Cargo 重载后，
共享执行器无绑核测量（不声称系统独占），两组中位每次耗时：

| fixture | 基线 ns | 直接写出 ns |
| --- | ---: | ---: |
| version-jcs | 1011.6 | 987.2 |
| request-jcs | 1271.6 | 1177.0 |
| utf16-key-order | 193.7 | 189.3 |
| integer-array | 137010.7 | 65733.5 |
| mixed-unicode-object | 99640.0 | 85979.1 |

整数密集和混合合成样例分别减少约 52.0% / 13.7% 编码耗时；无数字的 UTF-16
对照也出现约 2.3% 波动，因此不把小样例变化解释为稳定收益，不外推生产吞吐。
原始 CSV 行、精确 fixture JSON、摘要、源码/工具链溯源和 SHA-256 校验清单位于
[版本管理中的证据目录](../../tools/perf-check/results/canonical-number-encoding/)。
计时后仅为通过 clippy 将测试函数指针类型提为局部别名；溯源附最小补丁以复原计时源码。
二进制不入仓，身份哈希在 provenance 中记录。12 对样本中，整数数组每对改善
50.3–52.9%，混合对象每对改善9.6–18.6%；三个小样例的配对变化均跨过零。
