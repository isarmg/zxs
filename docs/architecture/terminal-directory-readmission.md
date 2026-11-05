# 终态重新启用保留的数据目录

## 边界和目标

最后一个 metadata 登记计划下线后，`TerminalManagement` 只消费 SYSTEM 中完整认证的旧固定成员证据。目录仍登记为 `Disabled`，数据、历史身份与 `binding_generation` 不变。没有活动 `Engine`，也没有 `N=0/Q=0` 写入权威。

管理员可明确重新准入一个历史角色为 metadata、当前仍为 `Disabled` 的原目录。这个操作与新增空目录组是不同的 `TerminalMetadataResumeAdmission` 变体。已 `Detached` 的登记不能恢复；仅 data-only 的登记不能独自建立非零 metadata quorum。本协议不擦除目录、不重新格式化、不复制载荷作为备份，也不改变其它登记或共享历史。

## 两种证明不能混用

- `FreshDirectories` 保持原有新增目录语义和未扩展记录的字节、哈希。原目录不进入 `added_roots`。
- `RetainedMetadata` 绑定原登记身份、原目录物理身份、原 `disk.json` seal、terminal registry commitment、准确的已停止 operation/transition 和先前 admission epoch。
- 两者共享完整旧 terminal/archive 验证、独立的新固定成员决定与首个真实 publication。保留目录的当前选定组为一个 metadata 成员，因此实际 `N=1/Q=1`，并不是降低旧 quorum 或把旧证据当新投票。

`ResumeCore` 的 acceptance 把准确 proposal hash、terminal hash、旧 archive hash 和新准入目录身份绑定。保留目录必须实际安装、重读旧完整控制档案并在自己的新 namespace 持久化独立决定。重读全部目标成员后才产生不支持反序列化构造的 `AdmittedTerminalReadmission`。

## 有限状态推进

1. 预览与资格投影只读认证过的 SYSTEM 元数据，不解析、stat 或打开被停用目录。资格取自 registry 的历史 metadata 角色，不能使用当前 `metadata_member=false` 猜测。
2. 显式 commit 提交准确原目录 FD。检查名字与持有 FD、历史 locator、slot/disk/binding generation、旧 seal、已停止 epoch，并要求所有旧运行时生命周期已经 `Closed`。
3. 先持久化 SYSTEM 的准确请求，再在原目录新 operation namespace 准备控制档案和决定。此时 `disk.json` 仍是原 seal，不能改成 `membership-pending`。
4. 新固定成员证书与实际根复验通过后，先在 SYSTEM 持久化新 epoch 的 probe certificate，再保存并推进准确 locator successor。
5. Locator 保留所有旧记录与 stop，只追加一个 readmission event。历史 root 路径、inode/device、disk/slot/binding generation 不变。旧 epoch 永久关闭，仅新 epoch 可被单独选为只读恢复 probe。
6. 实际新固定 quorum、完整档案、journal、首个 publication 和 current namespace admission 全部通过后，才形成永久 `disk-readmit` 收据与活动 `Engine`。

收据包含原 disk/slot/binding generation、`registration=enabled`、新的 `io_admission_epoch`、`authority_kind=terminal-metadata-resume` 和准确 `terminal_authority`。新 epoch 与此 operation 的 request digest 相同。阶段缓存、HTTP 成功状态、probe certificate 和可序列化的 locator plan 本身都不等于操作成功。

## 重启、重复和撤权

`metadata_resume_recovery` 从 SYSTEM 返回封闭 admission 变体、原 actor、operation、transition 和阶段。调用方先核对 admission 变体与原请求，再取得 pending owner；fresh 续办不能驱动 retained 请求，反之亦然。准备阶段重启须重新提交准确目录；已保存 generation selector 的阶段只沿其准确 hash 继续。

普通启动在任何数据目录 I/O 前读取 SYSTEM probe certificate，随后仍须在真实新固定成员上验证独立决定和 publication。缺失、损坏、换根、无关 actor/operation、未完成写入及撤权都必须保留原请求和错误，不能回落为 Genesis、重新添加目录或推断成功。服务端在明确 commit/resume 的权限线性化点重新验证管理身份；纯状态查询不自动重试目标 I/O。

重复计划下线和重新启用追加不同 epoch，保留所有原请求收据；旧 handle、observer、延迟任务及旧 Config 无法借新 epoch 复活。宿主的 `RetainedWorkspaceLimits` 仅在创建恢复 Engine 时显式传入，不写入持久化 membership hash。

SYSTEM 丢失后的显式根恢复仍先验证实际固定 quorum、持有真实根和 writer 锁并完成 workspace 预检，再从完整认证 lineage 解析当前 Enabled epoch，保存完整 probe certificate 和不可变 locator seed。Terminal retained 与活动池 readmission 都使用这条验证链；后续 Stop 必须关闭 seed 对应的准确 epoch。

Fresh 取消后的后继尝试可以选择 retained metadata，但取消证书和原请求保持不变。RetainedMetadata 不能借 fresh 取消接口取消；服务端在移动活跃 owner 前核对两种 admission，终态 head 轮换仍沿当前 main 的精确永久收据验证。

## 成本和验证边界

新增请求与 locator event 大小为 O(1)，registry 和 authority 继续使用当前完整、受限对象预算。没有删减 fsync 或旧完整档案验证；新目录决定、SYSTEM 请求/probe/selector/locator 和真实 publication 仍是独立耐久性阶段。历史链验证成本遵循现有 lineage 和 locator 链实现，本次不声称减少其复杂度。

新增回归源码覆盖单 Disabled voter、保留 data-only、旧句柄关闭、缺失/替换原根、actor/operation/discriminator 不符、locator 故障重启、缺失 probe、重复下线与启用以及原收据。服务接线须另外覆盖撤权和原请求精确结果。此批只允许源码、文档、官方 rustfmt 与 diff 检查；没有编译、运行测试、模型、CI 或真实部署结果，不能把静态检查表述成上述场景已通过。
