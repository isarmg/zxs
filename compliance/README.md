# ZXS 统一许可证、版权与来源说明

本仓库统一维护 Web 和 SMB 产品源码、构建工具及所需的 Rust 基础实现。所有许可证、版权通知和机器可读来源材料集中在本目录；产品、开发、运维与上游说明文档集中在 [`docs/`](../docs/README.md)。源码目录中的 crate 是同一构建依赖图中的编译组件，不是独立仓库；自有包归根工作区，基础组件通过本地路径依赖和根锁文件接入。

## 许可证边界

第一方工作区的默认许可证为 Apache-2.0，全文见 [`licenses/project/LICENSE-APACHE-2.0.txt`](licenses/project/LICENSE-APACHE-2.0.txt)。集中归档、目录合并和 crate 合并不会改变第三方源码的原始权利或许可证；第三方实现继续适用其各自保留的许可条件。源码中的版权、SPDX 和作者头继续保留。

逐项来源和许可索引见 [`NOTICE.md`](NOTICE.md)，活动源码收据见 [`rust-sources.json`](rust-sources.json)。原始许可证、版权、作者名单和通知分别按来源包标识存放于 `licenses/upstream/`；多个组件共用的通知放于 `licenses/shared/`，此前补充取得的上游通知全文放于 `licenses/supplemental/`。界面基础实现合入 `src/interface/` 后，Leptos 的 MIT 全文与原始版权通知仍见 [`zxs-web-core`](licenses/upstream/zxs-web-core-0.8.19/UPSTREAM-NOTICE.md)；过程宏对应通知见 [`zxs-web-macro`](licenses/upstream/zxs-web-macro-0.8.19/UPSTREAM-NOTICE.md)。

字体资源的原始版权与三组 SIL OFL 1.1 全文见 [`licenses/fonts/UPSTREAM-NOTICES.md`](licenses/fonts/UPSTREAM-NOTICES.md)。字体文件本身保留在 `src/web/public/fonts/`，原始字节和资源收据继续由 Web 构建校验。

命名表输入（`src/naming/data/`）与 NormalizationTest 夹具（`test/naming/data/`）来自 Unicode Character Database 16.0.0，Unicode License v3 全文见 [`licenses/upstream/unicode-ucd-16.0.0/LICENSE`](licenses/upstream/unicode-ucd-16.0.0/LICENSE)；逐文件摘要与官方地址见 [`sources.tsv`](../src/naming/data/sources.tsv)。夹具以 `upstream-fixture` 登记，实际取件提交及不可变来源快照见[导入收据](fixtures/unicode-ucd-16.0.0-normalization.json)，不是项目新作。`NOTICE.md` 是布局收据固定的证据文件，不随此项改写。

## 集中材料与可追溯关系

| 位置 | 含义 |
| --- | --- |
| `rust-sources.json` | 当前源码位置、使用关系、本地改造和逐文件收据，由统一源码管理工具维护 |
| [`provenance/origins.json`](provenance/origins.json) | 导入前的包身份、上游 URL／版本／提交／归档校验和、原始文件哈希及中央许可映射 |
| [`provenance/document-migration.json`](provenance/document-migration.json) | 原始文档和元数据路径到中央路径的映射及迁移前 SHA-256 |
| `provenance/packages/` | 原始发布清单、原始子锁文件、发布校验和、VCS 身份及上游 CI 元数据；仅作溯源，不是工作区或构建入口 |
| `provenance/merged-components/` | 被合并组件的原始清单与入口，保存合并前的证据 |
| `provenance/pre-monorepo/` | 固定来源收据和关键构建输入快照 |
| `provenance/shared/` | 原先共享的上游身份、补充通知收据和补丁记录 |
| [`../docs/upstream/`](../docs/upstream/) | 原始上游说明文档，保持归档字节，历史路径不表示当前目录 |

归档中的 `vendor/`、`crates/`、`integrations/`、`demos/`、独立 `Cargo.lock` 及历史工作区字段是迁移前的来源事实，不是当前构建指令。当前构建只使用根 `Cargo.toml` 与根 `Cargo.lock`。不得在归档目录执行 Cargo，也不得为了通过完整性检查覆盖原始来源哈希。

## 尚未关闭的许可资料复核

