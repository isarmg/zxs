# 外置测试来源与维护

可执行测试代码和仅供测试使用的夹具放在仓库根目录的 `test/` 下。Cargo 包身份、
清单和生产入口仍在 `src/` 或原有工具目录。显式 Cargo 测试目标和带条件编译的
Rust include 保留所属包的特性开关与私有模块语义；不建立独立测试工作区或锁文件。
目录分离不代表删减来源材料，也不代表所有保留的上游测试都已启用。

日常测试入口见[测试布局](architecture/test-layout.md)，命令及整体来源维护流程见
[源码治理工具](tools/rust-sources.md)。本文规定当前外置测试清单及仍需验证的来源
协议。不可变收据属于验证证据，不能作为过时记录清除。

## 当前清单与刷新

`compliance/external-tests/*.json` 按所属源码目录（owner）登记当前第三方测试材料
及明确标记的项目新作。每个 owner 列出精确目标根、当前文件哈希和原始来源路径，
包括从仍然存在的生产文件中抽取的片段。`compliance/rust-sources.json` 将这些映射
绑定到原始上游身份，并记录映射文件摘要；本地新作的标记不表示它来自上游。

验证拒绝文件缺失、内容变化、未登记新增、来源不明、根目录重叠、路径逃逸和符号
链接。`test/foundation/` 下位于已声明子目录之外的文件同样不能漏记。`refresh`
先核对显式清单，再更新来源清单，不会自动吸收新测试；同一上游身份下移除生产
文件必须有匹配的外置来源映射或其他已支持的显式处置证明，不能靠刷新抹去。

经审查的修改按以下顺序处理：

1. 修改并审查当前源码、测试及显式映射，保留历史来源标签和不可变证据。
2. 执行 `python3 tools/rust-sources/manage.py refresh`；需要特殊处置时附带下文计划。
3. 审查源码、映射、来源清单与生成文档的差异。
4. 执行 `python3 tools/rust-sources/manage.py verify --metadata`，并运行受影响的正常测试。

`verify` 不接受新哈希；`refresh` 接受的是经审查的当前内容修改，不声称修改后的
测试仍与最初分离时逐字节一致。缺失文件、未登记新增或丢失已归属片段仍会失败。
来源会计验证也不代替行为、协议、格式或 ABI 验证。

### 分组布局的额外边界

当前 Foundation 生产目录按职能分组，外置测试仍位于 `test/foundation/<包>`。
`foundation_layout_receipt` 和 `foundation_external_path_receipt` 将当前清单绑定到
固定布局与外部路径证明。验证器先认证真实当前文件，再通过精确、可逆的路径映射
供历史证明读取；不能用旧目录副本、符号链接或归档替代缺失的活动文件。
`CARGO_MANIFEST_DIR` 指向所属编译包的清单目录，不是外置测试目录；嵌套引用也必须
解析到该 owner 实际登记的目标。

