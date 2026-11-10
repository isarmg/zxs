# 0016：目标盘局部的外来历史执行意图

状态：首个有限实现；只保存意图，历史复制与 namespace 发布尚未接入。

## 背景

已有系统盘复核计划保存完整源观察和目标映射，但不是数据面授权。来源可能是包含多个原一级身份的备份盘；已缺失正文不能被解释为可剪裁历史。耗时扫描、目标变更、撤权、关闭和结果未知都必须在真正写入前保持明确边界。

## 决策

- 将保存执行意图设为独立、明确确认的操作。当前格式仅处理一个实物来源、全部来源历史、一个已登记一级空目标。系统复核 ID/摘要、当前主体、完整来源 checkpoint 和原始文件清单承诺、实际物理身份、目标 namespace 基线、全组映射及缺正文保留策略一起进入不可变摘要。
- 在来源扫描前取得已准入 backend 并持有到写入；不在长扫描结束后用健康样本 TTL 重新准入。实际保存锁内重新检查当前会话、service stop、Engine 写围栏和目标基线。准备对象不可序列化；磁盘记录不能构造成写能力。
- 意图通过准确目标 DiskRoot 和已有 writer fence 写入目标数据盘。其永久名称空间为 `(完整目标 registration identity, intent operation_id)`，不预留全池业务 ID，不修改 State.operations，也不锁定目标 namespace。另一目标可以有同 ID 的独立意图；以后冲突的业务 ID 阻止继续同步，不能当作导入回执。
- 新记录使用独占操作目录和不可覆盖原子文件。既有相同内容持有同一已核验 FD 同步，并核对命名对象；不同内容、坏记录、已有无完整 intent 的目录或不确定 I/O 保留并拒绝。状态只读目标事实，原样重试不打开旧源路径。
- 每条记录、每盘全部保留操作数量/字节和真实 metadata reserve 在首次写前检查。部分失败残留继续计费，不自动清理、不按 TTL 过期。目标是有界持久控制面，不是正文复制额度。
- 只有 `intent_saved` 观察态和 `execution=unavailable`。不复用离线 CapsuleReceipt、实验 DiskPublicationReceipt、管理 JSON completed 或 SQLite 缓存作为 namespace 决定。现有 execute 入口仍拒绝。

## 代价与替代方案

系统盘单独保存计划不能证明目标盘已保留什么；因此它只参与审阅绑定，不充当目标权威。反过来，把局部意图解释为全池 reservation 需要改变所有普通写和成员转换的准入，本切片不作这一承诺。全池操作归属和多历史原子导入必须作为后续真实事务完成。

源捕获是有预算的完整检查及文件清单扫描，不是 O(page) 工作。目标保存遍历有界保留目录；新增文件与目录、同名重放各保留必要的文件/目录屏障。两个 held root 的包含检查最多遍历各 256 层祖先；同文件系统的不同 mount view 暂拒绝，避免子目录 bind mount 隐藏祖先。瞬时 MNT_ID 不写入持久身份。这些限制不表示真实设备隔离或物理断电已获验证。

当前 managed-directory 身份与新的独立逐盘 authority 格式保持显式边界，不补默认 incarnation、不双读升级、不改源盘。后者的单盘 publication 也不能自动证明一次多来源 namespace 导入完成。

## 关联规则

- [执行意图范围与接口](../design/foreign-import-writer-first-slice.md)
- [API 与错误](../15-api-and-errors.md)
- [事务与恢复](../10-transactions-and-recovery.md)
- [同代磁盘资源](0007-owned-disk-resources.md)
