# 故意破坏规则的反例与产品故障测试映射

这些是故意破坏抽象规则的反例，用来确认检查器有检出能力；不是对产品实现的缺陷断言。原始机器轨迹保存在 `test/publication-model/evidence/run-*/results/counterexample-*.json`，同目录 `replay-*.json` 保存逐动作、逐状态摘要重放结果。

| 变体 | 最短轨迹动作数 | 命中的独立性质 | 产品故障测试映射 |
| --- | --- | --- | --- |
| `shrunken_quorum` | 21 | `fixed_quorum` | N=2 完成准备后临时掉一成员；另一成员 ACCEPT 完成仍不足固定 Q=2，不得宣称决定成立 |
| `early_ack` | 16 | `publication_barrier`、`fixed_result` | 已决定但 FINALIZE／本盘证明／PUBLICATION 尚未全体完成；不得确认成功 |
| `omit_sync` | 5 | `durability_barrier` | 正文写入、文件同步、名称安装完成但父目录未同步；不得越过内容准备屏障 |
| `timeout_rollback` | 12 | `no_timeout_abort`、`abort_resolution` | 第一份 ACCEPT 已发出后 RPC 超时；没有完整 Q 无候选观察及耐久恢复中止记录，不得返回确定未发生 |
| `lost_binding` | 39 | `unique_publication` | 成功响应丢失后沿原 operation_id 重试；永久摘要绑定与原收据必须阻止生成第二个业务效果／事件 |

动作数来自已执行的 BFS 反例，具体配置、故障预算和工具 SHA 以同目录 JSON 为准。动作数只在对应模型/配置及已检查的 BFS 深度内有最短性保证；不代表真实 I/O 最短故障序列。发现反例后搜索即停止，不把剩余图说成穷举完成。

缩减依靠从初始状态进行 BFS，父指针返回最少动作的可达违规前缀。每步保存状态 SHA256，重放必须检查动作启用、实际状态摘要和同一具名违规；修改动作或摘要的负测试必须失败。`test_mutations.py` 还要求这些轨迹不能作为 `correct` 变体的同一执行重放，防止只靠脆弱或无关断言产生假阳性。

恢复深层窗口另由 `test_recovery.py` 和 `test_model.py` 的定向轨迹覆盖：Q 中止而永久缺盘保持不可写、旧 ACCEPT 不复活、恢复中第二次断电、单处正常发布后内容损坏沿用原结果、无正常发布且无好源时明确降级、一好一坏副本先补齐、计划正常恢复后新损坏不得冒充历史正常结果。这些轨迹覆盖不等于较大配置从根的完整穷举。
