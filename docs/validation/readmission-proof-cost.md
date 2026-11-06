# 重新准入控制证明：有界成本验证

## 来源与问题

基线为公开 `f6761067a07a71065540c5ca006eca32ade40aa8`，本地等价提交
`b0eb510ed66cd652b2722ef9806f874ea803f4a5`，共同 tree
`95602b49196384172558a90cc328b38b29117565`。局部优化候选为
`e2085871a0050aa00ea4d502fa981233879ff502`，tree
`271f5cab5937416fd2ab7277df11b197cb7c51c4`。

同一基线的 GitHub Actions run `42188245775` 中，
`prepared_exit_stop_uses_readmitted_survivors_at_every_locator_barrier`
最终通过，依据超60秒警告至完成至少耗时4935.6秒。Engine 总结果为
1991 passed、0 failed、43 ignored，耗时6158.88秒；Server 为1234 passed、
0 failed、14 ignored，耗时651.76秒。根 Cargo 测试结束后，第一个隔离 IO
脚本再次编译 Engine，随后 job 被取消；真实隔离 IO 场景尚未开始。
不能把这一轮描述为该用例必现死锁，也不能把后置门禁计为通过。

原测试在四个 locator 持久化故障点各重建三盘退出/重新准入夹具。
本地有界基线在首次重新准入后，第二轮归档、登记、joint 的成本分别增长至
首轮约44、53、56倍；当时只有1个业务提交、约1.4 MB夹具文件。
90.6秒样本消耗87.28 CPU秒，说明该前缀存在明显计算成本，而非仅在等待。
没有取得内部函数 CPU 占比，以下重复验证结论来自源码调用图。

## 局部优化及安全边界

`readmission/core.rs` 新增私有、单次调用内有效的 `VerifiedProposal`。
同一不可变提案先完成归档、transition、registry 图、来源边界和身份认证，
再供全部 acceptance 使用。三盘 metadata 夹具中，`intent` 原先完整验证
提案4次，`verified_registry` 原先5次，候选均为1次。

原成员数量、固定 quorum、每个原成员决定、目标独立决定及其 hash 绑定均保留。
活盘身份、seal、journal、DecisionStore 与同步边界未删减。
该类型不序列化、不形成跨调用缓存；持久化声明和 acceptance 计算保持不变。
没有修改 lineage、状态机、fsync、原有测试或 CI 超时。

新增根测试
`test/engine/unit/engine/readmission_core_tests.rs`，包含合法证明与七类拒绝：
缺失原成员、替换原成员、外来成员、替换目标决定、错误 registry 图、错误
transition、篡改 receipt hash。每个反例均检查 intent 和 registry 两个入口。
该私有单元测试尚待正式 Engine libtest 执行，默认库构建不代表它通过。

## 实际构建与同介质观察

准确候选默认 Engine 构建一次成功：54.03秒，RSS峰值1,828,884 KiB，
单 compiler、单 CPU、debug0、CGU256、3 GiB地址空间上限。
库 SHA256 为
`08cf5d49aa9fc350ee7e14667b0c0ede1a12e51a46e11f5b82c750fea2184c8c`。

原样复用公共 API driver 源码，SHA256 为
`3d92866a8124d6fc62693a06118a4f2c59510f06e4a9e38db3af67c3f4ae741f`。
它只覆盖三盘夹具前缀，不含最后的私有故障注入、恢复及冷开断言。
首次候选样本的夹具在 tmpfs，而基线在 workspace；该样本保留，
但不用于提速归因。随后获准仅追加一次同介质样本，不重编译。
两边的 binary、夹具及 TMPDIR 均在 workspace，编译器、直接依赖、
native输入、默认features和单CPU设置一致。

| 两边都完成的阶段 | 基线秒数 | 候选秒数 | 单次观察减少 |
| --- | ---: | ---: | ---: |
| 第二轮退出归档 | 7.407894 | 4.574472 | 38.25% |
| 第二轮成员登记 | 20.600003 | 13.436902 | 34.77% |
| 第二轮 joint 决定 | 14.920617 | 9.533670 | 36.10% |
| 全部14个共同完成阶段合计 | 65.357801 | 48.111418 | 26.39% |

候选同介质样本于89.448秒退出，达到预设有界终止点；已完成第二轮退出，
仍在第二轮重新准入激活中，不能算完整夹具通过。CPU共85.91秒，RSS峰值
31,896 KiB。workspace最低空闲4,514,430,976字节，tmpfs最低
1,123,692,544字节，均未触及保护线。所有进程已结束，没有自动重试。

这是一次配对观察，不是重复统计基准。只比较同名且完成的阶段，
不把未完成阶段作为完成耗时，也不外推整条82分钟测试或workspace的降时比例。

## 已验证、待验证与证据

已完成：两文件 rustfmt、diff-check、静态编码等价核对、独立源码审阅、
准确默认 Engine 构建及上述有界观察。

待完成：新增私有对抗单元测试、准确候选的全部四个 locator 故障点、
缺失/损坏 survivor epoch、真实 retained quorum，以及完整 workspace 和
后置隔离 IO 门禁。不得以短样本或核心子集通过替代这些结果。

本地外置证据包 `zxs-readmission-proof-cost-evidence-20261106` 保留
`runtime.json`、原始阶段日志、进程 CPU/IO 采样、
`matched-workspace/result.json`、`matched-comparison.json`、源码/库/driver哈希及
独立审阅收据。基线原始包为 `zxs-exit-stop-cost-probe-20261106`。
产物与日志不随本说明提交。
