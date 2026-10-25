# ZXS 存储核心

`storage-engine` 实现单写者、完整文件版本、不可变发布证明与离线恢复。对外入口由 `lib.rs` 导出；实现模块默认私有，不允许调用方绕过 `Engine` 的业务约束、操作幂等与发布流程。

| 目录 / 模块 | 职责 |
| --- | --- |
| `engine.rs` | 生命周期、业务准入、操作查询、在克隆状态上组合一次业务变更 |
| `engine/config.rs` | 拓扑与额度配置、独占操作系统锁 |
| `engine/publication.rs` | 正文准备、精确元数据预检、持久决定与发布屏障 |
| `engine/recovery.rs` | 仅沿已验证发布前缀回放，协调未完成决定 |
| `engine/reads.rs` | 副本验证、读租约、导出、磁盘容量与状态 |
| `publication_evidence.rs` | 只读选择精确摘要的对象、完整快照及发布 seal；查询不执行磁盘修复 |
| `engine/workspace_storage.rs` | 工作区清单的双位置持久化与重启协调 |
| `engine/workspace_limits.rs` | 独立于持久配置的本机运行期保留证据上限 |
| `engine/index.rs` | 可重建 SQLite 投影 |
| `business/` | 仓库、命名空间、上传、文件分支、目录批量分支与历史查询 |
| `business/namespace.rs` | 路径校验、父目录约束、命名空间改动与工作区冲突规则 |
| `business/namespace/draft.rs` | 工作区提交内的临时路径索引与保序条目草稿，不用于读取或持久缓存 |
| `business/ancestry.rs` | 基于子节点索引的提交后代查询，供拆分与历史包校验复用 |
| `journal.rs` / `metadata_budget.rs` | 哈希对象、日志帧、证明与实际序列化大小限制 |
| `container_storage.rs` | 普通版本文件、真实容器改名、继承引用缓存与格式准入 |
| `historical_save.rs` / `maintenance.rs` | 历史另存分支、计划、清理与历史拆分 |
| `history_package.rs` | 历史包证明、精确图对象选择与修复、离线校验、导出及恢复导入 |
| `history_package/manifest.rs` | `HistoryManifest` 的历史域 State 投影与逻辑闭包校验；不执行存储 I/O |
| `history_package/graph.rs` | 确定性分页根、索引与叶页编码和解析；校验图形状与引用闭包 |
| `repair.rs` / `degraded_recovery.rs` | 修复原始候选与显式永久丢失恢复 |
| `wire.rs` / `protection.rs` | 冻结上传清单、请求摘要、后续版本放置与保护规则 |

## 权威数据与投影

业务操作先在 State 克隆上组合修改；分支创建只登记冻结引用，不新建空文件。实际内容写入才准备 FULL 和固定路由副本。DirectoryBranch 保持稳定 ID/名称，DirectoryTree 保存不可变组合；文件修改或子项选择只增加子树并推进 head。PREPARE 不表示可见，ACCEPT → FINALIZE → PUBLICATION 及同步收据共同决定公开状态，恢复沿原候选继续。

命名空间删除在实际执行处收集有序路径和条目种类，随不可变 Commit 保存删除时间；空效果不创建事实，移动、改名和视图选择不冒充删除。工作区与目录批量操作复用同一规则，失败不发布部分事实。删除事实不保存正文身份，不扩大历史保留根；历史包与拆分保留事实本身，活动投影不回查操作回执或父快照推测删除。

恢复读取 candidate/publication 时逐份验证日志绑定的精确摘要，不因首份对象缺失或损坏就忽略健康副本。仅用验证通过的对象修复安全的普通文件或缺失目标；所有副本失效时停止，不用多数票替代摘要，也不覆盖符号链接或硬链接目标。

SQLite、容器 container.json 及当前名称是可重建投影，不能替代发布记录和不可变证明。新建分支不生成占位文件；检测到 legacy 或 pending-branches 旧容器结构时返回 `FORMAT_UNSUPPORTED`，不删除或迁移原材料。活动文件分支、工作区和读租约固定正文；目录只固定选中及尚未完成分支的当前 head，不永久固定旧子树。

读取继承冻结父版本，范围编辑保留父未改字节；零字节或同摘要首写仍建立独立版本。历史另存只为 staged 文件生成 FULL。Branch/Commit 空映射及新增目录 map 按当前稀疏编码省略序列化；FULL 证明仍使用确定性编码。语义、目录子树与默认视图见[分支说明](../design/branch-system-logic.md)。

## 工作区提交的命名空间草稿

