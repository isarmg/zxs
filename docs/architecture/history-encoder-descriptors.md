# 历史编码描述符：来源与免读权限分离

本切片保持当前唯一盘格式、固定成员和决定顺序；不增加持久化字段、旧格式兼容、依赖或收据。源码和测试定义不是执行通过或性能结论，GitHub 门禁仍须绑定准确提交。

## 留存什么

`HistoryBinding.encoder: Option<Arc<EncodedHistory>>` 随已有 catalog 的 `MapRoot` 路径复制。描述符保存准确 history root、固定 30 个字段的实际 `MapRoot`，不另建全历史查找表。`MapRoot` 保存摘要、叶键、孩子及准确对象大小；不保存原始 Value、对象 bytes 或健康状态。无 descriptor 是明确的旧图读取分支，不是读取失败后的缓存 miss。

额外驻留为 **O(当前投影 key bytes + nodes)**，不是 O(delta) 或固定 RAM。旧 catalog 及未改字段／节点通过 Arc 跨代共享；一个仍被 detached preparation 持有的旧代仍会保留其变化路径。全量编码本就产生这些 MapRoot，本切片留下它们，不为建立描述符额外重编码所有历史。没有 TTL、全局 hash cache、第二种树后端或新配置旋钮。

## 为什么可以从 State 取同叶未改行

一般磁盘图的语义准入不能证明领域对象重编码等于旧原始行。例如 Branch 的 `file_id:null`、`file_node_id:null` 和 `pending_file_versions:{}` 解码后可能与省略字段相同，但再次序列化会省略它们。`AuthenticatedMetadataObject` 认证的是 raw JSON Value 的规范表示，不是任意领域对象的再次序列化。

`EncodedHistory` 字段私有，不可反序列化，只有以下生产构造链：

1. 完整 `prepare_encoded` 通过 HistoryManifest serializer 保留各字段 wire rules，再直接留下 `Encoder::map_root` 的实际输出；`prepare_indexed_encoded` 使用同一 Field 值规则和准确 State 派生 membership。
2. qualified 更新只从已有 descriptor 出发，调用现有 `Encoder::update_map`。变化 key 优先取 `HistoryEdits` 的 `Some(value)` 或 `None`；不在 edits 的同叶 key 才使用 `Field::value(after, key)`。删除优先级不可倒置：跨 owner 移动后该行仍可能存在于全局 State，却不再属于本历史。
3. unqualified 更新仍完整读验旧图并调用原 raw `edit_map_values`。仅当已有 origin 的准确 root hash 绑定认证旧根时，未改字段保留原 descriptor，变字段接该次真实 encoder 返回的 `MapUpdate.root`。没有旧 origin 时，读验成功、语义相同或 hash 名称存在都不能晋升 `Some`。
4. 新 history 或原有合法完整重建使用当前全量编码器，所有输出都是 created，不能按旧 hash 名称减掉安装和同步。

由此归纳，descriptor 的旧行来自同一当前编码器表示，而不是把任意 normalized 外部图重建成另一个根。decimal-string u64、缺省 content-state 投影、空映射与全部 owner 变化由独立完整 HistoryManifest 编码 oracle 覆盖。恢复／导入外部图仍走原边界，不签 descriptor 或 publication 权限；首次普通发布按原规则完整编码当前 State。

## 描述符不授权免读

唯一免读入口仍要求原 `PublishedDependencyReuse`，绑定准确成功 State、catalog、成员和配置、连续 writer epoch，以及原 periodic owner 的 coverage permit。纯 State 的 owner/name 索引不承载这项磁盘权限。

- qualified descriptor hit 仅编码变化字段路径和新 history root。其他字段保持引用与精确 maximum；下降预算不得错误沿用被删最大对象的旧 maximum。
- qualified miss 实读准确旧图，保留 raw delta。读取失败先被原 fault observer 观察，并原样传播；不是 catch-I/O-as-miss，也不靠缓存成功盖掉真实失败。
- 无 periodic owner、仅手工 scan、过期、成员／绑定变化或失效 epoch 时，即使 descriptor 驻留也完整读验／安装。原有 unqualified 精确重建规则保留。
- 所有 fresh encoder 输出都真实安装到每个 required member，包括同 hash 的叶、索引和 history root；碰撞比较与 file/directory fsync 不变。
- descriptor 仅随 DetachedPublication 转移；完整权威决定与 publication 完成、原 metadata epoch 仍有效时，才随 PublishedStateBase 变成下一次来源。权威失败或取消不签发新 base；独立 SQLite 索引失败只停用其候选加速，不撤销已完成的成员元数据证明。
- 准备后、安装后和全部 PREPARE 后／first ACCEPT 前仍核验同一个原 permit，不延长原期限、不更换 owner，不改变不可逆决定归属。

未触及旧节点的静默损坏可以后移到实际读取、scrub、导出或重开发现；即使同历史另一个字段发生变化也如此。描述符不进入这些读取路径，不是字节健康证明。已经观察到的故障仍使共享 provenance 失效；pending 来源正文例外不变。

## 验证范围

测试定义包含：完整字段／owner delta 与独立全量 hash/bytes 对照；多代变化、增长与收缩；未变大字段和删除最大对象的精确预算；raw null/empty 不晋升来源；真 history 变化的 owner stop/replacement、原期限、PREPARE 撤销、InDoubt 的阻断与独立 index 失败后的发布成功；未变旧依赖破坏后 qualified 写不修复而导出、scrub、重开实读发现。

隔离外部 I/O oracle 只观察其实际覆盖的 libc 调用并使用正控制，不代表所有 raw syscalls、物理 I/O 或物理断电验收。warm hit、miss、冷启动、scrub、修复与端到端规模测量必须分别报告；不能由更少图读取宣称整个 Engine O(delta)。
