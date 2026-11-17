# Worktree 自动名称与原始意图

本片在既有 `/api/directory-worktrees` 创建入口支持自动名称，不新增第二条创建协议。浏览器请求 `AllocateWorktree`，明确 `naming: "automatic"`，只携带准确原快照、父目录、根代数和原操作编号，不填写名称或重造分支名。

同一个 Engine 写锁内，先比对原操作、主体和完整原自动请求；已完成只返回原实例，未完成只报告 Pending。只有完全新操作，才从已准入原快照取得真实 BranchName，从当前已登记根取得实际组件预算（减 16 字节）与普通项、Unborn、同盘 named repository 占名，再合入该根同父目录所有 live / 未完成 worktree 原始保留名。既有 names allocator 按规范化 lookup key 选择 001 至 999 的首个空位，不覆盖、不回绕。旧名称只参与占用，不重新按新预算准入。

`WorktreeCreationRequest` 为严格且形状互斥的 Named / Automatic typed DTO。旧 Named JSON 与旧 `ReserveWorktree` / `CompleteWorktree` 永久字节、摘要不变，不补字段、不迁移。Automatic 新增 `snapshot.worktree.allocate` / `ReserveAllocatedWorktree` 原收据，保存两个独立事实：原 `AllocateWorktree` 请求，以及包含实际分配 Name 和实例/完成身份的 `WorktreeReservation` resolved plan。请求摘要仅覆盖原自动意图；完整 reservation commitment 同时覆盖意图及 resolved plan。完成仍用原独立 completion，不能用相同 operation 的手动名请求冒认自动请求或反向冒认。

原 operation 的只读 creation receipt 返回原意图，不把 resolved name 伪装成用户请求。实际名字来自精确实例 status。读回只验证源分支的正确拼写范围与同父目录占名一致性，不用较晚的普通 namespace 重算当时最低可用号。

范围保持现有初始 root 布局、物理 inventory、授权、写入和独立正文证明；不放松 Unborn 等尚未支持物理布局的拒绝门。软删除、恢复重命名和一般子目录尚未由本片实现。测试与实际环境验证由协调根针对最终合成源码执行，不能用静态审查替代。
