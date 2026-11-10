# 备份目标的原范围 SQLite 材料

本片基于真实 main `38fb9b0b` 实现，并对齐 `631500cc`，继续真正迁移前的目标数据库准备。新增生产入口与后台落盘，不把准备标记当作权威切换。按用户要求，**未编译、测试、审查或运行 CI**。

## 原操作的后继阶段

`Engine::start_backup_scoped_database_preparation(operation_id, principal)` 续接已经完成 FILE／目录仓库 SQLite 的原任务。原 actor、operation、源 Config／sequence／snapshot、全部一个或二至八个目标及其顺序不变。尚在运行的 worker 不并行启动另一个阶段。

在打开旧目标名字之前维持源绑定；先重新认证全部目标的原生前缀、完整历史正文和对象库，再持久写 SYSTEM `backup-preparation/scoped-databases.json`。随后由持有原 writer／目标锁的后台线程编码、逐盘无覆盖安装并验存。普通冷开优先续接本阶段；源已变化则保留旧任务进入 `SourceChanged`，不改采今日源、不重新解释已选目标。此入口尚未接入 helper 或 HTTP/Web。

## 真正生成的库及范围

每个目标保留完整集合，路径是隔离准备域而非运行盘根：

```text
scoped-databases/
  manifest.json
  system/index.sqlite
  original-disks/<原 disk_id>/.zxs-disk.sqlite
  original-disks/<原 disk_id>/.zxs-disk-history.sqlite
```

- SYSTEM 库复用现有系统索引 schema、历史成员投影及完整保留操作／路径／catalog 行编码。 catalog 的三十四个业务族含完整版本、标签/元数据节点、D、内容退役及规则等真实 State 行；与运行 SYSTEM 库共用声明，不在备份路径另做过滤。输入是该目标原生前缀所认证的完整 State，不复制正在打开的 `index.sqlite`。**它仍是当前系统投影，不是所有过去 metadata／标签状态的完整时间线。**
- 每个原盘的 current 库复用本盘版本／owner path／内容状态投影。
- 每个原盘的 history 库从 genesis 的每个准确 Candidate／Publication／原收据及 State 流式折叠，记录版本进入／变化／移出，非本盘操作不凭今日路径归属。保留原 disk／slot／binding identity，使用现有两种盘根 SQLite 编码和读取器。

原盘 scope 不能被改成候选目标身份。这里归档的是所有原盘已支持的范围，而不是给新目标签发当前登记。盘根历史还不含完整目录／标签／别名审计，SYSTEM 尚缺完整历史范围；原生 archive 仍保留全部已支持 published State／永久收据，不因 SQLite 投影有限而删减它们。当前 fixed-genesis 限制、缺退役 portable 回执等前置拒绝继续存在。

所有选定目标均有相同完整 FILE／目录库与本阶段全部原范围库。文件与父子目录仓库不分片，不改份数，不打破父子同层同盘规则。不搬现用容器、不 unlink 原数据、不用硬链接或 reflink 冒充独立副本。

## 目标自身验存与中断恢复

对象库和本阶段共用一个闭合文件集合实现：只允许计划中的正常目录／普通文件，未知项、链接、不同字节或 scope 名称冲突明确阻塞并保留。正确部分允许原任务续写；每个 peer 独立核缺失字节、目录开销和保留空间，另一盘空闲量不能抵扣。

写完后重新读取全部前置历史／原生／对象库。再分别从**每个目标自己的**原生 prefix 编码本阶段期望，逐文件读取真实 SQLite 并比较；不是拿第一个健康目标的结果替其余目标作证。全部一致后才逐目标写共同 `scoped-databases-prepared.json`。部分 marker 不意味着部分切换。

`offline_check_backup_scoped_databases(paths)` 只读取原目标全集，不读取运行 SYSTEM 根、不从源或另一健康 peer 补材料、不修复任何盘。其报告明确原盘数、每目标库数和字节。新 `ScopedDatabasesPrepared` 只是上述范围材料终态，不是 `ACCEPT`、注册证明、恢复令牌或业务成功结果。

## 有界资源及尚缺内容

固定最多 32 个原盘、每目标最多 65 个新增 SQLite、总新增数据库 64 MiB。盘根流式折叠全部原盘保留行／当前投影合计 64 MiB；逐库沿用 32 MiB SQLite、65536 行／24 MiB 内容限制。SYSTEM 编码输入及 SQLite 各限制 64 MiB。保留原生前缀 2048 出版／128 MiB 读取等原预算；超限整单阻塞，不省略尾部数据。内存只保留相邻 State、有限本盘行集及当前编码结果，不保留所有历史 State；多个有界缓冲可同时存在，并非总进程 64 MiB 上限。

新无覆盖持久安装继续文件／目录同步，正常中断重跑相同任务；错误与进度可查询，锁外进行进度磁盘同步。停止只在受控检查点生效，不承诺任意 I/O 故障时硬实时退出。SQL 投影不是普通操作热路径，不宣称常数成本或已测吞吐。

**最终权威切换仍未实现**：还需完整历史范围／增量追平、目标活动容器及三范围正式安装、代际控制归档、最终源写围栏、真实多下级活动路由、完整低级链 helper 接纳与一个不可分割的联合决定及冷恢复。当前代码不放宽这些拒绝条件。见[整体迁移 ADR](../adr/backup-target-replacement.md)及根 [TASKS](../../TASKS.md)。

SYSTEM 派生表扩展到 `kind=0..38` 后，旧隔离任务的 SYSTEM-current 文件若与新编码不同，仍按无覆盖协议拒绝并保留，不双读、不将旧副本当缺失后覆盖；新任务从认证原生前缀重编码。该组合尚未运行验证。

SYSTEM 审计后继共用原投影器，另保留完整 operation-principal 映射、以原 ordinal 定位的 events 数组和 sequence/business/topology 代数；不是只备份回执正文。control-auth 仍不写入派生索引，完整认证状态由原生 State 材料负责。仍未运行本组合，未改变最终切换义务。
