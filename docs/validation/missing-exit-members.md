# 缺席退出目标的固定多数准备

状态：本文对应新增源码与回归源码。此批只执行官方 Rust 1.99 `rustfmt` 和 `git diff --check`，未执行编译、测试、模型或 CI；不得把代码路径或测试名称当作运行通过证据。共享 bootstrap 接线必须与核心改动同时整合。

## 真实入口和准确范围

仍使用生产 `begin_disk_exit` → `build_managed_membership_control_archive` → `enroll_pending_members` → `resume_membership_joint_decision` → `activate_pending_disk_exit`，没有只供展示的独立证明入口。

- 唯一退出目标为原 metadata voter、其资源在恢复准入时缺席，而全部保留 target metadata 成员在线时，允许准备。保留成员必须达到原固定 Q；例如原 3/Q2 → 新 2/Q2、原 4/Q3 → 新 3/Q2。原 N/Q 来自完整 origin，目标 N/Q 来自完整 target，不从在线数量重算。
- 目标本来是 data-only 时，缺席不改变任何 metadata 成员、fence 或 archive 格式；全部原 metadata 控制材料仍须完整，仅最终 I/O 收束使用未打开目标路径。
- 本批不支持另一 retained metadata 成员也缺席。原因是此生产驱动仍需每个目标的独立 archive/bootstrap/joint 安装和全部目标的规定首条 D3 普通发布。不能把目标多数足够读取已有状态解释为全部目标已经完成首次发布。
- 原 2/Q2 丢一名 voter、最后 voter 退出、没有完整已发布边界、或必要历史对象在所有可读副本中均缺失，仍没有安全继续依据。缺失对象/原成员错误保留，既不改变门槛，也不通过旧快照跳过历史。

## 新证明怎样绑定

新 Exit intent 显式增加 `fence_scope: "retained-exit-quorum"`，它参与原 intent 域哈希。scope 只允许省去准确退出的唯一 old-only voter；目标配置仍保存全部历史拓扑记录。旧 full intent 不序列化该字段，其原字节和哈希不变。历史 Candidate、Publication、seal、Receipt 和永久业务结果均不改写。

scope 对应独立 archive 格式 `retained-exit-quorum-control-metadata` 与 joint 格式 `retained-exit-quorum-slot-decision`。各边界核验 intent、archive/joint 格式及成员集合一致；旧 full 格式仍要求全部原 journal，不能换一个 len 判断就接受少盘副本。归档的 `original_members` 始终报告原 N，`journals` 仅列实际读验、复制的原成员，绝不构造缺席 journal 或缺席票。

新 fence 之前，读完每个保留原 journal：必须有同一完整 publication 链、准确当前 source Candidate/snapshot、原成员/代际证明，以及所有被引用的 snapshot、history 和 authority 闭包。边界后的 ACCEPT、FINALIZE、PUBLICATION、REPROTECT、冲突 fence、短尾、缺页或坏对象均阻止新 fence。普通恢复因缺一名 voter 而 blocked，只能由这段更窄的完整证明进入 Exit；一般业务仍 blocked。

随后仅向已锁定且验证的保留原成员写同一 intent 与真实 durable fence。每名成员的原 N 身份保持不变，正常历史 FINALIZE 仍按其全部原成员规则核验。原 Q 与任何潜在决定 Q 的交集，结合完整日志检查和持久 fence，排除在未观察少数成员上独自成立的新决定。缺席盘的私有未决定 PREPARE 不冒充已发布历史；所有已发布历史及读到的决定引用仍完整归档。

归档先复制真实 journal/身份文件与全部引用对象，再只通过目标归档独立复验，最后发布准确 root。partial archive、partial fence 和失败 reply 保留；重开必须恢复同一 scope、operation、actor、配置和 hash，不能改写成另一个意图。后继归档按每个历史 epoch 的准确 scope 回放，原 full epoch 不被重新解释为多数 epoch。

## 安装、停止与恢复

pending 和 joint 只为 intent 指定的实际参与者建立本盘身份、archive、bootstrap、共享决定槽和回执。joint 的原/目标 Q 和 `union_members` 仍报告完整配置；缺席原盘不计票，也不计 certificate 覆盖。`all_certificates_synchronized` 在缺席 old-only 时仍为 false。退出消费的是真实固定两侧多数及每个 target 的完整 certificate 覆盖，而不是这个全并集观察值。

每个保留目录的独立完整闭包和 prepared Exit authority 必须先安装、读验，之后才同步系统侧 exact stop。不存在目标资源只产生 `Unopened` 状态，不是 I/O 关闭证明。取得新 authority 对应的 restricted config、准确 durable stop 和资源缺席验证后，还须由 `RecoveredDiskIo::admit` 检查同一逻辑 binding 的全部进程内旧 gate 已 Closed，并持有禁止新 gate 的 guard；任一旧读者/worker 未排空仍拒绝。该阶段不访问目标路径，不伪造物理 seal。

新资源投影、活动 marker、规定 D3 及当前 registry/namespace 安装顺序沿原协议。计划下线保留登记和可见数据，注销仅移除登记及准确目标独占当前索引；两者不擦除目标物理文件或历史，也不生成目标恢复备份。stop 后重启、陈旧 Config 和系统丢失的显式 retained-root 恢复仍只接受完整独立证据，不能重新访问目标补齐。

## 回归入口与验证要求

新增源码位于：

- `test/engine/unit/engine/membership_missing_exit_tests.rs`：真实 3→2、4→3 准备/发布，缺席 data-only，两侧固定计数、目标字节不变，partial fence/archive、stop/D3 中断，系统丢失仅从 retained roots 恢复，scoped 前代后继续 full Exit，scope 篡改及未完成 ACCEPT/缺历史拒绝。
- `test/engine/unit/engine/membership_retained_exit_adversarial_tests.rs`：11项独立负例，真实未完成决定/逐盘前缀、固定两侧多数与每目标安装、旧成员返场、全逻辑 I/O 域、partial marker、归档/joint/portable 篡改。stop 后尚无 active journal 时使用显式 activation 恢复，不扩大普通 open 权限。
- `test/engine/unit/engine/membership_control_archive_tests.rs` 的 `retained_exit_archive_*`：scope 重新标 full、缺 witness、改 fence owner、缺历史页、拿 Exit archive 作 Abort，以及更换归档绑定格式均拒绝且不修补字节。

整合后至少执行 `cargo test -p storage-engine --features fault-injection missing_exit`、`retained_exit_archive`，以及既有 `membership_`、`disk_exit_`、bootstrap/restricted discovery、历史、当前 namespace 和 HTTP continuation 回归。上述测试源码未由本批作者执行。临时目录故障注入也不替代物理掉电、跨服务进程尾部或真实设备验收。

## 成本

仅缺席 voter 的新 fence 准备增加一次完整只读前缀/引用闭包遍历；时间与所选原 journals 和实际读取的历史图总量成正比。后续 archive、每目标独立安装/重验仍保留原有多遍历史成本，可能随累计快照接近二次，不宣称性能提升。成员集合是 O(N) 大小，归档页/对象按既有哈希去重且受既有单对象预算约束。新增 scope 为常数大小，无每次业务写入开销。所有既有对象、journal、root、stop、certificate 的文件与目录同步屏障保留，没有减少 fsync 来换可用性。
