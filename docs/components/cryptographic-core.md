<a id="cryptographic-core"></a>

# 密码学核心

`cryptographic-core` 是 no_std 共享接口库，包含 `common_types`、哈希/MAC trait 及可选 `buffer` 模块。BLAKE2、Argon2、SHA-2、HMAC 和可选 BLAKE3 trait 集成使用同一套接口；算法实现及共享数组 `hybrid-array` 各自独立。

## 能力与安全边界

- `alloc`、`block-api`、`mac`、`oid` 和 `zeroize` 按需启用。
- `zeroize` 提供缓冲及 hybrid-array 清零能力，不增加 std 依赖，也不使数组在析构时自动清零。
- `dev` 提供固定哈希、XOF、MAC 和序列化测试辅助，不依赖外部二进制测试数据解析器。
- 不提供 `rand_core`、`getrandom`、`blobby` 再导出或旧版二进制加载/运行时输出序列化宏。
- 根级拒绝 unsafe；哈希及公共类型模块禁止 unsafe，仅缓冲模块允许其原有 unsafe 操作。

<a id="blake2-parameter-limits"></a>

## BLAKE2 接口与参数限制

BLAKE2 使用 `blake2::VariableOutput` 和 `VariableOutputReset` 提供运行时输出长度，包括 Argon2 H' 所需能力。固定输出缓冲器 `BufferedHash` 实现 `CoreProxy`。可选重置、分配输出、常量时间 MAC 验证与 `std::io::Write` 均保留；非法输出长度的重置调用不消耗原状态。

`new_with_salt_and_personal` 对超长 key、salt 或 persona 返回 `InvalidLength`：BLAKE2b 密钥最多 64 字节、salt/persona 各 16 字节；BLAKE2s 分别为 32、8、8 字节。上限不随截断输出长度缩小。返回 `Self` 的底层 `new_with_params` 使用断言，调用方必须自行满足约束。

空密钥不注入密钥块，其结果与相同参数的无密钥哈希一致。重置保留有无密钥块的区别；非空全零密钥仍按有密钥模式处理。`common_types::hazmat` 的 `u128` 数组状态按每个元素 16 字节的小端表示序列化。

Argon2 在计算每条 lane 的内存下限前验证并行度上限，超范围返回 `ThreadsTooMany`。调用者提供的 `AsMut<[Block]>` 只转换一次，同一内存借用贯穿填充与最终输出。

<a id="maintained-boundaries"></a>

## 检查

运行 `python3 .github/scripts/check-crypto-consolidation.py`。门禁使用仓库外临时 Cargo 工作区及本地、锁定、离线依赖，检查：

- Linux amd64/WASM no_std、最小特性组合及 `--all-features`，拒绝重复密码学 trait 提供方和 generic-array 依赖。
- BLAKE2 独立向量、有效输出长度、分块、密钥/个性化、参数拒绝和重置；Argon2 KAT/PHC 及 SHA-2、HMAC、BLAKE3 消费者。
- 原始缓冲测试、哈希序列化向量及导出宏、显式缓冲/数组清零、编译器强制的 unsafe 边界。

向量生成器为 `test/ci/crypto_vectors.py`，独立计算预期值且不修改上游数据。忽略的大内存 Argon2 用例不计作执行；nightly SIMD 的 `platform_intrinsics` 源码不代表当前稳定工具链支持。

## 来源与许可

主来源为 RustCrypto digest 0.11.3，合入来源为 block-buffer 0.12.1 与 crypto-common 0.2.2。原始源码和测试清单保存在 `compliance/cryptographic-core-source-archive.json.gz`，MIT/Apache-2.0 许可材料位于 `compliance/licenses/upstream`。BLAKE2 中的 digest 0.10.7 适配器另有独立来源身份，精确对应关系见[BLAKE2 适配器来源](crypto-consolidation.md)。