普通工作区提交使用事务内的 `NamespaceDraft`：`BTreeMap<lookup_key, slot>` 为冲突检查、暂存文件替换和父目录查找提供路径索引，`Vec<Option<Entry>>` 保存条目次序。草稿仅在这次提交组合期间存在，不保存授权、读取结果或跨事务缓存，也不改变通用 `namespace::find` 对未排序中间向量的支持。

源条目只做一次明确准入：检查路径、显示名称、规范化键和种类的一致性，拒绝重复键、缺失父目录或非目录父项；不要求输入已排序，也不把损坏来源隐式规范化或按首项/末项去重。索引按路径而非文件 ID 建立，同一文件的不同路径别名仍是独立条目。查询沿用 Unicode 规范化与完整大小写折叠，等价查询不变成改名，也不放宽新名称校验。

替换时清空旧槽并在末尾追加，新父目录按原顺序追加；因此与原有 `retain` 后 `push` 的可见次序一致。暂存文件循环结束后只展开一次，再执行原有命名空间操作、删除事实收集和最终 `validate_entries`。目录选择、文件上下文、来源版本、正文构建和最终发布规则均不变；本优化的资源及测试边界见[容量与性能说明](../19-capacity-and-performance.md#s19-04)。

## 共享规则与扩展位置

- 名称与路径只调用 `name-policy`，固定 Unicode 16.0 NFC / case folding。显示拼写可以不同，祖先关系必须按规范化键判断；移动子树时按路径组件处理显示名称，不能用另一种拼写的字节长度截断 UTF-8。
- 新写入备注调用 `storage_contract::validate_note`，最多 `MAX_NOTE_BYTES = 128` UTF-8 字节，空串清空，拒绝控制字符。历史包读取同样验证当前备注格式，不隐式重写不匹配的证明。
- 校验块大小由 `storage_contract::SUPPORTED_CHECKSUM_CHUNK_BYTES` 统一提供，编码器、配置和历史包使用同一集合。
- 新业务操作在 `business/` 或相应维护模块组合状态，经 `mutate` / `mutate_digest` 发布；不得自行写日志、提交 SQLite、改写既有版本正文。
- 目录分支选择按替换子树后完整仓库树检查 `Config.max_entries`，目录和文件均计入；单个冻结子树合法不代表与当前兄弟子树组合后仍合法。超额返回 `RESOURCE_LIMIT`，不改变选择、提交、代际、操作记录或发布日志；根目录选择替换整棵树，不叠加旧树条目。
- 新上传检查放在不可变代次复制之前，流式复制同时受到文件额度和剩余工作区额度约束。失败应保留已接受代次，避免留下一个没有清单引用、又阻止合法重试的代次。
- 工作区提交重试共用目标业务状态、所属主体及已发布请求摘要校验。原生请求使用相同冻结输入重算摘要后，从系统盘/数据盘寻找完全匹配的请求；不接受另一份可解码请求。恢复仅凭完整请求绑定协调工作区状态，wire 正文也双位置持久化；缺少旧正文证据时保留原工作区状态，不猜测或清除暂存字节。

运行期保留证据额度通过显式 Engine 构造参数传入，不修改配置锚或发布证明；正常打开、恢复与提交规划快照使用同一有效上限。运维参数与限制见[容量规范](../19-capacity-and-performance.md#runtime-retained-workspace-limits)。

## 验证

```sh
cargo test --locked --offline -p storage-engine -p storage-contract -p name-policy --lib
cargo test --locked --offline -p storage-engine --test directory_selection_limits
cargo check --locked --offline -p storage-engine
```

测试覆盖发布屏障、幂等、恢复、读租约、受支持的历史树拆分及完整包恢复。新分支回归检查无正文复制或空 stub、真实空首写、缓存/改名/旧 inode、来源清除、目录组合和不可变子树闭包、共享文件身份、选择代际、目录 head 固定与历史清理；含目录组合、文件节点或独立文件分支的历史拆分明确拒绝。目录额度回归覆盖两个合法四条目状态组合为六条目时的原子拒绝、释放额度后同操作 ID 重试，以及根组合在额度边界切换并重启恢复。所有 fixture 使用临时目录，不代表物理设备故障或发行验收。

打开已有池时，缺失根目录或权限安全且完全为空的根目录视为不可用成员，可由剩余配置元数据成员的多数证明恢复只读服务；不会在这些未验证目录创建锁或恢复文件。非空目录缺少 disk.json、身份冲突、符号链接和不安全权限仍拒绝启动；配置多数门槛不会随失联成员缩小。
