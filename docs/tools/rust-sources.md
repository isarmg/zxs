# Rust 源码治理工具

正式入口为 Web 和 SMB；`zxs` 启动守护进程，`zxs-rescue` 是离线救援与未初始化配置草稿工具。所有 Rust 包共享根 `Cargo.toml` 和根 `Cargo.lock`，活动依赖只使用仓库内 path。

- 当前源码、目标闭包、单锁文件与文件摘要：[compliance/rust-sources.json](../../compliance/rust-sources.json)。
- 不可覆盖的原始 archive 身份、原始文件摘要及材料位置：[origins.json](../../compliance/provenance/origins.json)。
- 统一法律材料：[中央通知索引](../../compliance/NOTICE.md)。
- 逐项用途与改造说明：[第三方源码清单](../THIRD_PARTY_RUST_SOURCES.md)。
- 唯一工作区及 native、Web、SMB 和工具入口：[解析策略](../../tools/rust-sources/workspaces.json)。

```sh
python3 tools/rust-sources/manage.py verify --metadata
python3 -m unittest discover -s test/tools/rust-sources -p 'test_*.py'
```

`verify` 检查源码、原始来源及中央法律材料的摘要，拒绝源码链接/特殊文件、仓库外依赖、registry/Git 依赖、独立工作区、独立锁文件、嵌套 Git 和散落的仓库脚手架。它同时检查所有活动 manifest 中的特性边，包括尚未启用的可选特性、`dep:alias`、`alias/feature`、`alias?/feature` 和依赖声明的 `features`；目标包或特性不存在时立即失败，不能靠默认编译没有触及来掩盖残留声明。

`--metadata` 另用空 Cargo home 离线解析[解析策略](../../tools/rust-sources/workspaces.json)登记的入口／目标组合，核对 normal/build 可达集合、编译／运行角色标签与已解析 features。Cargo metadata 先对整个工作区统一 features，再从各入口根遍历 normal/build 边：直接 dev 边被跳过，但其他成员及 dev 依赖的特性统一仍可能间接激活 normal/build 边。因此结果是保守可达超集，不是精确的单入口运行时或最终链接闭包；不能仅凭该集合断言每棵源码都由该入口实际使用，也不能直接据此删除来源。所有活动源码树和已合并实现都必须至少进入一个登记集合，实际精简仍需依赖、特性及源码调用审查。元数据解析也不等于实际编译、浏览器或 Samba 行为验收。

原生产品和构建宿主只支持 Linux amd64；当前登记及验收目标为 `x86_64-unknown-linux-gnu`，其他 Linux amd64 ABI 的必要共享源码保留为未验收预留。`wasm32-unknown-unknown` 只用于 Web 浏览器产物，不是额外原生平台。策略检查拒绝重新登记 ARM、未验收目标或把 Web 目标混入原生入口。

有意修改当前源码后，先保留来源身份和许可，更新明确的改造/合并说明并完成对应回归，再执行：

```sh
python3 tools/rust-sources/manage.py refresh
python3 tools/rust-sources/manage.py verify --metadata
```

`refresh` 显式登记当前文件摘要，并重新生成来源文档；它不能重新接受被篡改的原始身份、原始法律材料或原始归档文档。修改后的源码不再冒充原始发布包。整合进命名和界面组件的 Unicode/CSR 实现没有独立 Cargo 包，但仍有合并来源、当前文件摘要及漏洞匹配身份。

改造／合并说明中的已知历史路径按完整路径 token 迁移；重复执行 `refresh` 不会再次给中央资料路径添加前缀。`verify` 拒绝未迁移或重复前缀的说明，并检查这些已知迁移路径指向的源码、文档和原始资料仍然存在；未生成的 `build/web` 产物不要求存在。这一检查只涉及当前说明，不修改原始来源、原始文件摘要、许可证或已退役记录。

移除非目标平台或未使用的可选实现时，先同步删除依赖边、特性转发和相应源码分支，再通过明确的退役计划登记。计划必须匹配当前清单的完整最后源码收据，并包含移除原因、分类、文件数和字节数；执行时源码目录必须已经移除。普通 `refresh` 不会把目录丢失自动解释成一次合法精简。

计划是 `schema: 1` 的 JSON，`packages` 数组中每项包含以下字段。应在删除目录前从当前已验证源码保存计划，审查后再删除和执行；不要从改造前 archive 冒充当前最后收据。

| 字段 | 内容 |
| --- | --- |
| `directory` | 当前清单中待移除的 `src/foundation/...` 键 |
| `package` | 从当前清单 `packages[directory]` 完整复制的对象，包含最后逐文件摘要 |
| `reason`、`category` | 明确移除理由及分类，例如非目标平台或未使用可选实现 |
| `file_count`、`bytes` | 最后收据中的文件数及这些源码文件的大小总和，均为整数 |

