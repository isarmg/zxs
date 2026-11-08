# 原围栏下 registry 与整体 placement 的共同选择

本片基于真实 `c4f708df`（在 `ebe5a6c4` 的实体安装上续接），在[隔离实体安装](backup-target-entity-installation.md)之后增加**实际原成员不可覆盖决定及全目标凭据落盘**。不是目标最终业务激活，不修改活动 Config/Current，不开放目标 writer/reader，不返回迁移完成。按要求未审查、编译、测试或运行 CI；没有实际调用这些磁盘操作。

## 重要可用性边界

`Engine::start_backup_registry_placement_decision(operation_id, principal)` 是宿主必须单独认证和明确调用的核心入口，未接普通设置、HTTP 或 helper 自动准入。它会使用原 source fence 的同一个成员选择槽。**原成员选定足够多本决定后，认证 Abort 也可能不再达到原固定 quorum；本片尚不能解除源停写。** 因此不能将它接成普通备份设置的隐藏步骤，也不能把新的终态当成“迁移成功”。不调用它时，先前实体安装不会自动选本决定，原 source-only 认证取消语义保留。

开发阶段交付的是不可拆分共同决定及其恢复闭包，目标新身份业务授权、活动多下级路由、完整新范围历史库和最终原生发布仍缺。原盘、旧 State/图、永久收据和准备包均不删除、不重标、不覆盖。

## 同一决定包含的实际内容

内部 `PreparedBackupRegistry` 只能从原任务中每个持锁目标独立重新准入的材料构造。它先核原切换输入、所有旧实体 acceptance、全目标 complete-set marker，再读回实际完整映像，不能从一个进度枚举或自写 JSON 得到能力。

共同 projection 绑定：

- 原 fence、Config 摘要、完整 State 摘要、直接来源和被替换的原终端身份；原 Config 的全部盘显式保留，不暗示 Exit 或删除。
- 每个原明确目标的新身份、物理 device/inode、准确根、直接上游、level、逐跳名前缀及实际 entity manifest 摘要。沿用原同批全低级链；不补虚拟盘、不猜副本数。
- 每个目标的完整文件实体地址、全部 retained version IDs 和目录仓库地址；仓库 parent 取实际稳定 main 森林的最近严格祖先，文件 owner 取其实际仓库边界。不是按展示名称猜父子，也不是按版本分配不同目标。
- 每个目录仓库与子仓库都位于同一个目标投影内。目标集合由原任务给定，每个目标都是完整副本，不用不同盘的并集凑齐。旧异盘父子合流仍未支持。
- 原 native transition、每个原成员的真实身份、原 seal 摘要、真实 fence receipt 和完整冻结日志字节承诺。

文件/目录/State 不换 ID，不将旧 placement/receipt 改写为新身份。新的共同 projection 是单独的选定产物，不冒充旧 `State.placements` 或已生效 registry。隔离 FILE/目录库、SYSTEM-current 与原盘 scope 仍按其真实来源保留；没有把原盘数据库标签换成新目标权威。

## 持久顺序

1. 首次请求先核同一原 operation/actor、完整原成员 source fence、准确原 State/Config，并沿既有完整原日志/发布链检查器重新取得原成员证据；随后才打开准确目标。原实体任务必须已经存在。
2. SYSTEM 无覆盖持久化单独的 `backup-preparation/registry-placement.json`。这是显式请求续作意图，不是投票权威。旧 entity task 存在本身不会启动这个阶段。
3. worker 持有原 writer 责任、原成员 I/O lease 和全部目标独占锁；对完整决定/证书先编码限额和逐盘容量预检。每个原成员及目标各自存完整材料，不借另一盘空间。
4. 打开所有原成员原 fence 的 `metadata/membership-decisions` 槽，仅创建中性 binding；全部槽先检查为空或准确同一 `BackupRegistryPlacement`。已有 Abort/Joint/不同 core 不替换。
5. 全部目标保存同一 `backup-registry-placement/inputs.json`；全部原成员保存相同 domain-hashed core 和各自 acceptance。再独立读回所有映像。
6. 各原成员逐个以现有 no-replace/不确定结果重开/文件与目录同步原语选同一 core。**必须全部原 N**，不把在线数当新 N、不降为多数、不采用预计算 receipt 冒充实际选择。预计算向量仅用于首个决定前的完整体积/空间预算。
7. 再查原命名身份、原 seal、完整原日志字节，重读每个实际决定并同步；全部目标完整映像再次读回。只有此时才构造真实完整证书。
8. 完整证书先在所有原成员保存对象和原 fence 专属 `certificate.json`，再在每个目标保存同一个 `backup-registry-placement/decision.json`；读回精确内容后才报告 `RegistryPlacementDecided`。这是一个 registry/placement 共同选择，不是最终业务成功或源解围栏。

新 `BackupRegistryPlacement` 与原 Abort 使用**同一个不可变槽**。原 Backup 少数选择不投 Abort，剩余原成员仍能按原固定 Q 选择 Abort；全部 Backup 选择与 Abort quorum 不能同时成立。不同原操作、不同目标集合、不同格式或字节均保留拒绝，不添加兼容读取或覆盖。

## 原任务恢复与失败

冷开只在原 native source fence 和完整 State 已准入之后，优先检查本阶段任务，再考虑较早实体安装任务。重新构造源/目标能力、原 projection 与同一个 core；重试读回已存在同选择的真实槽并补准确缺失证书，不新采样源或重选目标。已选 Backup 的原成员少数和已落某个目标的证书保留；中断不是回滚。

缺原成员、目标离线/损坏、空间不足、原 seal/日志/命名身份改变、其它选择或未知 namespace 条目均阻塞，不删除任何原数据。原完整证书仍要求全部原成员可重验；**没有源全灭后的独立目标接纳/恢复能力**。SYSTEM 丢失不能只凭进度或目标自述自动开始新选择；原源 fence 仍使旧代安全停写。认证 Abort 形成新代后，旧 task 拒绝，不自动再采样。

进度只作诊断，保留 Copying/Verifying/Blocked/RegistryPlacementDecided 和具体错误；源 fence 存在时仍带 RecoveryRequired。任意底层 I/O 可阻塞；停止检查点不是硬关机时限。未知硬中止临时文件继续保留阻塞，不提供自动清理保证。

## 范围和成本

仍只支持原准备允许的 fixed-genesis、完整单主盘单位、替换单条旧末级边、显式 1–8 目标及该批完整多级树。placement 元素/retained versions 限制在 100,000 项以内，共同 core/证书分别最多 32 MiB；每个原成员/目标另保留 16 MiB 余量。全原日志/目标正文有多轮完整读取，成本与完整材料及参与盘数成正比，不在普通写入热路径，不宣称性能通过。

下一阶段必须将本决定接入真实活动 registry/locator、完整目标 scope 和新原生 generation 发布/读取，才能开放新写者并报告最终迁移。旧源安全退役、普通设置的自动完整流程、helper 外来高等级盘一次性全链接纳仍未完成。

## 已决定后的读取与恢复续片

[已决定目标恢复](backup-decided-target-recovery.md)增加已留证书时的原选择只读准入与补副本分支：缺实际票即拒绝，不能由证书补票。另可由宿主保存原决定摘要，在原源离线时严格打开全部原目标作冻结恢复读取；它仍不授活动 writer，也不解除本节说明的停写。
