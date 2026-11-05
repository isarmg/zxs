# 保留登记目录的认证重新准入

[成员代际 ADR](../adr/0013-generation-bound-membership.md) · [在线磁盘管理](../design/online-disk-management.md)

## 范围和对象

本协议仅用于仍有活动 metadata 组的 `Disabled → Enabled`。对象是登记目录，不是 OS 磁盘、独立文件系统或硬件故障域。允许恢复原 data-only 角色或原 metadata 角色；`Detached` 永久拒绝。终态管理池中的旧 Disabled 目录不属于本入口。

`DiskRegistrationIdentity`、历史拓扑定义、binding_generation、旧 Config 和 Candidate 编码不变。原目录数据、历史、旧 stop、所有永久操作收据不删除；不在下线或删除目标上创建恢复备份。本协议新增的仅为必要控制证据，存储于活动控制根、明确准入目标和 SYSTEM。

## 权威与类型

- `ReadmissionRequest` 绑定准确登记身份、目录 device/inode、停用前最后活动 seal、当前父 registry、准确 stop 操作与摘要、旧 admission epoch 和历史原角色。目标配置只能改变该目录的 metadata 角色。
- `ReadmissionCore` 绑定完整原配置及固定 N/Q、原独立成员 seal/fence/journal 归档投影、父和后继 registry 图、冻结业务代际、每个原成员和目标自身的不可覆写 `DecisionStore` 决定。
- 当前在线消费者要求所有原 N 的控制复制及决定完成，不按可达集合缩小 Q。新 metadata 角色的目标也独立持有完整控制复制和决定。多个目录可在同一文件系统，device/inode 二元组不可重复。
- 仅真实独立根上的不可覆写决定、完整归档和目录身份经同步复验后，才产生不可反序列化的 `AdmittedReadmission`。它准许追加本次 locator event，不准许清除旧 stop。
- 新 I/O admission epoch 为本次认证 intent hash，未进入 Config 序列化。只为目标建立新 gate；其他活动目录保留 gate。quiesce 的 registry → gate 同一临界区即永久撤销准确旧 epoch；进程关闭墓碑按 `(pool, disk, slot, binding_generation, epoch)` 保存，旧句柄、旧 observation、旧 workspace 或最后旧句柄释放都不能重开旧域。冷启动由认证 lineage 重建所有已退役 epoch，不从内存缺失猜测权限。

## 持久化顺序

1. 纯 metadata 预览确认当前 registry 与 namespace 代际一致，读取历史原角色，不探测目标。
2. 显式 descriptor 提交后，先确认全部旧逻辑域已经 Closed、真实 I/O 尾部为零，再验证准确登记目录和停用时 seal。冻结当前原活动成员。
3. 将完整控制归档安装到每个原成员及准确目标，分别写入并同步目标与原成员的永久决定。决定槽不允许换意图或换选择。
4. 复验真实根证据，先不可覆写保存 SYSTEM `readmission-probe-admissions/<intent>.json`，再保存准确 `readmission-activation.json` 计划。
5. 在原不可变 locator 链追加 readmission event 并完成 record/head 同步。失败只允许按完整计划从准确旧链重算并重复同步，不将未知尾记录当成功。
6. 只为本次目录建立新 epoch gate，安装原控制历史、当前 authority marker、journal 和新 seal。旧目标域永不 reopen。
7. 按新组固定配置完成真实 D3，恢复原 current namespaces，保存 Config 投影。完成后同步退役当前 SYSTEM 恢复计划，才返回成功；永久收据、不可变 authority 和每 epoch probe 证据不退役。

已有 locator stop 的 `admission_epoch=None` 不序列化；无 readmission 时 event 列表不序列化。因此原链记录的规范字节和 hash 保持不变。以后每个 stop 绑定其确切 epoch；同一目录跨生命周期可有多个永久 stop，而 `Disabled → Detached` 不替换原停用事件。

## 冷启动与恢复

普通启动不得仅凭 locator event 探测原 Disabled 地址。SYSTEM 保存的完整 portable 证据被严格解析为 `ReadmissionProbeAdmission`，只授权 locator 中准确 path、binding、device/inode、operation、epoch、旧 stop 的只读探测，不授予 voter 或 serving 权限。之后仍从真实独立根读取当前固定组 Q 的 seal、决定、归档和发布证据。每个 epoch 的证据永久保留，因此多次、多个目录重新准入不会逐次减少安全启动根。