```sh
python3 tools/rust-sources/manage.py refresh --retire-plan /tmp/reviewed-rust-retirements.json
python3 tools/rust-sources/manage.py verify --metadata
```

当前清单使用 schema 4，以 `packages`、`merges`、`retired`、`superseded` 分别记录活动源码、组件内合并实现、已移除实现和已升级替换的原始身份。退役记录保留最后源码摘要、原始身份及原因，记录摘要单独锁定，后续 `refresh` 不能覆盖。重新创建退役目录会使验证失败，重新引入必须经过显式来源登记。四类记录必须恰好覆盖全部原始身份，中央原始材料继续保留。

同目录上游升级须先登记新发布包的原始身份与集中材料，再使用显式审阅的 `refresh --upgrade-plan PLAN.json`。计划采用 schema 1；`packages` 每项包含 `directory`、与上次活动收据完全一致的 `package`、`successor_origin_directory`、`reason` 和非空 `source_prefixes`（例如 `["src/"]`）。工具逐文件检查指定完整源码前缀与新发布包的路径、字节一致，不能仅提升版本号。`superseded` 以旧 `origin_directory` 为键，保留原收据、原因、已核对前缀与直接后继；`superseded_records_sha256` 固定历史记录摘要。连续升级保留原直接后继链，最终必须指向同目录的当前身份或显式退役记录。后续本地修改仍使用常规 `refresh`，不能重写原始来源或升级历史。

原始发布包只在仓库外复核。重建器默认使用不可变的改造前发布包收据，保持 archive SHA-256、原始逐文件摘要、tar 路径/链接/特殊文件限制、大小上限与输出暂存校验；不能用当前整合后的清单替代原始收据，不能覆盖仓库内源码：

```sh
python3 tools/rust-sources/import_sources.py --output /tmp/rust-original-review
# 显式允许获取固定的原始发布包时才追加 --fetch。
python3 tools/rust-sources/collect_notices.py --output /tmp/rust-notice-review
```

许可收集器默认只列出待复核项；`--fetch` 才从已登记的 Git commit 获取候选通知。结果写到仓库外，由维护者审查后再追加来源材料。安全入口为 [security.sh](../../.github/scripts/security.sh)：分别扫描[独立扫描器锁](../../.github/security-tools/cargo-audit-0.22.2.lock)、产品根锁和全部活动、已合并实现的原始上游身份锁。已退役或已升级替换的旧身份保留历史收据，不冒充当前产品依赖；升级后的活动身份继续扫描，不因改名、path 化、合并或本地修改豁免漏洞、维护警告或扫描不完整。

扫描器在产品工作区之外按独立审阅锁构建，安装过程核对官方源码、原始锁及精确允许的锁变化；不增加产品工作区或产品锁文件。扫描使用隔离的固定策略，当前公告与 registry/yank 状态须实时读取，不能将工具缓存或历史通过结果当作本次安全结论。安装、网络要求和证据边界见[安全扫描器说明](../../.github/security-tools/README.md)。

<a id="http-owner-source-maintenance-chain"></a>

## HTTP 同所有者源码维护链

`proof_legacy_maintenance.py` 只为 `src/foundation/networking/http-transport` 提供
同路径、同身份的连续源码内容维护。既有首张 demand 比较修正收据保持原字节及摘要；
普通内容后继采用 schema 1，上游修复回移采用 schema 2；完整 `before` / `after`
所有者快照和精确补丁共同受收据摘要约束，
`parent` 必须指向紧邻前序，`before` 必须等于其完整 `after`。
经审阅的收据 ID 显式追加到工具的 `RECEIPTS`，普通 refresh 不发现或自动接受未知收据。

schema 1 后继只允许已有 `src/*.rs` 文件内容变化，外置测试快照不变。
schema 2 还允许已认证上游文件对应的既有抽取测试切片变化；生产及外测文件集合、
包名、版本、上游身份、许可证和布局不变。Cargo 清单、增加/移除/移动文件、升级、退役、
重命名及其它所有者均不在此适配器的授权范围。每次 refresh 只接受一个紧邻后继，
重复 refresh 必须保持同一最终快照；verify 要求完整已登记链。验证从真实当前文件
逐步逆向重建到原维护和布局证明，逐步核对完整集合与摘要，不用归档替代缺失活动文件。
沿用既有单收据解压、文件、补丁与活动字节预算，不新增维护次数上限。

schema 2 的 `source_backport.py` 从 origins、consolidation、layout 和 source-moves
重建生产／测试路由，按上游 base 的唯一逐行切片定位外测，记录确定性三方合并、
标识符变换和绑定具体冲突摘要的人工解决。`apply` 在当前认证布局上重新核对路由、
精确切片、文件字节及重放结果，拒绝越界、链接、缺失文件和未授权目标后才写入。
普通 `refresh` / `verify` 也从整链逆向恢复后的字节重新认证切片，不只是检查合并自洽；
这一步不递归读取仍在构造的 layout adapter。已有外部路径修正证明覆盖的测试仍禁止改写。
失败计划和冲突保留在仓库外 `attempts/`；失败写入恢复原内容，恢复失败附加到原错误，
不替换首要异常。该离线维护工具要求独占工作副本，不承诺与并行改写仓库的进程竞争安全。

