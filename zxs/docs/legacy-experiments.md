# ZXS 早期存储实验

这些记录保留早期格式和测量计划。日常使用请看[当前范围与验证](status.md)；实际首设的格式以 [SQLite 契约](sqlite-system.md)为准。

任务基线 `63772552`（2026-11-18）；该修订记录 3 项进程内 SMB 检查、58/58 契约检查。旧格式证据见 `src/pool`、`src/ordinary/journal`、`src/pool/portable` 及相应用例。

## 检查点（早期日志格式）

下表记录旧格式源码；所列物理夹具未运行。

| 检查点 | 已有路径 | 明确边界 |
| --- | --- | --- |
| 登记 `8d1ad744` | 只读核验、初始化空 SYSTEM/普通/独立回收根，按原请求查询或继续。 | 仅新池登记。 |
| B1 `f266c6e8` | 普通根创建空手动 FILE，分配 FileId、EntryId、HistoryId。 | 无正式节点；原态可读与原单结果分别核对。 |
| B2 `6dc86800` | 首次流式保存 revision 0→1，可按 FileId 重开；校验块 4 MiB。 | 初传无全池锁；已保留但尚未命名的重传保留兄弟 FILE 的读，新写等原单。 |
| B3 `c795fb67`、区间读 `c7d62d30` | 显式以初稿 revision 1 产生主分支上无父的首正式节点、同一 FILE 内独立的裸 FULL；按 FileId + NodeId 读。现行正式决定绑定实际节点索引整体摘要，区间读先验索引，再验相交的完整正文块。 | 原工作稿的清理与正式决定分开。只保证检查过的块；采用一种现行格式，旧实验帧不获兼容。 |
| B4 `6529615f` | 干净 FULL 上一笔覆盖、追加或改长，保存 revision 2；新区间与原 FULL 分开，旧 NodeId 仍可读。 | 100 GiB 基底＋4 KiB 更新夹具未运行；当时仅一笔工作保存。 |
| B5 手动连续保存 `3dc51658` | 按原前序、决定、revision 和长度顺次保存：覆盖、截短、增长、稀疏零与交错读。见 `src/ordinary/interval/`、`tests/unit/working_interval.rs`。 | 本轮保留原 extent；旧回执不改新稿。 |
| B5 首个正式 INCR `bc4ae7e1` | 第二个正式节点绑定父 NodeId/VersionId；新材料只存最终贡献字节。见 `src/ordinary/delta/`、`tests/unit/second_formal.rs`。 | 显式提交仍验证所有贡献字节，当前视图、两个历史节点及原回执独立。 |
| B6 从 I1 发表 I2 | 多笔顺次 SAVE 后冻结 I2；[两轮夹具](../tests/two_increments.rs)覆盖同一本机主体的覆盖、裁短再扩、失盘、MOVE、软删还原、原回执与三节点。 | 旧格式仅两代 INCR。未执行的 [100 GiB 夹具](../tests/unit/working_interval.rs)检查三份索引、实际相交块、原 inode/分配和 I2 的三个新增字节。 |
| D1 初次根间搬移 | 干净 FULL（可带 I1/I2）跨两普通根转移且不改名，保留标识与历史。见 `src/ordinary/custody/`、`tests/unit/custody_handoff.rs`。 | 切换后只从目标服务，旧盘离线也可继续编辑；旧首移只接初始放置的单 FILE。 |
| D2 专盘软删/还原 | 干净 FULL（可带 I1/I2）移入独立回收卷，再按原删除单还原；验分页、代次、恢复余量及清源。见 `src/ordinary/custody/recycle.rs`、`tests/unit/recycle_pair.rs`。 | 旧日志的未清保留会阻塞无关写；当时仅根层单 FILE。 |
| N1 普通空目录 | 原生创建/列举空目录和子目录，按 VolumeId＋DirectoryId 核根、父、子与原决定。见 `src/ordinary/directory/`、`tests/unit/ordinary_directories.rs`、`tests/ordinary_directories.rs`。 | 普通目录可占 FILE 创建或恢复的名字。 |
| N2 普通目录中的 FILE | `file-create-at` / `file-lookup-at` 及嵌套父目录内的保存、三代历史、MOVE/软删/还原。见 `tests/two_increments.rs`、`tests/unit/custody_handoff.rs`、`tests/unit/recycle_pair.rs`。 | 不同父的同名条目各自绑定原物；旧夹具不含目录整组操作或 SMB。 |

## 容量与读遍数的旧对照

旧单槽示例：100 GB 基底、十次 10 GB 工作写入，提交到清理期间的逻辑正文峰值约 300 GB（不重叠）或 210 GB（重复覆盖）；不含格式及文件系统开销。

旧引擎无故障逻辑遍数：同挂载不复制正文，首 FULL 至少核读三遍；跨挂载源和目标各至少三遍，INCR 另核其贡献材料。持同一经核原句柄时，落位复核名字、身份、成员和操作标记即可；失去句柄的冷重试完整核读。旧 `custody_handoff.rs` 用例覆盖目标可读而源盘/旧读阻止清退：旁边 FILE 可读，旧单槽下新建或保存暂拒且无回执；原单收尾后重试。
