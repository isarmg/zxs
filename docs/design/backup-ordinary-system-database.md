# Backup 普通 SYSTEM 数据库的静默与保留式安装

[任务清单](../../TASKS.md) · [原盘两库安装](backup-ordinary-root-databases.md) · [Engine](../components/engine.md)

本片让同一原 registry/placement 后台任务继续实际安装 SYSTEM 普通 `index.sqlite`，不再只是保存 generation-local 副本。旧 main、WAL、SHM、rollback-journal 整组保留；安装后仍不开放普通索引读者或活动 writer。它不是完整迁移成功、下一笔业务发布或源解围栏。按要求未审查、编译、测试、静态检查或 CI，未执行实际盘或服务操作。

<a id="quiescence"></a>

## 真正关闭旧 SQLite 资源

同一显式原任务取得全部原锁与来源句柄后，在 Engine 的索引互斥区内停止旧索引。同步 SQL 查询本来受该互斥区保护；逃逸的 `OwnedScopeRead` 现在另持一个跨 rebuild 不更换的读租约。因此旧 cursor generation 已替换，也不能把仍活跃的旧读连接漏算。

只要任何读快照对象仍被保留，就返回 `WorkspaceBusy`，不强制撤销或在其下面替换文件。调用方待读对象退出后，以同一原操作继续。全部读租约释放后关闭实际 writer；其既有 no-checkpoint-on-close 配置保留，不为了换名 checkpoint 或删除 WAL。关闭失败不给安装能力，具体原因留在 `index_health`；在原 Engine 中不假定重试即完成关闭。

成功后索引在这个 Engine 的整个剩余生命周期保持 unavailable。新索引查询回到已有 State 投影规则或明确返回不可用；rebuild 不能悄悄重新打开旧名。这个静默能力只由私有 Engine 路径构造、绑定实际 SYSTEM 根身份，并由持原锁的后台独占持有。它不是数据库内容或 writer 权限的证明。

<a id="selection"></a>

## 全体证明之后才改普通名称

后台先完成并重新观察全部原盘普通两库、原生实际 Publication、全部指定目标的完整 State/正文/对象库/namespace 和共同数据库，再从原 SYSTEM 固定的 `backup-common-databases/index.sqlite` 读取准确新编码。新材料的完整长度与摘要必须等于本次原生证明所推导的 SYSTEM 作用域。

绑定描述包含 source fence、实际 Publication、原盘完整安装摘要、SYSTEM 根与 writer 的实际身份、新库摘要和长度。保留选择另记录旧四个固定名称的实际 inode、设备、长度和完整摘要，以及私有保留目录的实际身份。所有路径来自程序固定名称，不从描述解码成任意 I/O 路径。

SYSTEM 先持久保存 `backup-system-index-selected.json`，再按 WAL、SHM、rollback-journal、main 的顺序将原字节无覆盖移入 `backup-preparation/system-index-displaced`。每次移动同步目标与来源目录，并按原准确 stamp 读回。未知对象、链接、多链接、变更 inode 或内容冲突均保留并拒绝；不 swap-back、不删除旧数据或旧收据。

全部旧文件确实保留且普通名只剩缺失或准确新 main 后，独立无覆盖写入发布后的 `index.sqlite`。普通名不接旧 WAL/SHM，也不使用旧 SQL 选择权威。再次核全体原生/目标证明和全部新旧实际文件后，才保存 `backup-system-index-installed.json` 并报告 `GenerationOrdinarySystemDatabaseInstalled`。

<a id="recovery"></a>

## 原决定恢复，不从标记选权威

正常冷开、系统恢复或显式 rebuild 在任何 SQLite 打开之前检查选择标记。存在即拒绝旧 State 的索引重建；即使标记损坏也只造成拒绝，不授权新 State。原 fenced Engine 仍先由原生证据恢复，随后原任务重核完整决定/目标集合和规范绑定，才能解释保留描述。

未完成时逐项分类实际名称：旧文件仍在原名则继续无覆盖移动；已在原保留名则补两目录同步；未知、双份或丢失的旧文件拒绝。新 main 已完整存在时只读比较，不重复覆盖。完成标记出现后，必须继续存在全部原保留字节、准确新 main、空的普通 sidecar 名称和原选择；不把缺失或损坏降级为可重新生成的未完成任务。

`Engine::inspect_backup_system_database` 只观察同一 actor/operation 的完整原任务。它不打开 SQLite、不修复、不授予普通读写者，也不重新采样旧数据库。对已选择但未完成的调用，状态保持可见，不能因为后台线程退出就宣称迁移成功。

<a id="limits"></a>

## 资源预算与剩余缺口

新 SYSTEM 编码最多 64 MiB，旧四个文件各最多 64 MiB，选择描述最多 128 KiB。处理逐文件，不把整个四文件旧集合同时持入内存；保留原 inode 的 rename 不额外复制旧数据，新 main 必须有独立空间，并保留配置 metadata reserve、1 MiB 与两份描述的余量。容量不足或旧 WAL 太大明确阻塞；空间检查不是物理配额预留。全体 native 重放和数据库编码仍有其原预算，不宣称已优化峰值、延迟或同步次数。

读快照对象未释放会阻塞开始；关闭、完整验存、同步和普通 I/O 可能耗时，不承诺取消或 Drop 的硬时限。任务在正式 SYSTEM 改名之前就暂停该 Engine 的可选索引，因此上游失败后也会保持 unavailable，需同原任务继续或原冷恢复，不能把此状态当普通索引损坏后自动修复。

本片**没有安装活动 Engine Config/Current、普通 generation locator、目标 writer 或下一笔 publication，也没有解除源全池围栏**。普通 SYSTEM 文件虽然是新字节，正常查询仍被静默状态隔离，不能让旧内存 State 消费新库。后续必须在同一真实原生决定下完成这些活动资源的共同切换，不能凭此完成标记开放 H03、外来占位链或部分父子仓库。旧 source 和全部永久收据继续保留。

后继[普通原生对象安装](backup-ordinary-native-metadata.md)继续同一原任务，将完整首笔／registry／Publication 放入全部原盘及目标的普通不可变地址，并让目标 Current 重放消费这些实物；本页的 SYSTEM 静默和原全池停写保持，活动 Config／writer 仍未安装。
