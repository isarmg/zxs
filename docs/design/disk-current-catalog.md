# 盘根当前范围目录副本

[系统全局变更库](system-change-database.md) · [本盘发布历史](disk-local-publication-history.md) · [数据模型](../07-data-model.md#s07-03)

## 范围与入口

`Engine::rebuild_disk_catalog(slot)` 在已正式登记、启用且实际身份一致的数据盘根写入 `.zxs-disk.sqlite`。`Engine::disk_catalog_snapshot(slot)` 读取这份真实数据库并与当前已准入 State 逐项比较。它们是 Engine 维护入口，调用方必须授予全局维护权限；本片没有新增主服务 HTTP/Web 磁盘管理、辅助工具按钮或隐式初始化。

这里只保存当前本盘的版本义务和 owner 路径：版本必须在其精确 placement 的 route 中包含本槽，且该槽 binding generation 与当前物理盘登记完全一致。可复用槽名不能让新盘继承旧盘的版本。每行保存完整 FileVersion、主/备角色、已明确的本盘 owner 路径和逻辑 content state。没有记录的路径保持 None，不猜测实际实体名；content state 不是实际正文健康证明。相同文件名但不同 file/history 身份保留独立记录。

本片是“本盘当前快照”基础，**不是过去所有本盘变化日志**。不把全池 OperationResult、历史 FilePathNode 或其它盘全量目录复制进来，也不从当前放置反推过去归属。目录专属空仓库、完整本盘历史路径事件、标签/审计及所有实体范围仍需后继补齐。系统全局目录与本盘当前目录不能取代三范围独立恢复协议。

## 安装、读取和故障

- 外部 slot 先查当前正式注册视图，Disabled/Detached/待准入盘不进入此入口；再核对配置、I/O gate 和具名盘根 seal。
- SQLite 只在内存连接打开；实际盘上文件经 held-root 有界读取，不把可替换路径交给 SQLite。数据库上限 32 MiB，规范行合计 24 MiB，最多 65,536 版本。
- 共用目录/文件数据库的安全 installer，增加显式 Disk scope，绑定 pool/disk/slot/generation 和目标目录 inode；首次 no-clobber，替换保留旧对象，持久暂存位于盘根 `.zxs-layout/database-installs`。不按文件名删除未知物，不使用普通路径覆盖。
- 先编码和核旧库，再检查保留配额及元数据恢复空间；安装后重新读回、比对准确快照并复核具名盘身份。中断可显式重试，旧对象及未完成材料保留。未增加永久业务收据或新权威决定点。
- 读取严格核 schema 闭集合、唯一 scope、全部规范行及摘要，最后与本次 State 投影相等。缺失、损坏、被替换、本盘完整行不一致或声称未来序号均报错；读请求不会暗中修复、回退空库或接纳数据库内容。
- 除下述正常发布/启动维护外，不新增独立后台轮询。`disk_catalog_observation` 分开返回物化时的 `snapshot.source_sequence` 和本次核验的 `verified_sequence`：本盘全部行仍精确相等且旧序号不超前时，无关盘或全局元数据发布不会使本盘副本失效，也不会触发写入。本盘路径、placement、版本或 content state 改变仍须重建，不能仅比较一个序号或计数。已有系统、文件及目录库的发布/恢复顺序不变。

`refresh_disk_catalogs` 接受 1..=256 个显式、不重复且已登记的槽，先完成整个逻辑目标表检查，再逐盘执行重建。每盘返回 Updated 或保留原来源的 Failed；没有整体成功布尔值，也不回滚前面已完成的派生副本。它不发现外来盘，不初始化系统，不把未知物失败静默跳过。

## 权威与成本

数据库及其自摘要不能授权恢复、删除、注册、换盘或迁移。重建只从已准入 State 产生派生材料；主服务不会因为缺这份库初始化新系统。重建 O(全部 placement 数量 + 本盘行编码量)，读取需要同样的当前 State 投影和全部有界 DB 行比较；并非增量事件索引或常数时间检查。记录及 SQLite 镜像同时在内存中，单文件上限不等于全进程 RSS 上限。

## 本次验收状态

2026-11-08：完成生产实现及三条真实 Engine/I/O 回归源码。按用户不再测试/审查/CI、写完提交的要求，仅做格式化，未编译、未运行测试或 CI。不得把已有目录/文件 installer 的通过证据称为本盘消费者已通过；后续恢复验证时须测本片的盘根身份替换、崩溃重试、跨盘范围、旧 binding、过期读取与未知对象保全。

后继本盘独立新鲜度和批量维护新增三条回归源码；同样仅格式化，未编译/测试/审查/CI。

## 本盘历史伴随索引

新增独立的 `rebuild_disk_history` / `disk_history_observation` 负责按完整发布链派生本盘过去变化，见[准确边界](disk-local-publication-history.md)。它不改变本文件当前快照格式，当前刷新不会删除历史文件；两者均为同一本盘范围的派生索引，不因此具备独立恢复或完整三范围权威。

## 正常发布后的自动当前库维护

`decide_publication` 完成所有权威发布并安装新 State 后，才执行当前盘根库维护。范围来自前后 State 的版本、placement、content state、file_locations 和 file_backup_paths 写集；版本移出时仍包含**旧盘及旧 binding**，新盘也独立记入。未知写集来源按全部正式启用盘重建，不以空 delta 掩盖。

同一配置/精确登记和原 State 身份仍成立、且本盘输入未改变时，不写 SQLite、不抬高物化序号，只将逻辑一致性观察推进到新 State。路径写集会找出该文件所有保留版本及前后 route，不用最终主盘给旧盘变化归属。新盘身份、无上次成功、确有变化或未知来源都走完整 bounded 当前投影和 held installer。

正常 `Engine::open` 只有在原权威恢复、工作区及系统索引准入完成后重建这些派生库。`Engine::initialize` 不因此初始化三范围权威；`open_for_recovery`/held PreservePending 不增加此轮写入，也不在每个历史恢复前缀写入旧数据库。其他尚未接入的恢复/切换入口不能伪装已同步：状态观察会拒绝把不同 State/Config 的旧成功标签显示为当前。

`disk_catalog_statuses()` 无 I/O 返回逐个准确正式登记的维护事实：Unbuilt、Excluded、Pending、Installed 或 Unavailable。Installed 同时保留物化序号、逻辑一致至序号和摘要，**不是盘在线/正文健康证明，也不授权读取**；真实读取仍核 seal/字节/State。Unavailable 保留刷新失败的具体错误链和发生序号，绝不将未知库清空来报告成功。刷新失败不更改已持久的业务 operation 结果，也不伪称数据库刷新完成。无关发布不会反复重试既有错误；本盘再次变更、显式刷新或正常重开会重试。

自动维护在单 Engine 写者中串行执行，不另起常驻线程。当前投影仍扫描 placement，文件路径变化还需查出保留版本；只保证无关且已证明输入不变的盘不重写，不声称常数时间。保留旧对象占用共用 installer 额度，达到空间/保留上限即准确报 unavailable，不隐式删除旧物。

历史伴随库由独立的完整前缀证明维护：正常冷开重建、正常发布比较前后本盘范围并延伸，见[历史连续维护](disk-local-publication-history.md#正常冷开与发布连续维护)。本片**不在每次保存时重放全部历史**，也不把当前库 Installed 状态当历史已追平。两个数据库分别安装、分别报错，不是跨库原子事务；代际归档与三范围独立恢复仍待实现。

本节新增生产接线按用户要求未审查、编译、测试或运行 CI。无 HTTP/Web 状态输出随本节自动新增，调用方尚需接入 Engine 维护状态。
