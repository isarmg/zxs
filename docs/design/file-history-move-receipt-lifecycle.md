# 跨盘移动的单一收据与冷 D 生命周期

[任务清单](../../TASKS.md) · [原生操作](file-history-move-native-operation.md) · [普通正文与未发布安装对](file-history-move-ordinary-preparation.md)

本片基于 `cb7fb288`，保留 `4d803435` 已发布的普通准备。`cb7fb288` 将当前 D 与全部原 delete/undo 收据逐笔对应；跨盘候选的源 D 则属于同一 `file.history.move`，不能再伪造一张 `file.delete`。本片把这个原收据接入同一冷生命周期，同时保持普通 move publication 的硬拒绝。**没有写 journal、决定、活动数据库、源 D 或目标 namespace，也没有 SMB 成功。** 原 `2a01`、`23621` 候选不变。

## 一张收据同时约束源 D 与目标到达

冷读用拥有字段的严格 wire 类型解码既有 borrowed writer 的完整结果，不新增格式别名或默认字段，不改变原规范字节。解码前限定完整结果 12 MiB、导入审计至多 65,536 条；不先克隆无界向量。

读边界核对：

- 原 operation/principal 与准确请求 digest；固定 genesis 的 membership epoch 为 1。只有原候选的 `Published / TargetMet / Healthy` 形状可进入此冷逻辑，`Staging`、降级、缺字段或额外字段仍拒绝。
- 原源文件、仓库、路径 key、source sequence 及紧邻下一槽；D 的 ID 按原 operation/源文件确定，作者、创建时钟及完整去重父集合与同一收据一致。
- 源分支映射的键精确等于 D 的全部 branch heads；每一分支/版本映射仍按原 operation 确定，版本向量与保护向量一致。每一版本保留同一完整历史路由、盘身份/绑定代和层级，不使用当前 placement 替换原路由。
- 导入审计仍使用 retained-copy 的同一所有权规则，source sequence/history/records hash 与源 D 及原收据相符。审计中的 D/undo 仅为目标历史事实，不成为目标当前 D。
- 原目标到达 commit 的稳定 ID、父 commit、作者、sequence、history、种类及单文件条目与该收据一致；事件指向这个原到达 commit，而不是假造的 delete operation ID。后续目标 HEAD 或当前放置变化不改写这个历史检查。

这一层核对逻辑材料内部的绑定。源前态完整重放、全部历史/标签/正文证明、原 SQLite 安装对、journal 成员与真正决定仍由各自边界负责。自洽的收据字节或一次 `RawState` 形状准入不能认证被改写的 publication root，更不能颁发提交能力。

## 与普通删除及撤销共用一条冷生命周期

原 delete、move、undo 按 publication sequence 排序；每一项必须有唯一原 event，不能用同槽另一操作替代。move 仅创建原源 D，原普通 `file.delete.undo` 必须移除这一张准确 D；再次普通删除继续使用同一状态机。重建出的活动 D 集合仍须精确等于 `State.file_deletions`，不允许漏源 D、添加目标 D 或用另一张节点替代。

D 的基础 ID/branch/parent wire 规则由普通 delete 与 move 共用；当前活动 D 的完整 retained frontier 仍由普通 HistoryManifest 准入逐历史核对。旧 move 已被准确 undo 后，不拿后来的源 HEAD 或目标路由重新解释旧收据。

普通热发布的 delete/undo 写集规则不放宽；native move 仍不能进入普通 `prepare_publication`。没有临时关闭 choice/blocked，也没有第二条可绕过共同决定的删除入口。

## 成本、源码范围与未验边界

只增加有界内存 wire/映射验证和原到达记录查找，复用已有冷序列遍历，无磁盘 I/O、文件同步、后台任务或新公开执行 API。空间与时间随保留收据总字节、版本/分支向量增长；读回不克隆第二份导入审计。未测最大规模和 CPU/内存峰值。

新增八项纯收据边界测试源码：单 move 只生成源 D、未发表/降级/未知字段拒绝、请求/D/时钟/映射绑定、准确到达事件、准确 undo、后续普通 delete、解码前审计数量上限，以及冷逻辑准入仍不能绕过普通发布硬门禁。它们不是完整 State、journal、物理副本或故障恢复证明；既有 `move_candidate` 集成源码继续承担真实普通对象冷读场景。

依用户要求，本片未本地编译、运行测试或触发 CI。`04af3778` 父组合曾有修正前 engine lib check 通过；所选 10 项集成运行全部在普通首次 commit 夹具 ENOENT 失败，未进入 move，0 通过/77 过滤。随后目录探测修正及所有新组合均未复验，不能用那次编译结果替代本片验证。

五项共同发布义务仍保留：统一永久收据的正式决定/重放，SYSTEM 与盘根历史新行，三范围决定后共同安装与恢复，双端读取围栏，以及准确解除 choice。此片只闭合冷收据与 D 的逻辑接线，不宣布跨盘事务完成。
