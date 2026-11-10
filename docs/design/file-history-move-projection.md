# 跨盘移动的共同元数据写集

本片独立基于 `main a3ad1907bb51410cdd06e556a445403ecfb1dfe2`，只依赖已经合入的 `c53da226` 源捕获，不依赖另一未合入的系统本地来源计划候选。

`Engine::project_file_history_move` 把目标 retained-history 图与源完整 frontier 的 D 构造在**同一个私有内存写集**中。它不是 journal `Candidate`、`OperationResult`、数据库安装证明或可恢复移动决定。没有执行、保存、反序列化或 SMB 成功入口；生产 State 和磁盘完全不改变。

## 共同转换

输入消费仍有效的 `FileHistoryMoveCapture`，绑定原稳定 file ID、唯一 occurrence、准确 State／Config、两端 namespace witness 和未提交工作区。只有原 capture 当前有效、actor 非空且 operation 尚未发布，才进行投影。

一次转换固定同一个 operation、actor、观察到的下一个 decision slot、时间及目标路由／binding generations：

1. 复用 retained copy 的目标图映射，保留全部准入版本、独立分支、父边、路径／标签节点、备注及旧 D/undo 审计。新目标采用本 operation 的确定性新身份，不借用源对象身份或引入实时依赖。
2. retained copy 的共享实现现在另外返回类型化审计，move 不把 JSON 结果再解析成自己的领域输入。普通 copy 的 kind、结果字段与 source-retained 行为不改变。
3. 同时构造一个源 D，其 `branch_heads` 包括归档与 alias 分支的各自当前末端；parents 是去重排序后的完整集合。D 的身份由 operation 与源 file ID 确定，时间和 sequence 与目标 arrival 一致。所有原 source branch HEAD、版本、namespace 位置和路径保留，文件不做物理删除。
4. 目标两个新 arrival commit 明确记为 move，不再声称“保留源可见”。旧历史事实不改写。新源 D 不进入目标的活动删除集，也不混入此前的导入审计前缀。
5. 对两端重新进行完整逻辑 `HistoryManifest` 图闭包检查，并保持现有已删除文件的冻结规则。**逻辑图闭包检查不等于独立正文或 SQLite 校验。**

公有投影只能读原 capture、原 operation／actor、拟用 sequence、拟议源 D、目标 file ID、完整 version／branch 映射和冻结的逻辑 binding generations。没有可变 State、payload、operation receipt 或发布权限。不能把其中拟议的 D 当成当前已经删除。

后继[联合恢复准备封套](file-history-move-recovery.md)可把原来源与完整联合逻辑摘要保存到系统根及全部配置元数据成员，并在冷重开按原时间／slot／route 重新捕获、比较及补齐准备。该入口不反序列化本类型，不把持久准备当作 journal 决定或目标完整证明；本类型仍没有发布权限。

## 精确重验而非重新规划

`recheck_file_history_move_projection` 重用 capture 的完整元数据／双端工作区围栏，并核对原 decision slot。State 未变但未发布槽位已经推进也拒绝；更换任一来源、目标、配置或冷重开后，不会用今日数据悄悄替换原投影。

内部转换只有冻结输入，没有时钟或磁盘 I/O，同一输入可重建相同写集。这个性质仅用于后续共同协议的实现基础，**不是冷恢复能力**。没有 operation／slot 预留，也没有跨进程幂等记录。相同未发布 operation 的新投影不能充当已经保存的原请求。重验成功仅是绑定 Engine 借用的元数据观察。

## 保留的安全缺口

本片故意不伪造 `file.delete`、`file.history.copy.retained` 或 `file.history.move` 成功收据。写集内的 D 没有可发布收据，因此现有生命周期发布准入仍拒绝把该草稿单独发布；没有放宽当前收据验证、热／冷 registry replay 或恢复判断。

完整移动仍须补：

- 所有目标 FULL 正文和全部副本的独立校验、完整文件／系统／盘根数据库及标签的精确安装证明。
- 源 D 与目标完整写集的一份永久 operation 收据、热 Prepared 与冷 Candidate 重建验证、N 成员 PREPARE／ACCEPT／FINALIZE／PUBLICATION 和原操作冷恢复。
- 源 D 生命周期审计与三范围 owner／change projection 的同一次原子归属，不能伪造两个 operation 或沿用只承认普通 delete 的验证器。
- 双端 SMB 目录页／已打开句柄的读取围栏，决定前失败保源，首次 ACCEPT 后 InDoubt，以及目标独立编辑与源隐藏的实际成功边界。

系统本地来源计划、共同逻辑投影、独立目标材料和最终决定是不同阶段。不能用前两个替代后两个；不能把现有 copy 与 `soft_delete_file` 两次成功拼接。本片未开放跨盘 rename，SMB 仍明确拒绝。

## 资源与验证状态

初始 source capture 的文件数据库上限仍为 65,536 行／12 MiB 编码记录，保留审计另受同样行数及 12 MiB 封套限制。转换复用 State 结构共享，并新增源 D、目标映射对象与审计；扫描和分配随 State owner 索引、源历史提交及完整文件图增长，不声称常数成本。投影比 Weak-only capture 保留更多内存：它持有包含共享旧记录的拟议 State，调用方用完须丢弃。没有正文缓冲、I/O、fsync 或性能验收。

新增八项根 `test/` 单元测试源码：完整共同图与无发布、冻结输入确定重建、标签／备注／撤销审计保存、State／工作区失效、仅 slot 推进失效、冷开不复活、已用 operation／空 actor／取消拒绝、普通发布拒绝与两端冻结分离。夹具只在被运行时使用临时目录，不安装拟议 State，也不是模拟 SMB 已成功。

按用户要求，本片没有代码审查、编译、格式化、测试或 CI。以上是实现与测试源码范围，不是通过证明；正文、独立库、真实 SMB、统一决定恢复和物理掉电均未验证。
