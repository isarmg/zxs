# 暂停交接：2026-11-05

用户要求保存全部现有成果、停止开发。此目录是未完成候选的补丁与验证摘录，不是已启用的功能；不要直接部署或把补丁当成测试通过。当前运行代码基于 main 2063a938。

- prefix-proof.patch：基于 9ae138d0 的统一冷恢复前缀证明候选。已知当前 main 的 plan.cancel / branch.archive 存在伪装 receipt.kind 后过早读取 payload proof 的安全阻断。候选含真实 D3、scope/版本证明，但最后静态审阅与定向测试未完成。workspace 原始完整请求未持久化，不能声称完整重算。
- merge-links.patch：历史关联候选；staged-merge-link-snapshot.patch 为停止时集成树暂存内容的原样快照。与 prefix、tags 和 backup 存在共享接线，需以主线为准逐块合并，不可重复套用。
- file-tags.patch：标签候选，保留 named HEAD 与 selected tree 区分；需结合统一冷恢复证明复核。
- backup-core.patch 与 backup-prefix-integration.patch：正常 backup 候选及依赖前缀证明的接线，必须一起审阅；current backup 的 degraded recovery 尚未实现并应保持拒绝。
- trusted-data-recovery.patch：灾前可信清单与 SYSTEM 丢失后显式 data-root 恢复候选，尚未合入。首次准入检查已加强，但后续 writer.lock、SQLite/WAL/SHM 和其他 SYSTEM 路径消费者仍存在并发祖先替换窗口，不能宣称全流程安全。

以上补丁均未完成组合编译或定向测试；来源基线和先后依赖以补丁内容为准。下次恢复先解决统一冷恢复安全边界，再审阅并逐批接线、运行针对性测试。不要恢复暂停任务或部署到真实设备，除非用户另行要求。

停止时最后一次 CI 快照：2063a938 的 run 41955406913，15/17 作业成功，workspace 与 Standalone ELF/Chrome 未结束；不可称为完整通过。此前两个终态失败用例与 Unadmitted 回归已在后续运行中通过；readmission 契约定向测试 6/6 通过。附带日志是摘录，不代表完整验收。
