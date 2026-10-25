# 数据与存储文档

[依赖文档入口](README.md)

序列化、编码、压缩、Schema 与 SQLite。本页包含 14 个当前实现归属、28 个上游来源身份。

先读当前维护说明和本地 API/Rustdoc，再按需查阅与来源身份对应、随源码保留的说明。这些资料可能描述已移除的平台、特性、独立包名或开发流程，不代表 ZXS 当前支持范围；合并来源不再提供独立 crate。

由 `python3 tools/rust-sources/documentation.py refresh` 生成；`verify` 检查覆盖、原件和链接目标。这里只选择使用入口，不列上游变更日志、贡献流程或发布记录。

## base64

[当前源码 / API](../../src/foundation/data-storage/base64/src/lib.rs) · [Cargo 清单](../../src/foundation/data-storage/base64/Cargo.toml)

| 上游来源身份 | 使用文档 | 许可与来源 |
| --- | --- | --- |
| base64@0.23.1 | [README.md](../../docs/upstream/base64-0.23.1/README.md) | [许可与通知](../../compliance/licenses/upstream/base64-0.23.1)；[来源](../../compliance/provenance/packages/base64-0.23.1/Cargo.toml.orig.txt) |

## compression

[当前源码 / API](../../src/foundation/data-storage/compression/src/lib.rs) · [Cargo 清单](../../src/foundation/data-storage/compression/Cargo.toml) · [当前维护说明](../../docs/architecture/compression.md)

| 上游来源身份 | 使用文档 | 许可与来源 |
| --- | --- | --- |
| flate2@1.1.10 | [README.md](../../docs/upstream/flate2-1.1.10/README.md) | [许可与通知](../../compliance/licenses/upstream/flate2-1.1.10)；[来源](../../compliance/provenance/packages/flate2-1.1.10/Cargo.toml.orig.txt) |
| adler2@2.0.1（合并来源） | [README.md](../../docs/upstream/adler2-2.0.1/README.md) | [许可与通知](../../compliance/licenses/upstream/adler2-2.0.1)；[来源](../../compliance/provenance/packages/adler2-2.0.1/Cargo.toml.orig.txt) |
| miniz_oxide@0.9.1（合并来源） | [Readme.md](../../docs/upstream/miniz_oxide-0.9.1/Readme.md) | [许可与通知](../../compliance/licenses/upstream/miniz_oxide-0.9.1)；[来源](../../compliance/provenance/packages/miniz_oxide-0.9.1/Cargo.toml.orig.txt) |
| simd-adler32@0.3.10（合并来源） | [README.md](../../docs/upstream/simd-adler32-0.3.10/README.md) | [许可与通知](../../compliance/licenses/upstream/simd-adler32-0.3.10)；[来源](../../compliance/provenance/packages/simd-adler32-0.3.10/Cargo.toml.orig.txt) |

## crc32fast

[当前源码 / API](../../src/foundation/data-storage/crc32fast/src/lib.rs) · [Cargo 清单](../../src/foundation/data-storage/crc32fast/Cargo.toml)

| 上游来源身份 | 使用文档 | 许可与来源 |
| --- | --- | --- |
| crc32fast@1.5.2 | [README.md](../../docs/upstream/crc32fast-1.5.2/README.md) | [许可与通知](../../compliance/licenses/upstream/crc32fast-1.5.2)；[来源](../../compliance/provenance/packages/crc32fast-1.5.2/Cargo.toml.orig.txt) |

## data-encoding

[当前源码 / API](../../src/foundation/data-storage/data-encoding/src/lib.rs) · [Cargo 清单](../../src/foundation/data-storage/data-encoding/Cargo.toml)

| 上游来源身份 | 使用文档 | 许可与来源 |
| --- | --- | --- |
| data-encoding@2.11.1 | [README.md](../../docs/upstream/data-encoding-2.11.1/README.md) | [许可与通知](../../compliance/licenses/upstream/data-encoding-2.11.1)；[来源](../../compliance/provenance/packages/data-encoding-2.11.1/Cargo.toml.orig.txt) |

## data-serialization

[当前源码 / API](../../src/foundation/data-storage/data-serialization/src/lib.rs) · [Cargo 清单](../../src/foundation/data-storage/data-serialization/Cargo.toml) · [当前维护说明](../../docs/components/data-serialization.md)

