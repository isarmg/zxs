# 跨盘移动的唯一普通 Candidate 与逐成员冷读

[任务清单](../../TASKS.md) · [原输入选择与围栏](file-history-move-joint-choice.md) · [完整历史数据库材料](file-history-move-scoped-databases.md) · [普通正文与未发布安装意图](file-history-move-ordinary-preparation.md)

本片最初基于本地 `557c1fe4` 的独立后继，串行发布对齐 `2cb21721`；前片选择/围栏现已发布为 `ccf3f49c`，本片保留主线其它切片。本片把选定原材料接到真实普通 `Candidate`、完整 State snapshot 和 history graph 对象，不另造 sidecar 投票协议。**仍没有普通 PREPARE/ACCEPT/FINALIZE/PUBLICATION，没有源 D 或目标可见性切换，也不返回 SMB 成功。** 未审查、编译、格式化、测试或 CI，未操作真实数据；新增六项集成测试仅源码。

## 一份候选，原文件语义不变

`prepare_file_history_move_candidate` 先完整执行前片选定材料的只读屏障：同一 choice 的所有副本、原 recovery、每个独立目标 FULL/复制身份、两端 FILE、原 native、所有 scoped 历史/提议库都必须齐全且与当前源相同。没有选择、不完整选择或原材料损坏不能进入候选安装。

原 native 的 STAGING 字节不改。新增私有 `NativeMoveCandidate` 只从该原 draft 派生，保留整个共同 State、原 operation/actor/event、源全部分支 D、目标完整保留历史及映射旧审计。仅把该候选将来成功时的三个结果状态固定为 `Published`、`TargetMet`、`Healthy`，供普通 snapshot 和结果大小预检。**这是事务候选中的预期结果，不是已发出的成功收据。** 类型不可反序列化、无公共构造器，报告不返回 OperationResult；通用 native move 发布硬拒绝继续生效。

从该候选 State 再走普通 FILE capture，两端完整记录 hash 必须分别等于原 native 已验 FILE。对完整已启用盘登记逐个比较原 draft 与候选 State 的完整盘根快照，含递归备份目录；不能假定结果状态不影响投影。这证明原两端 FILE 和提议盘根 current 不需被另一套结果偷偷替换。候选的 SYSTEM 索引单独编码为 `candidate-system.sqlite`，不覆盖原 STAGING 的 `proposed-system.sqlite`，不安装活动索引。

候选 tx 身份由准确 choice digest 域分离确定，不在冷重试生成随机 tx。普通 `CandidateData` 使用同一 operation、下一槽、原 snapshot、准确请求摘要、完整目标版本/placement、全体原元数据成员和完整 history root。普通 snapshot 与全量 history 编码器生成真实不可变依赖，普通发布预算器检查最终证明的全部形状与上限；没有伪造一组实际 ACCEPT 或 seal 来补预算。

## 真实普通对象与范围

全部参与根先无覆盖同步 `candidate-inputs.json`，绑定原 choice/native、候选/快照摘要、源目标 FILE 摘要、候选 SYSTEM 摘要和每个普通依赖的 kind/hash/长度。系统和元数据根额外保存准确 `candidate.json` 与候选 SYSTEM；仅正文参与盘只保存这个范围清单，不获得全池 snapshot、认证数据、SYSTEM 或普通候选对象。

清单持久化后，只有原元数据成员在自身真实 `metadata/objects/{kind}/{hash-prefix}/{hash}.json` 无覆盖安装：

- 完整候选 State snapshot 的所有 map、原永久 operations/principals/events、control-auth 和根 descriptor；
- 所有保留 history 的完整 graph 页与统一 catalog；
- 同一原生 Candidate 对象。

保留现有普通存储的受限路径、不可变字节比较及文件/目录同步。这个阶段仍不产生任何 journal frame；普通冷恢复不会因为某个孤立 Candidate 文件就把目标发布。源原 State 和选择围栏不变。

原清单缺失但对应候选 stage 文件或本盘普通 Candidate 仍存在时拒绝采用，不写新清单包住孤立候选。已存在的异候选、异依赖、损坏 JSON 和旧扁平布局保留拒绝。缺失依赖只有在原清单和全体原材料仍可重建时由显式 prepare 补齐；只读 `read_file_history_move_candidate` 不修复、不跨成员回退。

## 每个原成员必须独立完整

最终按每个原元数据根分别读取所有依赖，普通 hash 认证后逐项与从原源独立重建的规范 bytes 比较。然后只给普通 snapshot/history 冷读器该一个根：完整解码准入 State，核其 Candidate snapshot binding，逐项等于私有候选 State，再校验完整 history root 与该 State。一个成员缺页即失败，不能从另一个完整成员补成“全体已完整”。

这里的 `AdmittedState` 仅是普通字节形状/完整性准入，不被装入 Engine Current，不替代 journal 决定。遍历完成后再验全体参与清单与原 choice/FULL/FILE/scoped 屏障。冷进程先经原恢复封套恢复相同 projection，候选 tx/hash/全部原 State 和图重新生成，再验原对象；不信任存盘成功标记，也不读一份持久模板就直接发布。

## 中断、预算和下一步

首次写入前，全部参与盘配置容量计入前片 FULL、共同/native/scoped/choice、本片范围 stage 以及元数据成员全部候选对象；物理剩余空间按实际缺失依赖和 stage 再加 metadata reserve。每个对象边界检查取消，已有同字节对象同步而不换身份。中途失败返回 RecoveryRequired，保持选择围栏和所有已写文件；不撤销、不删对象、不改槽。

候选普通对象最多 32,768 项，普通对象（含逐项目录开销）加候选 SYSTEM 最多 128 MiB，清单最多 2 MiB；单对象继承 MAX_METADATA。新增 history 聚合准备入口在每个完整 history 进入共享输出前检查累计字节/数量，而不是收集所有图之后才拒绝。它仍可能同时持有前片 scoped 集合、原/候选 State、当前历史编码和本片输出，不宣称进程峰值只有 128 MiB。尚未测量成本或掉电表现。

**这仍不是完整执行闭包。** 后续需要把此唯一 Candidate 接到普通 N 成员决定，持有目标普通正文路径，完成 SYSTEM/FILE/盘根 current-history 的同决定安装和已有源/目标读取句柄围栏，并由原决定冷恢复到唯一结果。当前原 choice 仍无解除 API，不能自动接 SMB。全体元数据普通对象齐全也不能代替以上证明；五项原共同发布义务尚不解除。

六项未运行源码覆盖：必须先选定原材料、普通对象只在元数据成员且 journal 不变、冷重建同 tx/hash/inode、单成员缺对象拒绝回退并只显式补齐、普通 Candidate 无原清单拒绝采用、损坏候选保留拒绝与取消保留围栏。不是运行通过或完整移动验收。
