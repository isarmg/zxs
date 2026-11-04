# 最后元数据成员退出后的管理权威

本协议处理当前固定元数据组仅剩一个成员时的计划下线和取消登记。它不是 N=0/Q=0 的活动存储池，也不把系统目录加入元数据投票组。历史 `ValidatedConfig` 始终保留至少一个固定成员；此前每代的实际 N/Q、候选哈希、永久操作收据和发布链仍按原规则认证。

## 状态与入口

- `Engine::begin_disk_exit` 在最后一个成员退出时写入明确的 `ExitAuthority::TerminalManagement`。该意图的 target 保留旧配置，不伪造无成员配置。没有已认证发布边界时在写 fence 前拒绝。
- 旧固定组同步 exact fence，普通写立即封锁。该意图不能进入普通 pending-member/joint 路径。
- `Engine::prepare_terminal_disk_exit` 构建原有受保护系统目录中的完整 control archive，再准备精确 registry 后继和取消登记 occurrence mask。所有控制历史、旧成员自身日志、封印、历史发布和前代权威均独立复验。
- `TerminalDiskExit::complete` 使用同一 handle 重试。它依次执行系统权威持久化、exact bootstrap stop、真实 I/O 域关闭、完成记录持久化，最后返回独立 `TerminalManagement`。
- 服务层在成功后必须丢弃旧 Engine 并切换到管理后端。失败时保留同一 Engine/handle 和已关闭的业务准入，不重新执行原盘读取或回到普通 Ready。

`TerminalManagement` 不提供普通写、候选接纳或内容读取能力。它提供登记、身份、历史逻辑用量、认证状态及永久操作结果的只读查询。`RetainedRegistrations` 与 `EmptyRegistry` 明确区分：下线保留 Disabled 登记；取消登记移除该当前登记，但保留完整历史拓扑。其他 data-only 登记不会被当作空池或自动提升为 voter。

## 系统记录与关闭顺序

受保护系统目录保存：

- `membership-control-archives/<operation_id>/`：完整旧控制历史，沿用已有 archive 格式和校验器。
- `terminal-management/<operation_id>/authority.json`：精确意图、archive root、旧 PortableAuthority、registry 前驱/后继与 occurrence 图。
- `terminal-management/head.json`：由系统 writer 锁保护，选择当前操作和 exact authority hash。
- `terminal-management/<operation_id>/complete.json`：只有 exact stop 同步成功且真实 gate 达到 Closed 后才发布。

这些都是继续认证系统状态和历史所需的控制记录，不是用户 payload 备份。退出后不在目标盘创建恢复目录，不删除或重写原文件、旧历史对象或原永久收据。

现有 bootstrap locator 另有明确 `terminal-bootstrap-locator` 格式。它只收紧发现范围：普通发现直接拒绝，不打开任何存储根。普通 `append_stop` 仍拒绝停掉最后一个 retained root；只有接受不可反序列化的完整认证材料的 `append_terminal_stop` 能写入终态 stop。提示本身不授予管理权威。

系统完成记录与运行时关闭是两个证据。`ClosedDiskIo` 证明实际 descriptor、普通/control lease 和 writer-lock 尾部已经关闭；`RecoveredDiskIo` 还检查进程内此前逻辑域没有存活 I/O。只从资源表删掉一项，或者仅写 stop 文件，均不能报告完成。

## 重启与重试

`TerminalManagement::requires_management` 只读取系统 head 的存在性，用于在任何存储探测前选择入口；结果本身不是认证。`TerminalManagement::open` 持有真实系统 writer 锁，验证 head 指向的唯一 authority、完整 archive、精确 registry 和 mask，再重新同步 stop 和完成记录。整个路径不需要原盘、data-only 盘或配置缓存中的路径可访问。

在 head 之前崩溃时尚未写 stop，旧 fence 可以按原恢复路径重建。head 之后即使 stop 或 complete 未完成，也只恢复同一操作。删除/损坏 head 不能扩大探测范围：已经存在的终态 bootstrap ledger 仍独立关闭全部普通发现。缺失、损坏或冲突记录不会触发新 Genesis、清理旧目录或重置池。

完成结果由认证的旧 State 与终态意图确定性生成，序号为 fence+1、topology generation 加一，并追加本操作及 principal；旧收据不被改写。完成记录使这条管理操作成为持久结果，它不是零 voter 的普通业务 Publication。

本次控制操作结果为 Published，但 `current_health` 使用现有 Unknown：管理态没有进行内容探测，不能声明内容健康。管理状态独立明确 `active_metadata_members=0`、`writable=false`；任何 HTTP/UI 准入都不能从历史收据的健康字段推导可写性。

## 第一组元数据成员接续

重新启用或新增首组成员必须由独立 TerminalResume 协议消费已完成的 `TerminalManagement`。输入须包含确切 terminal authority hash、完整控制历史、原 registry/mask 与含终态永久收据的管理 State。新组 N 必须至少为一，并按新组自己的固定 Q、成员身份、真实同步回执和规定首发布接续；不能走普通初始化或伪造旧组空 quorum。

普通管理 open 遇到 `resume.json` 或 `resume-complete.json` 会关闭并要求继续原接续操作。接续消费者有专用 `open_for_resume`，只接受已经完成且完整认证的源管理态，并连续持有原系统 writer ownership。该专用入口不是普通写许可。

本核心补丁只为后续接续和服务后端提供真实认证接口，不声称单独完成 HTTP/UI 或 TerminalResume 的所有消费者。接续上线前必须验证 Disabled 的显式再启用、Detached 身份不能复活、当前 occurrence 删除不会从历史 HEAD 重新出现，以及新组发布后的冷启动选择。

## 资源与验证边界

每个 authority/head/graph 对象受现有 MAX_METADATA 单对象预算约束；控制 archive 保持现有分页和逐原成员日志验证。退出与冷恢复需要按控制历史规模扫描，正常业务热路径仅增加一次系统 head 选择，不增加历史扫描。关闭不等待任意 sleep：仍有实际 I/O 时返回具体 DiskUnavailable，调用方保留同一操作重试。

新增回归源码覆盖单盘下线/删除、其他 data-only 登记、历史/永久收据保留、无目标写入、原根消失后重启、真实 lease 尾部、坏 archive、未知 resume，以及 stop/close/publication 三个故障窗口。此批只进行官方 rustfmt 与 git diff 静态检查，未运行编译、测试、clippy、模型或 CI；物理掉电和真实设备验收亦未执行。
