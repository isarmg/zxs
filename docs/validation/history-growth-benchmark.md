# 永久收据与事件增长验证工具

本工具面向当前 main 的公开 Engine API。它固定活动文件／目录／分支与正文版本数量，
单独增长永久收据与事件；不修改产品、格式、永久保留规则或采用门槛。
源码与测试分别在 [工具目录](../../tools/history-scale-bench/driver.rs) 和
[确定性监督器测试](../../test/history-scale-bench/test_supervisor.py)。

## 默认停止与历史身份

[控制计划](../../tools/history-scale-bench/acceptance-plan.json) 的 reviewed/frozen candidate
均为 `null`，包括微型自测在内的所有测量都明确拒绝。没有 push 自动触发的基准。
后续由主任务明确选择、审阅精确当前源码 SHA，才可启用单独验证；不自动跟随 main。
10k 另需已记录的初档结果／资源审阅，100k 不在允许 profile 中。本次工具采用不授权重档运行。

固定历史对照为 `e2f33d9882cf7263412d9370b6a703e0b216357d`。旧实验候选
`209f31a4122174129bcf40b56c0598ac61e66352` 与旧工具
[f84505df](https://github.com/isarmg/zxs/tree/f84505df39b0bcdad1c38d26c82db8377400073f)
仅属于历史证据；新 main 适配器明确拒绝以 209 为候选。旧分段产品祖先不随工具采用。
当前 main 使用 `events_after`，工具按 257 条包装分页，不为工具新增产品 `events_page`。
255/256/257、1023/1024/1025 是保留的增长观测点；旧实验的封段解释不表示 main 已采用该布局。

[旧 13b 规模运行](https://github.com/isarmg/zxs/actions/runs/40823525831)
使用零正文、1 条 setup、N−1 次绑定，与后续固定正文 fixture 不混算。
[209 的 1k／边界运行](https://github.com/isarmg/zxs/actions/runs/40858144942)
共 20 样本，19 passed、1 failed；60 阶段为 58 passed、1 failed、1 not_executed。
1024 候选 reopen 虽写出完整业务 complete，但缺最终 RSS／退出证据，保持 failed，
其 recovery 未执行。监督器修复与新单测不能追认旧数据通过。10k／100k 在该轮未启动。
旧日志、失败和 Incomplete 都保留原 run/SHA，不改记为当前 main 执行。

## 固定工作负载与完整 oracle

每版、每轮和每种计数模式各建新池，不跨格式复用。使用一个逻辑开发数据盘、一个仓库、
一个分支，先创建一次固定 4 KiB 文件及一个正文版本；后续只切换同一 SMB 绑定的 enabled。
N 表示总收据／事件数，即 **2 条 setup + N−2 次绑定操作**。

原始收据流式写入独立 oracle 文件，只常驻三个选取的记录。每个 build/reopen/recovery
阶段均核验全部原 operation_id、owner、请求摘要和完整永久结果；两次遍历全部事件，
核对身份、顺序与游标。另核对固定版本 ID、完整 4096 字节、版本描述与完整固定历史图；
抽样重放原请求，重放 seed 的完整原 CommitRequest，拒绝摘要冲突并检查没有重复发布。

永久 outcome 查询与显式当前健康诊断分别计时。当前 main 调用
`operation_record → OperationOutcome::from`，不在该查询中做健康 I/O；历史对照返回原永久
收据，返回类型形状不同。真实健康单独执行 `prepare_operation_health(...).observe()`；
对照使用其原 `check()`，标明 legacy 适配。正文 commit 必须有健康观察范围且 Healthy，
无 commit 的绑定范围为 None，不能冒称 Healthy。这里测本机 Engine 消费者，不包含 HTTP 鉴权与传输。

三个阶段使用同一样本池、各自新进程。首次重开可能保存检查点，随后恢复到新系统根可能使用它；
原系统根与盘材料保留，不清缓存或删历史提速。重开／恢复计时和 RSS 快照在 oracle 扫描前。
已发布正文的历史成功收据与当前健康分开；工具在每阶段实际完整读回固定正文，
不把该额外 oracle 说成 Engine::open 自身重验了全部已发布正文。产品恢复语义见
[事务与恢复](../10-transactions-and-recovery.md)和[盘格式](../08-full-storage-and-disk-format.md)。

## 监测竞态与失败边界

`/proc` 状态与内存字段不构成跨时刻一致快照。单次缺 RSS 会保留两次原始 status、PID、
状态和路径，返回不可用采样；不补零。非终态缺失在现有轮询中最多待决 0.5 秒，
轮询睡眠及状态补读返回后均核对截止；即使随后观察到正常 RSS 或退出，逾界仍失败。
时间、磁盘和已知 RSS 超限仍立即按原预算停止；持续缺失或权限错误仍失败。
部分进程 RSS 缺失时，observed_tree_rss_bytes 只是已观察到的下界，rss_complete=false，
不伪造完整进程树 RSS。最终必须取得有效终态观察、wrapper 退出码和 GNU time 峰值。

采样、启动或收尾异常均保留显式 monitor_error/finalization_errors，并尝试写出 supervisor
终态记录、最终空间和设备统计。只有退出码 0、原预算全部满足、完整 driver 协议及必要
trace 都齐全才通过。complete 标记、一次补读或监督器自测成功不能替代这些条件。
持续缺 RSS、资源日志为空、非零退出和不完整 oracle 的确定性回归只模拟子进程／时钟，
不启动存储工作负载，不依赖真实时间竞赛或模型搜索。

## 构建与采用接线

正式单测、编译、格式与文档验证使用 GitHub，固定 Rust 1.99.0；不降级工具链。
静态 CI 通过 `.github/scripts/check.sh static` 发现本目录全部 Python 单测；
工具采用验证还实际执行 contract-check 的 `source_export` 集成测试，验证源码导出不依赖
被忽略的本地证据，并仍拒绝损坏的必需源码链接。历史日志使用固定 run/commit 的外部链接，
不把可缺失的本地输出改成必需源码。

[prepare.py](../../tools/history-scale-bench/prepare.py) 只接受显式精确候选或历史对照的干净
checkout，在已有 perf-check 中临时注册普通 binary，不改依赖和 Cargo.lock。
旧源码没有驱动时原子新建；已采用源码含跟踪驱动时只复用逐字节相同的文件，差异明确拒绝，
不覆盖。registration schema 2 记录 tracked/copied；[构建校验器](../../tools/history-scale-bench/verify_build.py)
按该模式精确核对修改／未跟踪路径、驱动摘要及实际 Cargo 产物，拒绝额外修改。

普通 binary 避免 engine 的 dev self-dependency 启用 fault-injection。不要用 example、
libtest、all-targets 或 all-features 的成本冒充普通构建。实际 artifact 必须是 engine
无 fault-injection/test-support、driver 仅对应适配特性、release opt3、test=false。
源码 SHA、原／临时 manifest、锁文件、驱动、命令、编译器、ISA 与二进制均绑定 provenance。
源码／驱动变化使旧二进制失效，不能让 209 旧结果代表当前 main。

[workflow 模板](../../tools/history-scale-bench/workflow.yml) 保留在工具目录，需主任务另行
复制到独立验证载体才可手动启用，不进入产品祖先、不取代正常 CI。每版一次构建，
所有规模下载复用同份产物；原 build run/carrier 与成功 selftest_run_id、当前测量 run/SHA
分别记录。仅监督器修复时可复用两 build job 均成功的原产物，但规模仍须有独立成功的
4 样本／12 阶段微型证据，旧失败不翻绿。

## 指标、预算与解释

无追踪计时和 syscall 计数独立，计数覆盖整个阶段，含 oracle 与重放，不能当每次追加的同步数。
保存首次／重复查询、全部逐条延迟、多轮波动、峰值与恢复后 RSS、对象数量和保留空间。
/proc rchar 与 syscall 返回字节是逻辑 I/O；RSS 不是分配次数。设备统计是 runner 共享设备观察，
一个逻辑开发目录盘不证明物理多盘性能、冗余或掉电持久性。无法重判的同步路径保持 unknown。

微型 profile 为 N8，计时和计数各一轮，每阶段 60 秒、cohort 240 秒；规模 profile 每阶段
600 秒、cohort 2400 秒。其余预算为 RSS 8192 MiB、单样本分配空间 12288 MiB、cohort
24576 MiB、最少空闲 2048 MiB。采样不是内核硬隔离，结束时还检查最终资源。
达到预算记录 incomplete，前置未完成的阶段／重复为 not_executed。强制停止时在途操作可能
已经持久化，已确认前缀不是池的最终总数；缺失最终峰值不能由采样值冒充。

完整 requested 矩阵和每阶段全部通过才可报告全通过。原始日志／oracle／资源／trace 与
来源材料保存为 workflow artifact；池目录不上传。工具不证明整个保存路径 O(log N)、
O(delta)、无限历史或容量采用通过，标准与后续运行由主任务决定。

## 2026-10-31 冻结产品的增长分档实证

以下结果只对应 `d3eefd289e53a2803703870fe4250d08cadff9b5`，普通 driver 二进制
SHA256 `87d044ff1d3f1c8f6cc7e12dbe51dfeafb4d63773ef8db3f924fa14268d01e03`；
不声称测量了后来 main。后续分档使用各自预先冻结的策略，不能拿上文一般 profile 的
预算替代实际 run 的准入合同。

[5000 run 41170017529](https://github.com/isarmg/zxs/actions/runs/41170017529) 的两轮
无追踪计时及六阶段完整 oracle 均通过；每阶段全部原收据／事件均为 5000，计数覆盖另算。
助手云重新从六份原始 driver JSONL 汇总，与 summary 逐项一致。组时长 5751.769126 秒，
未超预先阶段 3600 秒／组 10800 秒的预算；未追加重跑。

| 重复 | build（秒） | reopen（秒） | recovery（秒） |
| --- | ---: | ---: | ---: |
| 1 | 574.682344 | 1094.155360 | 1094.041628 |
| 2 | 587.975992 | 1197.008890 | 1193.600958 |

最高观察 RSS 为 69,238,784 字节，最大完整样本 536,358,912 字节，完整组 1,069,162,496
字节。原始 [ZIP artifact 13033487036](https://github.com/isarmg/zxs/actions/runs/41170017529/artifacts/13033487036)
SHA256 已匹配 `17c50d2540b60fe374e10a5d27fef534acbbcb59822225d0d64b4220496846cc`。
`all_growth_capacity_metrics_passed=false` 是工具刻意固定的范围声明：单档不认证整个增长／
容量主题，并非该档六阶段失败；`all_requested_samples_passed=true` 与之并存。

将上述最大值纳入原增长预测公式，保留 1.5 安全系数和原指数规则，8192 两轮预测 build
3124.762 秒、reopen／recovery 各 8560.958 秒、组 40553.355 秒，超过原 3600／10800 秒
预算。10000 和 100000 预测同样超限，因此三档均未执行，不通过扩大预算或减少 oracle
取得准入。预测不是实测时长或已证复杂度上界；此处是需软件优化后再评估的边界，不是
只能由人工完成的项目。

[2048 counts run 41168537536](https://github.com/isarmg/zxs/actions/runs/41168537536) 的
三阶段分别 1841.961196／2042.174015／2037.138643 秒，组 6067.530505 秒，均在该档预设
3600／10800 秒预算内。助手云以独立解析器流式重数原 trace 共 18,961,860 行，三阶段
池内系统调用总数、成功读写返回字节、fsync／fdatasync 成功总数均与原结果一致，未完成调用为零。
原 ZIP SHA256 `fe00d233c19bbda76cbdc4a887bdd43b8895c7ed3b7eebc243eaa2a63eca8d23`
及三个分片逐一匹配，compact 的 45 个原文件逐字节一致。原池未上传，所以不能独立重建
file／directory／unknown 分类，只核各类之和。追踪耗时不用于延迟结论；旧 environment
里的 untraced 文案错误保留原样，工具修正只影响未来输出，不重写证据。
