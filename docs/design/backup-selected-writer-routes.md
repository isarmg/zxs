# 已选完整 writer 路由材料

本片从真实 `ba86850e` 的原决定恢复继续生产实现，并独立对齐真实 `eafb997f`，保留 helper 的普通启动前置门禁。按用户要求未审查、编译、测试、静态检查或运行 CI；没有执行实际换盘、决定选择或解除围栏。

## 仍不允许活动 writer 或解除源停写

现有活动 `ValidatedConfig`、`PlanningContext::route` 和 `CurrentNamespaces` 仍按单链原生代准入，`PortableAuthority` 尚无完整 Backup generation。不能放宽原 Config 然后使用 `.find` 任意选一个下级，也不能把已选证书或本片的 JSON 当成新 generation publication。

本片不修改活动 Config/Current/State/Placement，不创建 `disk.json`、native journal 或新 generation marker，不发迁移成功回执，不获取新目标 writer，不删除或退出任何旧盘，也不清除源围栏。原池仍可能全池停写；足够原成员已选择 Backup 后，Abort 仍可能不可达。**本阶段不应作为实际换盘完成入口。**

## 实际后台路径

已有明确的 `start_backup_registry_placement_decision` 保存原 SYSTEM task 后，由同一 worker 继续：

1. 按已有协议决定或恢复同一原 native 选择。保存 `RegistryPlacementDecided` 诊断后，再从全部原成员真实不可覆盖 slot 读取已选决定；不用计划 receipt 或进度缓存代替。
2. 重核全部目标的原 native 前缀、完整 State、既有三范围材料、FILE/目录 SQLite、全部实体正文及同一 registry/placement 决定。
3. 从已验实体的真实 home 移除各目标自己的 `activation-image/tree/<逐跳前缀>/histories` 前缀。所有目标必须得到**完全相同**的文件、完整 version 集、目录仓库、实际父子关系和历史相对 home。任何不同都拒绝，不以跨盘并集合成完整单位。
4. 生成完整所选扇出表，绑定原决定、fence、State/Config、原 snapshot/sequence。保留原 primary 到 source 的完整低级链，以及仍保留的被替换旧末级身份；新节点的每个 upstream 必须是原 source 或同一完整批中的准确节点，不引入占位。
5. 在写入任何新材料前，分别核每个目标容量和已有文件。每个目标无覆盖持久化 `backup-writer-routes/routes.json`，逐个读回，重新验完整决定与实体，再持久化并读回 `prepared.json`。每个目标保留相同完整路由表，不分片。
6. 最后重新读真实原成员选择，才报告 `WriterRoutesPrepared`。该相位只表示准备材料，不等于可写、已正式加入或已切换。错误仍归原任务 `Blocked`，保留已写材料和原证据。

该扩展不因只有旧 entity task 而触发新的 native 选择。必须存在此前明确持久化的 registry decision task。已经完成的同一原 task 可在冷开或显式同任务续作时补这段材料；旧证书、旧 source fence 和原 receipt 字节不改变。

## 多下级、整体和逐跳语义

当前支持范围仍是完整固定 genesis 的单一级盘原链，替换一个末级边。用户已明确的 1–8 个目标全部各存完整整池单位；不是按容量自动决定一份、多份或选一个盘。复杂旧分叉、异盘父子仓库合流及部分单位迁移继续拒绝。

每个文件路由列出完整原 `version_ids`，没有接收部分版本或目标子集的参数。目录仓库路由列出原所属仓库、实际父仓库与完整目标集合；其子仓库和文件使用同一完整集合，不能借不同相对路径拆盘。

目标集合是**扇出集合，不是线性 route 向量**。例如原 `A` 后新选 `B`、`C`，再选 `D -> B`：B/C 各使用 `A/...`，D 使用 `B/A/...`。C 不会变成 B 的下一跳，也不会把兄弟名字拼入前缀。每个目标的输出 home 从其已验直接上游前缀与共同历史相对 home 组合，并指向既有隔离实体；不进行物理 rename。

## 读取与恢复边界

凭已独立保存的原决定摘要、全部原目标及原路径/物理绑定打开 `BackupDecidedTargets` 后，可调用：

- `inspect_writer_routes()`：重新验证全部决定/实体与两份完整路由材料，再返回准确材料摘要、目标集合、保留原链和每目标完整计数。
- `file_writer_route(history, file)`：返回完整文件版本集合及每个明确目标的现存隔离 home。
- `directory_writer_route(history, directory)`：返回完整目录仓库的同集合 home 及父关系。

这些返回值是冻结说明，不是有效 writer lease。它们不暴露 SYSTEM 私密认证、不返回可写句柄、不生成 AdmittedState，不接普通 Web/HTTP/SMB。每次调用均重新验全部实际目标，不从缓存终态、目标自摘要或另一盘替代损坏目标。

掉电可以留下路由文件或部分完成标记；同原任务续作会先重验全部原决定与目标，再无覆盖补缺。缺盘、身份变化、容量不足、未知文件、异字节或损坏立即阻塞，原数据不清理。出现运行态 `disk.json` 或 `metadata` 的目标，仍拒绝由隔离恢复能力继续操作。

## 成本和后续

路由材料最多 32 MiB，独立逐盘预留 16 MiB 与目录元数据空间；沿用既有实体/完整 snapshot 预算。每次准备和观察仍重新读取全部所选材料，成本随完整历史与目标数增长，不是普通写者的热路径。同步 I/O 无任意故障下的硬时间上限。

下一步需要使真正的 Backup generation 同时认证完整目标范围数据库、当前 registry/locator/placement、全部新目标 native seal/journal 和原操作的最终业务 publication。只有该真实恢复路径可证明新的 writer 与全部旧副本义务一致，才能安全解除源停写。本片的准备表或文件计数不补足任何这些缺失证据。