| 上游来源身份 | 使用文档 | 许可与来源 |
| --- | --- | --- |
| serde@1.0.229 | [README.md](../../docs/upstream/serde-1.0.229/README.md) | [许可与通知](../../compliance/licenses/upstream/serde-1.0.229)；[来源](../../compliance/provenance/packages/serde-1.0.229/Cargo.toml.orig.txt) |
| serde_core@1.0.229（合并来源） | [README.md](../../docs/upstream/serde_core-1.0.229/README.md) | [许可与通知](../../compliance/licenses/upstream/serde_core-1.0.229)；[来源](../../compliance/provenance/packages/serde_core-1.0.229/Cargo.toml.orig.txt) |
| serde_path_to_error@0.1.20（合并来源） | [README.md](../../docs/upstream/serde_path_to_error-0.1.20/README.md) | [许可与通知](../../compliance/licenses/upstream/serde_path_to_error-0.1.20)；[来源](../../compliance/provenance/packages/serde_path_to_error-0.1.20/Cargo.toml.orig.txt) |

## form-codec

[当前源码 / API](../../src/foundation/data-storage/form-codec/src/lib.rs) · [Cargo 清单](../../src/foundation/data-storage/form-codec/Cargo.toml)

| 上游来源身份 | 使用文档 | 许可与来源 |
| --- | --- | --- |
| serde_urlencoded@0.7.1 | [README.md](../../docs/upstream/serde_urlencoded-0.7.1/README.md) | [许可与通知](../../compliance/licenses/upstream/serde_urlencoded-0.7.1)；[来源](../../compliance/provenance/packages/serde_urlencoded-0.7.1/Cargo.toml.orig.txt) |

## json-codec

[当前源码 / API](../../src/foundation/data-storage/json-codec/src/lib.rs) · [Cargo 清单](../../src/foundation/data-storage/json-codec/Cargo.toml) · [当前维护说明](../../docs/components/json-codec.md)

| 上游来源身份 | 使用文档 | 许可与来源 |
| --- | --- | --- |
| serde_json@1.0.154 | [README.md](../../docs/upstream/serde_json-1.0.154/README.md) | [许可与通知](../../compliance/licenses/upstream/serde_json-1.0.154)；[来源](../../compliance/provenance/packages/serde_json-1.0.154/Cargo.toml.orig.txt) |

## leb128

[当前源码 / API](../../src/foundation/data-storage/leb128/src/lib.rs) · [Cargo 清单](../../src/foundation/data-storage/leb128/Cargo.toml)

| 上游来源身份 | 使用文档 | 许可与来源 |
| --- | --- | --- |
| leb128@0.2.7 | [README.md](../../docs/upstream/leb128-0.2.7/README.md) | [许可与通知](../../compliance/licenses/upstream/leb128-0.2.7)；[来源](../../compliance/provenance/packages/leb128-0.2.7/Cargo.toml.orig.txt) |

## leb128fmt

[当前源码 / API](../../src/foundation/data-storage/leb128fmt/src/lib.rs) · [Cargo 清单](../../src/foundation/data-storage/leb128fmt/Cargo.toml)

| 上游来源身份 | 使用文档 | 许可与来源 |
| --- | --- | --- |
| leb128fmt@0.1.0 | [README.md](../../docs/upstream/leb128fmt-0.1.0/README.md) | [许可与通知](../../compliance/licenses/upstream/leb128fmt-0.1.0)；[来源](../../compliance/provenance/packages/leb128fmt-0.1.0/Cargo.toml.orig.txt) |

## libsqlite3-sys

[当前源码 / API](../../src/foundation/data-storage/libsqlite3-sys/src/lib.rs) · [Cargo 清单](../../src/foundation/data-storage/libsqlite3-sys/Cargo.toml)

| 上游来源身份 | 使用文档 | 许可与来源 |
| --- | --- | --- |
| libsqlite3-sys@0.37.0 | [README.md](../../docs/upstream/libsqlite3-sys-0.37.0/README.md) | [许可与通知](../../compliance/licenses/upstream/libsqlite3-sys-0.37.0)、[其他许可](../../compliance/licenses/upstream/libsqlite3-sys-0.37.0/sqlcipher)；[来源](../../compliance/provenance/packages/libsqlite3-sys-0.37.0/Cargo.toml.orig.txt) |

## schema-validation

[当前源码 / API](../../src/foundation/data-storage/schema-validation/src/lib.rs) · [Cargo 清单](../../src/foundation/data-storage/schema-validation/Cargo.toml) · [当前维护说明](../../docs/architecture/schema-validation.md)

| 上游来源身份 | 使用文档 | 许可与来源 |
| --- | --- | --- |
| jsonschema@0.58.6 | [README.md](../../docs/upstream/jsonschema-0.58.6/README.md) | [许可与通知](../../compliance/licenses/upstream/jsonschema-0.58.6)；[来源](../../compliance/provenance/packages/jsonschema-0.58.6/Cargo.toml.orig.txt) |
| jsonschema-regex@0.58.6（合并来源） | 未附独立使用文档；[当前源码内 Rustdoc](../../src/foundation/data-storage/schema-validation/src/pattern/mod.rs) | [许可与通知](../../compliance/licenses/supplemental)；[来源](../../compliance/provenance/packages/jsonschema-regex-0.58.6/Cargo.toml.orig.txt) |
| jsonschema-value@0.58.6（合并来源） | [README.md](../../docs/upstream/jsonschema-value-0.58.6/README.md) | [许可与通知](../../compliance/licenses/supplemental)；[来源](../../compliance/provenance/packages/jsonschema-value-0.58.6/Cargo.toml.orig.txt) |
| referencing@0.58.6（合并来源） | [README.md](../../docs/upstream/referencing-0.58.6/README.md) | [许可与通知](../../compliance/licenses/upstream/referencing-0.58.6)；[来源](../../compliance/provenance/packages/referencing-0.58.6/Cargo.toml.orig.txt) |

