# 单一存储新格式初始化（实施中）

状态：WIP，未经 GitHub 编译或行为验证，不可作为主线验收。

## 已落盘的整合边界

统一 SYSTEM selector 使用 `unified-storage-system-v1`，DISK seal 使用
`unified-storage-managed-directory-v1`。两个此前独立候选的 selector 和 seal 均不作为
兼容输入；不在已有根写入或补齐新字段。手动稿和目录初始化共享同一运行时准入与完成证明。
显式 `initialize_storage` 是唯一初始化实现；旧 `initialize` / `initialize_validated` /
`initialize_held` 暂以具名错误写前拒绝，不能继续生成旧可写格式。

冷恢复把完成记录的原 bootstrap 根/锁身份传到实际持有消费者，恢复锁只允许 Existing，
不在缺锁时创建替代锁。固定 Genesis 的 Config 不能自行授权新格式；仅沿原完成证明和
相同 DISK selector 重新绑定，再验证固定成员与 seal。

原手动与目录命名的显式 fresh API 暂转发同一个实现，不代表支持两种磁盘格式。
后续消费者迁往统一入口，旧 WIP 数据不会升级。

## 尚未完成，不能被本次整合提交冒充

- 第二阶段已写入 `storage.initialize` 永久决定：完整 RootRecord 是唯一首个 operation
  的结果，原操作号、固定内部主体与根承诺进入 Candidate 对应的 State 快照。安装证明持有
  该 Candidate / Publication，每个 DISK 留同一份证明；Complete 仅在全部证明和成员收据
  复验后写入。冷开还读取并验证首快照的完整 operation / principal 闭包。
- 原操作显式重试可以从所有 DISK 完整安装证明重建缺失 Complete；新增 PendingBootstrapAdmission
  也可先从原持有成员 journal 的完整首个 PREPARE / ACCEPT / FINALIZE 收据、相同 Candidate
  及完整初始 State 闭包证明同一决定，再沿原描述符完成缺少的 publication / DISK proof / Complete。
  任何成员缺 FINALIZE、损坏尾部或冲突证明仍保留并拒绝，不降低成员数或重选候选。
  准入发生在 Engine/cache 构造之前，转交原 writer 锁与 journal；不会先重建 SQLite 再猜决定。
- 首写后无完整身份记录的中断材料仍保留并拒绝；需补决定前后逐边界恢复。
  已补从真实初始化 journal 保留原 FINALIZE 前缀的原操作恢复、损坏尾部零修复拒绝回归；
  这是保留实际收据的边界重建，不是已运行的故障注入结果。所有新增回归尚未运行。
- server 使用独立 control 根登记的 storage-system 子根；初始化与普通/保留 pending 冷开入口
  消费原 SYSTEM / DISK 描述符。激活前 SYSTEM-only probe 只检查原 selector、完成决定和
  bootstrap locator，不触及 DISK；之后完整资源准入仍重新验证所有 DISK 安装证明。
  首账号完成使用完整 pristine State 对照；激活完成回执重试允许已安装认证后继，但仍要求
  原 bootstrap 登记与初始 locator 生命周期，不能再重拿自己已持有的 SYSTEM 锁。
- SYSTEM SQLite / WAL / SHM 仍有独立路径型消费者，本次不能称全流程 held。
- 旧初始化入口退休会使仍依赖旧建池方式的测试及消费者显式失败；必须逐消费者调整真实
  fresh 准入，不能重新开放兼容来消除失败。
- manual / directory 两线原有行为、资源上限和故障测试保留，新增 dirty 来源禁令仅限
  目录首节点初始化，不决定其他目录提交/软删语义。

本地没有执行编译、测试、格式化或 CI 选择器；仅静态阅读和 diff 检查。
