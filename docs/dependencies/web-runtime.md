# 浏览器与 WASM文档

[依赖文档入口](README.md)

DOM、响应式界面、视图与 WASM 绑定。本页包含 18 个当前实现归属、19 个上游来源身份。

先读当前维护说明和本地 API/Rustdoc，再按需查阅与来源身份对应、随源码保留的说明。这些资料可能描述已移除的平台、特性、独立包名或开发流程，不代表 ZXS 当前支持范围；合并来源不再提供独立 crate。

由 `python3 tools/rust-sources/documentation.py refresh` 生成；`verify` 检查覆盖、原件和链接目标。这里只选择使用入口，不列上游变更日志、贡献流程或发布记录。

## dom-helpers

[当前源码 / API](../../src/foundation/web-runtime/dom-helpers/src/lib.rs) · [Cargo 清单](../../src/foundation/web-runtime/dom-helpers/Cargo.toml) · [当前维护说明](../../docs/components/interface.md)

| 上游来源身份 | 使用文档 | 许可与来源 |
| --- | --- | --- |
| leptos_dom@0.8.8 | 未附独立使用文档；[当前源码内 Rustdoc](../../src/foundation/web-runtime/dom-helpers/src/lib.rs) | [许可与通知](../../compliance/licenses/shared/vendor/web/dependencies)；[来源](../../compliance/provenance/packages/leptos_dom-0.8.8/Cargo.toml.orig.txt) |

## gloo-events

[当前源码 / API](../../src/foundation/web-runtime/gloo-events/src/lib.rs) · [Cargo 清单](../../src/foundation/web-runtime/gloo-events/Cargo.toml)

| 上游来源身份 | 使用文档 | 许可与来源 |
| --- | --- | --- |
| gloo-events@0.2.0 | [README.md](../../docs/upstream/gloo-events-0.2.0/README.md) | [许可与通知](../../compliance/licenses/supplemental)；[来源](../../compliance/provenance/packages/gloo-events-0.2.0/Cargo.toml.orig.txt) |

## gloo-timers

[当前源码 / API](../../src/foundation/web-runtime/gloo-timers/src/lib.rs) · [Cargo 清单](../../src/foundation/web-runtime/gloo-timers/Cargo.toml)

| 上游来源身份 | 使用文档 | 许可与来源 |
| --- | --- | --- |
| gloo-timers@0.3.0 | [README.md](../../docs/upstream/gloo-timers-0.3.0/README.md) | [许可与通知](../../compliance/licenses/supplemental)；[来源](../../compliance/provenance/packages/gloo-timers-0.3.0/Cargo.toml.orig.txt) |

## js-sys

[当前源码 / API](../../src/foundation/web-runtime/js-sys/src/lib.rs) · [Cargo 清单](../../src/foundation/web-runtime/js-sys/Cargo.toml)

| 上游来源身份 | 使用文档 | 许可与来源 |
| --- | --- | --- |
| js-sys@0.3.106 | [README.md](../../docs/upstream/js-sys-0.3.106/README.md) | [许可与通知](../../compliance/licenses/upstream/js-sys-0.3.106)；[来源](../../compliance/provenance/packages/js-sys-0.3.106/Cargo.toml.orig.txt) |

## reactive-state

[当前源码 / API](../../src/foundation/web-runtime/reactive-state/src/lib.rs) · [Cargo 清单](../../src/foundation/web-runtime/reactive-state/Cargo.toml) · [当前维护说明](../../docs/architecture/reactive-browser-runtime.md)

| 上游来源身份 | 使用文档 | 许可与来源 |
| --- | --- | --- |
| reactive_graph@0.2.16 | [README.md](../../docs/upstream/reactive_graph-0.2.16/README.md) | [许可与通知](../../compliance/licenses/shared/vendor/web/dependencies)；[来源](../../compliance/provenance/packages/reactive_graph-0.2.16/Cargo.toml.orig.txt) |

## view-alternatives

[当前源码 / API](../../src/foundation/web-runtime/view-alternatives/src/lib.rs) · [Cargo 清单](../../src/foundation/web-runtime/view-alternatives/Cargo.toml)

| 上游来源身份 | 使用文档 | 许可与来源 |
| --- | --- | --- |
| either_of@0.1.9 | [README.md](../../docs/upstream/either_of-0.1.9/README.md) | [许可与通知](../../compliance/licenses/shared/vendor/web/dependencies)；[来源](../../compliance/provenance/packages/either_of-0.1.9/Cargo.toml.orig.txt) |

