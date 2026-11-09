# 跨盘移动：单一决定的最小执行与冷恢复路径

[任务清单](../../TASKS.md) · [普通准备](file-history-move-ordinary-preparation.md) · [原 PREPARE](file-history-move-journal-preparation.md) · [原收据生命周期](file-history-move-receipt-lifecycle.md)

本候选以已发布 `bcf065da` 为父基线，直接消费既有准备源码，不再新增供用户点击的准备报告。**源码已接实际执行，不等于已运行或产品验收通过**。遵用户最新要求，本地没有编译、测试构建、测试运行或 CI；历史 10 项普通准备集成仍是 0 通过、10 失败、77 过滤，均在首次普通 commit 夹具 ENOENT 失败。其后目录探测修正及本组合没有复验。旧候选和补丁保留，没有操作真实磁盘数据。服务端独立入口 `265a437e` 已在同父基线上合流，见[默认共享入口](file-history-move-smb-entry.md)。

## 用户入口和范围

`Engine::move_current_file(&FileHistoryMovePathRequest, principal, operation_id)` 接受四个 String：源仓库、源路径、目标仓库、根单分量目标名，返回普通 `OperationResult`。只有一个原 `file.history.move` 的 Published / TargetMet / Healthy 结果能作为首次成功。先按原 actor、operation 和四个准确字段重放永久收据；不能先查今日路径，再把已隐藏源或后续改名误判成另一操作。

新操作一次捕获原 file ID、两端 HEAD、完整保留分支前沿和 clock。已有持久意图只重建原请求、原 slot、原 route、原 clock，不刷新为今日 HEAD。合流的默认 SMB 入口负责认证、两端当前授权/账户世代、两端 writer reservation，以及未知返回后的原 SMB 意图保留；Engine owner 锁内再次检查两端 binding 和原源身份。内存 reservation 不是持久围栏。

最小范围仍限定固定 genesis、全部原资源可用、一个盘根仓库中的单文件 occurrence、另一个主盘的空未初始化目标仓库。源可位于尚未初始化的普通子目录，不能位于独立目录仓库内。拒绝覆盖、非空目标、目录或多选、共享多 occurrence、成员变更/外来接纳、分裂路由、未支持的 placement/backup override 和不能证明的旧 graph-only 生命周期，不退回 copy + delete。

## 从原材料到实际决定

1. 持久保存原 recovery envelope **之前**设置进程拒写状态；冷开读取 SYSTEM 和原成员封套恢复拒写状态。即使尚未完成后续 choice，另一个普通操作也不能改变源、把原准备永久卡在过期基线上。缺原件只允许拒绝，不凭缺席清除围栏。
2. 内部调用既有独立 FULL、源/目标 FILE、native/scoped、choice、普通 Candidate、普通 FULL/安装对和 PREPARE 准备。所有准备 API 仍不单独返回成功。选定后其原槽和普通下一槽分离。
3. 从原出版前缀加**同一个 candidate-bound State**生成最终盘 current/history 对。原日志 writer 只缓存末槽，因此完整历史改读真实日志帧，并在已验证边界停止，不把末槽缓存当 genesis 前缀。原前缀每个 Publication 的摘要、完整快照和 seal 仍独立核验。
4. 首 ACCEPT 前完成所有原 FULL 和 pending FILE 对检查、原成员/资源检查、最终数据库准备和容量准入。执行对最终 history 每参与盘另留 32 MiB 有界额度；SYSTEM 预留实际准备镜像的三倍事务空间及原 metadata reserve。普通完整正文/保留安装配额不取消。决定恢复可使用原决定的恢复准入，不因自己的保留尝试重新获得一个新候选。
5. 专用执行器调用普通 ACCEPT → FINALIZE → preserving locator/FILE installation → FULL proof → PUBLICATION → member seal/local proof。一般的裸 State/native publication gate 仍拒绝 native move；没有替代 receipt 或额外 file.delete operation。
6. 所有原成员的真实 PUBLICATION 帧和 seal 读回完成后，才产生私有 `PublishedMove` 能力。它消费最终各盘 current/history 安装对、源/目标各 FILE 的完整实际字节回验，以及真正 SYSTEM writer 的事务及完整语义回读。SYSTEM 不临时替换 `Engine.state` 来取得安装权限。
7. 再次核完整目标 FULL，把原准备材料按原 operation 保存到 `file-history-move-completed/<operation>/...`。先在全部参与根归档 envelope/target/ordinary，最后才归档各份 choice；使用不覆盖的重命名和目录同步，不删除原正文、原 FILE 或安装历史。最后一次性替换可见 State/历史根/snapshot 并解除围栏。

