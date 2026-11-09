# 盘根本盘发布变更副本

[当前盘目录](disk-current-catalog.md) · [系统全局库](system-change-database.md) · [三范围目标](../07-data-model.md#s07-03)

## 已实现的生产入口及边界

`Engine::rebuild_disk_history(slot)` 为一个已正式登记、启用且物理身份一致的盘重建本盘发布变更；`Engine::disk_history_observation(slot)` 读取数据库并从已准入的原始发布来源重算核对。它们是需要调用方授权的 Engine 维护 API，本片不增加 HTTP/Web、辅助工具按钮或首次初始化。后继在正常启动/已完成发布后自动维护此派生历史，不增加新的业务决定。

本片保存 **版本进入本盘、版本离开本盘，以及其在本盘期间 owner 路径、主/备角色、逻辑内容状态或版本记录的变化**。每条记录含前后本盘行、原操作 ID、序号、candidate/snapshot 摘要。移出的版本仍保留本盘过去行。其他盘的行和全局 OperationResult 内容不复制进来；相同名称的不同 file/history 不合并。路径是已准入 State 中的本盘 owner 路径，不是所有分支 alias 的完整列表；逻辑内容状态不是实时正文可读证明。

这是真实落盘的派生历史，而不是观察当前 placement 后筛选全部旧 FilePathNode。新增准确原生递归目录副本事件，见[目录范围规范](disk-backup-directory-catalog.md)；还没有覆盖全部本盘目录事件、完整空目录仓库、标签/分支审计及磁盘代际切换，也不是完整三范围数据库或独立恢复权威。

## 来源闭合与时序

1. 从 Engine 已准入并持有的 journal writer 中收集到指定边界的 `PUBLICATION` 记录。同一序号不允许不同 candidate/publication；仅 Prepare/Accept 不产生历史行。
2. 每次按日志绑定的摘要读取 candidate、publication 和完整原始 State 对象图，核固定成员、完整发布 seal、序号和原操作回执；candidate 的 `previous_snapshot` 必须从空 genesis 连续延伸。
3. 对每个时点分别按精确 pool/disk/slot/binding generation 生成本盘行，再比较相邻本盘行。不能用最终 route 给过去的变化分配归属。所有已发布事务都参与链校验，只有本盘差异成为记录。
4. 结束边界必须有真实发布；若重建当前头，还须等于 Engine 已准入 snapshot，最终本盘行也须完全一致。序号可能因放弃的准备而跳号，不能靠整数连续性代替 snapshot 链。
5. 目前只支持固定 genesis authority。出现 membership generation 时明确拒绝历史重建/读取，不把当代活动日志或 checkpoint 当整段历史；原当前快照 API 不因此失效。后继必须从准确归档和各时点登记恢复来源，不增加旧格式兼容或多数表决。

`DiskHistoryHeader` 明确 `through_sequence`、`through_snapshot` 和已核 publication 数量。读返回的 `verified_sequence` 是当前 Engine 头，**不会修改较旧的 stored coverage**。旧库可作为完整旧前缀返回，但不是“截至现在没有变化”；调用方应同时展示这两个序号。数据库自摘要只证明内部表示，完整逐项来源重算才允许返回。

## 实体、格式与安装

本片使用盘根伴随文件 `.zxs-disk-history.sqlite`，与当前目录文件 `.zxs-disk.sqlite` 共属一个本盘范围：前者是发布变化、后者是当前义务。分开存放是为了不让当前目录刷新抹去历史，也不隐式升级既有当前目录格式；**不能把增加这个文件说成用户要求的三个完整数据库已全部实现**。后续物理格式收敛须另行明确，不能删除已保留历史。

SQLite 在内存中构造/解码，盘上字节只经持有盘根和 I/O lease 读取。格式 `disk-local-inventory-changes` 只有严格闭集合的 `identity` 和 `changes` 两张表，无附加 schema 对象。变化按 publication sequence/实体类型/稳定身份和路径排序，SQL ordinal 必须从 0 连续；首次读取不使用空键哨兵。规范 JSON、整库摘要、精确 scope 和声明覆盖均须一致，不能凭摘要授权修复或恢复。

共用 `database_install`，增加独立 `DiskHistory` scope 和固定叶名，避免同盘当前快照与历史材料交叉恢复。安装仍持有目标根身份，首次 no-clobber，替换保留旧对象，未知/损坏/别盘对象拒绝且保留。持久暂存位于同一盘根 `.zxs-layout/database-installs`，复用累计保留额度；安装前检查空间及元数据恢复余量，安装后读回核对完整生成内容并再次核具名盘 seal。中断可重新显式调用；不会修改业务 State、永久回执或发布决定。

## 资源和剩余成本

- 最多重放 2,048 个发布、保存 65,536 条变化；规范变化体总计最多 24 MiB，SQLite 镜像最多 32 MiB。超过预算返回具体失败，不返回部分完成或裁剪旧记录。
- SQLite native 限制覆盖值大小、SQL 长度、编译 VM 操作及 bind 参数；不是总执行步数/总 RSS 保证。
- 显式重建、正常冷开和独立读取仍只保留相邻本盘投影，不缓存所有历史 State。每个完整 State 按现有对象图准入规则读取；时间约为所有覆盖 State 的 placement 扫描量加本盘差异量。
- 正常发布的连续维护复用本进程建立的完整前缀证明，不逐次重放全部来源；本盘发生变化时仍读取、核验并重写完整有界历史 SQLite，成本随本盘已保留行增长，不是 SQL 原地追加或常数成本。
- 冷重建和读取仍需要可信来源副本；这不是断开源后仅凭本盘 SQLite 恢复。没有更改源成员 quorum、故障判定或三范围冲突策略。

## 本次交付状态

2026-11-08：生产源码和文档已写入候选。遵照当前要求，本片未进行代码审查、编译、测试、静态检查或 CI；没有宣称旧 installer、系统库或当前目录测试覆盖这份新增历史消费者。恢复验收后需补完整发布链、非本盘隔离、改名往返、移出历史保留、旧覆盖/缺失中间发布、代际拒绝、盘替换、恶意首行/schema 及安装崩溃回归。

<a id="cold-open-publication-maintenance"></a>

## 正常冷开与发布连续维护

正常 `Engine::open` 完成原权威恢复、工作区及系统索引准入后，对每个正式启用盘执行完整来源重建。只有安装与读回成功才保存一个小型进程内前缀证明：准确配置/物理登记、State 身份、已持久 header 和整记录摘要。不缓存各盘的完整历史 State 或变化向量。`initialize`、`open_for_recovery`/held PreservePending 以及每一个历史恢复前缀均不调用这轮自动维护。

正常发布在原决定及 State 安装完成后，使用该次真正的 candidate/hash 与前后 State 延伸前缀。旧 proof 必须仍绑定相同 Config、pool/disk/slot/binding、原 State、previous_snapshot 和当前已发布 snapshot；没有 proof 或已失去谱系就重新从完整来源重建，不能凭盘上自摘要、最新序号或最终 placement 补猜历史。

前后 State 的准确写集证明本盘输入不变时，只推进进程内观察覆盖，不改盘上的 header。若有可能的本盘变化，则比较前后本盘完整投影；只有实际不同才有变化行。写入前从 held root 读取旧历史数据库，核 schema、scope、规范行，以及与**进程内已认证旧前缀**相等的 header/整记录摘要，不能接纳自行重算摘要的删行库。变化行继承原操作、candidate 和 snapshot，包含旧盘移出与旧路径；中间已证明无本盘变化的 publication 仍计入新覆盖。之后复用原安全 installer，新旧库和未知物保全规则不变。

不相关的 publication 可以让 `observed_through_sequence` 比 `persisted_through_sequence` 新。这两个事实不合并：`disk_history_statuses()` 的 Tracked 显示持久覆盖、进程内观察覆盖、publication 计数和已持久摘要；它是无 I/O 的维护事实，**不是磁盘健康、读取准入或离线可恢复证明**。真实 `disk_history_observation` 仍从原来源重算并返回盘上真实覆盖，不利用这一状态跳过验证。丢失 State/Config/身份匹配时不能把旧 Tracked 状态显示为当前。

任何一次重建、读旧前缀、编码或安装失败均保留具体错误链和原观察序号，逐盘标为 Unavailable；已成功的其它盘不回滚，权威业务回执也不改判失败。无关 publication 不重复执行既有失败；显式重建、正常冷开或本盘实际输入变化重试。显式重建开始即废止旧进程内成功标签，失败也不会留下假的成功事实。未纳入/禁用盘不维护；删除观察记录不删除磁盘数据。

当前和历史数据库分别维护、分别报状态，不是跨文件原子事务。空间/累计保留额度或 2,048 publication 等上限仍适用，不能因已经发布而删除旧行或未知物。单 Engine owner 串行做有界 SQLite I/O，没有新增常驻后台线程。启用多盘时首建/冷建仍对各盘重放完整链，完整索引可能同时占据内存多份有界表示；没有将单文件预算当全进程 RSS 保证。

本增量接线遵照用户要求仅完成源码及文档，未审查、编译、测试或运行 CI。待恢复验收时应针对精确最终输入验证无关 publication 跳写、跨盘移出、删行后重算摘要、原 State/Config 被替换、崩溃后完整来源重建、部分盘失败及真实资源上限。代际归档、目录/标签/全部 alias 历史、状态 UI 与独立三范围恢复仍未完成。

## 不依赖 SYSTEM 的材料读取

新增离线只读入口可在明确所选盘上检查 SQLite 内部格式与 Entered/Changed/Departed 前后闭合，恢复覆盖边界的本地清单表示，并与所选当前库比较。它不读取原 publication，不替代本文件的已认证来源读取，也不在源盘安装结果，见[完整边界](offline-disk-database-material.md)。原 SYSTEM 恢复路径现在于权威恢复完成后调用盘根维护，逐盘失败保持可见。

## 目录副本变化

完整前缀和连续维护现同时计算版本及已发布递归目录副本；Entered/Changed/Departed 的目录变体保留旧路径及准确身份，没有文件版本也产生目录变化。原始 publication 链规则不变，SYSTEM-free fold 仅恢复本地表示。新格式拒绝并保留不支持旧字节，四条回归仅源码未运行；详见[目录范围规范](disk-backup-directory-catalog.md)。
