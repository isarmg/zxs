# ADR：递归备份的空 scope 目录发布

状态：有限实现，发行格式未冻结。只实现以前明确拒绝的“选定文件集合为空”的目录 scope；有文件的递归配置、文件正文及其收据表示保持原逻辑。Web 配置、备份列表接线、移动和改名自动跟随不属于本片。

## 持久权威与决定点

目录身份、各目标身份和完整逐级路径放在新空 scope 收据的 directory_namespace 中，由原生共享规划器从已准入 CurrentNamespaces、原前驱、冻结 RecursiveBackupCapture 及实际策略路由导出。严格新发布和冷重放重算完整收据，所以删字段、改目录身份/路径或换目标绑定都会拒绝。本片不增加 State/HistoryManifest 字段或用空文件版本伪装目录，不删除历史收据，不加入旧格式回退或迁移。

新空目录目标目前必须既是已启用登记盘，又是元数据成员。规划器检查原登记角色，准备和决定前再检查当前配置角色/身份、目录条目与空间预算。每个目标因而已有现行 Candidate/Snapshot/Publication 全成员日志和持有 seal 的闭包，即使 Prepared 文件列表为空也成立。仅数据角色的目标明确 OperationUnsupported；不能只因写了一个目录就说该目标具有发布证明。原先可用的单文件、有文件 scope 和数据盘专用文件备份不受这项空 scope 门禁改变。

正常路径在严格 State/Prepared 验证、容量预检与所有 Prepare 之后进入原决定流程；Finalize 完成后，才沿已准入持有 DiskRoot 创建 histories/<history>/directory-backups/<operation>/namespace/<published-path>，并以无覆盖原子写保存 record.json。祖先 mkdir/fsync 与文件/目录同步复用现有原语。物化失败保持原本 InDoubt 和决定证据，不能提前交换可见 State 或返回成功。成功收据的文件正文保护仍为 NotApplicable，不虚报 COMPLETE_VERIFIED 文件副本。

## 冷恢复与读取

只有完整发布前缀严格重放完成且当前恢复代固定配置的全部元数据成员在线、成员切换未冻结、恢复尾部未阻塞时，恢复流程才依据“仍与现行策略完全相等”的永久收据重建当前目录投影。缺成员或冻结的只读冷开不创建派生目录，也不写 record.json；读到已发布收据本身不授予磁盘修复权限。恢复写权限后，正常冷开仍重建缺少的派生目录；不以缓存目录/record.json 或同名字节建立发布权威。它不逐个重放所有过期历史物理目录，不重开已停止盘，不把旧身份材料投到换绑的新盘；冲突 record.json 保留并明确报错，不能覆盖冲突材料。

Engine::published_backup_directories(slot) 提供目录 ID、仓库/历史域、冻结路径和操作 ID。返回值描述已发布且仍是现行策略的目录快照，不证明盘当前健康，也不自动采样新目录或传播后续移动/改名。后续备份树读取可消费该 API，并保持既有全局冲突与分页快照校验；本片没有改 Server/Web 配置文件或备份列表，不能说网页已显示空目录。

每个已配置份数逐级走既有准确来源路由；当前 scope 的独立子策略保留，目录条目经现有继承判定筛选。本片不声称解决全部历史视图混合份数、后续策略变化的自动目录传播、外来盘导入或数据盘专用目标的本盘证明。

## 成本与验证边界

准备时只扫描已准入范围条目，保留目录记录，工作量约 O(D * route_length)，metadata canonical bytes 受 MAX_METADATA 限制；每目标目录数受 max_entries 限制。每个目录创建使用现有全祖先持久屏障，未削减同步。空间预检计入记录、目录组件估算及 metadata reserve，实际 I/O 仍可失败；该估算不是内核空间预留。

读取和冷恢复当前会扫描永久 operations，再解析空 scope 的候选收据；不 clone 全 State，仍为 O(operations + selected_directories)，这是明确剩余性能成本。物化投影不承担权威，不新增另一张并列表。

[公开回归](../../test/engine/integration/recursive_backup_empty_directories.rs)覆盖双跳真实空目录、冷重开、永久幂等、后续发布后的缺投影冷重建、决定后真实路径阻塞导致 InDoubt 并冷恢复、数据盘专用目标拒绝，以及不新增文件版本和不扁平化路径。缺元数据成员和成员切换冻结两例比较文件字节并断言指定目录投影保持缺失；前者还验证成员恢复后可写冷开重建。文件字节快照不是完整文件系统元数据无写证明。

[递归备份回归](../../test/engine/integration/recursive_backup_paths.rs)继续覆盖既有文件正文、独立来源链、软删除和停止来源拒绝；[私有规划与回放用例](../../test/engine/unit/engine/current_recursive_backup_tests.rs)覆盖目录行、目标身份和缺失材料的拒绝。测试定义与实际执行结果分别记录，源码接线不能代替运行。

物理掉电、真实 fsync EIO、浏览器/WASM 和生产硬件测试另列，临时目录 I/O 回归不能代替它们。
