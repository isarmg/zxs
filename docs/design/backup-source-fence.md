# 备份切换的原生源写围栏

本片基于 `47109fc3`，把已经验存的完整目标输入接到**真实原成员日志围栏与认证取消**，仍不是新目标最终权威切换。按当前开发要求只交付源码，未审查、编译、测试或运行 CI。

## 调用范围

`Engine::begin_backup_cutover_fence(operation_id, principal)` 只接受既有 `start_backup_cutover_preparation` 的同一原操作和 actor。宿主负责实际身份授权，不能把用户提供的 principal 字符串本身当授权。没有 HTTP、Web 或 helper 调用入口，也不因后台材料完成自动停写。

支持范围仍是固定 genesis、原末级替换、原完整池和明确 1–8 个完整目标。每个目标包含全部应保留文件历史、目录仓库及父子整体，不拆分单位；新树沿同批目标闭合到原来源，逐跳前缀与旧任务保持一致。活动 Config 的多下级路由、复杂父子旧异盘合流和外来完整链正式接纳未实现，不能用新围栏放宽原拒绝边界。

这是**全池且可能长期的业务停写**。当前没有最终目标激活，所以宿主不得把它作为普通备份设置的隐藏步骤或显示为迁移完成。材料阶段继续自动后台复制和原任务恢复；本片的源围栏与取消是明确的可信宿主核心 API，尚未由完整迁移调度器自动串联。

## 源围栏的真实持久顺序

1. 拒绝另一个操作、未发布写入、未结束 worker、已有不相干围栏。结束的 worker 必须先 join；异常不抹掉任务。
2. 读回 SYSTEM 的精确切换任务；核 pool/config/sequence/snapshot 与当前 State。重新获得所有原目标 writer locks 和 held 根，不从完成状态缓存推导健康。
3. 在每个目标重新读取完整原生前缀、正文和历史、FILE/目录数据库、SYSTEM/盘根原范围数据库及全目标输入确认。目标数量、顺序、device/inode、新身份和来源边均绑定原任务；任一目标缺失/损坏则在源围栏前拒绝，不用其它目标补足。
4. 固定原发布 Candidate/snapshot 边界。原 Config、原 metadata 成员集和 quorum 不变，严格类型的 `BackupCutover` origin 嵌入同一任务及验存报告；新目标没有投票资格。
5. 在可能发生不确定写入之前，Engine 保守关闭原代业务写。先在所有原元数据成员保存相同哈希的 `membership-transition` 对象，才逐个追加真实、同步的 `MembershipFence` 日志记录。
6. 原生 append 的实际 receipt 数构成 `backup_cutover_fence_status`。目标持有锁直到本次调用结束，写前后重核命名身份。即使结果返回错误也可能已有部分原成员被围栏，不能按错误码推测“没有发生”。

证据封套有 16 MiB 编码上限；原生意图仍受现有 metadata 上限。首次调用的目标重验成本为完整材料量，另有完整 State 摘要和原发布边界扫描，不放进普通业务写入热路径；同步成本为每个原成员的不可变意图安装与原生 fence append。操作是同步核心调用，底层 I/O 不承诺硬时限。

## 不确定结果与冷开

即使只发现一个原成员的合法 fence，既有冷准入也读取其准确意图，检查原 pool/config、actor、sequence/snapshot、完整目标任务与原成员日志，并保持原源阻写。相冲突的 fence、围栏后的业务记录、不同发布边界都拒绝；恢复不能重采另一个目标集合或把源当前 HEAD 当作原任务。

围栏证据保存在原成员 metadata 对象中，不依赖 SYSTEM 的进度 JSON 才能关闭旧源。SYSTEM-loss 后仍不等于自动发现完整迁移任务或完成目标激活。`backup_preparation_status` 在存在该源围栏时携带 `RecoveryRequired`、真实原成员 receipt 数和“未激活”说明；后继实体安装保留实际 worker 阶段／具体错误，不能将材料阶段当作写围栏已经解除。

同一 actor/operation 重试只补齐**同一个源围栏**。它不重新选择目标，也不声称目标此时仍健康，因此重试不要求打开已失联目标；完成原围栏只让原源继续安全停写，绝不授权目标。未来最终决定仍须重新获得并独立核对所有目标及其活动安装。

## 可恢复的原成员认证取消

在尚未选定后继共同决定时，源 fence 可按以下认证协议退出，不是直接清除 `blocked`。后继 [registry/placement 共同选择](backup-registry-placement-decision.md)与 Abort 共享同一槽；足够 Backup 选择后 Abort 可能不可达，不能再承诺可取消：

- 使用既有原成员完整 control archive 和 `certify_membership_abort` / `cancel_membership_handoff`，按固定原 N/Q 做不可变 Abort 选择。
- 新备份 origin 只能使用 source-only 取消分支。它要求目标 Config 与原 Config 完全一致、无新根登记、无 ExitEffect、无缩小围栏范围；原普通 Add 的准入不被放宽。
- 取消保留准确原操作和 actor，生成 `backup-cutover-cancelled` 永久结果，不生成 `membership-add`、新路由或迁移成功结果。原数据、图、旧回执和隔离目标材料不删除。
- 达到 Abort quorum 只证明取消决定。恢复普通写入仍须按既有完整 activation publication 与原资源闭包完成；部分 marker/日志持久化由原决定恢复，不能把取消请求失败当作回滚完成。
- 普通待成员 enrollment 和 Joint/Add activation 显式拒绝 `BackupCutover` origin，不能把准备盘塞入普通 Add 流程绕过未实现的最终切换。

取消会形成新的原成员代际和永久结果，已冻结准备任务随之变为旧证据；本片不自动重采、清理或创建下一任务。当前准备仅支持 fixed-genesis，因此取消后的新准备与后继代际迁移仍须后续协议，不以取消为隐式升级。

## 尚未完成

本片只关闭“目标已备妥之后，原源如何真实停写并可认证取消”的前置缺口。仍缺：完整 SYSTEM 历史／盘根目录标签审计、活动文件/目录实体安装、目标原生接纳与共同最终决定、同一 registry+placement 变更、活动多下级写入/读取、最终切换后的冷重放、helper 一次性完整外来链接纳、自动串联上述阶段和旧副本安全退役。

保留原数据和不开放目标权威是本片边界，不是完整迁移的替代定义。没有新目标 `activate`/`finish` 成功 API；不得把实际源围栏 receipt、全目标准备确认或认证取消当作最终迁移已完成。

## 后继隔离实体安装

[目标实体安装](backup-target-entity-installation.md)从完整原围栏继续，复制 FILE／目录 SQLite 与各保留正文到隔离映像，并在冷开按原任务续作；这不是此处仍缺的活动实体／最终权威发布。原生 source fence 和认证取消含义不变。