## view-renderer

[当前源码 / API](../../src/foundation/web-runtime/view-renderer/src/lib.rs) · [Cargo 清单](../../src/foundation/web-runtime/view-renderer/Cargo.toml) · [当前维护说明](../../docs/architecture/reactive-browser-runtime.md)

| 上游来源身份 | 使用文档 | 许可与来源 |
| --- | --- | --- |
| tachys@0.2.19 | [README.md](../../docs/upstream/tachys-0.2.19/README.md) | [许可与通知](../../compliance/licenses/shared/vendor/web/dependencies)；[来源](../../compliance/provenance/packages/tachys-0.2.19/Cargo.toml.orig.txt) |

## walrus

[当前源码 / API](../../src/foundation/web-runtime/walrus/src/lib.rs) · [Cargo 清单](../../src/foundation/web-runtime/walrus/Cargo.toml)

| 上游来源身份 | 使用文档 | 许可与来源 |
| --- | --- | --- |
| walrus@0.27.2 | [README.md](../../docs/upstream/walrus-0.27.2/README.md) | [许可与通知](../../compliance/licenses/upstream/walrus-0.27.2)；[来源](../../compliance/provenance/packages/walrus-0.27.2/Cargo.toml.orig.txt) |

## walrus-macro

[当前源码 / API](../../src/foundation/web-runtime/walrus-macro/src/lib.rs) · [Cargo 清单](../../src/foundation/web-runtime/walrus-macro/Cargo.toml)

| 上游来源身份 | 使用文档 | 许可与来源 |
| --- | --- | --- |
| walrus-macro@0.26.0 | 未附独立使用文档；[当前源码内 Rustdoc](../../src/foundation/web-runtime/walrus-macro/src/lib.rs)（含生成类型的注释）；配套 [walrus 使用文档](../../docs/upstream/walrus-0.27.2/README.md) | [许可与通知](../../compliance/licenses/upstream/walrus-macro-0.26.0)；[来源](../../compliance/provenance/packages/walrus-macro-0.26.0/Cargo.toml.orig.txt) |

## wasm-bindgen

[当前源码 / API](../../src/foundation/web-runtime/wasm-bindgen/src/lib.rs) · [Cargo 清单](../../src/foundation/web-runtime/wasm-bindgen/Cargo.toml)

| 上游来源身份 | 使用文档 | 许可与来源 |
| --- | --- | --- |
| wasm-bindgen@0.2.129 | [README.md](../../docs/upstream/wasm-bindgen-0.2.129/README.md) | [许可与通知](../../compliance/licenses/upstream/wasm-bindgen-0.2.129)；[来源](../../compliance/provenance/packages/wasm-bindgen-0.2.129/Cargo.toml.orig.txt) |

## wasm-bindgen-cli-support

[当前源码 / API](../../src/foundation/web-runtime/wasm-bindgen-cli-support/src/lib.rs) · [Cargo 清单](../../src/foundation/web-runtime/wasm-bindgen-cli-support/Cargo.toml)

| 上游来源身份 | 使用文档 | 许可与来源 |
| --- | --- | --- |
| wasm-bindgen-cli-support@0.2.129 | 未附独立使用文档；[当前源码内 Rustdoc](../../src/foundation/web-runtime/wasm-bindgen-cli-support/src/lib.rs) | [许可与通知](../../compliance/licenses/upstream/wasm-bindgen-cli-support-0.2.129)；[来源](../../compliance/provenance/packages/wasm-bindgen-cli-support-0.2.129/Cargo.toml.orig.txt) |

## wasm-bindgen-shared

[当前源码 / API](../../src/foundation/web-runtime/wasm-bindgen-shared/src/lib.rs) · [Cargo 清单](../../src/foundation/web-runtime/wasm-bindgen-shared/Cargo.toml)

| 上游来源身份 | 使用文档 | 许可与来源 |
| --- | --- | --- |
| wasm-bindgen-shared@0.2.129 | 未附独立使用文档；[当前源码内 Rustdoc](../../src/foundation/web-runtime/wasm-bindgen-shared/src/lib.rs) | [许可与通知](../../compliance/licenses/upstream/wasm-bindgen-shared-0.2.129)；[来源](../../compliance/provenance/packages/wasm-bindgen-shared-0.2.129/Cargo.toml.orig.txt) |

