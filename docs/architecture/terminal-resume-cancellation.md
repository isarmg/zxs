# 终态接续的未发布取消

本文定义源码协议与验收目标，不表示已运行测试。源测试位于 `test/engine/unit/engine/terminal_resume_cancel_tests.rs`，由整合方统一构建和执行。

## 权限与不可逆边界

本实现只取消使用新目录的 fresh resume，必须提供原 actor、operation ID 和 TransitionIntent 摘要；不引入终态 retained-directory readmission。取消不改变原终态正 N/Q、fence、archive、registry 或 payload，也不触碰旧 stopped 根。

令原请求的新固定 voter 数为 N，Q = floor(N/2) + 1。每个真实新 voter 的 `DecisionStore` 同一不可变槽只能选择 `TerminalResume` 或 `TerminalResumeCancel`，绑定同一原请求及自身 physical identity。取消需要至少 N − Q + 1 个真实、同步并读回的 Cancel 选择，因而与任何可能的 Resume Q 相交。旧 terminal 权威不能代替这些新成员选择；data-only 目录不投票。

取消先在全部持有的新目录上核验实际槽，不使用 HTTP phase 或内存 votes 判断。若已观察到 Q 个 Resume，返回 `ContinueOriginal` 及真实 `ResumeQuorumAccepted`；若完整 generation selector 已存在，返回 `GenerationSelected`。两者均不写取消，不自动继续 activation。任何可见但同步结果不确定的 Resume 决定也计入不可反悔集合。

足够空槽存在时，先写原 proposal 绑定的 `resume-cancel-intent.json`，再依次使用既有 DecisionStore 原子 no-replace、文件同步、父目录同步及命名读回选择 Cancel。部分失败保持原 owner 和原目录材料；后续 Resume 被取消 intent 或真实 Cancel 槽阻断，必须继续原取消。完整 blocking certificate 落入 SYSTEM `resume-cancel.json` 后才能报告 Cancelled。证明编码预算在任何新投票前预检，不能先消耗票再发现累计历史超限。

## 不可变 attempt 链

首个 `resume.json` 的字节、字段缺省语义及哈希保持不变，绝不删除或覆盖。完成取消证书的哈希精确命名下一槽：

`terminal-management/<terminal-op>/resume-attempts/<previous-cancel-certificate-hash>/resume.json`

后继 proposal 的 `previous_cancellation_hash` 必须与前一证书一致。加载从首 proposal 顺序跟随已认证取消哈希，不按目录名、时间或最大序号猜选。目录枚举只拒绝不属于该精确链的材料；未知记录、非私有目录、缺前置证据、冲突字节和未知 staging 都保留并关闭。

每次取消保留此前 proposal、cancel intent、真实本地选择、pending seal、ready 记录、reservation、archive 和任何失败证据。新请求须使用可准入的新目录，不能为了重用旧 pending 目录删除或改写它。每个 attempt 的 generation/bootstrap/completion 文件位于该 attempt 的精确目录，不能落回首个单槽。

`ResumeCore` 携带按顺序排列的取消证据。每条证据只含当次 proposal 及其本地选择；proposal 仅回指前一取消哈希，不包含自己或后继，因此无哈希环、无递归对象膨胀。首个没有取消历史的 proposal/core 省略新增可选字段，原哈希不变。

## 永久结果与状态来源

永久 kind 为 `terminal-metadata-resume-cancelled`，原 op、actor、request digest 不变，`result.state` 为 `cancelled`，`authority_kind` 为 `terminal-resume-cancellation`。结果绑定 terminal authority、proposal、取消 core 和固定新 N/Q；`writable=false`，health 为 Unknown。

它是认证管理结果，不伪造普通 journal publication。`sequence` 表示已完成 terminal 管理锚，与 terminal 主结果相同，不消耗新活动序号、不增加业务或拓扑代数。State 的永久结果及 actor 表按 OperationId 定位；当前 candidate/publication 的序号校验只约束该 candidate 自己的 op。新 Resume 的 `management_state` 从原完整冻结 State 构造 terminal 结果并加入全部取消结果，随后只有成功 Resume 使用下一活动序号。第一笔 activation 的完整状态复算验证这份投影，后续 Add/Exit 继续保留全部收据。

`TerminalManagement::metadata_resume_cancelled` 只读 SYSTEM 完整认证来源和 exact attempt 链，核验原 actor/op/digest，可在现有 owner 持有 SYSTEM 锁时查询，不取第二把锁、不访问新旧目标目录、不取消也不补写。已取消且没有后继 attempt 时，`metadata_resume_recovery` 返回 None。普通 `TerminalManagement::open` 重复真实历史文件与目录同步后恢复管理 owner，其 `operation` 查询保留所有取消结果。已包含于认证冻结 State 的取消结果按同一永久操作表查询。

## 资源与上层接线

`cancel(&mut self, ...)` 在错误时保留同一 pending 对象。新 directory FD、writer lock、源 archive 归属不转移；同步 I/O 调用的局部句柄全部在返回前结束，没有脱管子任务。成功取消后仍持有全部新 locks，直到显式 `into_cancelled_management` 重新同步整条历史的文件及父目录后，才丢弃全部真实 pending resources 并交出原认证 management owner。任何重同步失败都返回原 I/O 来源而不是成功 owner，需显式恢复。它不清文件、不重置池、不重开旧 stopped 目录。

HTTP 应先保存原请求 proof，再调用核心；收到 ContinueOriginal 只显示准确阶段及继续原操作，不能冒充取消或隐式 activation。保存回执失败时，纯 SYSTEM 查询仅提供认证观察，不能据此完成 HTTP；必须取得已重复耐久性屏障的实际管理 owner，或已准入后继 Engine，才恢复永久结果。UI 只在准确 live owner 的 `Cancellable`/`Cancelling` 观察时开放取消。可读 Cancelled 但尚未交接时只允许显式继续取消，不显示完成。冷启动只有 SYSTEM selector、尚未重供 FD，或后台正在改写决定时显示 Unknown，不凭旧 HTTP phase 声称可取消。已有新 Q 或 generation selector 显示只能继续原恢复。

## 成本与未执行验证

每次实际取消读取全部固定新 voter 槽并持有全部新目录；写入每个仍可选择的槽，完成证明包含至少 N − Q + 1 票。各票及 SYSTEM intent/completion 沿用真实文件和目录同步，无旧 target I/O。链验证按历史取消数和各 proposal/证据长度线性读取；每个新 core 携带完整取消历史并受现有 MAX_METADATA 限制，不裁剪永久结果。

源测试覆盖 N=1..6、少数 Resume 后 blocking 取消、实际新 Q 和 generation selector 不可取消、部分 Cancel 冷重入、actor/op/digest 错配、纯查询无写、真实锁释放、data-only 材料保留、初始字节不变、多次取消后新 Resume/冷启动、未知节点保留，冷 generation selector 的取消不得隐式 activation，以及真实文件/目录 fsync 尾部的锁持有、失败和显式恢复。真实 syscall 尾部测试依赖既有 `test/engine/unit/repair/io_tails.c` interposer，默认忽略；整合方需使用 `LD_PRELOAD=repair_io_tails.so` 配合 `--ignored --test-threads=1` 单独执行。

本作者仅执行指定 rustfmt 和 diff 检查；未运行编译、测试、程序、模型或 CI，未推送远端，也不宣称物理掉电或真实部署验收完成。
