# 整体备份的多级切换输入

本片基于 `5b7f0a29`，新增真实后台控制材料落盘与恢复接线。它是最终权威决定的输入准备，**不是最终切换实现**；不写成员投票、`disk.json`、Config、Current、placement 或成功业务收据。按用户要求未审查、编译、测试或运行 CI。

## 显式的完整新子树

`Engine::start_backup_cutover_preparation(BackupCutoverPreparationRequest)` 延续已有 scoped database 任务的 actor、operation、冻结 source 和全部物理根；请求仅给每个原目标的全新 `disk_id`、`slot_id` 和直接 `upstream_slot_id`，顺序必须等于原目标顺序。

- 同一来源可以有多个新子节点。一个新子节点也可以有多个新子节点；不复制活动 Config 的单 successor 限制，不按数组第一个元素选择备份盘。
- 每条新边必须沿同批新目标最终到达原来源。缺祖先、另一个既有来源、环、重用旧盘/slot、重复 ID、不同物理目标集合都拒绝；不创建占位盘，不接受后续再补祖先。
- 新身份的绑定代际是 1，级别由完整边推导。旧来源、被替换的原末级备份身份及全部历史 Config 保留；当前只支持原来的末级替换准备范围，不自动覆盖更复杂的旧下游。
- 路径前缀按直接上游的冻结目录 basename 逐跳递推。例如新 `B` 的来源是 `P`，前缀为 `P/`；新 `C` 来源是 `B`，前缀为 `B/P/`。名称走既有 portable name 校验，不拼接绝对路径、不使用新目标自身名字作为其首层来源前缀。
- 这不是分片分配器。每个明确选中的节点都承诺同一完整池单位，所有保留文件版本、目录仓库及其父子整体在该节点共同保留。无每文件目标子集，也不由候选数量猜测一份或多份；复杂跨旧盘父子合流仍未支持。

新树是专用的 prospective 类型，不伪装成可运行的 `ValidatedConfig`，不删除旧路由、`same_disk` 或冷回放的相等约束。新树不自动选择 metadata 成员或 quorum。

## 实际持久化顺序

1. 结束并 join 旧准备 worker。核原 actor/operation、source pool/config/sequence/snapshot，先核源绑定，再打开旧目标名字。
2. 持有全部原目标目录、writer locks 与来源 writer 生命周期。既有准入继续核完整已 Enabled 原祖先及生产环境独立设备。
3. 在每个目标独立重读完整原生 prefix、所有正文/历史、FILE/目录 SQLite 与原范围 SYSTEM/盘根 SQLite，不用第一份健康副本补另一份缺失。
4. 所有目标身份、原操作、显式新子树及原 scoped 任务写入 SYSTEM `backup-preparation/cutover-inputs.json`。它早于任何目标新阶段写入，不替换不同输入的旧任务。
5. 后台向每个目标无覆盖写 `cutover-inputs.json`。再次逐目标完整验存后，写各自 `cutover-target-prepared.json`，其中绑定共同材料报告、该新节点和原物理 device/inode。
6. 读回全部目标的精确确认，才写每个目标的共同 `cutover-inputs-prepared.json`。每次写入使用既有受限 FD/no-clobber/fsync 原语并重核目标名字；异内容保留，空间不足不由其它目标补贴。

每个节点的确认仅证明输入准备完成，不是 metadata 投票、恢复权威或最终决定。SYSTEM 任务和目标材料损坏、缺失同批目标、某个确认不一致都不降为部分成功。

## 恢复与查询

普通 `Engine::open` 在原准备分支中优先恢复本阶段，原 operation 重试只接受相同新子树。任务缺部分精确字节时按同任务续作；源已发布变化则在重开目标前显示 `SourceChanged`，不重采源、不改旧收据。普通 I/O/同步仍可能阻塞，owner Drop 不承诺任意故障下硬时限。

`backup_preparation_status` 使用 `CutoverInputsPrepared` 终态，详情明确尚未切换。`offline_check_backup_cutover_inputs` 需要全部原目标及其顺序，只读重验每个目标材料/确认和完整新子树；不读取源来兜底，也不输出可安装的运行态 Config。旧阶段查询与冷续作优先级保留本阶段，不用旧完成标记降级覆盖新任务。

新增输入/确认各受 4 MiB 上限及每目标 16 MiB 余量保护，目标最多 8 个；拓扑推导至多 O(8² + 原祖先链)。正文/快照/数据库验存仍使用各自既有完整预算，新增去重仅计 stable file_id 的单位数量，不按正文哈希合并副本。

## 尚未完成的决定闭包

完整 SYSTEM 过去时间线/盘根目录标签审计、真实活动实体安装、目标共同决定、当前 registry 与 placement 的同一原生变更、决定后冷重放、活动多下级写入/读取路由和 helper 一次性正式接纳仍未完成。本输入阶段不冻结旧源、不释放旧数据。后继[原生源围栏](backup-source-fence.md)另提供真实旧代停写与固定原成员认证取消，仍不授权目标、不提供 `activate`/`finish` 迁移成功入口。

后续最终决定必须重新验证该原输入与当时源边界，保留永久旧收据，并同时绑定全部目标确认、活动布局、完整数据库历史和源围栏。不能从本准备状态直接推导出允许原子切换。