## serialization-derive

[当前源码 / API](../../src/foundation/data-storage/serialization-derive/src/lib.rs) · [Cargo 清单](../../src/foundation/data-storage/serialization-derive/Cargo.toml) · [当前维护说明](../../docs/components/data-serialization.md)

| 上游来源身份 | 使用文档 | 许可与来源 |
| --- | --- | --- |
| serde_derive@1.0.229 | [README.md](../../docs/upstream/serde_derive-1.0.229/README.md) | [许可与通知](../../compliance/licenses/upstream/serde_derive-1.0.229)；[来源](../../compliance/provenance/packages/serde_derive-1.0.229/Cargo.toml.orig.txt) |

## sqlite-access

[当前源码 / API](../../src/foundation/data-storage/sqlite-access/src/lib.rs) · [Cargo 清单](../../src/foundation/data-storage/sqlite-access/Cargo.toml) · [当前维护说明](../../docs/architecture/sqlite-access.md)

| 上游来源身份 | 使用文档 | 许可与来源 |
| --- | --- | --- |
| sqlx@0.9.0 | [README.md](../../docs/upstream/sqlx-0.9.0/README.md)；[FAQ.md](../../docs/upstream/sqlx-0.9.0/FAQ.md) | [许可与通知](../../compliance/licenses/upstream/sqlx-0.9.0)；[来源](../../compliance/provenance/packages/sqlx-0.9.0/Cargo.toml.orig.txt) |
| sqlx-core@0.9.0（合并来源） | 未附独立使用文档；[当前源码内 Rustdoc](../../src/foundation/data-storage/sqlite-access/src/query_engine/mod.rs) | [许可与通知](../../compliance/licenses/upstream/sqlx-core-0.9.0)；[来源](../../compliance/provenance/packages/sqlx-core-0.9.0/Cargo.toml.orig.txt) |
| sqlx-sqlite@0.9.0（合并来源） | 未附独立使用文档；[当前源码内 Rustdoc](../../src/foundation/data-storage/sqlite-access/src/sqlite/mod.rs) | [许可与通知](../../compliance/licenses/upstream/sqlx-sqlite-0.9.0)；[来源](../../compliance/provenance/packages/sqlx-sqlite-0.9.0/Cargo.toml.orig.txt) |

## toml

[当前源码 / API](../../src/foundation/data-storage/toml/src/lib.rs) · [Cargo 清单](../../src/foundation/data-storage/toml/Cargo.toml) · [当前维护说明](../../docs/components/toml.md)

| 上游来源身份 | 使用文档 | 许可与来源 |
| --- | --- | --- |
| toml@0.8.23 | [README.md](../../docs/upstream/toml-0.8.23/README.md) | [许可与通知](../../compliance/licenses/upstream/toml-0.8.23)；[来源](../../compliance/provenance/packages/toml-0.8.23/Cargo.toml.orig.txt) |
| serde_spanned@0.6.9（合并来源） | [README.md](../../docs/upstream/serde_spanned-0.6.9/README.md) | [许可与通知](../../compliance/licenses/upstream/serde_spanned-0.6.9)；[来源](../../compliance/provenance/packages/serde_spanned-0.6.9/Cargo.toml.orig.txt) |
| toml_datetime@0.6.11（合并来源） | [README.md](../../docs/upstream/toml_datetime-0.6.11/README.md) | [许可与通知](../../compliance/licenses/upstream/toml_datetime-0.6.11)；[来源](../../compliance/provenance/packages/toml_datetime-0.6.11/Cargo.toml.orig.txt) |
| toml_edit@0.22.27（合并来源） | [README.md](../../docs/upstream/toml_edit-0.22.27/README.md) | [许可与通知](../../compliance/licenses/upstream/toml_edit-0.22.27)；[来源](../../compliance/provenance/packages/toml_edit-0.22.27/Cargo.toml.orig.txt) |
| toml_write@0.1.2（合并来源） | [README.md](../../docs/upstream/toml_write-0.1.2/README.md) | [许可与通知](../../compliance/licenses/upstream/toml_write-0.1.2)；[来源](../../compliance/provenance/packages/toml_write-0.1.2/Cargo.toml.orig.txt) |