以下三项的历史来源记录仍缺少独立许可／版权文件，但不能把历史清单等同于当前二进制依赖清单。源码与发布包的许可元数据、来源版本／提交及历史复核说明继续保留；集中整理不代表已取得缺失原文，也不代表已完成独立许可复核。

- `convert_case_extras@0.2.0`（`d523585a01afddd13944be1d88faa4d1db6a3eb4`）：已退役。原消费者改用 `convert_case` 的直接操作，当前根锁文件和活动依赖图不再包含此包；历史来源、退役源码归档与通知记录仍保留。其资料缺口不应被误报为当前二进制的活动依赖，但分发历史源码材料时仍须保留该说明。
- `drain_filter_polyfill@0.1.3`（`9dc585f3eec048bfc3faf8656e07c4a19273c38f`）：已退役。原消费者改用标准库 `retain`，当前根锁文件和活动依赖图不再包含此包；历史来源与通知记录仍保留。退役不等于补齐了历史许可资料。
- `erased@0.1.2`（`60eebad28ab2578a6322199099b8a07035438008`）：仍为活动 Web 依赖。实际链路为 `web-app → web-interface → view-renderer → erased`，渲染器在类型擦除容器中使用 `ErasedBox`；因此不能仅凭集中通知已打包就将 Web 发行的许可资料复核标为完成。

`erased` 的[官方 0.1.2 发布包](https://static.crates.io/crates/erased/erased-0.1.2.crate)，其 SHA-256 为 `a1731451909bde27714eacba19c2566362a7f35224f52b153d3f42cf60f72472`，与不可变来源收据一致；对应[固定提交的完整 Git 文件树](https://github.com/JonathanBrouwer/erased/tree/60eebad28ab2578a6322199099b8a07035438008)。发布包与固定文件树均没有独立许可／版权文件，发布清单仅声明 MIT 和作者名单。作者字段不应被改写成未经上游确认的版权年份或版权通知。

公开分发前，`erased` 应保留为需人工决定的许可资料复核项，而非自动通过项。可执行的关闭路径是：取得适用于该版本的上游许可与版权通知，记录权威来源及哈希并追加中央材料；或者由负责发行的人员完成独立许可复核并记录依据。若选择替换依赖，应另行进行独立设计、功能与内存安全回归、显式退役和来源校验，不能为了清除清单而删除来源或直接改写许可。本段记录技术证据和待办，不作法律保证。

原始说明见 [`provenance/shared/vendor/rust-notices/provenance.json`](provenance/shared/vendor/rust-notices/provenance.json)。发布材料必须携带本目录中的实际许可、版权和通知，并如实列出尚未关闭事项；许可证声明本身不构成维护状态、安全审计或性能验收证据。

## 安全补丁升级

同一路径的真实上游升级通过显式升级计划导入；当前清单中的 `superseded` 保存被替换活动包的完整收据和新来源关系。历史来源与中央材料保持不可变，活动审计只匹配实际保留的版本。TLS 组件的当前要求见 [TLS 信任维护](../docs/security/tls-trust.md)。

## 来源验证输入

`validation/` 保存源码及测试夹具转换的机器可读校验输入，包括平台裁剪和日期时间夹具证明。它们参与完整性核对，须保留原始字节；不作为当前测试已通过的记录，也不能替代当前代码回归。一般评审、改进日志和单次测试输出不放入本目录。

## 补充依赖文档

[`dependency-documents.json`](dependency-documents.json)登记从匹配版本官方发布包补齐的原始说明，以及与已保存原文逐字节相同的共享映射。归档摘要须匹配当前来源身份，每份文档保留原路径、当前路径、字节数、摘要与许可指向；不修改已有来源原件。当前阅读入口见[第三方依赖文档](../docs/dependencies/README.md)，校验由 `documentation.py verify` 执行。

## 仓内 Samba 原生源码

完整官方 Samba 4.23.0 源码归档、来源收据和法律材料入口见
[`sources/samba/README.md`](sources/samba/README.md)。它是明确登记的原生构建输入，
不是 Rust 包；4.23.0 历史源码与证据保持完整。当前 Debian 13 系统 Samba 的配套 VFS 使用精确匹配的 Debian 源包与补丁，由签名 apt 来源取得并额外校验固定摘要，不把此归档冒充 Debian SDK。
原始 GPL 及各组件许可、版权不因纳入仓库而改变。
