# 终态剩余成员：原生决定持久化与冷态原请求恢复

[辅助工具](../components/helper.md) · [决策](../adr/terminal-retirement-decisions.md) · [只读检查](helper-terminal-removal-inspection.md) · [任务清单](../../TASKS.md)

本片在 `ea7407e8` 上开发后对齐 `1a778c2e`，仅交付源码，未审查、编译、测试或运行 CI，也未实际调用工具/修改系统。它推进**原生决定与恢复**，不是剩余成员注销完成，更不是外来完整链接纳完成。

## 生产 API 与实际写入

`TerminalManagement` 增加 `preview_terminal_retirement`、`stage_terminal_retirement`、`terminal_retirement_status`、`resume_terminal_retirement`。预览返回字段私有的原生 Plan，来源是当前已验证 SYSTEM 终态 owner；提交前复验原 terminal operation/authority、registry root/generation、sequence/topology generation、目标完整 pool/disk/slot/binding 及 actor/operation。永久历史已使用的 operation、新编号替代原未决选择、来源变化或目标改变都拒绝。

本片写者只写原终态操作目录中的两个固定文件：

1. `retirement-intent.json`：精确源/目标/actor/operation 的不可变原意图；
2. `retirement-selected.json`：只选择该意图摘要和 operation，必须在第一文件同步之后。

沿用受持有目录限制的 bounded strict JSON、无覆盖原子发布和 file/parent 同步。相同重试重新建立同步屏障，不把前次失败当作未写入。没有额外打开、stat 或遍历数据盘，没有改 Config、当前 registry、State sequence、operation map、namespace、SQLite、标签或物理字节。

`IntentStored` 表示原意图存在但选择未完成；`DecisionStored` 表示原选择已同步，**仍没有完成移除**。状态没有 Published、D3、OperationResult 或可写成功标记。再次打开终态 owner 会验证现存原意图/选择，孤立选择、坏正文或错来源失败并保留原文件，不回落空池。

原主体、operation 和摘要三者相同才可冷态继续。缺选择可以补同一个选择，不能以新参数覆盖原意图；有选择但丢意图不是可自动恢复的新请求。持久化失败可能留下部分或全部原材料，不能清理、复位或改编号。

## 恢复互斥与可见状态

新建 fresh metadata resume、Disabled readmission 预览、held 重入、冷 open-for-resume、held completion 和最终 activation 都在访问新目标前检查未完成选择。当前 terminal registry 保留原成员，readmission 能力显示不可用；已有被选定的注销不被恢复流程偷偷复活，也不把其余成员隐藏成空池。

Server 的终态存储状态带必需可空 `pending_retirement` 字段；有未完成选择时为 `recovery_required`，列准确原 operation/actor/target/digest/阶段。当前 Web 接受这一组合并明确告知“登记未移除、元数据恢复被阻断”，不给初始激活或完成移除的假成功。既有 helper 终态检查展示同一未决记录并返回 `original_terminal_retirement_pending`，仍 `executable:false`、不创建新的操作。

**当前没有完成该决定或取消它的能力。** 因此 staging API 未接到无认证 helper Web/CLI 写入口。调用者必须明确选择这个原生开发阶段；不能因为普通状态、检查或前置规划就调用它。最终 writer 补齐前，产品不能宣称“剩余盘已移除”或“可以重新启用存储”。

## 尚缺的闭包

后继仍须实现：完整 terminal registry successor/历史定义保留、逐目标实际 Stop/关闭证明、SYSTEM 最终决定与永久回执、当前投影与冷恢复、metadata-resume 对新后继的消费。这些义务未通过复用最后 voter 的旧权威或当前不可变选择绕过。

外来二级及以上盘仍要求一次完整低级链和最终目标 writer/独立准入。当前 Backup Candidate/隔离材料仍不是可激活权威，不能解除源围栏或让 helper 报加入完成。H03 继续未完成。

## 成本及验证界限

每次选择重试最多写两份有界 SYSTEM JSON，复用原每文件原子发布/fsync；不删永久材料、不扫描盘目录。状态/原角色诊断增加固定文件读取，现有逐登记角色查询可能重复这些读，仍是管理路径而非文件热路径，未做性能测量。归档全量认证成本保持原终态 open 的范围。

新增测试源码覆盖真实持久选择、intent-only 冷续作、actor/ID/digest 错配、目标路径全替换仍不触碰、注册/operation 未变、metadata resume 阻断、孤立/坏选择失败、过期源不保留新意图、同一未决选择排他，以及 Web 未决状态解析。全部未运行；没有编译、真实掉电、服务身份进程、浏览器或组合通过结论。
