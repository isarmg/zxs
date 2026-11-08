# 完整终态链的原目标决定准备

[整链提案](terminal-chain-readmission-proposal.md) · [完整检查点](terminal-retirement-portable-checkpoint.md) · [任务清单](../../TASKS.md)

基于真实父 `c89416b3`。本片将整链原生提案接到真实原目录锁、完整控制归档安装、独立目标决定与原请求冷态续作。**这是有持久影响的准备阶段，不是完整 readmission，不提供活动 writer。** 外来完整低级链 H03 不因此完成。

未审查、编译、测试、浏览器验证或执行/查询 CI；新增测试只是未运行源码。没有操作真实系统、服务或磁盘。

## 精确来源先于目标写入

`TerminalManagement::synchronize_chain_readmission_target` 要求原完整终态链已经完成，拟接纳对象仍为原 Disabled metadata 登记，提案 principal/operation/digest 与准确当前来源一致。Detached、外来根、非原 metadata、缺必要 Enabled 低级祖先或已有未完成移除仍拒绝。

调用方显式持有所选目录 fd。原生入口要求所有该身份的旧进程 I/O lifetime 已闭合，按真实 bootstrap Stop 核对原 pathname/物理目录，再取得目录锁和原 `.writer.lock`，核原 seal、pool/slot/绑定代数、设备/inode 与原停止格式。它不重新初始化目录、不改写 `disk.json` 或旧 Stop，也不将原目录伪装成 fresh root。

在写任何新目标材料之前，完整且有界的 `chain-readmission.json` 在原 terminal operation 目录中不可覆盖持久化。该选择含原 TerminalAuthority、完整 checkpoint/registry/namespace 提案、原物理身份与 seal。它保留准确原请求，后续不能换 actor、operation、提案、盘或目录。此时普通服务仍无法启动，新终态移除被拒绝。

目标或存储错误可能发生在来源已经保留之后。返回错误不代表“没有发生写入”，不做伪回滚，也不删除原选择。当前没有取消、更换目标或自动解除来源保留接口，因此 helper 将此阶段放在独立风险说明和确认后；仅需存材料者不应执行它。

## 真实目标决定

原 source 控制归档安装到所选目标的准确 `metadata/membership-pending/<selection-hash>/control-archive`。目标副本通过共享原生归档验证器重放全部原固定成员历史、发布、永久回执、registry 和命名空间；再用完整终态链检查点核准确 successor，不能仅对文件存在或源摘要做比较。

随后原 `DecisionStore` 在真实 `metadata/membership-decisions/<selection-hash>` 创建不可覆盖的物理成员绑定与独立 `TerminalChainReadmission` choice。该 choice 与 `TerminalResume`、Joint、Abort、Backup 都不同，旧的 original-terminal-only ResumeCore 消费者不能把它误当自己的票。

提案、完整来源归档、实际 vote 和目标 `chain-certificate.json` 均在该目录读回；原 `.writer.lock`/物理/seal 和原 SYSTEM 选择在关键点再核，DecisionStore 保留原有同步与冲突裁决。只有这一步成功，SYSTEM 才保存 `chain-readmission-target.json` 证书。预先编码的预算 receipt 从不作为真实 vote 使用。

两种明确阶段：

- `source-selected`：原请求已保留，SYSTEM 尚无完整目标证书。目标可能已有部分材料甚至已写真实 vote，不能依此假设目标未变。
- `target-decision-preserved`：SYSTEM 保存通过独立目标读回的原决定。仅显式同步成功那次返回 `target_reverified_this_call:true`；后来的 source-only 状态读取返回 false，不声明目标现在仍可用或已完成 fsync 再验证。

`active_writer_admitted`、`metadata_resumption_supported`、helper `completed_readmission`、`service_started` 仍全部 false。持有目标 `.writer.lock` 是一次准备调用的互斥所有权，不是活动业务 writer，也没有新 admission epoch、可 serving 的 Current 或真实首笔 Publication。

## 恢复与源围栏

冷打开原 SYSTEM 后，先恢复原已完成终态链，再完整验证保留选择与当前 checkpoint 精确相等。已有完成 frame 可以重复既有同步屏障；不能借此选下一笔移除。未知文件、证书无原请求、被替换的来源、目标 choice/物理身份不符均保留证据并拒绝。

`chain_readmission_status` 只读取原 SYSTEM 选择与证书，不打开目标。因此目标失联时仍可查看原操作和保留阶段，但不会伪称目标在线。显式同步才重新打开原目录、核物理身份、安装/核对相同字节并重验原 vote，绝不另建 operation 或签发替代身份。

原 `require_no_terminal_retirement`、`require_no_evidence` 和普通 `TerminalRegistry::require_resumable` 执行门禁保持。新目标证书没有 PortableAuthority/Generation/Engine 转换，不能用来启动旧 ResumeCore 路径。仍缺最终整链 ResumeCore、首笔 Publication、Current/locator、真正 writer 和新 epoch 的冷恢复；正式 readmission 继续拒绝。

## 辅助工具独立 Web

保留原提案下载入口，增加：

- `POST /api/disks/terminal-chain-target-status`：读取原保留状态，不探目标。
- `POST /api/disks/terminal-chain-target-synchronize`：明确确认后保留或继续准确原请求，并准备实际目标决定。

下载结果额外提供小型 host/context、主凭据 revision 与 native preparation 计划；同步前 root 编排和 service UID worker 核这些身份。磁盘路径仅从当前 native 选择取得，不接受用户传入 root/force/外来链。主认证仍是本机既有权威，浏览器不能指定 principal；当前凭据身份必须与原 operation actor 一致。

控制台保持手工 root 二进制、0.0.0.0、无登录、不自启，Origin/CSRF、IP Host、单工作槽、关闭排空和网络风险提示不变。UI 必须同时勾选确认并确认弹窗，明确来源会保留、没有取消换盘、不是服务恢复。断连或错误显示准确原 operation/digest，先查询，不清理原证据。改变账户、选择或写者确认会撤销屏幕上的旧同步计划。

此阶段不修改 systemd/SMB/密码、不运行服务，不执行破坏性清盘。工具源码实现的目标写入仅在未来操作者明确执行接口时发生；本轮开发没有运行这些接口。

## 预算与待完成

提案、选择和证书各用现有 64 MiB native 对象预算，首次源写前预检；helper 同步/查询 IPC 请求和响应仍限 64 KiB，不传输完整大提案。来源重放和目标独立归档读取成本随完整历史与终态链增长，当前没有新增后台轮询或无界重试。完整源验证在选择/读回时重复以保持持久证据有效，尚未测量最坏链内存、延迟和 fsync 次数。

本片新增 4 项真实根/独立选择/冷态/目标失败测试源码与 2 项 HTTP 边界源码，均未运行。真实断电、磁盘替换、Samba/多进程竞争、浏览器重复点击、发布包与最终激活仍无通过证据。