数据库名称交换不是跨文件系统的物理原子交换。公共可见性由原 journal 决定及一个 Engine State 替换统一控制：任一交换、读回或归档失败时，旧 State 的源保持可见、目标保持不可见，普通写入继续被围栏拦截。未来 State 的中间 SQLite 文件不授予 namespace 权限。源 D 只冻结源，不擦除源 FULL。

## 中断与冷恢复

- 无 ACCEPT 的原 PREPARE 仍留旧源；显式原 path 请求可续作，不自动创建新 operation。
- 冷准入只接受原 choice 的准确 slot/candidate/transaction，成员本地阶段必须依序 PREPARE、ACCEPT、FINALIZE、PUBLICATION。重复、跳步、其它候选/协议/后继槽拒绝。出现 ACCEPT 时须保留每个原成员 PREPARE。
- 对已接受原候选，先恢复旧出版前缀，再从 native receipt 的原请求/route/clock 独立重算完整 State；重算必须与已认证 snapshot 完全相同，并命中 choice 原 native 摘要、完整 versions/placements。不能拿一份损坏 sidecar 或“完成”标记代替这个证明。
- 普通 `Engine::open` 沿这一个决定继续普通 journal、物理范围和归档，然后同时显露两端。已有 PUBLICATION 但范围/归档未完也走该路径，不提前宣称首次成功。部分归档不要求已归档 singleton 原址重新出现，也不会重复制出新原件。
- `open_for_recovery` 遇只有 ACCEPT 的未发表候选仍保留旧 State，不自动完成它；已有发表决定沿既有恢复规则补齐。这不是零写入入口。
- torn suffix 在只读恢复中保留。在正常恢复中，只有原合法前缀、原选择以及完整未接受准备或已认证接受候选建立之后，才通过现有 journal 恢复原语修复无效尾部；只续原候选。没有有效证明、缺源/目标 FULL、缺成员、错身份或未知安装对象时停下并保留数据。
- 首 ACCEPT 后或其写入结果未知后的错误为 InDoubt，不能视为已中止。原 operation、旧可见 State、choice 和剩余数据都保留。完成后重复请求返回原永久结果，不重新搬运；后续另一次受支持移动可使用空出的 singleton 名称，旧材料和旧收据仍保留。

## 成本、限制和证据

原准备闭包仍有多次完整正文/State/数据库读回；最终每盘对集合最多 128 MiB，历史仍受 2,048 个出版与 32 个固定盘等原界限约束。每份普通 FULL 是独立副本，不用硬链接/reflink。最终安装保留旧字节，归档不回收空间。没有测量吞吐、锁持有时长、峰值内存或总 fsync 数，未提供性能结论。

新增 7 个集成函数的**源码**覆盖保留分支/标签/正文、三范围与冷开、原操作重试/后续改名、连续两次移动、非空目标/成员变更拒绝、早期意图围栏、12 个决定后中断点、普通 FULL 损坏不从 stage 冒充原件恢复。原准备测试按更早围栏和受证明 torn-tail 恢复语义更新。合流入口另有 16 个服务端测试函数，仅检查适配和成功门的源码，不能充当核心发表证据。它们尚未执行；临时根故障点也不能替代真实掉盘/断电。组合编译、根测试、SMB transport、并发/账户撤销、真实盘与断电均待后续获准验收。本候选不宣称所有跨盘移动或生产可用闭环已验收。
