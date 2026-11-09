# 跨盘移动的原输入选择、写入围栏与冷恢复

[任务清单](../../TASKS.md) · [原完整时间线与 scoped 材料](file-history-move-scoped-databases.md) · [单一原生操作](file-history-move-native-operation.md)

本片最初基于已发布 `5c14a7a`，串行发布对齐 `cb55c58e`，保留主线其它切片。在原 FULL／两端 FILE／native／SYSTEM 与盘根 scoped 材料之后，增加准确原 operation 与下一槽的不可变持久选择。**它只是共同 journal 决定之前的输入选择和拒写围栏，不是 ACCEPT、投票多数、成功收据或已发布移动。** 未审查、编译、格式化、测试或 CI；未操作真实盘。八项集成与两项单元测试只有源码。

## 为什么先固定原输入

只保存可重复生成的提案，不能阻止另一个普通发布占用下一槽，也不能让新进程知道上一次已开始选择哪一套目标材料。新 `select_file_history_move_joint_choice` 先重新重建并验读原恢复封套、全部目标独立 FULL 和复制身份、两端 FILE、原共同输入、原生操作及全部 scoped 数据库，随后才选择：

- 准确原 operation、actor、pool、完整 Config hash、源已发布 sequence／snapshot 和唯一紧邻下一槽；
- 原 recovery／decision-input／native／scoped 四套摘要；
- 排序去重的全体原参与盘注册身份和完整元数据成员，源盘、目标盘、元数据盘不互相替代。

系统根和每个原参与盘在既有 `file-history-move-decision-inputs/joint-choice.json` 保存完全相同的规范字节。没有可变 `accepted`／`completed` 字段，也没有供调用方设置的“已验完”布尔。当前只接原固定 genesis、最多 32 个参与盘及不超过 64 KiB 的选择；继承 scoped 时间线限制。不支持的 configuration generation 或改绑保留原字节并拒绝，不引入旧格式兼容。

第一份选择写入前，配置容量计入 FULL、共同输入、native、全部本根 scoped 库及选择文件。每根继续检查物理 reserve、真实 seal／I/O gate、writer.lock、持有根／stage／原输入清单。保存采用无覆盖原子文件、实际 bytes 验读、文件和目录同步；已有准确字节只同步、不换 inode。异常原样保留，不能覆盖异记录或把孤立材料改成新操作。

## 同一进程不能被另一个发布越过

开始写第一份选择前即保留 Engine 内存围栏。写入或末端重验失败，围栏不清除，可能已落盘的选择及全部原材料不回滚、不删除。第一次写之前的取消、源变化、缺 scoped 库或容量不足不凭空建立新的选择。

普通 Engine `require_write`、新规划上下文，以及之前已经脱锁准备但尚未决定的 `decide_publication` 都拒绝跨越该围栏。仍保留已有的未知决定／缺元数据成员／workspace preparation 围栏，不能为了恢复 move 暂时把 `blocked` 设为 false。

原 move 的读取和重建走单独的受限源观察入口，只允许相同选择 operation。公开的新 capture 仍拒绝；冷进程只能经原恢复封套、原 actor／digest／clock／slot／route 重建，随后逐根验读或补齐同一原材料。它不是普通写许可，也不能把旧捕获用于另一个 operation。只读 `read_file_history_move_joint_choice` 对缺副本直接失败；显式 select 只在全部原材料再次完整验证后补齐缺失副本。

`inspect_file_history_move_joint_choice` 只是当前内存拒写围栏的诊断身份，不证明持久副本齐全、正文可读或可以发布。取消后不自动清除已经建立的围栏。**当前没有取消／解除选择的 API，也没有完成选择后自动放行普通写的路径。不要把该准备 API 自动接入 SMB rename。** 后续必须由真正原共同决定及完整恢复协议完成收束，而非删除 sidecar 解锁。

## 冷启动先发现，再恢复旧源

Engine 持有原 writer 锁后，先读取系统根和实际已准入 I/O 根的固定选择地址。不探测未列入 I/O 能力的路径，不创建选择目录。启动记录严格解码，核对规范字节、配置、盘身份、槽及完整元数据集合；持有原文件再复核名字／inode／内容／stage／根／seal。损坏、异选择、范围外副本或绑定改变拒绝启动，并保留证据。

任何一个仍存在的合法原选择都足以**增加拒写限制**，不能据此作发布决定。系统选择丢失但原参与盘仍有选择时，仍保留围栏，不在 startup 顺手补系统文件。缺失的全部参与材料只能通过原显式恢复路径重新证明。

恢复只接纳选择之前已存在的完整 PUBLICATION 屏障。出现选择源之后的 journal 槽、prepared candidate 或 torn tail，先于恢复 writer 转换拒绝；不截断、不重绕下一槽，不把更晚发布覆盖成原源。选择存在时不修补缺失 publication、不做普通 pending completion、layout 修复、purge、尾部修复或 checkpoint 保存。原已发布源 State／snapshot／下一槽在末端必须精确匹配选择，才交回仍受围栏保护的 Engine。普通 startup 的 descriptor 同步和可丢弃 SYSTEM 索引重建不是新的 move 决定；不宣称启动零写入或零 fsync。

现存源内容与 namespace 仍按原已发布状态读取，目标仍不可见，源 D 尚不存在。损坏原 FULL、FILE 或 scoped 材料会阻止显式续作，不会因为已有选择就跳过重验。换系统根／改配置恢复、归档 generation 和外部工具的共同 authority 切换尚未包含在该协议，不从一份选择推断这些能力。

## 成本与仍缺闭包

普通写入热路径增加 O(1) 的 Option 检查，不增加文件 I/O。启动增加系统根及实际准入根的有界选择读取与身份复核，单记录最多 64 KiB；完整选择／续作仍承担上一片全部历史重建和验存成本。每个参与根至多增加一份选择文件及相应文件／目录同步；幂等重试为补足可能缺失的持久化屏障继续同步。没有吞吐或掉电测量结果。

五项原共同发布义务全部保留：统一永久收据的最终准入、SYSTEM 与盘根的共同安装、普通唯一 Candidate／N 成员 ACCEPT／PUBLICATION 及其故障恢复、双端已有句柄／目录页读取围栏仍须接通。选择副本全齐不等于 N 成员决定，SQL 自摘要不等于独立正文证明。通用 native move publication 的硬拒绝不解除，不提供 copy 后 unlink，不生成成功 SMB 响应。

新增未运行源码覆盖：完整选择后源可读与目标未发布、普通写拒绝、缺 scoped 与早取消不建立选择、冷重建保留 D／clock／inode、系统副本缺失仍围栏且只显式补齐、部分写失败、损坏／冲突保留拒绝、更晚 journal 不回绕，以及已脱锁普通提交拒绝和持久记录严格解析。这些不是运行结果，更不代表真实 SMB／磁盘故障验收。

独立后继：[唯一普通 Candidate 与逐成员冷读](file-history-move-native-candidate.md)。仅安装原普通不可变依赖，选择围栏及无 ACCEPT 的边界保持。
