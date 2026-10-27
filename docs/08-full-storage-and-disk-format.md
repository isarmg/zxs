# 08 全量存储、盘格式与单盘恢复

[文档目录](README.md) · [上一章：数据模型、身份与索引](07-data-model.md) · [下一章：用户指定的分层备份拓扑](09-backup-topology.md)

<a id="s08"></a>

完整文件、副本材料、规范编码、盘上布局和离线恢复。

## 本章目录

- [8.1 完整版本与独立文件系统](#s08-01)
- [8.2 单盘布局](#s08-02)
- [8.3 版本与完整内容描述](#s08-03)
- [8.4 全量封存与验证](#s08-04)
- [8.5 单盘恢复与外部修改](#s08-05)
- [8.6 盘格式与规范编码](#s08-06)
- [8.7 分页历史清单与独立历史包](#s08-07)

<a id="s08-01"></a>

## 8.1 完整版本与独立文件系统

每盘独立 Btrfs、UUID 和设备身份，不组跨盘条带。保持文件数据校验；NOCOW 会隐含关闭文件数据校验，不能借禁止共享为由全盘启用。[S10][S16]

逻辑层记录不可变 FileVersion，内容层只保存 FULL 普通文件，放置层记录每个指定物理盘上的完整副本。修改、追加或截短后保存，最终都生成新完整文件，不依赖前一版本补齐字节。

| 场景 | 持久化结果 | 读取与恢复 |
| --- | --- | --- |
| 普通内容保存 | 变更文件的新完整版本及校验清单 | 直接范围读或离线打开，无旧版本依赖 |
| 新建文件分支 | 仅发布记录及冻结来源引用，不创建占位文件或内容版本 | 可见内容继承冻结父版本；首次写入拥有 FULL，切换不影响其他文件 |
| 目录分支与子树 | 分支引用冻结文件版本并复用共享文件分支，修改仅生成不可变子树 | 不复制正文或创建私有文件分支，命名分支 ID 和名称保持 |
| 旧关系标记 `MergeLink` | 仅持久化两个已有节点间的元数据关系 | 不产生内容文件，不推进HEAD或目录引用 |
| 文件分支合并 `FileBranchMerge` | 复用已有节点，或新增复用原正文的合并元数据节点；保存来源和新目录引用 | 归属继续写入、其余输入保留只读历史；原正文与证明不重写，见[合并规则](design/file-merge-tags-backup.md) |
| 历史树拆分 | 新树保留文件的独立完整副本及元数据 | 两树可以分别导出和恢复 |

网络范围上传、服务端读取旧版未变范围，只能作为构建工作文件的过程；封存的正式版本仍是完整文件。接口不提供其他保存模式及转换操作。

<a id="s08-02"></a>

## 8.2 单盘布局

```text
<disk-root>/
├── README.txt
├── disk.json
├── histories/<history-id>/
│   ├── versions/<version-id>/
│   │   ├── version.json
│   │   ├── checksums.bin
│   │   ├── payload.json
│   │   ├── commit-proof.json
│   │   └── publication-proof.json
│   └── file-containers/<file-id>/
│       └── 报告.docx/
│           ├── container.json
│           └── versions/
│               ├── v000001_20261018T133000Z_ab12cd34.docx
│               └── v000002_20261018T134500Z_ef56ab78.docx
├── metadata/application-log/journal.bin
├── metadata/objects/<object-type>/<hash前两位>/<hash>.json
├── metadata/pool-membership/config.json
├── metadata/publication-seals/<candidate-hash>.json
├── staging/<operation-id>/
└── quarantine/<incident-id>/
```

所有新初始化存储盘（含非元数据成员）均持久保存 `metadata/pool-membership/config.json`，作为本盘核验固定池成员配置的锚。缺少该锚的旧盘不兼容；读取和离线检查不补写配置、不从回执数量推导成员集合，也不自动迁移。

不可变元数据对象按散列前两位分桶，每类最多 256 个桶，完整散列文件名、规范编码、摘要和日志引用保持不变。当前格式只接受 `metadata/objects/<object-type>/<hash前两位>/<hash>.json`；目标地址存在旧平铺对象时拒绝读取与写入，清点拒绝非分片项，不自动迁移、删除或复用平铺对象。按准确摘要修复当前格式的依赖仍然有效；离线导出只读且不修复。目录枚举和恢复检查点必须拒绝链接及错误分桶。

`versions/<version-id>/` 保留不可变版本说明、校验及发布证明；正文唯一实体位于 `file-containers/<file-id>/<当前文件名>/versions/<storage_name>`，是可以直接读取的普通文件。外层 `file-id` 为稳定身份，内层是实际目录，不是界面别名。业务改名真正重命名该内层目录；历史版本文件的内部名称及已有证明保持不变。下载当前或历史版本时使用该文件当前的业务名称，已删除文件保留最后登记的名称。

位置投影的 namespace 根维护内部标记 `.zxs-namespace.json`。新候选在复制正文副本前预检实际主盘及备份投影的根部分量冲突，按[命名约束](05-naming-and-paths.md)在决定前返回 `NAME_COLLISION`，保留现有状态、正文和工作材料；不改标记或容器格式，不迁移或清除历史材料。已经发布的历史投影仍按固定身份和现有布局校验重放，不重新套用新请求的准入检查。

版本内部名称为 `v<文件内序号>_<UTC时间>_<内容散列前8位>.<扩展名>`，例如 `v000001_20261018T133000Z_ab12cd34.docx`；序号至少六位，按同一文件已有版本递增，不等同于全局 `version_sequence` 审计序。时间另以 RFC3339 写入 `created_at`。新版本扩展名采用当前文件名；没有扩展名时不追加句点。它们不使用容器目录名称作为版本身份。

分支的 pending_file_versions 是已发布的继承归属记录，不对应空文件。新创建、恢复和导入均不生成 pending-branches/*.pending，也不为继承引用新建空容器。容器内出现 pending-branches 或 legacy 项时返回 FORMAT_UNSUPPORTED，保留包括零字节占位文件在内的原始材料，不自动清理或迁移。container.json 的 inherited_branches 是来源引用缓存，可见 version_id 等于父真实 ID，自有版本为空。

文件备注、版本备注、当前名称和选中分支在已发布 State 中持久化；容器内 `container.json` 只是从该状态重建的浏览缓存，标记 `cache_only`，不替代不可变 `version.json`、校验和或本盘证明。修改备注不改写旧版本描述。缓存的完整 JSON 在 ACCEPT 前通过实际编码器校验单对象大小，并暂存在匿名文件中；准备过程不更改已发布的物理名称。新事务若超过缓存读取上限，会在写入决定记录前返回 RESOURCE_LIMIT，保留原发布状态。名字与缓存投影在事务决定后、PUBLICATION 前执行；中断后可以按已确定决定重放，不能把单次目录重命名成功当作发布完成。

`storage_name` 与 `created_at` 是当前版本结构的必需字段，缺失时拒绝读取，不回退到旧正文布局。准备、修复或历史包处理中的 `content/<creation_name>` 是相应流程的中间材料路径，不构成旧盘格式兼容承诺。新版本准备正文验证后在同一文件系统上独占重命名到容器，并同步源、目标目录；正常完成后不保留第二份准备正文。

每份有效副本包括完整内容、版本说明、校验及本盘证明。commit-proof 证明候选和本盘内容准备，publication-proof 携带[第 10.10 节](10-transactions-and-recovery.md#s10-10)的公开决定及本盘验证闭包。只有前者时，不把文件标为已完成正常发布。不允许从其他盘、其他树或在线服务补齐内容或必需证明后才声称本盘可以独立恢复。

<a id="s08-03"></a>

## 8.3 版本与完整内容描述

```json
{
  "history_tree_id": "history-example",
  "version_id": "version-example-002",
  "file_id": "file-example-001",
  "creation_primary_slot_id": "P01",
  "logical_size": "1048576",
  "version_sequence": "2",
  "content_hash": "<完整字节BLAKE3-256>",
  "checksum_chunk_bytes": 262144,
  "checksum_manifest_hash": "<分段校验清单哈希>",
  "creation_name": "报告.docx",
  "storage_name": "v000002_20261018T134500Z_ef56ab78.docx",
  "created_at": "2026-10-18T13:45:00Z",
  "source_version_ids": ["version-example-001"],
  "created_as_full_checkpoint": false
}
```

```json
{
  "payload_id": "payload-example-002",
  "version_id": "version-example-002",
  "kind": "FULL",
  "size": "1048576",
  "content_hash": "<与版本描述一致的完整字节哈希>"
}
```

例中 ID 和摘要是占位符，不是真实测试向量，`storage_name` 的短散列必须与实际 `content_hash` 前八位一致。`storage_name`、`created_at` 是当前结构的必需字段；缺失时拒绝读取，不回退到旧正文布局。source_version_ids 仅表示审计来源；读取不访问源版本。健康与清除状态由事务日志和事件决定，不能原地改写 version.json 掩盖损坏或删除。

<a id="s08-04"></a>

## 8.4 全量封存与验证

使用显式读写创建新普通文件，处理短写、空间不足、校验、文件与目录同步。禁止用硬链接、reflink、快照或隐式去重冒充独立完整文件。新版本封存后不原地更新；备份修复恢复相同字节，不改变版本身份。

来源可以是完整上传、服务端工作文件或已有完整版本；source_version_ids 只记录审计来源，不形成真实 FULL 的读取依赖。创建分支只发布冻结引用，不读取来源正文或新建空文件；实际写入才封存独立 FULL。目录分支复用共享文件分支，修改只生成目录子树，不自动增加命名分支。元数据登记与内容保存分别报告，不把引用当作新增内容保护成功。

新正文从准备路径移动到容器时使用目录描述符相对的独占重命名，拒绝覆盖目标；重试遇到目标已存在，先核对长度和完整散列，再清理同内容准备副本。稳定文件目录出现多个命名容器、符号链接或正文硬链接时明确失败。读取缺失正文仍报告预期路径，供既有丢失分类和修复流程处理，不凭空创建替代文件。

<a id="s08-05"></a>

## 8.5 单盘恢复与外部修改

离线工具扫描版本描述、内容墓碑、本盘清单和证明，验证后直接导出完整文件。尚未完成的提交不冒充已发布版本；文件还在但证明不全时标记未知并保留。普通数据盘只保证恢复本盘实际持有的内容，全局历史依赖完整控制元数据恢复包。

拆盘按相应 Btrfs 版本的只读且避免日志重放写入的方式挂载；单写 ro 不代表所有恢复性写入均被阻止。[S21] 外部修改后重新接入须隔离并显式导入为新版本，不接受为旧不可变版本的合法原地更新。不能两台服务器同时写管理同一盘。

<a id="s08-06"></a>

## 8.6 盘格式与规范编码

当前唯一盘格式采用以下约束；对应机器可读 schema、固定测试向量及未覆盖范围见[契约附件](contracts/README.md)。文档中的简写 ID 只用于说明。实际 ID 为实体前缀、下划线及 32 位小写十六进制随机值；前缀固定为 pool/repo/history/topology/placement/br/commit/link/tree/entry/file/ver/payload/rep/plan/ws/tx/op/disk/lease/session/recovery/req。随机值来自密码学安全随机源，创建时仍检查唯一性；已登记ID不重新生成。用户指定的slot_id另用稳定ASCII标识（如P01/B01），限定字母开头、后接字母数字或下划线/连字符、最多64字节；大小写不敏感唯一，改展示名不改其引用身份。

| 项目 | 规范要求 |
| --- | --- |
| JSON 编码 | UTF-8，无 BOM；拒绝重复字段、孤立代理项和不符合 schema 的类型；写盘使用 RFC 8785 JCS 规范表示 [S23] |
| 字符串 | 名称在业务校验阶段检查 NFC；哈希阶段不得再次规范化、折叠大小写或改写字符串 |
| 数字 | 文件大小、偏移、日志序号、代际及 token 统一为无符号 64 位十进制字符串，范围 0～18446744073709551615，无正号或多余前导零；小型枚举和段大小用 schema 限定的整数，禁止浮点字段 |
| 数组顺序 | Tree 条目按 lookup_key 的 UTF-8 字节序排序；同键冲突先拒绝；副本成员和证明回执按 disk_id 排序；有业务顺序的操作数组保持原顺序 |
| 对象哈希 | 元数据采用 `BLAKE3("zxs:" + object_type + NUL + JCS(payload))`；类型标记为 ASCII，NUL 是单个零字节；自包含的 hash 字段置于外层，不参与 payload，避免自引用 |
| 内容哈希 | 对完整内容字节计算 BLAKE3-256；不套用元数据域前缀，空文件也有确定哈希 |
| 分段清单 | 30 字节头：7 字节 ASCII 类型标识 `ZXSCHKS` 后接 NUL、u16 算法编号 1（BLAKE3-256）、u32 段字节数、u64 逻辑长度、u64 段数；整数小端。随后为顺序 32 字节段哈希，无尾部扩展；长度须等于 `30 + 32 × 段数` 且段数符合内容长度。空文件段数零，尾段按实际长度计算，整份原始清单哈希写入描述 |
| 日志帧 | 12 字节头：8 字节固定魔数 `ZXSLOG\0\0`、u32 payload 长度，整数小端；随后为 JCS payload 和 32 字节 BLAKE3(头+payload)。payload 内有记录类型及前帧哈希，首帧前哈希为 64 个零；payload 最多 16 MiB，越界先拒绝分配。尾部半帧不构成记录；中间损坏或有效分叉不能跳过 |
| 大型元数据 | Tree 分页为不可变节点，候选通过哈希引用依赖清单；日志不内嵌无限增长的文件列表，所有引用依赖须先按相同保护规则持久化 |
| 扩展与未知字段 | 仅 schema 声明的可选扩展可忽略其展示意义，但必须保留并参与哈希；未知必需特性拒绝写入和自动重放，已知普通内容可在只读导出模式单独验证 |

请求摘要也使用有类型标记的规范 payload，字段集合及排除规则见[第 15.7 节](15-api-and-errors.md#s15-07)；服务端自行计算并比对，不能信任客户端给出的摘要。JSON 网络传输可以带空白，参与哈希时必须归一到同一 JCS 字节序列。[S23][S24]

当前只维护一套存储格式，不提供旧格式读取分支或自动迁移流程；发行版本号不代表存储格式代际或兼容承诺。读取严格校验字段结构、固定类型标识和域哈希；不匹配的材料失败关闭，不自动删除、覆盖或转换。当前格式中已发布的历史候选及其依赖仍必须按准确摘要恢复，不得重新编码替换证据。完整文件导入另按[第 10 章](10-transactions-and-recovery.md)事务执行。

必须提供跨 Rust、浏览器和离线工具一致的正常、边界整数、重复字段、字段重排、未知必需特性、截断帧及位翻转样例。未经这些样例验证，不能把示例 JSON 当作已冻结的可互操作格式。

当前可校验的字段契约、固定样例及验证入口见[契约附件](contracts/README.md)。附件逐项声明覆盖范围；尚未交付的 Commit/Tree 分页与完整日志回执仍是冻结门槛，不能因部分 schema 通过便宣称盘格式已冻结。各读取器须针对同一结构定义通过恢复样例，不提供旧格式兼容承诺。

<a id="s08-07"></a>

## 8.7 分页历史清单与独立历史包

[ADR 0004](adr/0004-paged-history-manifests.md)的分页历史图由[ADR 0006](adr/0006-bounded-metadata-roots.md)统一为有界元数据根。本次实验源码只维护一种格式；语义 marker 不等于发行版本或兼容分支，也不表示已发布或完成物理设备耐久验收。

候选必须携带 `history_roots: {kind, hash}`，引用 history ID→`history-manifest` 域摘要的 radix 目录，不再接受 `history_manifest_hashes` 平面映射。历史根字段为必需的 `format: "radix-map-roots"`、`repository`、`history`、`maps`。`maps` 必须包含全部30个固定映射字段，连空映射也有显式引用；不得遗漏字段或接受未知字段。展开后的 `HistoryManifest` 是语义模型，其扁平 JSON 摘要不是分页根身份，不能替换目录中已绑定的摘要。完整恢复验证目录全部条目、history ID与根身份及每个历史图，不从 State 重新计算另一个目录冒充原绑定值。

| 对象域 | 内容与边界 |
| --- | --- |
| `history-manifest` | 必需 `radix-map-roots` 标识、仓库/历史身份及固定30映射集合 |
| `metadata-index` | `prefix` 为0..63位小写十六进制；`children` 为1..16个单十六进制位键的引用。按键文本 UTF-8 字节的 BLAKE3 摘要（不另做Unicode规范化）逐位分路，最多64层 |
| `state-page` | 普通 JSON 实体映射；每页最多 128 项，64 KiB 为多实体页的拆分目标；不可分单实体可独占一页，但整个编码页仍不得超过 64 MiB 读取上限 |

Snapshot、历史目录及历史投影共用同一 radix 算法；引用 `kind` 仅允许 `state-page` 或 `metadata-index`，不接受旧 `history-index`。域哈希分别使用 `zxs:state-page\0` 与 `zxs:metadata-index\0`；字段与 owner 语义仍由准确根和准入检查约束。分区参数属于格式常量，与本次准入预算无关。根、索引、叶页和候选/组合证明均先冻结规范字节并检查实际大小，再复制新正文及推进 PREPARE/ACCEPT；全部配置成员先安装依赖，后推进决定。每层读取验证域哈希、引用种类、前缀、深度和条目归属；拒绝重复实体、未知字段、空索引和非根空叶页。缓存对象在不同逻辑位置复用时仍检查前缀，不能跳过语义校验。上述映射字段与对象形状见[机器契约](contracts/README.md)，图闭包与历史语义仍由核心验证。

新包布局为：

```text
<history-package>/
├── manifest.json                         # 准确分页根
├── publication.json                      # 与根版本一致的 PackageProof
├── objects/metadata-index/<前两位>/<hash>.json
├── objects/state-page/<前两位>/<hash>.json
└── versions/<version-id>/                # 完整正文、版本描述及 checksums.bin
```

`publication.json` 包含 `manifest_hash`、原 `candidate`、`publication`、`publication_seal` 和必需的 `history_root_proof`。证明链为：发布闭包 → `Candidate.history_roots` → 单个 history ID 的目录 inclusion proof → 准确根 → 该历史全部可达索引/叶页 → 逻辑语义 → 完整正文和校验清单。证明为 `{nodes: [...]}`，按根到叶顺序携带1..65个实际JSON节点；每层验证域哈希、前缀/路由和叶界限，拒绝目标值不符、截断或多余节点。它不携带整目录或其他历史子图，不能替代恢复时的完整目录验证。`HistoryPackageReport.manifest_hash` 表示绑定根，不是展开清单摘要。包必须携带全部可达依赖；离线检查和恢复不依赖原池、SQLite 或网络。可选的展开便捷文件不是权威输入，不能替代缺失图对象。没有外部可信成员配置时，独立包验证只证明包内证据与内容完整、自洽，不独立证明来源池的成员权威或最新状态。导入先验证全包，再执行身份冲突检查、路由重绑定、正文复制及目标池新的本地发布；外来发布不能替代目标池的配置锚和发布权威。

备份前缀优先按包内真实版本的原路由位置重绑定。空目录等尚无版本路由证据的设置，只在其备份槽位 ID 与目标备份槽位精确相同时保留前缀，不按名字排序猜测层级；已知原路由中因减少份数而排除的层级不会重新加入。目标新增层级或未知改名槽位缺少前缀时拒绝导入，不同前缀映射到同一目标槽位时拒绝冲突，不先发布部分结果。

导出选择与当前逻辑历史匹配的原发布证据并复制其准确对象图。根的必需语义 marker 不是项目发行版本字段；读取采用严格固定形状，拒绝 markerless/flat 根、旧目录字段及旧索引引用，不升级证明、不兼容双读、不迁移或删除已有数据。拒绝后保留原材料，不以重置运行池解决格式差异；实现格式变更不授权真实部署。当前格式生成的保留历史仍按准确绑定证据恢复。

有界根移除全页 hash 列表和全部 history 根映射随总量增长的根瓶颈，但单个大型实体（例如 `Commit.entries`、完整控制认证对象）、本次 Candidate.versions、成员集合及组合证明仍受实际字节与元数据上限约束。Snapshot/catalog 编码按可信写集更新路径，预算使用驻留精确摘要；完整旧依赖读验、碰撞比较和全部成员同步仍保留，history 投影仅稳定归属分支值更新使用窄增量。一般 owner closure、冷准入/故障重建及其他全量成本仍在，不得宣称端到端 O(delta) 或性能已经提高。

[S10]: https://btrfs.readthedocs.io/en/latest/btrfs-scrub.html
[S16]: https://btrfs.readthedocs.io/en/latest/Reflink.html
[S21]: https://btrfs.readthedocs.io/en/latest/btrfs-man5.html
[S23]: https://www.rfc-editor.org/rfc/rfc8785.html
[S24]: https://www.rfc-editor.org/rfc/rfc8259.html


### 当前快照与元数据对象读取边界

元数据对象只支持 `metadata/objects/<kind>/<hash前两位>/<hash>.json`。目标摘要存在旧平面文件时读取和写入均拒绝；对象清点拒绝非分片项。程序不自动搬迁、覆盖或删除旧平面文件。

Snapshot 必须携带 `format: "radix-map-roots"`、十进制 UInt64 字符串 `sequence/business_generation/topology_generation`、准确35个 `maps` 根、独立 `events` 根及 `control_auth: {kind: "state-control-auth", hash}`。control_auth 正文仍为原 `Option<Value>`（包括 null），作为独立对象认证，不把它解释为新的认证 DTO。events 使用20位补零 ordinal 文本键；完整读取验证从0连续排列并还原数组，空事件同样有空映射根。拒绝旧 `scalars/pages`、页 hash 列表及缺 marker 结构。Candidate 自身没有 format 字段，其必需 `history_roots` 定义当前目录结构；配置、盘封印和检查点也不添加发行版本号。

候选绑定的准确快照根、索引、叶页及独立控制认证对象可以分别从不同的可信元数据成员读取；不要求单个成员恰好保存完整闭包，但全部依赖都必须通过各自摘要及结构校验。副本选择不使用多数票，不尝试替代根或重新编码状态来补齐缺页。该读取过程不改盘；持有写者锁的全部配置成员恢复路径才修复这些已认证对象的副本。缺少任何必需的有效对象时拒绝恢复，不承诺从任意损坏中重建。

内容墓碑的 `content_state_proof` 替代嵌入单个 `content_state_page`。验证链为封印与发布 → Candidate →准确 Snapshot/序号 → `maps.content_states` → 目标版本的路径证明与 Purged/Migrated 状态；原本地版本发布、配置成员和盘封印校验保留。清除按 key 取得路径证明，先持久化墓碑再删除正文并同步父目录；证明缺失或不符不得删正文。

恢复已验证的发布闭包时，本地 `commit-proof.json` 或 `publication-proof.json` 若与权威证明冲突，先把原普通文件移入其所在证明目录的 `proof-quarantine/`，再重建准确证明；不会把冲突文件当作另一发布结果的依据，也不删除它。链接、特殊文件或无法安全访问的路径仍失败关闭，不以健康副本绕过路径安全检查。

---

[文档目录](README.md) · [上一章：数据模型、身份与索引](07-data-model.md) · [下一章：用户指定的分层备份拓扑](09-backup-topology.md)