缺少或损坏 probe 证据、当前实际 Q、准确目录身份或旧 I/O 尾部闭合时明确拒绝，不凭 event、缓存状态、一个目录中的多份复制或缩小分母继续。

显式冷恢复为 `resume_disk_readmission_operation(root, operation_id, principal, limits)`。在任何存储根打开前检查 SYSTEM 计划的原操作和 actor；随后先认证计划规定的原成员证据，再探测目标。没有计划时，`readmission_recovery_pending` 仅返回 false，调用方可恢复旧 source 并继续同一已保存操作；损坏计划等其他错误不作为 fallback。恢复保持 workspace limits。旧已发布计划在正常 open/open_for_recovery 完成真实状态认证后同步退役，避免服务先保存下一 Fencing intent 的崩溃窗口误续旧操作。

## SYSTEM 丢失后的显式恢复

`restore_from_disks_and_data_roots` 先按当前已认证成员配置验证真实固定 Q，再取得准确目录与 writer 锁并完成 workspace 限额预检。仅对本次明确提供、仍为 Enabled 的目录，沿完整认证 lineage 找到其当前 readmission epoch，复核原登记、device/inode 与当前 seal。它不访问未提供的历史根，data-only 根不计入 Q。

恢复只重建新 SYSTEM 的探测证书和 locator：每个活动 epoch 的证书绑定完整当前 authority、原 readmission request 及这次实际持有的恢复地址；新 locator 的 `recovered_readmissions` 保存已认证的起始 epoch，不伪造已经丢失的历史 stop 记录。空集合不改变既有记录字节；任何后继记录都必须保留相同起始集合，以后仍按原 stop/readmission 追加协议推进。缺少证书、错误 lineage、物理绑定不符或真实 Q 不足均失败关闭。

起始 epoch 不授予 voter 或服务权。下一次启动仍先在存储根 stat/open 之前读完整 SYSTEM 证书，再在准确地址上验证当前实际固定 Q 与发布状态。允许显式恢复调用提供已验证的同一目录新地址；普通 Config 编辑不能搬移已恢复 epoch。原 payload、历史与操作收据不改写，旧 epoch 墓碑继续从完整 lineage 重建。

## 当前 namespace 和历史

后继 registry 精确继承历史拓扑和 detached 集合，只将准确 Disabled registration 改为 Enabled，并恢复历史原 voter 角色。current namespaces 从原已准入视图继续，保留被隐藏路径、已删除身份、不可用版本和历史掩码；不会从旧 Commit 重新拼装已删除条目。仅原 owner 为本目标的保留当前 scope 恢复写资格。旧 workspace registry baseline 永久过期，需新建或重新准入本代 workspace。

## 开销与证据

新增成本为原 N 与目标各一份完整控制归档及不可覆写决定，每 epoch 一份有界 SYSTEM portable 证据和临时计划，一次不可变 locator 追加，以及已有 marker/seal/D3 同步。归档规模随控制历史增长，authority 受现有 MAX_METADATA 限制；达到上限拒绝而不裁剪永久收据。进程墓碑内存随本进程已认证退役生命周期数量增长。当前 portable 边界校验会重复验证图，不宣称性能提升或减少 fsync。

新增真实源码回归覆盖 metadata/data-only、D3 后拒写、重新准入后真实写入和冷启动、旧 gate/observer/workspace 不复权、永久幂等、重复生命周期、全部目录都有新 epoch 的冷启动、Detached 拒绝与掩码延续、目录替换、未闭合尾部、缺原组证据、缺 probe 证据，以及归档、决定、probe 证据、计划、locator、marker/seal、D3/Config 各阶段中断。新增 SYSTEM 丢失后的 metadata/data-only 目录重定位、epoch 重建、再次生命周期、真实 Q 缺失与起始证书缺失回归源码。同一底层 AtomicFile/locator record/head writer 继续使用原有故障回归，不引入第二套持久化实现。

本批仅执行固定 rustfmt 及 `git diff --check`；未运行构建、测试、模型或 CI，也未提交、推送或部署。上述源码回归尚待实际执行，不是通过声明或物理掉电验收。
