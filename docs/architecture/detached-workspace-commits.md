<a id="detached-workspace-commit-preparation"></a>

# 脱离锁的工作区提交准备

HTTP 与 SMB 应用边界通过拥有独立所有权的准备 API 处理普通工作区内容提交。
同步核心 API 供嵌入式调用方和恢复流程使用。

<a id="boundaries"></a>

## 阶段边界

1. 持有活动 Engine 互斥锁时，对请求授权，捕获其精确的工作区、配置、状态和发布基线，
   并获取该 Engine 的一个准备准入名额。此步骤不读取载荷。
2. 释放互斥锁。私有且不能发布的规划快照验证请求并构建尚未发布的变更。
   源内容哈希、完整载荷复制、验证、元数据预检和按内容寻址的元数据写入在此执行。
   相互独立的文件载荷准备最多使用四个工作线程；同一文件的各版本及其保护路径
   仍按顺序处理。此阶段不写入 PREPARE、ACCEPT、FINALIZE 或 PUBLICATION 记录，
   也不将活动工作区标记为已提交。
3. 重新获取活动互斥锁。在应用边界刷新受保护账户状态，重新检查当前仓库授权、
   工作区所有者与隔离栅栏、租约有效期、精确工作区内容、配置、发布快照、序列号、
   决策槽，以及业务/拓扑代次。随后发布单个串行化决策并更新活动工作区。

准备过程占有准入名额期间，活动 Engine 会保守阻止其他普通写入、受保护管理员凭据快照发布、
清除清理和降级恢复发布。这些操作返回可重试的 COMMIT_PREPARATION_BUSY，
读取仍然可用。该机制防止载荷安装期间发生物理容器迁移。
工作区中止仍可使已准备工作区失效，但不会移除其暂存输入。
最终主体、业务状态与精确基线检查仍然有效：任何过期基线都会在 ACCEPT 之前产生明确、可重试的
冲突，而不是结果不确定的操作。重试应使用相同操作 ID 和请求摘要。
范围与限制见[写入准入细节](detached-write-admission.md)。

<a id="verification-and-lifetime"></a>

## 验证与生命周期

脱离锁的发布准备会完整验证每个副本，并保留不透明、不可序列化的证据，绑定持有的
文件描述符、元数据、磁盘身份和候选。加锁的决策路径重新检查这一精确证据，
而不再重读载荷。为载荷保留的文件描述符总数上限为 128 个（每副本四个）。
更大的候选只保留磁盘身份句柄，并在准备期间、首条决策记录写入之前，以及每份载荷收据
签发之前，顺序完整复验副本。这种有界描述符回退支持大批次，但会在最终临界区内完整
读取载荷。它不会用仅含时间戳的证据替代持有 inode 的证明。
发布仍要求完整副本通过验证；恢复不持有运行期证明，因此会执行完整验证。

准备对象持有池现有进程锁的重复句柄。即使原始 Engine 所有者在工作期间被释放，
这些锁仍然有效。完整文件上传准备也遵守相同的池/磁盘锁存活要求；完成上传前同时核对刚写入文件的持有句柄和当前目录项身份、长度与元数据，不能仅相信准备阶段保存的摘要。请求取消后，准备准入名额仍跟随正在运行的阻塞任务，
在任务完成或持有对象释放时归还。已取消或过期的准备可能留下未被引用的不可变载荷
或按内容寻址的元数据，但不能发布过期工作区，也不能移除原始暂存输入。

<a id="cost-and-limits"></a>

## 成本与限制

每个 Engine 最多有一个脱离锁的工作区准备，并非由无关池共享进程级全局限制。
活动 Engine 与冻结规划快照通过私有 `Arc<State>` 共享同一个不可变状态，锁内捕获不深复制完整 State。
规划下一状态时仍在锁外执行一次完整深复制，并只修改该独立值；正常脱锁保存的 State 值峰值为
共享的旧状态与独立的下一状态两份。工作线程共享规划快照，不会为每个文件复制 State。
维护与历史计划的原地写入显式使用 `Arc::make_mut` 保留旧共享值；准备准入与最终基线检查不因此放宽。
公共 State 数据模型、序列化和持久证明不含 Arc，也没有引入逐表写时复制容器。

