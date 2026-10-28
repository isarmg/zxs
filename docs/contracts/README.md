# 核心契约附件

[文档目录](../README.md) · [盘格式](../08-full-storage-and-disk-format.md#s08-06) · [API](../15-api-and-errors.md#s15-10) · [当前范围与限制](../23-delivery-and-decisions.md#s23-05)

本附件为 `0.1.0-draft`，用于使部分规范可以执行字段与向量校验。仓库已有核心、服务端和客户端实现；本附件的覆盖仍限于下表，不表示完整 API 或盘格式已冻结，也不替代实际 I/O 和平台验收。

## 文件与覆盖范围

| 文件 | 当前覆盖 | 尚未证明 |
| --- | --- | --- |
| [core.schema.json](core.schema.json) | UINT64 精确上界、实体 ID、Branch / Commit 待写入映射、泛型 Entry 及文件分支投影、唯一管理员认证请求/响应、版本描述、内容提交请求、操作结果、完整上传状态、内容保存 PUBLICATION 字段、当前有界快照、分页历史根、共享索引/引用/叶页及路径证明形状 | 完整命名规则、父边和来源闭合、授权、实际 I/O、Tree/日志完整格式及所有管理操作 |
| [openapi.json](openapi.json) | 登录/当前会话/注销/改密、保存、结果查询、版本/校验/内容/上传状态、文件管理（含零正文分支创建、冻结父版本与自有版本分开的继承投影、目录分页与精确选择、目录批量分支准备查询和128字节单行备注）及管理员系统状态、真实功耗和监测历史 | bootstrap 与凭据恢复、工作区创建/上传写接口、事件及其余尚未建模的管理接口 |
| [config-constraints.schema.json](config-constraints.schema.json) | 第 21 章关键常量、分支冻结引用/首写物化、当前 JSON/CLI 资源额度及明确标注的分页/告警设计目标投影 | 当前进程 JSON Config、完整部署配置、物理身份、拓扑和启动状态验证 |
| [vectors.json](vectors.json) | 正反字段样例、旧账户权限字段拒绝、分支映射键/值和文件作用域拒绝、Branch / Commit 当前稀疏编码、快照35映射与历史根30映射、必需标识、共享引用种类/深度/未知字段及旧格式拒绝、路径证明节点数边界、规范字节与 BLAKE3、空文件和短文件分段清单 | 跨 Rust/Web/离线工具的一致实现及断电恢复 |
| [contract-check](../tools/contract-check.md) | Rust 实现的 schema/OpenAPI 有效性、两者定义一致性、固定向量、配置投影、文档本地链接和 JSON/TOML 语法校验 | 模型检查、平台测试、网络端点行为和性能 |

认证契约仅描述现有四个端点：`POST /api/auth/login` 返回 `token/csrf_token/principal/expires_at`，`GET /api/auth/me` 返回 `principal/csrf_token`，`POST /api/auth/logout` 返回 `logged_out`，`POST /api/auth/password` 以 `current_password/new_password` 换取 `changed/reauthentication_required`。响应不带角色、禁用或仓库准入字段；请求和响应 schema 均拒绝额外字段。Cookie 与 Bearer 两条认证路径共享当前唯一管理员，Cookie 变更请求仍需 Origin/CSRF。Schema 不替代密码验证、撤销持久性或真实 HTTP 安全测试。

仓库与备份契约不保存 `grants`、`account_scope_repository_id` 或 `can_manage`。对象归属、只读别名、主分支绑定、工作区所有者、幂等主体和既有额度仍保留；移除用户 ACL 不代表解除业务限制。

`ProtectionPolicy.inheritable` 是必填布尔值：`true` 为现行 scope／路径规则，`false` 为仅供显式文件绑定使用的保留设置，不参加精确 scope 或祖先继承。缺失、null、字符串和数字角色均拒绝，不补默认值、不迁移旧记录。正向量覆盖现行路径、目录、文件及保留设置，核心/OpenAPI schema 与 Rust 编解码共同验证两种角色。保留设置可有有效的本地 scope 引用，但必须有文件绑定；scope 引用有效性、唯一现行规则及绑定归属由核心校验，不冒充 schema 的能力。重配沿用现行 ID 并推进原代数；范围外绑定保留旧设置，空现行 scope 保留，无引用的保留设置清理。导入完整验证并沿用包内稳定身份，无效 scope 直接拒绝；拆分才清除不能映射的 scope 并降为保留设置。两者均保留规则角色，目标规则按既有流程从代数 `1` 开始，不新增格式版本号或改变 UTC 约定。

配置附件不定义新的进程输入。第 21 章 TOML 只是文档：`max_workspaces` 对应引擎 JSON 字段，`http_transfer_slots` 和两组 `http_spool_*` / `smb_spool_*` 对应现有 CLI 入口；可调值按范围校验，不把示例默认值钉死为 `const`。HTTP 上传/读取各有独立传输池，两协议 spool 又分别计费；每组 spool 要求所有者额度至少 64 KiB、总额不低于所有者额度且恢复余量为正。`per_account` 保留既有所有者计费含义，不表示多账户产品模型。固定后端/SMB 连接、会话和句柄边界只在正文说明，不伪造配置开关。

配置附件中的通用分页、事件重放窗口、工作区提醒和 `observability` 字段保留为受检设计目标，不表示对应配置入口或完整机制已实现。当前文件管理目录分页已由 OpenAPI 描述；现有事件轮询的序号、1000 项上限和未实现的续订边界见[API 规范](../15-api-and-errors.md#s15-09)，不能与配置附件的目标混同。配置向量覆盖独立协议预算、非默认有效值、零值、64 KiB 下界、UINT64 上界、交叉关系和旧资源字段拒绝。投影提取对 `resource_limits` 保留完整字段集以拒绝旧字段混入；其他目标配置章节仍只提取 schema 明确列出的字段。通过检查不证明整份 TOML 可部署，也不证明实际启动或 I/O 行为。

成员数不是固定三盘限制：配置投影支持任意非空固定成员集，决策与恢复多数均为 floor(N/2)+1；正常发布和确认仍要求全部已配置成员。单存储盘是最小配置，后续备份层可选。配置中的三个成员只是示例，不允许按当前可访问盘数自动缩减成员或门槛。

PUBLICATION 的结构允许非空、按身份排序且无重复身份的回执集；不同内容但相同 disk_id 仍须拒绝。Schema 和独立向量没有可信成员配置，不能证明回执集完整；运行时必须对照持久化的可信配置验证精确成员身份和数量、全部成员完成屏障及成员代际，不能用回执数量推导新成员集。单/双成员字段向量不是三成员池缺失回执的授权。

PUBLICATION 中回执使用合成身份与摘要；通过字段或覆盖集合检查不证明磁盘曾同步。展开清单只是字段语义，生产日志须使用有界分页引用并保留完整证明闭包。成功保存后当前健康变为不可用的样例，只验证这两个维度可以分别表达，不模拟真实故障。

文件大小等大整数使用十进制字符串；schema 的 UInt64 正则精确限制最大值。语义检查另验证盘位连续、物理成员不重复、所需内容证明覆盖、上传完整性及 HTTP、SMB 各自独立的 spool 所有者/总额关系。OpenAPI 内嵌与 core 相同的定义供工具离线使用，校验器会拒绝二者漂移。

持久状态、写请求和固定向量的 JSON 解析先拒绝重复字段、BOM、孤立代理项和浮点数，再做 schema 和 JCS；解析后只检查普通字典无法找回重复字段信息。`/api/system/status` 与 `/api/system/history` 的临时监测响应允许有限小数，不放宽持久状态或写请求规则。固定向量中的 JCS 属性排序包含非 BMP 键样例，用于检测错误地按 Unicode 标量顺序排序；该向量不表示 PortableName 允许 emoji 文件名。

备注写接口通过 `x-maxUtf8Bytes: 128` 标明 UTF-8 字节上限，配合控制字符模式和服务端字节计数校验；JSON Schema 的 `maxLength` 本身只计算 Unicode 字符数。文件、版本和节点备注在写入及历史包读取时均遵守当前128 UTF-8字节、无控制字符规则；不合格式的历史包被拒绝，不截断或迁移备注。读响应原样输出已验证备注；响应 schema 的较宽字符串形状（包括 `FileHistoryNode.note`）不替代引擎语义校验，也不承诺旧格式可读。

## 运行校验

使用仓库 Rust 工具链，从仓库根执行：

```bash
CARGO_INCREMENTAL=0 cargo run -p contract-check \
  --locked --offline --target-dir target/native
```

Rust 依赖源码已包含在仓库中，首次构建也使用唯一根锁文件和 `--locked --offline`，不要求预先缓存或下载 registry 依赖；Rust 工具链及必要系统构建工具须另行准备。校验过程不访问网络、不写数据盘，也不生成业务对象。OpenAPI 官方元 schema 已固定随工具保存，JSON Schema 解析禁用网络与文件远程引用功能；自研规则只有 Rust 实现。校验失败返回非零退出码；输出 PASS 仅表示上表声明的文档契约范围通过。

## 冻结前仍需完成

1. 补齐尚未覆盖的现有 API 请求、响应、分页和错误，以及 bootstrap、凭据恢复与会话生命周期的边界测试。
2. 补齐 Commit/Tree 分页、日志帧 payload、成员回执、PUBLICATION 分页清单及各类恢复记录 schema；覆盖截断日志、中间损坏和未知必需特性。
3. 固定 Unicode 16.0.0 表及校验值，交付可复用命名向量；完成 Rust、浏览器与离线工具交叉验证。
4. 按[第 22 章](../22-acceptance.md#s22-10)执行发布证据模型、真实 I/O 与 SMB 样机，并使用[报告模板](../templates/validation-report-template.md)保存证据。

字段与文本变更须同步更新固定向量和 OpenAPI；未经变更说明不得用重新生成期望值来掩盖兼容变化。外部规格依据为 [JSON Schema 2020-12](https://json-schema.org/draft/2020-12)、[OpenAPI 3.1.0](https://spec.openapis.org/oas/v3.1.0.html) 和 [RFC 8785](https://www.rfc-editor.org/rfc/rfc8785.html)，这些资料不为本系统业务协议提供正确性证明。

仓库记录必须显式携带 `smb_enabled`，缺失不能默认启用；同时必须携带 `smb_binding_generation`，按规范 UINT64 十进制字符串编码；新仓库初值为 `"1"`。它独立于普通历史发布代数，每次显式重绑定或禁用推进，用来拒绝持有旧绑定的 SMB 会话。拆分禁用源仓库也推进其代数；新拆分仓库和导入仓库保持 SMB 禁用并从自己的初始代数开始。此字段为当前持久模型和 `HistoryRepository` schema 的必填项，不为缺失字段的旧记录补默认值。普通目录组成选择不推进此代数，也不改变已固定读取版本。

分支创建只登记元数据与冻结来源引用，`pending_file_versions` 的键/值分别为 FileId/VersionId；Commit 逐快照保留此状态，当前分支首写不会改变旧快照。泛型 Entry 及文件管理的 version_id 都指可见真实版本；继承时为冻结父 ID，own_version_id 为 null，首次写入后才为自有 ID。新增校验同时拒绝 pending 来源逃逸文件作用域、快照来源与条目不符，以及可见父内容被误标为已拥有内容；Branch / Commit 的空映射采用当前稀疏编码省略序列化，并核对规范字节与摘要；这不接受其他旧格式字段。

目录分支以 `DirectoryBranch`、`DirectoryTree` 和 `DirectoryReference` 区分命名身份、不可变组成及子目录精确引用；`DirectoryNamespaceBatch` 每项要求源分支、依据子树与稳定条目 ID，非删除操作还要求明确目标分支及依据子树。登记目录分支不提前创建文件分支或占位文件。路径、跨分支冲突与实际归属由核心统一校验，schema 合法不等于业务可发布。

`FilePathNode` 描述重命名的路径节点并引用现有正文，`FileHistoryNode` 把内容与路径节点放在同一历史图；`FileNodeId` 允许 `ver_`、`fnode_` 与 `fmeta_`，正文读取仍只接受真实 `VersionId`。`FileContext.node_id` 和 `Branch.file_node_id` 冻结具体图节点，创建分支可以传 `source_node_id` 保留此来源。节点备注独立于正文版本备注保存，使用同一128 UTF-8字节、无控制字符规则。

`FileMetadataNode` 保存不可变标签/合并子节点，`FileBranchMerge` 独立保存全部合并来源和安全图边。文件合并请求要求明确确认、结果归属及每个参与head，标签批量请求固定源节点与branch head，scope备份配置同时固定稳定身份和自身代数。标签的16项/128字节限制、无控制字符和全部预检由共享Rust验证及业务事务执行；schema检查不代替真实发布。目录分支合并待用户确认，暂不实现，能力为false。

`Snapshot`、`PagedHistoryRoot`、`MetadataIndex`、`MetadataReference` 和 `MetadataLeaf` 描述本次有界根实验源码的存储形状。两个根都必须携带 `format: "radix-map-roots"`，这是布局语义标识，不是项目发行版本号；分别要求准确35个 State 映射和30个历史映射，空映射也有引用。快照的三个代数使用 UInt64 十进制字符串，events 是独立映射根，control_auth 引用独立 `state-control-auth` 对象；该对象仍按原 `Option<Value>` 编码（包括 null）。历史根内仓库/历史身份使用 `HistoryRepository`、`HistoryIdentity`。`Candidate.history_roots` 绑定 history ID→manifest hash 的映射根，替代旧 `history_manifest_hashes`；当前 Candidate 自身没有 format 字段。原 `FileFeatureStateFields` 仍只是 State / 逻辑 HistoryManifest 的局部投影。OpenAPI 只镜像存储组件，未改变 HTTP DTO 或新增端点。

字段向量使用合成摘要，只检查 schema，不能证明引用对象存在或摘要正确。已有历史根正反向量均使用有效 marker，另行覆盖 markerless 根、旧 `history-index` 引用和旧 flat Snapshot 的拒绝，避免负例只因缺 marker 通过。原固定 canonical bytes/BLAKE3 向量保持不变，不由被测编码器生成期望。

`InclusionProof` 只描述 `{nodes: [...]}` 的对象节点外形及1..65数量上限；65节点正例仅证明 schema 边界，不是有效 Merkle 路径。真实验证还须按根引用逐步核对域哈希、路由与前缀、叶界限、准确目标值，以及截断/多余节点。规范编码字节上限（128项、64 KiB 多实体页目标、不可分单实体页 MAX_METADATA 上限）、events 的20位 ordinal 与连续性、图闭包及完整历史语义均由核心验证，不冒充 JSON Schema 的能力。`PackageProof.history_root_proof` 绑定目录中的单个 history，`ContentTombstone.content_state_proof` 绑定准确快照中的目标状态；完整 Candidate、PackageProof、墓碑和回执闭包尚未全部建模。单路径证明不能替代完整恢复的 catalog walk，也不携带其他历史子图。发行格式仍未冻结，合入与验证结果另行记录；参见[第8.7节](../08-full-storage-and-disk-format.md#s08-07)和[ADR 0006](../adr/0006-bounded-metadata-roots.md)。

规范 JSON 整数写出优化的边界、差分测试与完整编码测量方法见
[规范 JSON 编码验证](../validation/canonical-number-encoding.md)。

工作区 capacity 的两个 scope 均新增必填 `cold_records`（规范 UInt64 十进制字符串）；`records` 只计未冷存驻留记录，`metadata_bytes` 仍含全部完整清单与生命周期预留。`automatic_retirement` 是布尔能力字段，当前 Engine 返回 true，仅指普通空 Aborted 原路径冷存，不能推断通用清理或无限历史。缺少新字段的旧响应不兼容；Schema、OpenAPI、固定正反向量与 Rust/Web DTO 同步，仍须执行契约验证后才能声称通过。
