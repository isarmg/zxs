# 当前磁盘登记后的计划取消准入

本页记录 `plan.cancel` 的原生调用、封闭写集和恢复边界。它不扩展 `content.purge`、`history.split` 或历史另存的执行权限，也不表示测试已运行通过。

## 真实调用和来源

`Engine::cancel_plan` 继续先读取 `Engine::plan` 的实际结果。已发布计划优先取自当前 State；尚未发布的准备文件通过已验证 system 根、同挂载安全相对路径和有界严格解析读取。计划 ID 和 input digest 必须相符，调用主体必须等于计划原主体，只有 Ready 可取消。

取消不要求预览仍适用于当前 business generation。取消陈旧计划本来就是合法动作；执行计划的 generation、范围和正文检查不因此放宽。Committed、Completed 和已有 Cancelled 计划不能被新操作重新取消或执行。

纯本地 Ready 准备留在原 `system/plans` 文件，不以缓存形式插入已发布 State。当前原生保存入口已经遵守该边界。若普通 mutate 会把多个无关 Ready 准备顺带写入候选，live 差分看似未变，冷重放却新增了无权执行的计划。已存在的已发布 Plan 记录不删除、不迁移。服务端只有按 ID 查询/提交/取消入口，没有依赖 State 枚举本地准备的计划列表；`plan()` 继续读取未发布文件。

## 取消结果与永久收据

新操作仅产生一个完整、不可执行的 Cancelled Plan。新收据增加 `cancelled_plan_hash`，使用 `plan-cancellation` 域对这个已取消记录求哈希，绑定其 ID、种类、主体、预期代数、输入及原 input digest。它只证明取消结果，不把收据里的资料当作 Ready 来源或正文权限。

原生 request digest 仍按原契约计算：`plan.cancel`、原主体和 `{plan_id}`。同操作重试严格核对 operation ID、kind、目标 ID、原主体和 request digest，然后原样返回永久结果；不重复发布，也不读正文或清理。新增结果字段不改写任何旧收据、旧哈希或不可变历史。既有收据无需补写该字段才能重试。

若上一已认证 State 已有同 ID Plan，下一 Plan 必须只是原记录的 Ready → Cancelled，其他全部字段严格等值。若上一 State 没有该 ID，允许发布原生入口实际校验过的本地计划的取消墓碑。冷恢复认证的是该永久取消结果，不声称其前一个未发布 Ready 文件已具有发布权威。Cancelled 状态不会被 `commit_plan` 或历史另存入口当作可执行来源。

## 封闭写集和原子性

`plan.cancel` 不加入通用 `require_operation` 白名单。原生 mutation 转到专用 metadata-only planner：先核对 current registry 已准入，再要求 prepared payload 为空，最后核对精确收据和封闭写集。未完成证明的候选不能离开 planning。发布阶段和冷重放中的 `CurrentNamespaces::advance` 在无 namespace 变化的快返之前再次调用同一专用证明：

- 只有一个目标 Cancelled Plan，可新增或精确替换原 Ready 记录；其他计划完全不变。当前没有其他已准入操作发布计划状态变化，因此没有新的取消收据时，任何实际 Plan 变化都拒绝；同时伪造其他 kind 与 Ready/Committed/Completed 状态也不能绕过冷重放。
- 只有一个新永久操作、对应原主体和一个匹配事件；既有取消收据、主体和墓碑不能被改写或删除。
- sequence 必须前进，business generation 恰好增加一，topology generation 不变。
- 收据外壳、取消结果、目标哈希、原生 digest、无正文保护结果均精确重建。
- State 以穷尽字段解构关闭写集：仓库、历史、分支、不可变 Commit/DirectoryTree、版本、放置、内容状态、认证、规则、选择、备注等均严格不变。新增 State 字段必须显式决定语义。
- `file_names` 和 `file_locations` 驱动物理容器路径，严格等值；冷解码丢失追踪不会授权从旧 HEAD 修补它们。