下文描述命名、移动和整合收据的协议，不表示所有计划都能直接用于当前分组布局。
分组后的根重命名、整合、裁剪、外置测试文件移动及触及固定路径编辑的移动仍需
专门后继适配器。独立包退役、包内生产文件移动和受支持的上游升级也须通过布局
专用检查；共享来源、链式来源、固定证明或已登记外部路径修正的 owner 不能绕过
这些限制。具体边界见[Foundation 维护边界](architecture/foundation-groups.md#source-maintenance-boundaries)
及 `tools/rust-sources/source_layout.py`、`source_layout_upgrades.py`。

## 项目新写的回归测试

全新编写、并非从上游代码抽取的测试仍在所属包的 `test/` 根下显式登记，
不得虚构上游来源路径。映射保持 `schema_version: 1`，仅这类新行增加
`source_kind: "project-authored"`；已有历史行不加标记、不改写。
首次登记时，`source` 等于实际 `path`，`source_sha256_before` 等于该文件
首次登记的实际字节 SHA-256，`sha256` 记录当前内容。`refresh` 同时检查首次
路径和实际字节，不会把任意旧哈希当作本地创作证据。

后续经审查的内容修改只更新 `sha256`；来源类型、初始路径、初始哈希及
原有 origin 标记必须保持不变。所属包的 upstream/version 身份不变，
但这不表示新测试来自该上游版本。新标记不能抵消上游文件移除，也不能
把旧的抽取行改成本地新作。精确清单、路径安全、禁止丢失既有行以及显式
退役归档规则继续适用。此支持仅覆盖本 owner 下的新作登记和后续内容
编辑，不授权跨 owner 转移或根重命名；这些变更未有专门证明时保持拒绝，
不能通过删除标记或重写初始来源绕过。

## 不可变分离与路由证明

### 初始测试分离

`compliance/test-source-relocation.json.gz` 保存压缩的反向行差分收据。
`source_relocation.py` 固定其压缩字节摘要，并限制压缩及展开大小。首次引入
外置清单的 `refresh` 检查所有保留源码和抽取片段，用当前字节及反向编辑重建
分离前源码，并要求结果匹配前序来源清单中的源码哈希。空文件、截断替换、不同来源
归属或改动后的当前片段不能悄然通过首次审计。

该收据是不可变历史证据，不是可变的当前清单。最后一个当前外置 owner 退役或
恢复为生产文件后，来源清单仍保留 `initial_test_relocation_sha256`，继续要求初始
分离证明存在且摘要正确；后续普通刷新不重写它。

### Ring 与平台测试

Ring 另使用摘要由验证器固定的 `compliance/ring-test-relocation.json`。
验证器先认证精确的当前生产和测试清单，再逆转唯一可逆的源码路由修改，重建
此前源码视图。目标裁剪、退役归档、样式、原始重叠及测试材料证明保持原字节，
并对该视图验证；历史视图不能补充缺失的当前文件。

生产 DER 输入及无条件编译的公开测试支持 API 仍属生产源码；可执行断言及其
输入向量属于外置测试。`polyfill/cstr` 的无条件编译期断言通过外置 include 保持
原来的编译行为。

平台测试 include 路径修正单独由 `compliance/platform-test-routing-corrections.json`
认证。验证器固定此收据，检查修正后的当前字节，并逆转精确路由修改和整文件夹具
移动，再应用未改写的平台分离证明。当前字节变化、错误逆转、旧夹具路径重现或
收据篡改均拒绝；原平台和运行时收据保持冻结。

## 外置测试 owner 的退役与升级

普通 `refresh` 拒绝外置 owner 或片段消失。显式 `--retire-plan` 或 `--upgrade-plan`
中涉及此 owner 的条目必须含 `external_action: "remove"`，且 `external_tests`
必须等于来源清单中该 owner 的完整最后收据，包括 origin 身份、根、文件、哈希、
数量和字节数。刷新前须删除当前映射中的 owner 条目及它登记的全部测试根。
缺少明确处置、前序收据不匹配、映射残留、根目录残留（即使为空）或符号链接均拒绝。

退役将完整外置收据存入 `retired[directory].retirement.external_tests`，并将其
数量和字节数计入退役汇总。升级将其存入 `superseded[prior_origin].external_tests`；
旧版测试不能不经证明就归属后继版本。嵌套收据由已有不可变 retired/superseded
记录摘要保护；即使不再有当前外置 owner，初始分离证明仍需检查。

升级支持移除旧外置测试，不支持原位替换整套旧测试。应先在经过发布包认证的
`source_prefixes` 内恢复后继版本测试，再以独立、经审查的变更外置。若需直接保留
或替换外置测试，必须另设计后继保留证明，不能豁免该 owner 的检查。同一包的
升级和退役必须分次刷新。后续来源归属正确的测试可复用已 superseded owner 的
旧测试根；已 retired owner 的根必须持续不存在。其他活动 owner 仍受防丢失检查。

## 整文件分类纠正

误判为仅供测试的文件可在普通刷新中返回精确的原生产路径，但必须完整保留字节：

- 前序 owner 对该 `source` 只有一条归属，原始来源哈希等于外置文件哈希。
- 前序生产清单没有此文件；新生产清单及实际字节都匹配已登记哈希。
- 旧外置路径已不存在，前后包身份保持同一 owner 和 origin。

抽取片段、内容变化、其他路径、其他 owner 或仍保留的外置副本均不适用；
`project-authored` 行也不能借此消失。此例外证明材料回归生产，不是批准删除。

## 显式源码命名变更

`refresh --rename-plan PLAN.json` 描述经审查的 Foundation 包根、包名及对应外置
测试根重命名。schema-1 计划的 `packages` 条目包含 `directory`、精确前序
`package`、`destination_directory`、`destination_name`、`reason`、完整审查后的
`files_sha256`、精确前序 `external_tests` owner（或 null）及审查后的
`destination_external_tests` owner（或 null）。完整 owner 含 origin、文件数和
总字节数。目标目录末级名称须与包名一致。

生产相对文件集合及外置片段数量必须保持双射。可选 `file_moves` 显式列出生产
相对文件名变化，未列文件保持原位，不授权新增或丢失文件；路由、导入等内容
变化须逐项记录审查后的哈希。

schema 5 起，来源清单以仅追加的 `renames` 保存原包和外置 owner，并以
`rename_records_sha256` 认证。外置 `source` 和 `source_sha256_before` 保持历史
标签，只有已验证的同 origin 重命名历史允许当前 owner 引用旧来源前缀。旧生产
根和测试根必须不存在；普通刷新仍拒绝片段丢失、未知文件、归属变化或历史重写。
后续本地内容修改使用显式普通刷新，不重写命名收据；没有命名历史的 schema 4
来源清单仍可验证。

同一身份的重命名与退役/升级须分次变更。已重命名 owner 可随后携带精确外置
收据显式退役。重复重命名、重命名后的上游升级、已升级来源的重命名须另有链式
适配器；固定 Ring/平台视图来源须有匹配的历史视图适配器。原始上游名称、归档、
许可证和安全身份不重命名、不覆盖；`project-authored` 不因此获得根重命名支持。

## 保持身份的包内文件移动

`refresh --move-plan PLAN.json` 在不改变包根、包名、版本或上游身份的前提下，
移动生产或外置测试文件名；协议也可处理此前已完成普通包根重命名的来源。
计划采用 `schema: 1`，每个 `packages` 条目包含 `directory`、精确前序 `package`、
全部精确重叠 `merged_packages`（没有时为空列表）、`reason`、完整审查后的
`files_sha256`、`file_moves`、精确前序 `external_tests`（或 null）、审查后的
`destination_external_tests`（或 null）及 `external_file_moves`。

生产文件映射使用包相对路径，外置映射使用仓库相对路径。必须列出每个移动文件，
包括目录移动的全部后代；未列出的文件留在原位。隐式及显式映射共同构成双射，
拒绝丢失文件、虚构来源、未知目标或覆盖未移动文件。后置内容哈希及字节总量须
匹配磁盘。外置 owner 的根、原始 `source` 标签、原始哈希、片段数和归属不变。
至少一个映射非空；通用协议支持仅移动外置文件，但当前分组布局仍有前述限制。

schema 6 起，来源清单追加内容寻址的 `source_moves`，保存精确前序快照和审查后的
映射。后续移动追加新收据，不重写旧移动或根重命名记录。普通内容修改仍经显式
刷新处理。历史 origin 与独立认证的来源清单核对，移动不能新增或移除 origin。
schema 4、5 仍可读取；刷新保留所需更高 schema，不能降级。

同一来源不能在同次刷新中包内移动并根重命名、升级或退役；先完成根身份变更，
再为独立移动计划固定当前收据。不相交 owner 可使用不同显式计划，但仍须通过
当前布局门禁。文件会计只证明声明的材料保留，不证明行为兼容。

## 经审查的多来源整合

`refresh --consolidate-plan PLAN.json` 显式组合实现。schema 8 起，来源清单保存内容
寻址的 `consolidations` 及对应 `consolidation_records_sha256`；旧的无整合历史清单
仍可读取，刷新不降低 schema。当前分组布局及固定证明仍有额外限制。

schema-1 计划的 `packages` 条目包含 `primary_directory`、`destination_directory`、
`destination_name` 和审查原因 `reason`。每个 `inputs` 条目含精确前序 `package`
及完整 `external_tests` 收据（或 null）。`destination_files_sha256` 是完整审查
后的生产树；`file_routes` 按输入目录列出每个前序包相对文件，`destination`
指向当前相对文件。只有冗余根 Cargo 清单可使用仅含 `archive_base64` 的逐字节
归档路由；下节的实现/构建脚本归档另有严格规则。`new_files` 独立登记新增衔接
代码，与路由目标不重叠，二者共同覆盖完整目标树。生产路由可多对一，记录的是
经审查的内容迁移，不证明字节保留或语义兼容，仍须功能测试。

`destination_external_tests` 是合并后的完整 owner。每行保留历史来源、原始哈希
及元数据，增加或保留原输入的 `origin_directory`，只更新当前路径和哈希。
`external_file_routes` 将每个前序仓库相对测试路径映射到唯一 `destination`，
并提供 `replacements`：有序的 base64 `old`/`new` 字节对。首次验证逆转唯一、
非空、有界编辑，重现该测试最后的当前哈希。未改内容的文件无需 replacements。
整文件替换、消失、额外文件、符号链接、歧义编辑和错误 origin 归属均拒绝。
每次外置转移限 2,048 个文件、64 MiB 当前字节、512 处替换及 2 MiB 解码补丁。
授权精确匹配 origin/source/原始哈希三元组，不放行宽泛的合入来源前缀，
也不授权 `project-authored` 跨 owner 转移。

主来源与被吸收来源保留各自上游版本、安全和许可身份；被吸收来源登记为合入，
不能报成删除。迁走的旧生产根和旧外置根必须不存在；历史收据及原始材料不可变。
后续经审查的当前内容编辑使用普通刷新，不重写首次转移证明或丢弃 origin 标记。
已整合 owner 的重命名、包内移动、升级或退役仍需专门生命周期适配器；不相交的
普通计划可并行，重叠计划不能静默绕过检查。

### 明确移除实现的归档

整合可移除不再支持且无引用的 Rust 实现，但不能假称其字节已合入当前文件。
生产路由须精确包含 `kind: "archived-source"`、`sha256`、`archive_base64` 和
`reason`。哈希及解码字节须匹配精确的最后当前清单；原因须实质解释移除，
去除首尾空白后长度为 40–10,000 字符，并随收据保持不可变。

此处仅接受 `src/` 下 UTF-8 `.rs` 实现文件，不接受 crate 入口 `src/lib.rs`、
`src/main.rs`、测试、夹具、示例、基准、清单、法律或来源材料。受保护路径段及
文件主名不区分大小写，包括 `tests.rs`、`fixtures.rs`、`license.rs` 和
`provenance.rs`。每文件解码上限 1 MiB，每次整合最多 64 个归档、解码总量
4 MiB；解码前也检查编码长度。缺失字节、通用丢弃路由或生产转测试归档均拒绝。
冗余根清单的归档仍使用独立格式和身份检查。

`archived-build-script` 仅能保留次要输入精确的根 `build.rs`，字段为 `kind`、
`sha256`、`archive_base64`、`reason`、`destination_manifest_base64`。原根清单
须按输入哈希精确归档，其包名/版本和显式或隐式 `build.rs` 声明须匹配。
主来源脚本、自定义构建路径、原生 `links` owner 和法律材料归属均不适用。
额外目标清单归档须匹配目标 `Cargo.toml` 哈希及名称，并声明 `build = false`；
目标不得有根 `build.rs` 或退役脚本的精确副本。后续当前清单也须保持禁用构建，
不能出现 `links` 或根 `build.rs`。UTF-8、原因、单文件及总量限制同上。这不放宽
`archived-source`，不批准任意入口删除或测试迁移；两份清单与脚本都保留为证据。

主来源脚本须使用独立的 `primary-build-script-retirement` 路由，在上述构建脚本
字段之外增加 `original_manifest_base64`、`replacement_sources` 和 `toolchain`。
原清单按主输入哈希和身份认证，其正常目标路由仍保留；目标也必须保持主版本。
`replacement_sources` 记录 1–16 个已登记生产 Rust 文件的精确 UTF-8 字节及
SHA-256，必须含 `src/lib.rs`，每文件上限 256 KiB、总量 1 MiB。测试、夹具、
法律文件、自定义构建入口和非源码路径不能作替代证据。

`toolchain` 须声明 `rust_version: "1.99"`、按顺序排列的 targets
`["x86_64-unknown-linux-gnu", "wasm32-unknown-unknown"]` 及实质性的 `assumptions`
（40–10,000 字符）；目标清单必须声明同一 Rust 版本。这是项目专用的有界处置，
不授权其他工具链或目标。它保存原脚本和替代证据，不声称行为或字节码等价，
特性、生成 API 和目标行为测试仍是必需的工程证据。固定来源整合即使有专门
授权器也不能用此路由；固定主来源的历史投影仍禁止任何主来源归档。

归档实现只存在于显式整合收据，不伪装为 `src/` 中的维护文件，也不计入活动生产
清单或充当功能等价证明。原 origin/version/license 身份仍由主 owner 或合入
owner 表示，独立冻结的上游证据不变；外置转移仍须完整双射和可逆证明。

### 普通重命名或包内移动后的整合

可选 `input_history` 按当前输入目录绑定从上次来源清单原样复制的精确 `renames` 和
`source_moves` 收据映射。必须包含所有相交收据，拒绝任意子集、无关 origin、
过期当前快照或歧义重命名链。绑定属于内容寻址的整合收据，后续验证始终须等于
原历史。最多链接 64 份收据，每输入最多一次重命名，总收据仍限 32 MiB。

早期快照是历史清单，不要求每个旧文件仍被维护；此前移动与本次整合之间经合法
审查的修改或裁剪不要求已删除文件重新出现。完整最后当前包和外置快照必须精确
匹配整合前来源清单，每个当前文件仍须显式处置，原版本、法律身份和 origin 记录不变。

已重命名输入经认证的最后当前快照解析到独立检查的当前主来源或合入来源；
验证器不把已消失的重命名根当成活动目录打开。旧重命名根和迁走的整合根必须
保持不存在。历史外置标签只获得该输入精确、已验证的重命名前缀，再执行逐
origin、逐文件转移证明。此绑定本身不授权已升级输入、旧式合入来源或固定证明。

### 已整合 owner 再次参与整合

可选 `input_consolidations[directory]` 必须含 `receipt_id` 和 `merged_packages`：
前者引用紧邻、尚未被消费的前序整合收据，后者为该输入当前全部精确合入行，
保持清单顺序及共享文件清单。输入的 `package` 和 `external_tests` 仍须精确匹配
最后当前快照；继承 origin 不形成额外物理副本，外置来源标签不能缩减。

验证证明图无环、前序唯一消费、来源与物理根不重复，最多深度 64、每个输出
4,096 个 origin。只有终端 owner 保持活动；目录复用只能沿显式前序边发生。
已有历史收据不重写，也不递归嵌入副本。直接 `input_history` 与同一输入的
`input_consolidations` 不能重叠，祖先的历史链接仍独立验证。

无前序整合证明的旧式多来源 owner、已升级输入、固定基准搬迁来源、已终结的
固定证明整合链及嵌套构建脚本退役均须另有适配器。此协议不解除当前分组布局
限制。更多身份约束见[源码身份](architecture/source-naming.md)。

## 活动历史证明路由与固定主来源整合

活动证明覆盖的根不能使用缺失文件的退役归档。内容寻址且有界的 `proof-routing`
事务认证完整活动清单，逆转唯一字节替换，以重放原样的平台、运行时和 Ring
证明。当前路径和包路由也驱动真实 Ring 测试矩阵，不是只让历史验证通过。
收据在辅助工具中显式固定并登记到来源清单；每次刷新每个 origin 只接受一个
以精确前序快照为基础的新步骤，不能降级 schema 或替换既有整合历史。

终结性的 `consolidation` 证明绑定引用精确已登记的整合记录。原固定主来源的
生产和测试清单保持双射，每个原文件都由当前对应文件重建。导入 origin 和新增
衔接代码接受完整合并物理清单认证，但不进入主来源的历史文件投影。原主来源
不能归档文件，也不能与导入 origin 共用一个路由目标；第二个固定输入须另有
适配器，当前拒绝。

大型变换须拆为有实际审查意义的同根阶段，每阶段遵守 512 处替换及 2 MiB 解码
补丁限制，最后进行根/整合事务。最终整合使用紧邻前阶段的精确快照，而不是未
分阶段时的最初清单；其外置转移还须遵守独立限制及逐 origin 的精确归属。

固定来源整合路由仍为终结：普通 `edit` 不能跟在 `consolidation` 后面。
同根维护使用独立 `proof-maintenance` 认证适配器；重命名、退役、生产文件增删
及其他生命周期变更仍拒绝。无关普通 owner 保持原有审查刷新行为。

### 整合后的同根维护

`tools/rust-sources/proof_maintenance.py` 的 `RECEIPTS` 是仅追加的审核固定列表，
对应 `compliance/proof-maintenance/<sha256>.json.gz`，来源清单同步登记
`proof_maintenance_receipts`。每张 schema 1 收据包含 `reason`、终结路由摘要
`terminal`、前张维护摘要 `parent`（首张为 null）、精确 `layout` 标识、完整
物理 `before` / `after` owner 快照（`package`、`merges`、`external`）和
`files`。`files` 只列全部发生内容变化的既有文件，每项使用原路由格式的
before/after 路径、摘要及 base64 `old` / `new` 唯一可逆替换；不能用整文件
归档替代补丁。至少一项既有文件改变，仍遵守 512 处替换、2 MiB 补丁及
64 MiB 活动材料上限。

生产清单、owner 身份、所有 merged origin 和测试 roots 不变。现有外测保留
原 `source`、`source_sha256_before` 与 origin；新增文件只能是既有测试根下
明确登记的 `project-authored` 测试，其初始 source/path 和摘要相同。第二次
维护可以继续修改源码或这些测试，但不能重置首次归属。新增测试仍须登记
外测映射并加入真实测试门禁，不能仅靠维护收据证明测试已经执行。

审核新收据时，从旧清单复制精确物理 owner 快照，并记录当前物理 after 快照；
将 JSON 按键排序、UTF-8 编码，gzip 使用 `mtime=0`，以压缩字节 SHA-256 命名，
追加固定摘要后执行 `python3 tools/rust-sources/manage.py refresh` 和
`python3 tools/rust-sources/manage.py verify --metadata`。
每次 refresh 每个 owner 只接受一张新维护收据，before 必须等于旧清单中的
对应完整 owner；不能跳过中间步骤。其它 owner 的后续正常维护不冻结在这张
收据上。禁止修改旧收据、原始 origins 或 layout/corrections 历史来配合新代码。

验证严格按实际变换的逆序进行：一次有界读取物理当前文件并核验完整 after
清单，逆维护补丁恢复 before 字节，再执行旧 layout 的精确逆变换，最后验证
原 consolidation 的完整 owner equality 及固定主来源旧路由。外测 bytes 也
必须逐阶段准确。新测试只属于当前 owner，不混入旧历史文件双射。漏文件、
额外文件、额外换行、错序、歧义补丁、来源漂移或缺失登记都会失败；未登记的
语义编辑仍不能穿过旧 layout 或终结证明。历史收据和所有原校验均保持原样。

本轮 Tokio 收据认证 `fs::asyncify` 保留 `JoinError` 的源码改动、Cargo 测试
目标及 `fs_asyncify` 取消回归。其证明是字节与归属关系，不替代运行时测试。

HTTP demand 的同根维护使用窄范围适配器
`tools/rust-sources/proof_legacy_maintenance.py`：它仅接受现有 HTTP legacy
consolidation `9a4a44802e8f493dcc1a1546caa723ea774c956de5d18c474076f85fd1280ac4`
及其精确 layout owner，不把它伪装成新增的 routing terminal。独立仅追加固定列表
与清单字段 `legacy_proof_maintenance_receipts` 登记同一 `proof-maintenance`
目录中的 schema 1 收据；当前只支持这一初始维护步骤。

收据绑定完整物理 before/after owner、layout/corrections 标识及唯一允许改变的
`src/common/demand.rs`。before 生产清单必须等于不可变 layout 安装清单，外测
逆既有 correction 后必须等于原 layout owner。当前完整生产/外测文件清单与
字节先核验 after，再仅在历史投影中逆维护补丁，随后执行原 layout 严格逆变换。
当前来源清单仍记录新字节，旧 origins、consolidation、layout 和收据均不改写。

本次五处 `usize::from(State::Want/Closed)` 明确原子状态比较的操作数类型，
避免现有 tracing fmt/json 组合引入 `json-codec` 的 `usize` 比较实现后产生类型
推断歧义；不改变 `SeqCst`、状态转换或增加运行时检查。独立回放与篡改负测见
`test/tools/rust-sources/test_legacy_http_maintenance.py`；证明不替代实际组合编译
和 demand 行为回归。