标识符映射按完整 token 和 `use` 树的继承前缀判断，保留 raw identifier 的 `r#`、
别名、本地模块与 Unicode 标识符；注释中的路径单独处理，不把散文引号当 Rust 字面量。
普通/raw/byte/C string 及 char/byte-char 的非 raw 标识符后缀属于同一字面量 token，
包括 keyword 拼写及 Unicode 后缀；空白、注释、数字起始与 `r#` 的 `#` 不并入后缀。
源码标识符必须属于运行该工具的 Python Unicode XID 支持集；不支持的 token、未闭合字面量
或注释、无法解析的 `use` 树（包括宏 DSL 的 `use $…`）明确停止并要求审阅。
该工具不解析任意宏 DSL 或执行 Rust 名称解析，映射表仍须绑定已登记的来源变更。

当前实际回移为 hyper `97fff93e53f9e6a993f2d6a25d50883b7b8752a7`（#4222），
收据 `c75e73b0b8aab76fc3934f6c72620127795b8c95ec7bc0f52d173664e477938d`。
它保留 `http-transport@1.12.0` / `hyper@1.12.0` 来源和扫描身份；不将本地回移伪装成升级。
收据的 `consumer_tests.executed` 为 `{}`，仅证明当时没有登记执行结果，不能回填或改写。
后续 CI 的提交、命令、日志和结果独立保存，不由收据存在推断测试已通过。

`test/foundation/http-transport/unit/src/proto/h1/dispatch/tests_770.rs` 保留上游测试，
但 HTTP 门禁把组件作为依赖编译，不运行这一单元模块；当前也没有引入其
`tokio-test` / `pretty_env_logger` 依赖。实际可执行的等价回归位于
`test/architecture/http-transport/backport.rs`：公开 API 覆盖 reset / EOF、
正常及 `poll_without_shutdown` 驱动、协作预算耗尽时 drain Pending／唤醒、原始错误链、
保持 sender 和 connection 存活时的取消，以及 upgrade 移交。它们是项目新作，不冒充
上游测试已执行；既有 25 项行为测试仍保留，门禁总数为 30。源码治理、消费者回归及
公告扫描分别验证；公告 finding 政策不因回移而改变。

维护命令和生成输出边界见 [HTTP 回移验证](../validation/http-backport.md)。

## 基础组件测试覆盖映射

`python3 tools/rust-sources/foundation_coverage.py --write` 依据来源清单的 profile 解析、每个 profile 入口的 `cargo tree`、外置测试根、已审阅的 `foundation-test-gates.json` 和 `check-foundation-behaviour.py` 套件表，生成 `test/ci/fixtures/foundation-test-coverage.json` 与 [覆盖映射](../validation/foundation-test-coverage-map.md)；`rust-sources` 套件以 `--check` 核对。分类与验收见[测试覆盖闭环](../validation/foundation-test-coverage.md)。

## 依赖文档导航

[依赖文档](../dependencies/README.md)按当前职责聚合本地原文、维护说明及源码 API；不把退役来源或上游已删除能力列为可用接口。`python3 tools/rust-sources/documentation.py refresh` 更新索引，`verify` 检查内容与覆盖是否过期；静态及来源门禁都执行验证。上游原件与其来源收据继续受原有完整性规则保护。

## 显式追加来源

新增职责组件不借普通 refresh 吞入。`refresh --import-plan <receipt-sha256>`
只接受 `source_imports.py` 已审阅登记的内容寻址计划；当前用途是绑定转换测试的文本工具。
计划绑定完整前序 catalog 摘要、原布局收据、精确新增身份/目录、官方发布包摘要、
全部原文件到中央原件的映射、许可、当前文件初始摘要、根测试归属及未保留能力说明。
新增来源复用既有 append-only origins 快照和布局历史投影；旧原件、旧历史字段及
布局安装收据不被重写。当前目录全集仍精确比较，仅增加计划列出的所有者。

原档 tar 只作为只读来源证据：拒绝链接、特殊文件、重复项、路径越界及摘要不符，
不在原档执行构建。导入前先验证现有原始记录、物理目录全集和全部声明特性边；
普通 refresh 继续负责已有文件修改、外置测试守恒和真实可达闭包。失败且 catalog
未推进时恢复原 origins 字节；origins 与 catalog 各自先写同目录临时文件再原子替换，写入失败不截断原收据。已提交的 catalog 不做隐式回退（包括随后文档写入失败）。导入收据证明来源与
布局，不证明组件、golden 或产品行为已通过测试。