原生取消不准备 payload、不调用 `cleanup_purged`，不删除本地计划文件、不终止历史工作区、不释放已有读租约或工作区提交准备。通用发布仍承担当前元数据成员的日志、证明和同步；失败不安装新 State 或 current view，不冒充已回滚未知决定。

## 磁盘与复杂度

取消不执行启盘、不修改设备或挂载、不擦盘、不写恢复备份。Stopped 根即使被换成普通文件，元数据取消也不得尝试打开它；当前分支的 writable 和隐藏 occurrence 不变。

live 校验使用 `changes_since`，只比较实际被触及的 map key 和追加事件。没有可用追踪的冷 State 逐记录核对相同语义。未添加全 State clone、序列化或哈希；仅对选定 Plan 的 input 和取消结果做有界哈希。既有 mutate 的 State 结构共享和既有持久化阶段不变，没有减少 fsync、改变 quorum 或新增性能数值承诺。

## 测试源码

`test/engine/unit/engine/current_plan_cancellation_tests.rs` 通过独立根测试接线，覆盖：

1. 真实 Engine 和 D3 后，同时三个本地 Ready 计划，取消一个后其他计划仍可查询；两次冷恢复后按原主体继续取消并永久重放，所有本地文件原字节保留。
2. PlannedOffline 和 Deregister 后，目标根替成普通文件；取消与冷恢复不重新准入正文或写工作区，移走目录原字节不变。
3. 真实 detached workspace preparation 产生 Busy；两次拒绝仍保留准备、暂存文件、原计划及读租约；原准备随后可完成。
4. 错误主体、空主体、错误 ID、已用 operation、错误 input digest、原文件身份和非 Ready 状态不发布。
5. 42 种候选篡改在 live 快返与丢失追踪的冷路径均拒绝，含完整 Plan 字段、哈希、operation、原收据、主体、事件、历史、名称、物理路径和正文状态。
6. 已发布 Plan 的精确来源约束，以及没有新取消收据时旧主体、墓碑和收据仍不可修改；另有组合篡改用其他 kind 与三种非 Cancelled 状态验证 live/cold 都拒绝。
7. 登记前取消收据在 D3 后原样重放；历史另存计划取消后，原 Sealed 工作区和暂存字节在 live/cold 均保留，计划执行入口保持 PlanClosed。
8. 专用 planner 接受精确候选，但通用 kind 入口仍拒绝；错误摘要、取消哈希、额外字段写入、payload、缺少墓碑、错误结果状态、错误 business generation 和缺失 registry admission 都在候选交出前失败。
9. 启用 `fault-injection` 时，决定前 `after_payloads` 失败和决定后 `after_finalize` 不确定结果都保留原 State/view、本地准备及读租约；冷恢复继续原决定，不把未知结果冒充回滚。

测试内目录更名、替换文件和损坏记录仅用于临时夹具。它们不是产品设备操作，也不代表物理断电或 syscall I/O tracing 验收。

## 本次验证状态

- 已执行：官方 Rust 1.99.0 rustfmt，edition 2024，`skip_children=true`；`git apply --check --whitespace=error`；源码调用链、全部 State 字段和专用 planning、current 快返和冷重放调用链人工核对。
- 未执行：cargo build/check/test、任何测试二进制、产品、模型、CI、真实设备或部署。本轮仅交付隔离补丁，由根任务统一集成与验证。
- 基线：`db110ff9`。该版本的普通维护和历史另存准备已只保存本地 Ready 文件，本补丁不改动这两处既有边界；仅选择性复用旧候选的取消证明与回归，并接入当前主线的专用 metadata-only planning 模式。

根任务可在允许的统一验证阶段选择 `storage-engine` 的 `current_plan_cancellation` 单元测试过滤器，再验证维护、历史另存、D3、恢复和本地计划可见性的相关回归。格式和静态核对不等于这些运行项通过。
