# 终态剩余登记先保留原生决定，再建立后继发布

状态：分阶段实现。逐笔管理后继及已认证旧 Stop 消费已接线，仍非完整终态管理或发行验收。

## 背景

最后元数据成员退出后的 TerminalManagement 只携带该次完整终态权威。其剩余登记不是可写成员池，也不能仅从内存列表删除。原生后继必须绑定既有终态/registry、准确目标、主体与 operation，并且在原决定未完成期间不能通过 metadata resume 重新接纳被选择的成员。

## 选择

在原 SYSTEM writer 下保存不可变 `retirement-intent.json`，完整绑定来源权威/registry/sequence/topology generation 与目标绑定；只有该意图同步后才写不可变 `retirement-selected.json`。两个文件使用现有受限目录和无覆盖原子写/同步语义。严格冷读核对全部来源和选择，不由缺文件、损坏或选定标记推断成功。

`IntentStored` 与 `DecisionStored` 都是未完成状态，不创建 OperationResult，不改变当前登记或业务 State，不授予目标 Stop/D3、registry successor 或可写权限。新操作与 metadata resume 在未完成意图存在时拒绝；原 actor/operation/digest 可以重建相同写入屏障。普通原历史回执仍可查询。

只有具备准确 successor、Stop、真实 I/O 排除、最终结果及冷恢复的完整执行路径才向 helper 暴露确认按钮。[首笔后继](../design/terminal-retirement-successor.md)现实现这一路径，后继的[逐笔链](../design/terminal-retirement-chain.md)已增加下一笔和已认证旧 Stop 消费，仍拒绝元数据恢复；不能把一次首笔完成扩成全功能。诊断端公开未完成原因和准确原 ID；不能把库内 staging 能力列为辅助工具已完成注销。

## 代价与替代方案

仍没有取消能力；调用 staging 只允许同原决定完成，不能另选请求。即使首笔完成，也持续阻断 metadata resume，直到后继注册/命名空间消费者完成。因此直接 staging 只供明确的原生集成者使用，不能作为普通设置隐式调用，也不能在用户只请求检查时创建。拒绝了直接改列表、保存 helper 意图后返回完成、借旧终态 authority 伪造后继，以及无条件清文件/换编号重试。

最终移除仍须保存历史定义与正文，并正确处理剩余 registry、旧引用、Stop、冷恢复和 metadata resume 的防复活闭包。这个选择不授权物理擦除，也不代替外来备份盘一次完整低级链接纳所需的最终 writer。
