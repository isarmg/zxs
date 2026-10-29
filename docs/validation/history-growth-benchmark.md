# 永久收据与事件增长基准

工具以 `e2f33d9882cf7263412d9370b6a703e0b216357d` 为固定对照。
冻结候选为 `209f31a4122174129bcf40b56c0598ac61e66352`，tree
`d8c56a63ff52eb324067c0362bcd746189761c3f`。源码冻结不是容量或采用验收结论。
源码、驱动、构建输入和实际执行 SHA 必须随每次结果保存。
工具不修改生产算法、格式、永久保留规则或公开 API。

## 验收控制器与冻结

[acceptance-plan.json](../../tools/history-scale-bench/acceptance-plan.json) 把已审阅候选
`reviewed_candidate_sha` 和最终获授权候选 `frozen_candidate_sha` 分开。
当前两者均为上述冻结 SHA。先运行 `small-selftest`：8 条总收据，计时和独立计数各一轮，
每阶段 60 秒、整个 cohort 240 秒。它只核对工具接线、协议和 oracle，不形成容量结论。

[controller.py](../../tools/history-scale-bench/controller.py) 在任何构建之前选择矩阵。
`acceptance` 模式要求冻结，并仅选择边界和 1k；运行编排器独立重读同一 plan 再次准入。
通过这两档并审阅结果与资源趋势后，才能在 `ten_thousand_review` 记录对应候选、
两档 run ID 与理由，再单独选择 `10000` 模式。该字段当前为 `null`，10k 仍拒绝。
候选变化后须重核源码与适配契约，不能把临时候选或旧二进制自动当成冻结版本。
这是操作防误用门槛，不是认证或用户授权机制。

冻结后仅准备以下矩阵；100k 不在当前授权的控制器中：

| profile | 总收据／事件目标 | 无追踪计时 | 独立追踪计数 |
| --- | --- | --- | --- |
| boundaries | 255、256、257、1023、1024、1025 | 各一轮 | 1025 一轮 |
| 1000 | 1000 | 两轮，反转版本次序 | 一轮 |
| 10000 | 10000 | 两轮，反转版本次序 | 一轮 |

规模阶段预算为每阶段 600 秒、每 cohort 2400 秒。所有 profile 另设采样 RSS
8192 MiB、单样本保留空间 12288 MiB、累计空间 24576 MiB、最少空闲 2048 MiB。
达到预算保留已确认前缀并标记 `incomplete`；后续阶段／重复明确为 `not_executed`。
采样不是内核硬隔离，结束时还检查最终时间、峰值和空间，允许观察到超调。
驱动默认拒绝超过 16 条；编排器只有通过冻结 plan 后才传显式规模开关。

## 工作负载与完整性 oracle

每版、每轮、每种计数模式各建新池，不跨版本打开格式。一个逻辑开发数据盘、
一个仓库、一个分支，在准备阶段只创建一次固定 4 KiB 文件及其一个版本；随后仅
调用真实 `Engine::set_smb_binding` 交替切换同一绑定的 enabled。活动文件、目录、
分支及正文版本数不随规模增长；历史收据与事件继续永久累积。

`--operations N` 是包含仓库创建与固定正文提交的最终总记录数，
即 **2 条 setup + N−2 次绑定操作**。256／1024 边界按总数定位。
请求 ID 确定且分散，oracle 流式写盘，不额外常驻保存全部历史数组。
追加计时只覆盖引擎操作；基准自身记账与核验另行执行。

每个 build、reopen、recovery 阶段都必须完成：

- 全部原 operation_id、owner、请求摘要、原永久结果、sequence 与代际绑定核验。
- 两次完整事件遍历，包括页顺序、身份、游标与最终数量，页大小 257。
- 早／中／晚永久结果查询，以及单独记录的显式健康诊断。
- 早／中／晚所选原请求及固定正文提交请求重放、绑定／正文摘要冲突拒绝、无额外收据／事件／正文效果。
- 固定版本 ID 的完整正文读取，逐字节比较 4 KiB，并核对固定活动集。

三个阶段使用同一样本池，分别在新进程执行。首次重开可能写恢复检查点，后续从原盘
恢复到新系统根可能使用它；原系统根和盘材料保留。两次不是等价的无检查点冷启动，
操作系统页缓存也可能保留。

### 永久 outcome 与当前健康

