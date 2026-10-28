# ZXS Unicode 表生成器

Rust 工具 `unicode-build` 从 `src/naming/data` 读取固定的 Unicode 16.0.0 原始输入，先按[来源清单](../../src/naming/data/sources.tsv)核对 SHA-256，再生成命名策略使用的全部 Unicode 表。它不访问网络，不使用宿主 Unicode 属性或区域设置。

```sh
cargo run --locked --offline -p unicode-build -- --check
cargo run --locked --offline -p unicode-build -- --replay "$(mktemp -d)"
cargo run --locked --offline -p unicode-build -- --write
```

| 模式 | 行为 |
| --- | --- |
| `--check`（默认） | 校验清单中每个输入的 SHA-256，重新生成全部输出，要求与源码树逐字节相同，且与清单登记的输出 SHA-256 相同 |
| `--replay <空目录>` | 只把清单登记的输入复制到一个空目录，在该目录内生成输出（生成器不能读取源码树的其它文件），再与源码树及清单的输出摘要逐一比较；目录非空即拒绝 |
| `--write` | 显式重写输出文件及清单中的输出摘要；输入摘要不随之改变 |

更新 Unicode 版本属于命名策略迁移，不能只改变哈希后静默覆盖。

生成器的当前摘要与不可变来源证据分开维护：官方 NormalizationTest 夹具另以
[`upstream-fixture` 导入收据](../EXTERNAL_TEST_SOURCES.md#imported-upstream-fixtures)
登记真实来源及许可，不标记为项目新作；该收据不由 `--write` 更新。

## 调用链与用途

命名 crate 只有下列 Unicode 运算进入生产路径：

| 运算 | 生产入口 | 读取的表 |
| --- | --- | --- |
| NFC（规范分解后规范组合） | `lookup_key`／`path_key` 的非 ASCII 路径、`NotNormalized` 建议文本 | 组合类 `CANONICAL_COMBINING_CLASS_*`、规范完全分解 `CANONICAL_DECOMPOSED_*`、组合表 `COMPOSITION_TABLE_*` 与 `composition_table_astral`；Hangul 按算法处理 |
| NFC 快速检查 | 创建名称时的 `validate_name`／`validate_path` | `qc_nfc` 与组合类；`Maybe` 时回退完整 NFC 比较 |
| 完整默认大小写折叠（C、F 状态） | `lookup_key`、保留名判断 | `unicode16.rs` 的 `FOLD` |
| 字母／标记／数字允许集、默认可忽略拒绝集 | `validate_name*` | `unicode16.rs` 的 `ALLOWED`、`IGNORED` |

`lookup_key = NFC(full_casefold(NFC(name)))` 的输出写入 `Entry.lookup_key`，决定持久化条目排序、`file_contexts` 映射键及内容哈希；读取时 `Entry::deserialize` 重新计算并拒绝不一致。仓库记录与历史包导入门禁保存 `PortableName`／`16.0.0`。因此表裁剪只能删除生产路径从不执行的数据与 API，任何保留表的字节都不能变化。

## 版本边界

命名使用的全部表来自 UCD 16.0.0：`unicode16.rs` 由本工具生成；`unicode/tables.rs` 原为 `unicode-normalization` 0.1.24 的 `scripts/unicode.py` 以 UCD 16.0.0 生成，现由本工具的 Rust 实现重现其保留部分。`name_policy::UNICODE_VERSION` 由生成文件提供，不再手写。

以下版本属于不同用途，**不与命名统一**：服务端列表排序使用标准库 `str::to_lowercase`（版本随固定工具链，已绑定游标摘要）；`unicode-normalizer` 等 ICU4X 基础组件使用其自身数据与 17.0.0 测试向量。

## 输入、摘要与许可

[来源清单](../../src/naming/data/sources.tsv) 是唯一权威记录：每个输入的仓库路径、SHA-256、Unicode Consortium 原始 URL，以及每个输出的 SHA-256。新增的 `DerivedNormalizationProps-16.0.0.txt` 提供 `Full_Composition_Exclusion` 与 `NFC_QC`；`NormalizationTest-16.0.0.txt` 只作为测试夹具放在 `test/naming/data/`。由于构建环境无法直接访问 unicode.org，这两个文件取自 Unicode Consortium 维护的 [unicodetools](https://github.com/unicode-org/unicodetools) 仓库 `unicodetools/data/ucd/16.0.0/`（提交 `578f6c05`）；同一目录下的 `UnicodeData.txt`、`DerivedCoreProperties.txt`、`CaseFolding.txt` 与仓库既有输入逐字节相同。数据文件适用 [Unicode License v3](../../compliance/licenses/upstream/unicode-ucd-16.0.0/LICENSE)。

<a id="trim-list"></a>

## 裁剪清单

| 项 | 生产可达 | 处理 |
| --- | --- | --- |
| 组合类、规范分解、组合表（含星芒平面）、`qc_nfc` | 是 | 保留，生成器逐字节重现 |
| 兼容分解表、`nfkd`／`nfkc`、`decompose_compatible`、`Decompositions` 的兼容分支 | 否（仅测试） | 删除 |
| 流安全表、`StreamSafe`、`quick_check` 的流安全分支、`is_nf*_stream_safe*` | 否（NFC 路径固定传入 `false`） | 表与 API 删除；`stream_safe.rs` 移出编译（见下） |
| CJK 兼容变体表、`Replacements`、`cjk_compat_variants` | 否（无任何调用，测试亦无） | 表与 API 删除；`replace.rs` 移出编译（见下） |
| 组合标记表 `is_combining_mark`、`is_public_assigned` | 否 | 删除 |
| `qc_nfkc`、`qc_nfd`、`qc_nfkd`、`is_nfkc*`、`is_nfd*`、`is_nfkd*` | 否 | 删除 |
| `nfd` 字符串方法 | 否（测试用于构造输入） | 删除；测试直接使用保留的规范分解迭代器 |

判断依据是从公开 API 与实际消费者出发的调用链，而不是行数或 `allow(dead_code)`；`unicode/mod.rs` 的 `allow(dead_code, unused_imports)` 随裁剪移除，由编译器继续报告未用项。

`src/naming` 是合入 `unicode-normalization` 的集成 owner。来源会计要求其原始生产文件要么仍在树中、要么由外置测试映射归属；生产文件裁剪适配器（`source_trimming.py`）只接受独立的 `src/foundation` owner，外置测试行也不能删除。因此 `stream_safe.rs`、`replace.rs` 以原字节保留但不再由任何 `mod` 声明编译，对应的 `test/naming/unit/unicode/stream_safe/tests.rs` 与 `__test_api.rs` 同样作为未接线材料保留；它们引用的表已删除，不计为可执行测试。

## 证明顺序与验收

1. 裁剪前在当前实现上计算行为摘要：全部 Unicode 标量的 `lookup_key`、`validate_name`／`validate_lookup_name` 结果、`is_nfc` 与 NFC 输出，加上 `NormalizationTest-16.0.0` 的全部源列及折叠上下文；摘要写入回归测试。
2. 生成器重现保留的五个表段，与裁剪前 `tables.rs` 中对应文本逐字节比较。
3. 删除不可达的表与 API。
4. 同一行为摘要保持不变；新增 `NormalizationTest-16.0.0` 的 NFC 一致性测试（独立官方数据，不与实现共享表）；覆盖组合字符重排、补充平面组合、规范等价、完整折叠及拒绝边界；既有命名与消费者测试通过。
5. `--check` 与 `--replay` 在 CI `build-tools` 中执行；生成器自身的测试位于 `test/tools/unicode-build/`。