规划快照只复制本次目标工作区的完整清单，不会复制全部保留工作区；完整容量计数仍随快照保留，最终提交与持久化继续在活动 Engine 上执行。其他工作区的记录、恢复材料和累计上限均不改变；目标清单优化只减少工作区清单的复制成本；独立下一状态与容量索引仍有复制成本。

其他变更类型、包导入/导出、历史保存操作和恢复仍走现有同步路径。
本文不声称全部 Engine 工作都无锁。确定性测试检查阶段边界、验证、过期状态、授权、
故障点和工作线程数量上限；不能从这些结构测试或 tmpfs 计时推导吞吐量结论。

<a id="public-commit-envelope-and-seal"></a>

## 公开提交 envelope 与 seal 类型边界

HTTP `seal` 的核心返回 `SealedWorkspace`（OperationId、u64 fence/topology、类型化 placement），只在响应序列化时产生原字符串整数格式；创建工作区 seal 状态、持久化、placement 路由和 manifest 的先后顺序不变。历史保存调用仍消费同一核心操作，不走隐藏的旧 Value API。

HTTP commit 有界 strict JSON 与 path/body 身份绑定后，阻塞任务持有 backend_slots，在 HTTP 边界构造私有字段的 `PublicCommitEnvelope`。公开 DTO、operation ID、fence 只解析一次，canonical request digest 只计算一次；保留不可变原 Value 作为落盘证据而非从领域字段重建另一份请求。该 portable contract 不依赖 engine，server 的 no-core 构建继续可用。`expected_topology_generation` 等原字符串语义保留，例如 `0007` 不是被宽松归一化为 7，而仍交给原 topology precondition 拒绝。请求预算仍覆盖解码／摘要 CPU 与后续阻塞工作；JSON 对象／路径错误在预算前，envelope 语法错误在预算后、当前 Backend 准入前。因此格式错误不再占引擎锁，语义冲突顺序不变。

Backend 显式接收 envelope 并返回领域 OperationResult，HTTP 最后调用原 operation wire 投影。Application 不再 DTO → Value，Engine 不再对自己生成的 Value 重解析。prepare 保存 `Option<PublicCommitEnvelope>`；SMB 内部提交仍为 `None`，两种摘要契约没有合并。两个旧 JSON actions 已删除。

公开规划先按当前规划 State 检查已登记 operation 的摘要／actor 冲突，再核对 workspace/repository/branch/head/fence/message 及 envelope 路由绑定；replay 跳过仅适用于新保存的 live topology/manifest 条件，不跳过不可变绑定。新保存随后按原顺序检查 strict、topology、manifest、claimed digest、placement 与 sealed 状态。`ReceiptCheckedEnvelope` 是规划内一次性私有证明，不进入 Prepared，不构成跨 State 的授权／未登记缓存。finish 回到活动锁后仍检查当前 receipt 的 actor、请求绑定与 expected digest；当前账号和工作区所有权仍由应用收尾重新检查。

实际普通登记在准备持有期间受全局写 fence 拒绝。回归先证明同／异摘要的登记均返回 COMMIT_PREPARATION_BUSY，再在明确标注的单元 fixture 中释放准备占用、真实登记 receipt，测试 finish 对相同摘要重放、不同摘要拒绝；不同 actor 记录用测试内状态交错构造，不声称它能绕过生产所有权。现有 public/SMB 回锁撤权、同步／脱锁 replay、恢复与永久收据测试继续执行。固定 canonical body 字节与摘要向量包含 Unicode、u64::MAX 和保留的 topology 拼写；实际系统／数据盘 wire-request.json 与原边界 Value 的 canonical 字节逐字节相等。