## wasm-binding-macros

[当前源码 / API](../../src/foundation/web-runtime/wasm-binding-macros/src/lib.rs) · [Cargo 清单](../../src/foundation/web-runtime/wasm-binding-macros/Cargo.toml) · [当前维护说明](../../docs/tools/web-bindings.md)

| 上游来源身份 | 使用文档 | 许可与来源 |
| --- | --- | --- |
| wasm-bindgen-macro@0.2.129 | [README.md](../../docs/upstream/wasm-bindgen-macro-0.2.129/README.md) | [许可与通知](../../compliance/licenses/upstream/wasm-bindgen-macro-0.2.129)；[来源](../../compliance/provenance/packages/wasm-bindgen-macro-0.2.129/Cargo.toml.orig.txt) |
| wasm-bindgen-macro-support@0.2.129（合并来源） | 未附独立使用文档；[当前源码内 Rustdoc](../../src/foundation/web-runtime/wasm-binding-macros/src/lib.rs) | [许可与通知](../../compliance/licenses/upstream/wasm-bindgen-macro-support-0.2.129)；[来源](../../compliance/provenance/packages/wasm-bindgen-macro-support-0.2.129/Cargo.toml.orig.txt) |

## wasm-encoder

[当前源码 / API](../../src/foundation/web-runtime/wasm-encoder/src/lib.rs) · [Cargo 清单](../../src/foundation/web-runtime/wasm-encoder/Cargo.toml)

| 上游来源身份 | 使用文档 | 许可与来源 |
| --- | --- | --- |
| wasm-encoder@0.245.1 | [README.md](../../docs/upstream/wasm-encoder-0.245.1/README.md) | [许可与通知](../../compliance/licenses/supplemental)；[来源](../../compliance/provenance/packages/wasm-encoder-0.245.1/Cargo.toml.orig.txt) |

## wasm-text-parser

[当前源码 / API](../../src/foundation/web-runtime/wasm-text-parser/src/lib.rs) · [Cargo 清单](../../src/foundation/web-runtime/wasm-text-parser/Cargo.toml) · [当前维护说明](../../docs/components/wasm-text-parser.md)

| 上游来源身份 | 使用文档 | 许可与来源 |
| --- | --- | --- |
| wast@245.0.1 | [README.md](../../docs/upstream/wast-245.0.1/README.md) | [许可与通知](../../compliance/licenses/upstream/wast-245.0.1)；[来源](../../compliance/provenance/packages/wast-245.0.1/original/Cargo.toml.orig.txt) |

## wasm-text-printer

[当前源码 / API](../../src/foundation/web-runtime/wasm-text-printer/src/lib.rs) · [Cargo 清单](../../src/foundation/web-runtime/wasm-text-printer/Cargo.toml) · [当前维护说明](../../docs/components/wasm-text-printer.md)

| 上游来源身份 | 使用文档 | 许可与来源 |
| --- | --- | --- |
| wasmprinter@0.245.1 | [README.md](../../docs/upstream/wasmprinter-0.245.1/README.md) | [许可与通知](../../compliance/licenses/upstream/wasmprinter-0.245.1)；[来源](../../compliance/provenance/packages/wasmprinter-0.245.1/original/Cargo.toml.orig.txt) |

## wasmparser

[当前源码 / API](../../src/foundation/web-runtime/wasmparser/src/lib.rs) · [Cargo 清单](../../src/foundation/web-runtime/wasmparser/Cargo.toml)

| 上游来源身份 | 使用文档 | 许可与来源 |
| --- | --- | --- |
| wasmparser@0.245.1 | [README.md](../../docs/upstream/wasmparser-0.245.1/README.md) | [许可与通知](../../compliance/licenses/supplemental)；[来源](../../compliance/provenance/packages/wasmparser-0.245.1/Cargo.toml.orig.txt) |

## web-sys

[当前源码 / API](../../src/foundation/web-runtime/web-sys/src/lib.rs) · [Cargo 清单](../../src/foundation/web-runtime/web-sys/Cargo.toml)

| 上游来源身份 | 使用文档 | 许可与来源 |
| --- | --- | --- |
| web-sys@0.3.106 | [README.md](../../docs/upstream/web-sys-0.3.106/README.md) | [许可与通知](../../compliance/licenses/upstream/web-sys-0.3.106)；[来源](../../compliance/provenance/packages/web-sys-0.3.106/Cargo.toml.orig.txt) |
