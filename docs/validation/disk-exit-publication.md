# 计划下线与直接删除登记：规定发布接线

状态：本轮源码和定向回归已编写，尚未执行编译、测试或 CI。格式整理和差异检查不是运行验收。本文描述与生产 I/O、bootstrap 及当前命名空间消费者联合整合后的边界，不宣称独立补丁即可使用。

## 规定顺序与真实权威

`begin_disk_exit` 只创建原成员固定 fence，返回准备进度。它不声称目标已经离线。受管理的 control archive 和各成员自己的 pending/joint journal 先完成原 Q 与目标 Q 的独立验证。历史 Candidate、Placement 和原永久结果不改字节，不凭在线成员数修改 N/Q。

`activate_pending_disk_exit` 随后执行：

1. 从冻结源 State 和已准入当前视图生成源 registry、完整 occurrence mask 和准确 successor。首个 Exit 从原登记产生源；后继必须从准确前代 registry 继续，不能由历史 Config 重建活动登记。严格 `disk-exit-generation` core 同时绑定原 source publication、源业务/拓扑代数、退出身份、actor/operation、joint authority、control archive 和确定对象图；输入图不反向引用 D3，避免环形哈希。
2. 在每个保留目录独立验证完整控制归档，并持久写入 `metadata/membership-exits/<intent>/authority.json`。这不是活动 marker，更不是写在退出目标上的恢复文件。所有新图/归档写入都在保留目录；目标物理数据不删除。
3. 只有上述闭包已存在，才在受保护系统目录同步准确 stop。stop 后、活动 marker 前的崩溃，可只从保留目录的 prepared authority 继续。没有可信闭包不能靠重新访问目标补证。
4. 关闭目标新普通 I/O，撤销可撤销资源，释放唯一未使用 control responsibility，等真实尾部、root descriptor 和 writer lock 全部关闭。忙尾部返回失败/进行中而非成功；同一运行时保留原 gate，重试不重建或读取目标控制材料。
5. 在资源集及 writer 集移出准确已关闭目标，并只对保留目录安装新代际 marker、seal、journal 和 D3。Config 的历史拓扑保留；其资源准入另由已认证 I/O 集限制，不能借删除 Config 行来缩小 quorum。
6. D3 候选与永久结果完全按 authority 推导，保留冻结的 immutable commits/versions 和当前 heads。最终准入重读保留成员实际 journal receipt、Candidate、Publication、完整 seal 和 snapshot，核验固定目标 Q、准确 durable stop 和不可伪造 I/O closure，才构造 `AdmittedDiskExitPublication` 给当前 namespace/registry 消费者。

`disk_exit_status` 区分 Fenced、Activating 和 Published。Activating 必须直接重试激活，不允许后台再次打开原目标重做 archive/enrollment；Published 需要已准入当前投影及永久结果，不能从准备 marker 推断。

## 重启及后续普通发布

bootstrap 在首次存储根 stat/open 前从受保护 locator 收紧探测集合，并在保留根认证完整 lineage。陈旧 Config、prepared marker、活动 marker、丢失系统目录的显式保留根恢复都不能扩大根集合。

进程内重启还需独立的 `RecoveredDiskIo`：即使新 Engine 没有目标 handle，旧 Engine 的 reader/worker 也可能仍持有真实资源。恢复准入须检查同一逻辑 binding 的所有进程内物理 gate 已 Closed，并在 Engine 生命周期保留禁止新 gate 的 guard。仅 `ordinary == 0` 或“当前资源表没此盘”均不够。

退出代际恢复禁止用 checkpoint 越过 D3 投影安装。准确 D3 先安装 registry/namespace view，后续每个真实快照再推进该 view，不能从历史原始 head 复活隐藏 entry/file 身份。普通新 Candidate 持续绑定同一个已准入 registry；无 registry 的既有 Candidate 保持原格式字节。

## 后继 Add、再次退出和 Disabled→Deregister

后继 authority 分别验证成员链和 registry 链。带 registry 的 Add 在完整前驱状态上追加新绑定和 voter，同时保留旧 Disabled/Detached、历史拓扑和全部隐藏身份；未带 registry 的既有 Add 字节不变。Abort 继承原 registry，不能借取消操作复活目标。只有实际完成前代发布并拥有当前 namespace view，才能创建下一 fence。

冻结归档已保存每个历史 epoch 的独立原成员 journal、Candidate、Publication、seal 和 snapshot。恢复按完整已验证发布顺序重放前代 D3 和普通快照，到达本次准确 source；每次 D3 重算规定 registry/mask，普通发布推进已有 owned view。历史重放证明仅创建 owned view，不能安装运行 Engine。运行安装仍要求本代 retained D3 和全部历史逻辑 IO 域的真实排除证明。任何带 registry 的代际，包括后继 Add 和 Abort，均禁止 checkpoint 越过这个过程。