当前产品服务端的永久结果路径是 `authorized_record → Engine::operation_record →
OperationOutcome::from`；投影明确不包含 `current_health`。驱动直接调用 Engine
和契约投影，记录的是这个核心消费者的成本，**不是 HTTP、鉴权或真实 daemon 端到端延迟**。
旧版沿原 `operation_record` 返回完整永久收据（含 legacy health 冻结字段），当前版
额外执行 `OperationOutcome::from`；两边均不调用健康检查，但返回类型形状不同。
完整 native 收据仍与原 oracle 逐条比较，不删除 persisted 字段核验。

真实诊断另走 `prepare_operation_health(...).observe()`；它读取所引用 commit 的完整
版本范围并实际验证内容。没有 commit 的绑定操作返回无健康观察范围，不能解释为
Healthy；固定正文提交必须有观察范围且内容健康。旧版使用其原 `check()` 诊断接口，
单独标记适配语义。诊断、正文读取、全收据／事件 oracle 都保留，不藏进 outcome 计时。

### 恢复验证边界

当前恢复仍扫描日志、验证候选链接，读取全部收据／事件段，对每个 commit 的
命名空间完整准入，并遍历、核对历史目录投影。新 memo 只在同次准入复用已认证对象，
不把旧历史跳过。参见[事务与恢复](../10-transactions-and-recovery.md)、
[ADR 0011](../adr/0011-paged-namespace-roots-and-segmented-logs.md)和
[盘格式](../08-full-storage-and-disk-format.md)。

已发布版本的 named payload 在恢复检查点中不重验正文；恢复历史成功时重建原证明，
保留后来损坏或清除后的原成功收据。未发布待补决定的内容完成则有实际正文验证屏障。
本工具不改变这一区别。每阶段的完整 GET 与显式健康诊断另行实际验证固定正文，
不能把它们的成功说成 Engine::open 自身重读了全部已发布正文，也不覆盖故障注入／损坏恢复矩阵。

驱动不注册周期巡检 schedule，不取得 `PublishedDependencyReuse` 许可。
当前候选同次读取 memo／facts／proof hash 复用可能减少重复成本；没有周期覆盖许可时
仍保留所需旧依赖验证、对象碰撞比较与同步。一个逻辑目录盘不证明多物理盘性能。

## 每版一次构建与二进制复用

正式构建、自测和运行只在 GitHub Actions，固定 Rust **1.99.0**。
本地／云工作区只读源码、编辑、Git 和按文件格式检查，不本地执行驱动或测试。
[workflow.yml](../../tools/history-scale-bench/workflow.yml) 是专用验证载体模板，
只有手动 `workflow_dispatch`，默认 selftest。首次 selftest 每版仅一个 build job；
后续所有规模必须填写原二进制的 `build_run_id` 与 `build_carrier_sha`，
跳过 build jobs，下载原两份二进制，不能每档／每次 dispatch 重编。
若仅监督器修复，可以在 selftest 模式复用两版 build job 均成功的来源 run，
即使原 run 的小测因监督器失败；原失败保持失败。规模运行还须填写成功的
`selftest_run_id`（默认与 build run 相同），控制器核对该 run 成功、selection 绑定
相同候选与原构建，以及 4 样本／12 阶段完整通过；随后 runner 再核对全部摘要。
测量工具的 SHA/run、自测 run 与原构建身份分别记录。

[prepare.py](../../tools/history-scale-bench/prepare.py) 接受固定对照或显式精确候选 SHA
的干净 checkout，核对 Rust pin，在已有 perf-check 中临时注册同一普通 binary。
不新增依赖、工作区或锁文件；接线只存在于隔离验证 checkout。
旧版启用无依赖 `history-scale-baseline`，新版启用 `history-scale-current` 适配特性，
都使用原引擎实现。不用 example/libtest/all-targets；引擎的 dev self-dependency
会使 example 暗中启用 fault-injection。

[verify_build.py](../../tools/history-scale-bench/verify_build.py) 从实际 Cargo compiler-artifact
核对普通 release、目标和 features；有 fault-injection/test-support、warning 或缺失完成标记则失败。
每份 provenance 绑定精确 source SHA、原／临时 manifest、Cargo.lock、Cargo 配置、
同一驱动摘要、真实命令、rustc/cargo、编译 flags/ISA、实际 features/profile、二进制和日志摘要。
编排器再次核对 checkout、工具驱动和二进制摘要，并把 provenance 随各 cohort 保存。
源码、驱动或构建输入变化必须生成新来源，不能复用失效产物。

