# 跨盘移动的原完整时间线和未接受数据库

[任务清单](../../TASKS.md) · [单一原生操作](file-history-move-native-operation.md) · [共同决定输入](file-history-move-decision-inputs.md)

本片最初基于 `main 160a642f`，发布对齐 `c1788d3a` 并保留主线新盘根目录行投影，新增 `prepare_file_history_move_scoped_databases` 与只读 `read_file_history_move_scoped_databases`。它将原来仅有 FILE 的准备推进为真实 SYSTEM／盘根数据库材料，不把准备当共同决定。**未审查、编译、格式化、测试或 CI，未操作真实盘。**

## 原历史和本次提案必须分开

准备首先完整重验原恢复封套、全部目标 FULL 和复制身份、两端 FILE、原共同输入以及原 native-operation/native-state。随后从准确原固定成员 journal 的全部实际 PUBLICATION 重放，不从可丢弃的活动 SQLite 复制，不从今天的最终 State 猜过去：

- 每个已发布时点独立验证 Candidate 的固定成员、PUBLICATION seal、完整 snapshot 及其原 operation；同一 sequence 的不同有效来源拒绝。复用盘根历史的认证前缀消费者，不新增另一套多数决定规则。
- 每个原时点保存一份 `published-system-{sequence}.sqlite`。每份由该时点的完整 `AdmittedState` 重新编码现有 SYSTEM 索引：完整保留收据、路径事件、三十四个业务 map 族、operation principal、保持 ordinal 的全部事件及 State 三项代数。私有认证数据不进入这套索引。它不是把当前备注、位置、策略回填到旧时点。
- 各参与盘保留准确原注册身份的 `published-current-{disk_id}.sqlite` 和 `published-history-{disk_id}.sqlite`；后者覆盖从空 genesis 到原源头的完整本盘变更。末端本盘行和完整源 State 必须分别等于原捕获，不能重标到提议 sequence。
- `proposed-system.sqlite` 与 `proposed-current-{disk_id}.sqlite` 单独来自私有原生 move 模板。同一 STAGING operation／principal／event 携带源全部分支 frontier D 和目标映射旧审计；其提议 sequence 仍未预留。SYSTEM 编码器有显式 unaccepted move 入口，不能把这个 State 构造成 `AdmittedState`。

**没有在盘根已发布历史中追加本次 move。** 新源 D 和目标状态目前在未接受 SYSTEM、已有两端 FILE 以及提议 current 中；盘根 current 的版本库存本身也不替代 FILE 的完整 D／分支语义。最终共同 Candidate 及真正 PUBLICATION 尚未建立，不能生成一条声称该移动已经发布的盘根历史记录。

当前完整时间线只接受最多 32 个盘位的固定 genesis 配置及最多 2,048 个连续原发布。成员 generation／归档、降级恢复覆盖的历史没有完整重放配方时明确拒绝，不从较晚边界开始、不忽略 recovery outcome，也不伪造完整覆盖。已有原材料保持原样。

## 一个原清单，严格范围和真实文件

全部参与根在既有 `file-history-move-decision-inputs/` 下保存相同 `scoped-databases.json`。清单绑定原 operation、原 native digest、原 decision-input digest、原已发布边界、未接受提议 sequence，以及每份库的范围、实际长度、内容摘要；每个 SYSTEM 旧时点还绑定准确 Candidate／snapshot／PUBLICATION hash。

系统及元数据成员保存整套历史 SYSTEM 和参与盘数据库。仅正文盘只保存自身准确 disk identity 的三份盘根库，不保存 SYSTEM、其它盘的行、完整池 State 或私有认证材料。全体根仍有相同的范围摘要清单，不能靠一个在线盘替代全部参与者。

原 `inputs.json`、`unaccepted-state.json`、native 文件和 digest 均不变。本片不写普通 `index.sqlite`／盘根库别名，不创建 Candidate／PREPARE／ACCEPT／成功标记，也不关闭或切换活动 SQLite writer。

所有文件沿既有私有 stage、持有根、writer.lock／实际磁盘 seal、I/O gate、原 manifest、无覆盖原子写和文件／父目录同步。实际存盘 bytes 必须逐份等于从认证源重建的预期 SQLite，不信任库自摘要或已保存的报告。读回持有原 fd，复核 inode／属性、名字、stage 与所属根；末尾再次验证全部原 native／FILE／FULL 及源捕获。

## 中断及冷进程

第一次写库前先保存不可变 scope 清单。范围清单缺失但任一期望 scoped 库仍存在时拒绝采用，不用一份新清单为孤立旧字节补来源。已存在的异清单、损坏／异内容数据库原样保留，不能覆盖。

同一原 scope 清单下，显式 prepare 可以补齐完全缺失的库；只读 read 对缺失立即失败，不顺手修复。任何写入后失败保留可能已写部分，要求继续原 operation，不返回失败回滚、更换操作或清槽。冷进程先恢复原 move projection，再从原认证前缀和模板完整重建所有预期文件，逐根比较；不把存盘模板或清单自摘要当准入许可。

配置容量在第一份 scoped 写入前检查全部参与者，计入既有 target、共同输入、native 和 scoped 材料；实际物理 reserve 在每个持有根安装时重查。后续失败仍可能留下部分文件，不能撤销已同步字节或制造成功。

## 成本和未完成边界

整套 scoped SQLite 总量上限 128 MiB，单份 SYSTEM 最多 64 MiB，盘根库保留原 32 MiB／65,536 行等限制，清单最多 2 MiB。SYSTEM 保存全部实际时点意味着成本随完整保留历史和各 State 大小增长，不是增量时间线算法。内存保留有界输出集合，外加当前解码 State、正在编码的一份数据库和原 move 材料；不宣称总峰值只有 128 MiB。元数据成员各保存全集，磁盘总开销随原成员数增加。取消在发布和库边界检查，原盘根单库有界编码期间不声称即时抢占。

五项共同发布义务仍保留：本片增加真实历史／提议材料，但没有普通数据库共同安装、统一永久结果准入、唯一 Candidate／全部成员决定及原决定恢复、双端现存句柄／目录页的读取围栏。因此不解除通用 native move publication 拒绝，不返回 SMB rename 成功，不隐藏源，不提供 copy 后 unlink。

新增八项集成测试源码覆盖原历史与提案分离、元数据／正文范围隔离、冷续作和原 inode、缺一文件的只读拒绝与显式补齐、无清单孤立材料拒绝、有效 SQLite 但缺原 operation 的拒绝与保留、native 前置／取消、无完整 generation 时间线的拒绝。**这些测试未执行**，不构成生产、真实 SMB、断电或原子发布验收。
