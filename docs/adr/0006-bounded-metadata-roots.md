# ADR 0006：有界元数据根与路径证明

状态：本次 C 格式切换的实验源码决策，发行格式未冻结；不表示已发布、中央合入或验证门禁已通过。本决策补充 [ADR 0004](0004-paged-history-manifests.md)，不启用 [ADR 0005](0005-incremental-publication-integrity.md)的旧依赖免读。

## 背景与选择

分页历史图已经使用有界 radix，但 Snapshot 的全页 hash 列表和 Candidate 的全部 history 根平面表仍随总量增长。只扩大单对象上限或换个平面容器不能消除根瓶颈；把整个目录嵌入独立包也会扩大包体和披露范围。因此三个消费者共用 `metadata_map` 的确定性16路算法，不复制 State/History/Catalog 三套分桶器。

| 对象 | 当前结构与边界 |
| --- | --- |
| `Reference` | 严格 `{kind, hash}`；kind 仅 `state-page` / `metadata-index`，hash 为64位小写十六进制 |
| `MetadataIndex` | `prefix` 为0..63位小写十六进制；`children` 为1..16个单十六进制位到 Reference 的映射；不落盘可信计数或预算摘要 |
| `state-page` | 普通规范JSON映射；最多128项，多实体页最多64 KiB，不可分单实体可至 MAX_METADATA（64 MiB）；空映射有明确根，非根空叶拒绝 |
| `Snapshot` | 必需 `format: "radix-map-roots"`，三个 UInt64 十进制字符串代数，准确35个 maps 引用、events 引用和独立 control_auth 引用 |
| `Candidate` | 必需 `history_roots: Reference` 绑定 history ID→manifest hash 的目录，删除 `history_manifest_hashes`；Candidate 自身不添加 format |
| `PagedHistoryRoot` | 必需相同 format、repository、history 和准确30个 maps 引用 |
| `InclusionProof` | `{nodes: [实际JSON对象...]}`，按根到叶顺序，至多64个索引和一个叶，合法成员证明总数1..65 |

键路由仍为键文本UTF-8字节的 BLAKE3 摘要，不另做Unicode规范化；ID边界已规定的文本表示保持不变，逐 hex nibble 分路。索引对象域改为 `zxs:metadata-index\0`，叶仍为 `zxs:state-page\0`；对象域与哈希共同确定地址。实体语义和 owner 由 consumer 准入保证，同 hash 不等于同业务类型。

Snapshot 的 `sequence/business_generation/topology_generation` 不再藏于任意 scalars 对象。`events` 以20位补零 ordinal 为键，完整读取必须从0连续、无重复并恢复数组。`control_auth` 为 `{kind: "state-control-auth", hash}`，正文仍采用原 `Option<Value>`（包括 null）；它是一个独立完整逻辑记录，更新成本按其真实字节计，不声称拆分认证模型。

## 证据消费者必须同步切换

- **完整恢复/导入：** 验证 Candidate 绑定目录的全部条目、各 history ID 与 manifest 身份及所有历史图，并按相应入口执行逻辑 HistoryManifest/State 语义准入；Snapshot 验证固定字段、索引/叶页、events 和独立认证对象。不能从 State 重算目录根替代原绑定值。
- **完整巡检：** 实读 retained 对象闭包，检查域哈希、schema、引用边和ID覆盖；共用 `decode_history_root`、`MetadataIndex`、`PageShape` 等结构校验，不重新构造全量 HistoryManifest 执行逐项逻辑语义准入。巡检的覆盖报告不能冒充恢复/导入语义验证。
- **独立历史包：** `PackageProof.history_root_proof` 绑定目录中的目标 history ID→manifest hash。包链为发布/封印→候选→目录路径→准确 manifest→该 history 全图→语义与正文。只携带该目录路径，不携带其他history子图或全目录；离线验证不依赖原池、SQLite或网络，restore仍先完整验证，再向目标池重新发布。
- **内容墓碑：** `content_state_proof` 替代 `content_state_page`，绑定发布/封印→候选→准确Snapshot及序号→content_states根→目标版本的Purged/Migrated状态。本地版本发布、配置成员和盘封印验证保留；按key取得证明后先持久墓碑，再删正文并同步目录。
- **操作凭据点读：** operations/principals按准确key只读取所需路径，区分不存在和路径损坏，不把点读当作完整State准入，也不读取无关版本、提交或认证正文。

每个证明节点均按前一准确引用验证对象域哈希、前缀/分路、叶页界限和叶键归属；消费者要求精确目标值，拒绝额外节点、截断、错误路径或别的合法对象。路径证明的结构上限不是完整图验证的替代品。

## 编码、内存与预算的已实现范围