模板的 build 与测量可在不同 runner，保存两个环境；同 cohort 的两版运行在同一 runner，
轮次反转版本顺序。不同 cohort 的 CPU／文件系统可能不同，跨规模绝对耗时不能全部归因于 N。
release binary 仍使用 `Config::development` 单逻辑盘，不能称为真实生产设备配置。

## 指标与结果解释

无 strace 的计时与 strace 计数独立。追踪覆盖整个阶段，包括 oracle、正文验证与重试；
阶段总同步数不是单次追加的同步数。原始逐条延迟、首次／重复查询和所有失败均保留。

| 指标 | 范围与限制 |
| --- | --- |
| 驱动单调时钟 | 实际引擎调用，受 runner、页缓存和监督器影响 |
| `/proc/<pid>/io` | 含基准 I/O；rchar 不是物理读量 |
| strace 字节 | syscall 返回逻辑字节，计数与无追踪时间分开 |
| RSS／HWM | 进程与基准一起占用，不是分配次数；恢复后 RSS 单列 |
| 设备统计 | 共享／虚拟设备观察，不能归因于独立物理设备 |
| fsync／fdatasync | 文件、目录和 unknown 分类，无法解析的不猜测 |
| 对象／空间 | 含永久历史与保留材料的数量、逻辑与分配空间 |

冷准入可能建立全量 operation_id 索引，其 RSS 和首次查询条件须说明。
样本不足时保留原值与重复范围，不伪造稳定百分位。强制停止时在途操作可能已持久化，
已确认前缀不是池的最终总数；缺失 GNU time 最终峰值不能拿采样值冒充。
退出时 `/proc` 权限错误或缺失 RSS，只在复读证明进程已消失或 Z/X 时丢弃该无效采样，
保留原状态、路径与诊断；仍存活的权限错误或缺 RSS 仍失败，不改权限或伪造零值。

requested.json 必须完整收尾且每个样本三阶段全通过才可报告全通过。
原始日志、二进制来源、完整命令、环境、oracle-evidence、资源与同步汇总由 workflow
artifact 保存 30 天。池目录不上传；oracle 与原日志仍保留。取消／失败产物同样上传。

## 准备、自测与旧证据

工具自测入口，在 GitHub Actions 执行：

```sh
python3 -m unittest discover -s test/history-scale-bench -p 'test_*.py' -v
```

把工具补丁提交后，在独立验证载体分支复制模板到 `.github/workflows/history-scale.yml`，
手动 dispatch `mode=selftest`。载体不得进入产品祖先，不修改正常 CI，不推 main。
自测通过后 `mode=acceptance` 加原构建 `build_run_id`／`build_carrier_sha`，
以及成功的 `selftest_run_id` 运行边界和 1k；若自测与构建同 run，后者可省略。
`mode=10000` 仍等待实际初档结果与资源趋势审阅；不启动 100k。

旧 `13b664e9fd41a356ffb7ed982d21e97abb123626` 的
[原规模运行 40823525831](https://github.com/isarmg/zxs/actions/runs/40823525831)
使用工具 `17997b3b5313c64d2d0ff95e732037caf3c23d80`，载体
`e7adc933656152d28ca9d92ccd9807c52778ce61`：32 样本为 19 passed、1 failed、
4 incomplete、8 not_executed。旧样本是空命名空间、零正文、1 条 setup、N−1 次绑定，
不能直接代表新候选或本轮固定正文 fixture，原失败和 Incomplete 不改记。
[监测修复定向运行 40828568739](https://github.com/isarmg/zxs/actions/runs/40828568739)
仅证明旧基线新池 1025 条监测回归通过，不替换旧失败，也不为新候选提供容量证据。

源码入口为[驱动](../../tools/history-scale-bench/driver.rs)、
[编排](../../tools/history-scale-bench/run.py)、[摘要校验](../../tools/history-scale-bench/summarize.py)、
[计数](../../tools/history-scale-bench/tracing.py)、[控制器](../../tools/history-scale-bench/controller.py)。
规范入口为[完整编码规则](../../禁止防御性编程.md)、[工程规则](../20-engineering.md)
和[测试布局](../architecture/test-layout.md)。不另建工具 README 或进度报告。

通过、失败、未完成、未执行分别报告。局部指标或微型自测不能推出整个保存路径
O(log N)/O(delta)、无限历史、真实物理掉电或产品采用通过。
