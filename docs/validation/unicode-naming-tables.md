# 命名 Unicode 表再生与裁剪验证

[生成器设计](../tools/unicode-build.md) · [命名组件](../components/naming.md)

本记录只说明下列提交上实际执行过的检查。工具链为本机可用的 Rust 1.97.0（`--ignore-rust-version`），仓库固定的 1.99.0 无法在验证环境下载；CI 仍以固定工具链为准。

## 提交顺序

| 提交 | 内容 |
| --- | --- |
| `89672fe3` | 设计：调用链、版本边界、输入与裁剪验收 |
| `ee676af9` | 在**未裁剪**实现上加入官方 `DerivedNormalizationProps-16.0.0`、`NormalizationTest-16.0.0` 与行为摘要测试；两个摘要值在该提交取得 |
| `d1f57746` | 生成器重写、来源清单、表裁剪；摘要常量未改动 |

## 输入来源

验证环境不能访问 unicode.org。两个新增文件取自 Unicode Consortium 的 `unicode-org/unicodetools` 仓库提交 `578f6c0516f5f268292ebda50b51848407a06478` 的 `unicodetools/data/ucd/16.0.0/`。同一目录下的 `UnicodeData.txt`、`DerivedCoreProperties.txt`、`CaseFolding.txt` 与仓库既有输入 SHA-256 相同（`ff58e582…`、`39d35161…`、`6f1f9c58…`），作为来源一致性的旁证；新文件的官方下载摘要仍未与 unicode.org 直接比对，记为未执行。

## 保留表逐字节相同

`unicode-build --replay` 在空目录生成新的 `tables.rs` 后，与 `ee676af9` 的 `src/naming/src/unicode/tables.rs` 比较：九个保留段各自在旧文件中恰好出现一次，除 726 字节文件头外没有其它文本。

| 段 | 字节 |
| --- | --- |
| `CANONICAL_COMBINING_CLASS_SALT`／`_KV` | 6,891／10,804 |
| `COMPOSITION_TABLE_SALT`／`_KV` | 6,841／29,758 |
| `composition_table_astral` | 2,004 |
| `CANONICAL_DECOMPOSED_CHARS`／`_SALT`／`_KV` | 55,476／15,271／60,420 |
| `qc_nfc` | 4,016 |

`qc_nfc` 由新增的 `DerivedNormalizationProps-16.0.0` 直接生成且与旧表一致，说明该输入与原上游生成时使用的数据相同。生成文件由 22,802 行降为 7,084 行；`unicode16.rs` 只新增 `UNICODE_VERSION` 一行。

## 行为不变

`test/naming/unit/golden_tests.rs` 对全部 1,112,064 个标量及 `NormalizationTest-16.0.0` 的 19,965 行 × 5 列输入，按长度分隔累计 `lookup_key`、`validate_name`、`validate_lookup_name`（含错误值及建议文本）、`is_nfc` 与 NFC 输出。摘要 `0x113385b8b69b4cfd`／`0x83966791e017e27c` 在 `ee676af9` 取得，`d1f57746` 上保持不变。另以官方向量检查 NFC 一致性及 Part 1 以外标量的不变性，覆盖规范重排、补充平面组合、Unicode 16 新增组合、完整折叠与拒绝边界。

| 命令（`d1f57746`） | 结果 |
| --- | --- |
| `cargo test --locked --offline -p name-policy` | 30 通过 |
| `cargo test --locked --offline -p unicode-build` | 7 通过 |
| `cargo run --locked --offline -p unicode-build -- --check` | PASS |
| `cargo run --locked --offline -p unicode-build -- --replay <空目录>` | PASS |
| `cargo clippy --locked --offline -p name-policy -p unicode-build --all-targets -- -D warnings` | 无诊断 |

<a id="source-accounting"></a>

## 来源会计

最终集成执行 `manage.py refresh` 时发现：`src/naming` 的原始生产文件和外置测试行不能直接删除（集成 owner 没有生产文件裁剪适配器，外置测试核算不允许缩减）。`stream_safe.rs`、`replace.rs` 及 `test/naming/unit/unicode/{__test_api.rs,stream_safe/tests.rs}` 因此恢复为原字节、未接线保留；编译图、表与公开 API 的裁剪结果不变。UCD 许可证说明改记于 `compliance/README.md`，`compliance/NOTICE.md` 是布局收据固定的证据文件，恢复原样。

后续集成审阅发现候选清单将官方 NormalizationTest 误标为 `project-authored`。
该候选记录尚未成为主线来源身份；修正采用显式 `upstream-fixture` 和固定来源收据，
不为已接受的历史身份添加改写例外。来源清单须以主线 `806c32c9` 的已接受清单为
刷新前序，在 GitHub 验证环境重新生成；修正后的外置来源测试、完整来源验证和
Unicode replay 尚须由该提交的 CI 证明，不能沿用上表的旧执行结果。
集成审阅另从收据固定的 `unicode-org/unicodetools` 原始文件 URL 重新取件，
SHA-256 为 `d811971453e7075e1ad56fb1b301eece5aa80757b81f6156e74a1bfb3ae5ceb1`，
与当前测试夹具完全一致；这仍不等于与 unicode.org 下载地址直接比对。

## 未执行或受限

- WASM 构建：环境缺少 `wasm32-unknown-unknown` 标准库，Web 包中命名表的体积变化未测量。
- 服务端、引擎等消费者的完整测试套件在最终集成时与其它任务一并执行，结果见交付报告。
- 未与 unicode.org 官方文件直接比对新增输入的摘要（见上）。
