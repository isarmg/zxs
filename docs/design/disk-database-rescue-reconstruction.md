# 盘根数据库救援归档与独立 SQLite 重建

[离线材料](offline-disk-database-material.md) · [本盘历史](disk-local-publication-history.md) · [目录清单](disk-backup-directory-catalog.md) · [辅助工具](../components/helper.md)

## 实际新增能力与边界

盘根 `.zxs-disk.sqlite` 保存本盘当前版本和递归目录副本清单，`.zxs-disk-history.sqlite` 保存本盘已保留的变化及覆盖边界。本片把原来只能读取、折叠、输出 JSON 行的材料，接到**正式救援归档和可重建的独立 SQLite 镜像**。

`DiskDatabaseMaterial::archive_bytes()` 将完整本地记录、明确盘身份、所选范围与原缺失状态编码为规范 JSON。`reconstruct_disk_database_material(archive, expected_identity, source)` 不打开 SYSTEM、盘根、seal、配置、其它数据库或正文，仅凭归档和另外提供的明确身份在内存生成一个 SQLite，重新读取并逐条比较全部记录，最后才把镜像交给调用方。原盘、原 SYSTEM 和原 SQLite 全部不存在也不妨碍这一表示重建。

这是新的可用救援结果，**不是活动安装或三范围独立权威恢复**。没有调用 `PreparedInstall`、原生 ACCEPT/PUBLISHED、登记、初始化、Stop/epoch 切换或任何备份写者。新镜像不得手工覆盖活动库；普通 Engine 的准入和派生安装仍需原完整权威来源。系统全局库、盘根本盘库、文件/目录内部对象库是不同范围，不能用三份字节多数决定权威。

## 明确选择重建来源

每次重建必须指定 `DiskDatabaseRebuildSource`：

| 来源 | 生成内容 | 不能推断 |
| --- | --- | --- |
| `stored-current` | 原归档中实际读取的当前清单，保留原 `source_sequence` | 不能因序号更大就证明其最新或权威；原 current 不存在时拒绝。 |
| `history` | 原完整保留变化、原覆盖 header 的历史库 | current 不能补出缺失历史；原 history 不存在时拒绝。 |
| `current-from-history` | 从空集合按全部原历史变化折叠得到的当前清单，序号恰为原 `through_sequence` | 不能补造边界后的变化；即使原 current 更新，也不把它伪装成更完整的历史。 |

即使只请求其中一个输出，整个归档中所有实际保留的 current/history 都要验证。当前序号落在历史覆盖内时必须等于该时点的完整折叠，不只比较最终序号或计数；矛盾的另一份不能被选项隐藏。若操作者已明确只读取一份完好源，另一份保持“未检查”，不能混成“已确认缺失”。历史为空但 header 符合真实 genesis 表示时允许空结果；所有所选材料都不存在时不创建空数据库。

原文件版本行、原 owner 相对路径、完整前后变化、递归备份空目录和旧路径均保留；归档不包含供打开其它根使用的宿主路径。owner 路径本来就是盘根范围的一部分，不下放到文件/目录内部对象库。

## CLI

仍用 `disk-databases` 只读查看摘要或 JSON 行。新命令：

```sh
zxs-helper disk-database-rescue export selected-disk.json saved.disk-db.json
zxs-helper disk-database-rescue export selected-disk.json --scope history saved-history.disk-db.json
zxs-helper disk-database-rescue rebuild saved.disk-db.json identity.json --source stored-current current.recovered.sqlite
zxs-helper disk-database-rescue rebuild saved.disk-db.json identity.json --source history history.recovered.sqlite
zxs-helper disk-database-rescue rebuild saved-history.disk-db.json identity.json --source current-from-history folded.recovered.sqlite
```

`selected-disk.json` 使用[既有选择格式](offline-disk-database-material.md)：`root` 加 `expected_identity`。`identity.json` 只包含明确登记，不包含路径，例如：

```json
{
  "pool_id": "pool_11111111111111111111111111111111",
  "disk_id": "disk_22222222222222222222222222222222",
  "slot_id": "data-one",
  "binding_generation": "1"
}
```

