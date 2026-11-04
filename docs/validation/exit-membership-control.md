# 退出成员联合控制准备实验

本批只实现退出操作的类型化意图、old-only 控制参与者、独立两侧票数及其恢复边界。源码与八组定向测试已编写，尚未编译或执行；本地仅整理源码格式与差异，待 GitHub 独立验证。不得将前批纯添加通过结果外推本批。

这里的“盘”是 ZXS 登记的数据存储目录；`metadata_member` 仅表示该目录的元数据投票角色。目录可以位于同一文件系统，不要求独立 Debian 系统设备。本批不涉及挂载、格式化、系统盘管理或真实目录操作。

## 已实现的协议边界

- 实验入口 `begin_membership_exit_preparation` 接受准确 disk_id、`PlannedOffline` 或 `Deregister`、actor 和 operation_id。不可变 intent 绑定退出类型、disk/slot/binding、原/目标配置和精确冻结边界。不同类型不能用同一操作幂等键互换。
- 目标配置保留全部历史拓扑记录，仅将目标目录的未来 voter 标志移出投票集合。当前 Engine 配置、登记、内容可见性和历史 Placement 不变。非投票目录也可形成显式退出意图，但两侧投票集合保持不变。
- 控制参与者为原与目标 voter 的并集。proposal 将 target dependencies 与 old-only dependencies 分页分开绑定；前驱 fence、原目录 seal、物理目录身份、控制归档、bootstrap、control receipt 和 acceptance 均按原准确身份检查。
- `fenced-exit-control-decision` 是控制准备证书，不是下线、注销或新一代业务发布。原 pure-add intent/proposal 省略退出字段，保留现存对象的哈希表示；没有旧程序兼容或旧二进制测试。
- 每票按准确成员身份分类为 old-only、retained 或 new-only。old-only 只计入原 Q，retained 分别计入两侧，new-only 只计入目标 Q；非投票目录 readiness 不是票。两个 Q 都来自各自完整配置，不能按在线数降低。
- 退出控制证书的可搬运对象图只要求真实有效的 oldQ 与 newQ，不要求每个 old-only 必须有票，也不要求每个目标成员都有票。它仍要求完整、已绑定的控制依赖闭包。检查某实际成员的准入则另外读取该成员真实 joint journal，缺该成员票不能借可搬运证书冒充本地准入。
- 状态分别报告原/目标 N/Q、各侧已同步票数、union 大小及实际 certificate 持久位置的两侧计数。`all_certificates_synchronized` 只表示覆盖完整，不定义证书多数法则。
- 同一当前代际的 active journal 写入退出 fence；控制材料仍位于 intent 独立 pending namespace。后继来源绑定真实前驱 authority，不能重标 Genesis。正常冷恢复保留原代际已发布数据并维持 fence；不会从控制证书发布下一代。

## 不得越过的完成边界

生产 `activate_pending_membership` 拒绝任何退出效果；代际 authority 验证也拒绝将退出控制证书解释为活动权威。若当前代际已有后续 fence，`resume_membership_activation` 在推广 seal 或执行恢复发布之前拒绝，避免把已有前驱 marker 当成下一次操作已经准备好。

本批不写注销恢复入口，不改变 registered/disabled/detached 状态，不筛选当前文件列表，不撤销目录 I/O，不关闭旧资源，也不产生退出成功的永久业务结果。删除登记最终不擦正文，独占文件退出当前列表且根目录保留可恢复登记材料；计划下线仍保留登记与内容可见性，禁止 I/O。这些生命周期效果必须由后续独立的 registry、恢复材料、持久 I/O 收束和认证发布闭环实现，本证书不能代替它们。

当前运行时准备路径仍沿既有全原成员 fence/归档检查，并收集、复制、确认全部 union 的实际控制材料。因此尚不能在退出成员已经缺席时启动或完成这段准备；这是当前采集驱动的能力缺口，不是联合多数的最终协议规则。可搬运证书 verifier 已与此限制分离，不能把“缺一个 old-only”无条件判为票数不足。

最后一个 voter 退出还缺终止池权威，入口在写 fence 前明确返回未实现。不能伪造 N=0/Q=0 活池，也不能把阶段缺口定义成最终产品禁止删除最后目录。未开放 HTTP/Web，不含自动目录发现。

## 待 GitHub 执行的八组测试

现有 membership 定向过滤会自动选择 `membership_joint_exit_`：

1. 3→2 计划下线准备和 4→3 注销准备：真实原正文可读、原文件字节及永久 State 不变、登记和历史记录保留、角色及两侧计数准确；直接激活与自组代际 authority 都拒绝。
2. 从实际持久票生成 3→2、4→3 的全部非空子集。按固定两侧 Q 接受或拒绝，覆盖无需 old-only 的合法多数、仅新 Q 成立及仅旧 Q 成立；冒名、重复票和外来成员拒绝。缺本地票时不能通过实际成员准入检查。
3. 每个 joint vote 后的中断，分别正常冷开和丢失 ZXS control-state 目录后恢复。准确 effect、intent/proposal、正文和幂等键不变，同一意图续作且不产生成功结果。
4. 退出目标删除历史节点、换根、换 binding 或额外改变 voter 均拒绝；退出意图不能冒充 pure-add 证书格式。
5. 最后 voter 的终止权威缺口在任何 fence 写入前拒绝，目录字节不变。
6. 首个 fence 成员完成后中断，冷开后按同一意图补齐，原代际始终 fenced。
7. 已有 nonvoter 的退出控制准备，固定 voters/Q 不变、登记和资源仍保留，不以准备状态冒充下线完成。
8. 完成真实纯添加后再准备退出：前驱 authority 精确绑定，已有 marker 不能绕过下一 fence；显式激活恢复拒绝且各数据目录字节不改，普通重开仍为旧代际受限状态。

故障点与临时目录测试不等价真实硬件断电。已有纯添加、历史证明和恢复过滤仍须回归，本批没有新增完整 CI 或本地重编。

成本沿用控制归档验证与分页对象写入；新增并集身份集合和成员分类，不读写用户正文。尚未做大规模归档性能验收，不宣称此阶段消除了全旧成员依赖或历史验证的累计成本。
