# 已决定备份目标的冻结读取与原决定续作

本片基于真实 `9375fae1`，在 `6b016b79` 的 [registry/整体 placement 共同决定](backup-registry-placement-decision.md)上增加两个实际生产路径：**完整 native 决定的原样恢复**，以及**不依赖原盘或 SYSTEM 的目标冻结读取**。按用户要求未审查、编译、测试或运行 CI。没有实际执行备份、选择决定、读取用户正文或解除围栏。

## 不是最终业务激活

新的目标 reader 仅能读被原共同决定选定的冻结实体，不是普通 Engine/SMB/HTTP 的当前视图。它不能转换为 `Config`、`DiskResources`、`Generation` 或 writer。活动多下级 Config/placement、新目标范围的完整历史库、共同原生 publication 和旧源解除围栏仍未实现；原池继续停写，旧文件、数据库、日志、收据与原选择均保留。

此前全部原成员选择 `BackupRegistryPlacement` 后，Abort 可能无法取得原 Q。本片不伪造 Abort，不把冻结读取成功当作解除该可用性限制，也没有自动接普通设置/helper。不能将这一开发阶段用于实际换盘成功交接。

## 原决定恢复不补投缺失的票

冷开仍必须先准入原 native source fence 和原 State，再恢复准确原 SYSTEM task。新增分支先查原成员和目标是否已经留有完整决定证书：

1. 若有任何证书，严格解析完整 schema、规范字节、域摘要、原 transition 和每个原成员的身份/完整 fence receipt；证书必须覆盖原固定全部 N，不以在线集合替换。
2. 重核原真实 seal、冻结日志字节和物理身份，再打开原成员的同一不可覆盖 decision slot，**只读取**。每一实际 slot 都必须已经选择证书中的同一 core/acceptance。缺票、Abort/Joint/不同选择或坏证书立即阻塞，不能退回新选择路径。
3. 独立重新验证全部目标的完整 State、原生前缀、三种既有 scope 材料、FILE/目录库、实体正文、完整低级链及共同 placement。预检容量后，只补同一已选 core/证书的缺失副本，存在异字节不覆盖。
4. 最后通过纯读取路径再次核全部原成员选择和每个目标的精确 inputs/decision 副本，才返回已有 `RegistryPlacementDecided`。不修改选择，不添加成功收据或解除围栏。

如果根本没有任何完整证书，仍由此前明确持久化的原 decision task 续作原选择；它不是从进度缓存新发起任务。未知残片、目标损坏和 I/O 错误保留，不自动清理。

## 独立保存可信摘要

`Engine::inspect_backup_registry_decision(operation, principal)` 返回 `BackupRegistryRecoveryReport`。宿主先认证并授权原 actor；核心要求原 operation/actor、已结束的受管理 worker、完整原 source fence、每个原成员的实际选择和全部目标材料。该方法不选择缺票、不补证书。

报告的 `decision_hash` 是 `BackupRegistryDecisionHash`，可在成功读取之后由宿主独立保存。报告另列原 fence、完整 State/Config 摘要及全部明确目标节点。报告不是新权威或迁移完成回执。

摘要从字符串解析只验证 64 位小写十六进制表示。**不能在恢复时从待验证目标读出摘要，再拿同一摘要作为信任依据**。目标自摘要/自述不证明它就是用户原来选定的材料；未独立保存可信摘要时，本入口不提供自动发现或猜测权威。

## 原源离线时的完整目标冻结读取

`open_decided_backup_targets(paths, expected_decision_hash)` 只打开调用者明确给出的 1–8 个目标路径，不打开原源盘或 SYSTEM：

- 先用外部保留摘要绑定实际证书，再绑定原安装任务；每个目标路径、顺序、device/inode、完整父子/逐跳链必须与原任务相同。缺任一目标、重复根、换名/重绑或部分集合均拒绝。
- 持有全部目标目录锁，重新验完整 snapshot、所有 native 证明、原 scoped DB、独立 FILE/目录 DB、全部实体和正文，以及和原决定相同的 registry/placement。不能用跨盘并集凑齐一个文件或目录仓库。
- 同一份完整 inputs/decision 必须已存在于每个目标，离线 reader 不创建或修复它们。若根存在运行态 `disk.json` 或 `metadata`，拒绝使用隔离 reader，避免绕过未来活动代的撤销/退役门禁。
- 返回 `BackupDecidedTargets`，只给出说明报告和 `read_version_range(slot, version_id, offset, length)`，不返回可写 fd、私有认证或完整 State。锁随对象生命周期保留。
- 每次正文读取先重新验全部目标和决定，再从指定目标实体中的 `AVAILABLE` 正文同一个 fd 做完整流式摘要验证；缓冲所需区间但在完整摘要/长度通过前不返回任何字节。不回退另一盘或源盘掩盖损坏。不存在 AVAILABLE 正文（包括 Purged/Retired）拒绝。空区间也验完整正文。

这是冻结历史恢复读取，不是目标成为运行态成员；文件是否在当前业务 namespace 可见不由此接口决定。它必须是单独授权的恢复入口，未接普通 Web/HTTP/SMB/helper，也不自动把恢复读取内容纳入系统。原源全失时可读取的是**完整原明确目标集合和已独立保存摘要所绑定的冻结材料**，不是任意单盘独立启动完整系统。

## 成本及剩余步骤

共同 core/证书各最多 32 MiB；继承完整 snapshot/原生前缀/图片与数据库既有预算。读取区间最多 4 MiB，正文流式块 64 KiB；每次重新验完整集合并完整扫描所选正文，成本随原材料大小及目标数增长，不是常规读热路径。底层 I/O 仍可能阻塞，无任意故障下硬完成时限。

下一步仍必须将同一已选决定接入实际新 scope、完整活动 registry/locator、原生 generation 的发布和读取，才能合法解除源围栏。不能把本片的 `AdmittedBackupDecision` 或冻结 reader 当作该最后一步已完成。