身份文件由操作者另外选择。归档不能自己决定“希望恢复的是哪一盘”；pool/disk/slot/generation 都必须完全相等。身份不是原发表链认证，输出仍标明 `source_publications_verified:false`、`content_availability_verified:false`、`authority_restored:false`、`active_database_installed:false`。

export 持有现有 ZXS 生命周期租约并复用安全冻结读取。操作者仍须停止其它主机、旧程序或不参与租约的写者。rebuild 是纯归档操作，不要求原服务主机/SYSTEM 存在，也不取得该服务租约。未增加 Web 接口或静默后台恢复。

## 保全与真实性

- 原选择严格复用 held FD、明确 seal/登记、schema、规范行、摘要、变化前置条件和结束重核。现在在读取前后都拒绝所选 SQLite 的 `-wal`、`-shm`、`-journal`，无论它们是文件、链接还是目录；不删除、不 checkpoint、不猜测是否可丢弃。未选择的库及其 sidecar 不访问。
- 归档格式为 `disk-local-database-rescue-material`，内容域摘要为 `disk-database-rescue-material`。输入必须恰为支持格式的规范字节，额外字段、重复/不规范记录、无序/重复键、外部路径和错误 scope 均不能靠重新摘要后静默丢弃。不是 JSONL 老输出兼容或旧格式迁移。
- 重建复用当前 SQLite 编码器和解码器，先受限编码、再完整重读。目标保证记录语义与声明覆盖一致，不声称重新生成的 SQLite 页字节必与旧文件相同。
- helper 只接受新的 `.disk-db.json` 或 `.recovered.sqlite` 输出名，不能选择 `.zxs-disk.sqlite`、`.zxs-disk-history.sqlite` 等活动名称。沿用持有父目录、无符号链接逃逸、0600 临时文件、文件同步、无覆盖发布、父目录同步；已有文件/链接/目录不替换。失败可能发生于 rename 后，保留实际不确定结果和恢复信息，不擅自重试或清理已发表文件。
- 自摘要和前后结构闭合不证明原 publication/正文真实。整体改写后仍自洽的材料不是受信认证材料；不恢复主体权限、不复活退役版本、不正式接纳外来盘，不根据 owner 路径寻找或导出正文。

## 预算与后续

归档上限 64 MiB；每份 SQLite 仍为 32 MiB；当前库版本和目录合计至多 65,536 行、规范行 24 MiB；历史至多 65,536 条变化、2,048 个 publication、规范行 24 MiB。边界先限制输入，再限制各族行数/规范字节；历史 fold 保持完整有界集合和 O(变化数 × log 活动对象数) 成本。重建暂存原归档、解码记录、最多两份独立 SQLite 及折叠结果，编码/规范比较会有额外瞬态内存，以上不是进程 RSS 上限。

本片不修改源业务写者，不添加普通后台全历史扫描。下一步仍包括无预先归档且本库缺失时的完整独立来源重建、全部主目录/alias/标签范围、代际材料、三范围共同准入/活动安装和正文可用性。

2026-11-08：基于 GitHub main `92fc12ec` 写入生产代码、CLI、规范及九项测试源码，未修改先前对象库救援候选。按用户要求**未审查、编译、格式化、静态检查、测试或 CI**，没有真实磁盘操作。九项源码覆盖原 SYSTEM/原盘消失后重建、显式来源/缺失、错身份和矛盾 current、缺失前置历史、非规范/额外字段、所选 sidecar 保全、空目录路径历史、helper 安全输出名及租约/CLI。它们不是运行通过证据。

## 已发表镜像的准确读回

后继 [救援数据库读回](rescue-database-readback.md) 保留所选完整语义 snapshot，并核验实际发表后的准确 inode/文件及全部记录。新增 `disk-database-rescue verify ARCHIVE IDENTITY --source SOURCE IMAGE.recovered.sqlite` 可冷态只读核验，不依赖原盘或 SYSTEM、不修改输出。相同记录但不同 sequence、不同 current/history 族及读验期间变化均不能冒充正确副本；报告区分归档摘要、实际文件摘要和语义 records hash。发表后错误明确保留文件，不能自动重建。后继组合的生产 check 和所选 helper/平台/目录回归已验证；盘根 Engine libtest 尚未执行，具体范围见读回协议交付事实，未复跑 CI。
