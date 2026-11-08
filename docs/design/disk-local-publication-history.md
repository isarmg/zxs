# 盘根本盘发布变更副本

[当前盘目录](disk-current-catalog.md) · [系统全局库](system-change-database.md) · [三范围目标](../07-data-model.md#s07-03)

## 已实现的生产入口及边界

`Engine::rebuild_disk_history(slot)` 为一个已正式登记、启用且物理身份一致的盘重建本盘发布变更；`Engine::disk_history_observation(slot)` 读取数据库并从已准入的原始发布来源重算核对。它们是需要调用方授权的 Engine 维护 API，本片不增加 HTTP/Web、辅助工具按钮、首次初始化或自动业务写入。

本片保存 **版本进入本盘、版本离开本盘，以及其在本盘期间 owner 路径、主/备角色、逻辑内容状态或版本记录的变化**。每条记录含前后本盘行、原操作 ID、序号、candidate/snapshot 摘要。移出的版本仍保留本盘过去行。其他盘的行和全局 OperationResult 内容不复制进来；相同名称的不同 file/history 不合并。路径是已准入 State 中的本盘 owner 路径，不是所有分支 alias 的完整列表；逻辑内容状态不是实时正文可读证明。

这是真实落盘的派生历史，而不是观察当前 placement 后筛选全部旧 FilePathNode。还没有覆盖全部本盘目录事件、空目录仓库、标签/分支审计及磁盘代际切换，也不是完整三范围数据库或独立恢复权威。

## 来源闭合与时序

1. 从 Engine 已准入并持有的 journal writer 中收集到指定边界的 `PUBLICATION` 记录。同一序号不允许不同 candidate/publication；仅 Prepare/Accept 不产生历史行。
2. 每次按日志绑定的摘要读取 candidate、publication 和完整原始 State 对象图，核固定成员、完整发布 seal、序号和原操作回执；candidate 的 `previous_snapshot` 必须从空 genesis 连续延伸。
3. 对每个时点分别按精确 pool/disk/slot/binding generation 生成本盘行，再比较相邻本盘行。不能用最终 route 给过去的变化分配归属。所有已发布事务都参与链校验，只有本盘差异成为记录。
4. 结束边界必须有真实发布；若重建当前头，还须等于 Engine 已准入 snapshot，最终本盘行也须完全一致。序号可能因放弃的准备而跳号，不能靠整数连续性代替 snapshot 链。
5. 目前只支持固定 genesis authority。出现 membership generation 时明确拒绝历史重建/读取，不把当代活动日志或 checkpoint 当整段历史；原当前快照 API 不因此失效。后继必须从准确归档和各时点登记恢复来源，不增加旧格式兼容或多数表决。

`DiskHistoryHeader` 明确 `through_sequence`、`through_snapshot` 和已核 publication 数量。读返回的 `verified_sequence` 是当前 Engine 头，**不会修改较旧的 stored coverage**。旧库可作为完整旧前缀返回，但不是“截至现在没有变化”；调用方应同时展示这两个序号。数据库自摘要只证明内部表示，完整逐项来源重算才允许返回。

## 实体、格式与安装

本片使用盘根伴随文件 `.zxs-disk-history.sqlite`，与当前目录文件 `.zxs-disk.sqlite` 共属一个本盘范围：前者是发布变化、后者是当前义务。分开存放是为了不让当前目录刷新抹去历史，也不隐式升级既有当前目录格式；**不能把增加这个文件说成用户要求的三个完整数据库已全部实现**。后续物理格式收敛须另行明确，不能删除已保留历史。

SQLite 在内存中构造/解码，盘上字节只经持有盘根和 I/O lease 读取。格式 `disk-local-publication-changes` 只有严格闭集合的 `identity` 和 `changes` 两张表，无附加 schema 对象。变化按 publication sequence/version ID 排序，SQL ordinal 必须从 0 连续；首次读取不使用空键哨兵。规范 JSON、整库摘要、精确 scope 和声明覆盖均须一致，不能凭摘要授权修复或恢复。

共用 `database_install`，增加独立 `DiskHistory` scope 和固定叶名，避免同盘当前快照与历史材料交叉恢复。安装仍持有目标根身份，首次 no-clobber，替换保留旧对象，未知/损坏/别盘对象拒绝且保留。持久暂存位于同一盘根 `.zxs-layout/database-installs`，复用累计保留额度；安装前检查空间及元数据恢复余量，安装后读回核对完整生成内容并再次核具名盘 seal。中断可重新显式调用；不会修改业务 State、永久回执或发布决定。

## 资源和剩余成本

- 最多重放 2,048 个发布、保存 65,536 条变化；规范变化体总计最多 24 MiB，SQLite 镜像最多 32 MiB。超过预算返回具体失败，不返回部分完成或裁剪旧记录。
- SQLite native 限制覆盖值大小、SQL 长度、编译 VM 操作及 bind 参数；不是总执行步数/总 RSS 保证。
- 只保留相邻本盘投影，不缓存所有历史 State。每个完整 State 仍需按现有对象图准入规则读取；时间约为所有覆盖 State 的 placement 扫描量加本盘差异量，不是增量或常数时间 API。
- 维护和读取目前都会重放来源，且需要有可信来源副本；这不是断开源后仅凭本盘 SQLite 恢复。没有更改源成员 quorum、故障判定或三范围冲突策略。

## 本次交付状态

2026-11-08：生产源码和文档已写入候选。遵照当前要求，本片未进行代码审查、编译、测试、静态检查或 CI；没有宣称旧 installer、系统库或当前目录测试覆盖这份新增历史消费者。恢复验收后需补完整发布链、非本盘隔离、改名往返、移出历史保留、旧覆盖/缺失中间发布、代际拒绝、盘替换、恶意首行/schema 及安装崩溃回归。

## 与自动当前库维护的区别

后继普通发布当前库维护使用前后 State 的精确版本/路径写集及两边 route/binding，确保最后一份版本移出后旧盘当前库也刷新。它不写历史伴随库，不把一次 current snapshot 成功当完整历史成功。历史读取始终同时给出旧 coverage 与当前观察序号；要追平本片历史需显式重建。未将完整历史重放塞入每次发布热路径，后续自动增量历史必须单独保持原始发布来源、冷恢复及配额闭包。
