# Backup 首笔 Publication 的通用 generation 冷重放

[任务清单](../../TASKS.md) · [普通 SYSTEM 安装](backup-ordinary-system-database.md) · [Engine](../components/engine.md)

本片将 Backup 原生首笔 Candidate 接入通用 generation／冻结归档的语义消费者。此前这些消费者一律拒绝 Backup，且普通 `result`／`next_state` 只有 Add、Exit、Abort 和 readmission 的配方。本片不是新的首笔发布，也不是活动 Current／writer 安装。按要求未审查、编译、测试、静态检查或 CI，未操作真实盘。

## 一份配方，不重新解释旧收据

Backup 仍使用原 `BackupCore::first_state` 和 `first_candidate`：从原决定认证完整冻结来源，重建同一完整单元路由、State、snapshot、历史根、原主体和永久结果。所有 AVAILABLE 历史版本都携带其完整新路由义务；不可用版本及原 D/undo、退役和旧收据不被转换成可用正文。父子同层同盘、多下级完整链及原 voter 集继续由原证书和投影约束。

`Generation::result` 与 `next_state` 对 Backup 明确分派到这一配方，不会把备份操作解释成 `membership-add`，不再额外增加一次 topology_generation，也不以通用的空 `versions` 结果替代实际首笔全部正文义务。

通用首次 Candidate 认证先检查来源代际、原操作、请求摘要、事务 ID、原边界和固定原 metadata 集，再按原完整来源重建有界首笔编码并比较整个 Candidate 哈希。这个比较也覆盖新旧版本完整路由、所有 snapshot／历史根，而不是只比较序号或一个预期结果字符串。它沿既有 `verify_activation_candidate` 和冻结归档消费路径生效，不增加 caller 提供 State 的捷径。

## 不改第一笔的不可变表示

已决定的首笔仍是原 `membership-generation-candidate`，其 registry 字段保持不存在。后来派生的已发布 registry 不回填进这个 Candidate，否则会改变已持久接受、Finalize、Publication 和正文证明所引用的哈希。

`PortableAuthority` 对 Backup 的当前及已认证祖先 Candidate 只接受准确首槽：不是首槽、任意 lifecycle registry 包装、错操作／摘要／原边界／metadata 集均拒绝。其他 generation 的普通验证分支不变。单独通过这一层只说明原首槽 envelope；仍须既有实际日志、Publication/seal 和完整 State 消费者，不把内嵌的 PUBLISHED 结果视为实际发布。

## 可读语义不授予可写资源

通用 `promote_generation_resources` 和 `complete_generation_activation` 在任何 Backup 资源修改前显式拒绝。即使原首笔能冷重放，也不能借 Add 分支建立错误投票者／普通封套、用空正文集合重复发布、重建被选中的 SYSTEM SQLite、写 Config 或释放来源围栏。

`restrict_io`、活动 registry／namespace 准入仍保留原拒绝；隔离实体不能因通用 Candidate 语义已知就成为普通 serving locator。比原首笔更晚的 Candidate 继续明确拒绝，而不是沿固定成员验证自动获得第二笔写权限。没有新的 writer 提取、活动标记、数据库解静默或源围栏清理 API。

因此真实系统仍可能处于原全池停写、SYSTEM 索引 unavailable 的状态。原任务、原数据、旧 SQLite 保留集和永久收据都不删除；这项冷重放能力也不能修复任何缺失实际材料，不能作为 H03 外来链接纳成功。

## 成本和后继

精确首槽重放复用已有完整编码预算：每个原生对象遵守原 metadata 上限，准备集合最多 8,192 个对象／128 MiB。来源 State 的扫描、路由投影、snapshot／历史根编码仍按完整冻结单位处理，不宣称增量复杂度、低峰值或硬时限；语义消费者不做 I/O 或持久化。正常后续槽不会重复这项展开，因为当前仍被明确拒绝。

下一阶段仍需共同的普通活动 locator／Current、全部目标 writer 生命周期、第二笔 publication 与源围栏安全解除。只有原决定和实际全体材料重新成立之后才能接入，不能把本片的通用语义支持当作它们已实现或已验证。

后继[普通原生对象安装](backup-ordinary-native-metadata.md)已把同一首笔和 registry／Publication 安装到各原盘／目标的普通不可变地址，并从目标实物重放只读 Current；它仍不接活动 selector／writer、第二笔或源解围栏。