Snapshot 的单一字段声明同时生成编码/更新/解码覆盖，真实 `MapChanges::Keys` 仅编码对应键；OperationId先转原规范文本。events只在 `SequenceChanges::Append` 的真实谱系证明下更新suffix；未知谱系或任意可变访问使用 `All`，不凭长度猜append。catalog按受影响history更新对应路径。

`MapRoot` 以 Arc 路径复制保留键、节点和精确摘要，不保留全量 canonical JSON 缓存；根 clone 为 O(1)，总驻留内存仍为 O(keys + nodes)。变更叶及共享祖先按一批实际写集合并更新；split/collapse由精确条目数和canonical条目字节决定，避免分区随修改历史变化。冷准入、`All` 或故障后的准确字段重建可能为 O(field)。

正常元数据预检消费驻留的最大对象长度、实际最长目录 inclusion path，以及同一路径上的版本ID/状态/墓碑证明精确摘要，再组合本次真实 Candidate/Publication/Seal/Snapshot 外壳。空history目录没有成员证明，不能为不存在的包构造假想预算。没有磁盘自报summary，不用平均深度、假最长key或不同路径的独立最大值冒充精确值；恢复从准确绑定图的完整认证重建摘要。

history投影目前仅支持已存在且history/file归属稳定的分支值更新，真实文件分支改名消费此窄delta；仍完整读取旧图、执行manifest语义校验和全部成员安装。分支增删、跨owner移动、其他字段变化或未知谱系仍按完整投影。一般owner closure仍是后续必做项，须覆盖版本/commit、文件/目录引用计数、选择项/alias/path/meta-node、仓库/策略以及删除/移动/拆分，不能把“知道哪个history变了”当作字段级增量。

## 完整性与持久化不变

本次仍实读并认证所有旧依赖，随后按准确可达闭包向全部成员验存；已有同hash对象仍比较真实bytes并执行原文件/目录同步。缺源后的重建只在当前可信State及准确根相同的既有边界内使用，不能授权恢复、导出或导入从SQLite/任意State重定根。

驻留MapRoot与发布谱系均不证明对象当前健康。已知缺失/损坏使相关发布沿用失效；未决决定、恢复、换成员、配置/基线不匹配及索引失败等边界继续按原协议处理。没有启用 PublishedGraphEpoch 跳读，没有删除碰撞比较、fsync、全成员发布或成功后索引确认。新编码对象恰好同hash存在也不是已完成屏障的证明。

所以这次只关闭平面根和预算扫描中的指定成本，不宣称整体 O(delta)、更快吞吐或更低延迟。history全投影、State容器、全旧图读验/安装、恢复和暂存字节仍可能随总量增长；应分别测编码、读取、分配、同步与端到端时间。

## 单一格式与安全拒绝

不提供旧格式兼容、双读、alias、迁移或隐式升级。markerless/flat Snapshot/历史根、旧 `history-index`、`history_manifest_hashes` 和旧包/墓碑字段均拒绝；字段缺失不补默认marker。拒绝必须保留材料，不重置运行池、不删历史、不改写原证明。格式实现不是部署、凭据或权限变更授权。

[核心契约](../contracts/README.md)覆盖本次存储组件、固定字段、marker及proof外形，并在OpenAPI中仅镜像相同离线组件，不改未变HTTP DTO。schema不计算域哈希、不验证路由/闭包，也不证明实际持久化。固定canonical期望不由被测Rust编码器重新生成。

## 必须分别提供的验证

1. 严格字段与格式拒绝：35/30字段覆盖、marker缺失/不支持、旧字段/引用拒绝且原bytes保留。
2. 独立full oracle与路径更新：随机插删、同值、128项与64 KiB split/collapse、OperationId文本、append/All及未知谱系；不把oracle放生产热路径。
3. 证明与预算：真实所有候选路径编码的max对照精确摘要，空目录、单叶、65节点边界；hash/route/extra/truncated篡改拒绝，预算超限先于正文复制与决定。
4. 完整consumer：普通/降级恢复、checkpoint、跨成员缺失页、独立包导出/离线检查/导入、墓碑及操作点读；无好副本失败与保留文件。
5. 原持久化屏障：同hash已有对象、故障重试、文件/目录sync、ACCEPT/FINALIZE/PUBLICATION/封印及索引失败；不签发虚假的新谱系。
6. 全门禁、规模测量与物理设备验收分列：源码/静态schema、编译、单元回归不互相替代，未执行不能写成通过。

关联：[盘格式](../08-full-storage-and-disk-format.md#s08-07)、[事务恢复](../10-transactions-and-recovery.md)、[容量成本](../19-capacity-and-performance.md)、[操作手册](../25-operation-manual.md)。
