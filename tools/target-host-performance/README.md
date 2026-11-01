# 目标主机优化的云环境测量入口

本目录编排隔离 Development 探针，不修改产品或持久化协议。
原始负载、正确性断言和计时范围见[性能检查](../../docs/tools/perf-check.md)及
[恢复测量](../../docs/validation/recovery-deduplication.md)。云结果明确标作
`cloud-development-proxy-experiment`，不能代表 Ultra 288V、12 块 E1.S 或真实 Samba 网络性能。
已执行范围、基线数字及候选边界见[目标主机性能基线](../../docs/validation/target-host-performance.md)。

根协调者先冻结源码，再集中构建；所有运行必须使用同一次构建的确切 target、profile、
features 与 Cargo.lock。新开一次构建前结束现有测量，不在另一个 worktree 复用来源不清的 target。

```sh
cargo build --release --locked --offline -p perf-check --bin perf-check --bin recovery-startup
python3 tools/target-host-performance/capture.py \
  --repo . --binary target/native/release/perf-check \
  --workload probe --history 8 \
  --output test/evidence/target-host-performance/probe8-r1 \
  --build-command 'cargo build --release --locked --offline -p perf-check --bin perf-check --bin recovery-startup' \
  --profile release --target x86_64-unknown-linux-gnu --features default
```

下一档只替换二进制为 `recovery-startup`、workload 为 `recovery-fixed`、history 为 64，
并使用新的 output。起步每档一次，确认实际耗时和空间后才选择 3 次重复或更大规模。
8 与 64 是实验输入，不是产品阈值。起步预算为 60 秒、采样 RSS 2 GiB、文件系统空闲预留
4 GiB；参数可按云实例实际上限调整，超限保留失败及残留材料，不自动重试或删除。
监测每 250ms 读取一次子进程 RSS 和空闲空间，有开销与采样盲区，不能当作瞬时硬隔离。
既有探针只在私有 TMPDIR 下创建自己的池，不接受已有池或真实设备参数。

每个 output 必须原先不存在。驱动复制并校验预建二进制，再执行该冻结副本，保存：

- 原始 stdout/stderr、逐次资源采样与 wait4 的退出码、CPU、峰值 RSS、缺页和块 I/O 计数；
- commit/tree、跟踪源码 diff 原文与摘要、Cargo.lock/toolchain/二进制/驱动 SHA-256；
- CPU、亲和性、cgroup CPU/内存、实际挂载、系统内存与实验目录空闲量；
- 声明的构建命令、target/profile/features、可选 `--build-receipt` 集中构建原始收据。

源码和二进制哈希是现场观察；构建来源匹配依赖根协调者的冻结构建记录，驱动不会通过
文件名推断编译特性。`provenance_stable` 只证明运行前后源码/diff和冻结二进制一致。
`process_completed` 只表示进程正常退出；必须另核对既有探针的完整输出与全部正确性断言，
不将缺输出或非零退出视为成功。wait4 块计数与 `/sys/fs/cgroup/io.stat` 不能冒充逐操作
物理盘字节，RSS 也不是分配次数。

每次使用新 fixture；OS 页缓存没有清空，首次打开也紧随写入，不能标作物理冷读。
`perf-check` 的 SQLite 触发器与完整状态比较有观测成本，按其原有说明解释。重复进程的
P50/P95/P99只在样本数足够并明确总体后报告，不能由单次四个增长检查点构造尾延迟。
本入口不提供吞吐达标阈值，不重新启用已停止的历史规模验证流程。

`tools/perf-check/src/bin/resource-phases.rs` 另通过公开 Engine API 拆分真实保存阶段：
`create_workspace`、`stage_bytes`、`prepare_workspace_commit` 捕获、`execute`、
`finish_workspace_commit`，逐事务输出 JSONL。`execute` 包含规划、正文及元数据准备，
不能仅凭该时间归因到其中一项；同步单进程也不测服务端 mutex 等待。
检查点单独计 State clone、重开和完整性检查，并核对永久收据/所有者、原请求重放不改变
State、最老正文和实时命名空间。clone-only 不含析构、比较或编码，不代表真实规划复制总成本。

集中构建时增加 `--bin resource-phases`，运行驱动选择该二进制及
`--workload phases-fixed` 或 `phases-growing`，`--history` 为 1..128 次、
`--payload-bytes` 为 64 或 128。建议先 8 次，再按预算测 64；使用新输出目录。
检查点会重开并影响随后进程状态，原始记录保留每次历史大小和重开位置，
不能把所有不同历史规模的事务混为固定规模的 P99。

生成证据保存在 `test/evidence/` 的既有忽略规则下，保留原始输出；验收说明沿用
`docs/validation/`，源码不增加另一份项目进展报告。