Disabled→Deregister 复用第一次停 IO 的 operation/intent 和逻辑 guard。该盘不参加新的原/目标 voter 控制、不会重新获取 handle，也不会追加第二份 stop。新 marker 前崩溃可仍选到旧代及新 fence，须从保留根继续同一准备；有新 marker 后只恢复新代。这与首次 stop 后从 prepared authority 恢复的切点不同，管理 continuation 必须使用 Engine 的实际 phase，而非仅保存的 HTTP phase。

Engine 将所有历史 `RecoveredDiskIo` 按 disk identity 保留至自身释放，下一准备和激活不会替换丢失早期 guard。所有资源操作遍历由完整 authority 得到的 io_disks，历史 Config 行和每个 epoch 固定 N/Q 均不变。

当前重建采用全历史顺序回放，时间随归档发布及元数据闭包增长；不保存可绕过验证的 namespace cache。运行中仅保留前后 State 与 owned view，便携 core 仍受 MAX_METADATA 限制。尚无性能或掉电运行验收。

## 明确尚未闭合的范围

- 缺席退出目标的采集驱动已增加源码接线：唯一 old-only voter 缺席时，以显式 scope 绑定原固定多数、完整冻结历史和全部 retained target 控制材料；data-only 目标缺席沿完整原 metadata 格式。另一 retained metadata 成员缺席仍未支持。证明、恢复与尚未执行的回归边界见[缺席退出目标](missing-exit-members.md)。
- 重新启用和重新登记仍未实现；不会因旧目录重新出现而恢复登记、IO 或历史身份。后继 Add 只能追加新的 disk/slot/binding，不能改写历史行或重用已退出身份。
- 最后 voter 的管理终止池 authority 未实现，在任何 fence 或 stop 写入前拒绝，不能产生 N=0/Q=0 活池。
- 当前消费者只开放实际已经准入的写流程；尚未接入的 merge/rebase/历史导入等入口保持明确拒绝。目录可以在同一文件系统，未增加设备管理、挂载、格式化或物理擦除。
- Exit portable bundle 遵守既有单对象 MAX_METADATA 上限；超限在 stop 前失败。尚未做大规模分页便携证据封装和性能验收。
- SMART 设备 I/O 已改为服务内受管同步 NVMe/SAT 查询；外部 smartctl 仅从 stdin 离线解释回复，不持有目标路径或设备 FD。跨服务进程重准入依赖内核同步命令结束及现有 writer ownership，仍需真实进程崩溃与硬件故障验收，不能以源码改动宣称已验证。详见 [SMART 生命周期](smart-query-lifetime.md)。

## 本轮待执行回归源码

`disk_exit_publication_tests` 覆盖真实 3→2 下线/删除 D3、实际 descriptor closure、目标完整目录字节不变、共享备份可读且不变 primary、忙尾部同操作续作、源代数/缺 mask 拒绝、最后 voter 和重复下线拒绝、陈旧 Config 重开，以及 stop/closed/部分 D3/配置投影前中断后仅从保留目录继续。

后继源码还覆盖目标路径已不存在的 Offline→Detach→Add→再次 Detach→冷启、Add 后 certified Abort 保留原 registry/过滤视图、registry 链缺失和源代数篡改、Archived proof 不得安装 Engine，以及 Disabled→Detach 的准备/部分发布中断。上述用例全部尚未执行。

还必须联合执行原 Add/Abort、历史/内容、bootstrap 首次 syscall 排除、当前可写 namespace/旧 workspace、HTTP continuation 和缺副本恢复回归。临时目录故障注入不等于物理掉电，不可代替真实部署验收。

## HTTP 后继与系统配置准入

HTTP 只把同一操作的 Fenced/Activating 当作续作，上一操作 Published 不阻止新请求。Disabled 仅允许独立注销，不重复下线。旧完成结果从已认证永久 operation receipt 查询，即使头已变成 Add、Abort 或其它 Exit，也不依赖当前头的退出状态；管理记录不是成功依据。

启动配置准入流式读取受保护系统目录中的既有 Add/Exit 意图，每条记录仍按原严格格式和单记录上限验证，只保存配置摘要边。当前配置必须命中其中一类的最新意图端点，并沿完整的实际登记边回到初始激活配置；Disabled→Detached 的同配置边不产生循环。这个只读链不修改 Config、成员数或阈值，不授予存储访问，之后仍由 Engine 验证实际成员代际、locator、journal 和 D3。任一必需边丢失或配置编辑不匹配时拒绝，不重置数据。源码用例包含 Offline→Detached→另一盘退出→Add、旧 HTTP 完成投影丢失、配置祖先边丢失、冷启与目标目录已移走；尚未运行。

链读取逐条解码意图，总扫描字节与记录总字节成正比，摘要图插入和查找为 O(E log E)，内存为 O(E + 单条受限记录)，其中 E 为不同配置边数；不加入生命周期次数上限，不复制完整历史 State，不触及任何数据盘。
